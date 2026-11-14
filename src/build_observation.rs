//! Narrow evidence from Core's structured Build update and Docker Build stage.
use crate::client::CoreClient;
use komodo_client::entities::{Operation, ResourceTarget, update::Update};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageEventKind {
    Tagged,
    ManifestPushReported,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ImageObservation {
    pub reference: String,
    pub digest: Option<String>,
    pub kind: ImageEventKind,
    pub stage_index: usize,
    pub stage_success: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BuildObservation {
    pub version: Option<String>,
    pub version_evidence: &'static str,
    pub images: Vec<ImageObservation>,
    pub image_evidence_incomplete: bool,
    pub image_evidence_ambiguous: bool,
    pub registry_push_failure_reported: bool,
    pub deployment_result_evidence: &'static str,
    pub evidence_limit: &'static str,
}
fn reference(client: &CoreClient, s: &str) -> Option<(String, Option<String>)> {
    if s.is_empty()
        || s.len() > 512
        || s.starts_with('-')
        || s.contains("://")
        || client.redact(s) != s
    {
        return None;
    }
    let (name, digest) = match s.split_once("@sha256:") {
        Some((name, digest))
            if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            (
                name,
                Some(format!("sha256:{}", digest.to_ascii_lowercase())),
            )
        }
        Some(_) => return None,
        None => (s, None),
    };
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/:[]".contains(&b))
        || name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return None;
    }
    Some((name.into(), digest))
}
fn plain(line: &str) -> Option<String> {
    if line.len() > 4096 {
        return None;
    }
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() != Some('[') {
                return None;
            }
            let mut complete = false;
            for c in chars.by_ref().take(32) {
                if c == 'm' {
                    complete = true;
                    break;
                }
                if !c.is_ascii_digit() && c != ';' {
                    return None;
                }
            }
            if !complete {
                return None;
            }
        } else if c.is_control() && !matches!(c, '\t' | '\r') {
            return None;
        } else {
            out.push(c);
        }
    }
    Some(out)
}
fn completed_reference(text: &str) -> Option<&str> {
    let mut fields = text.split_whitespace();
    let reference = fields.next()?;
    let completion = fields.next()?;
    if completion != "done" {
        let elapsed = completion.strip_suffix('s')?.parse::<f64>().ok()?;
        if !elapsed.is_finite() || elapsed < 0.0 || fields.next()? != "done" {
            return None;
        }
    }
    fields.next().is_none().then_some(reference)
}
pub(crate) fn project(client: &CoreClient, u: &Update) -> Option<BuildObservation> {
    if u.operation != Operation::RunBuild || !matches!(u.target, ResourceTarget::Build(_)) {
        return None;
    }
    let version = (!u.version.is_none()
        && [u.version.major, u.version.minor, u.version.patch]
            .iter()
            .all(|n| *n >= 0))
    .then(|| u.version.to_string());
    let mut result = BuildObservation {
        version,
        version_evidence: "update.version when nonzero and serialized; this is the attempted version, not proof of build success",
        images: vec![],
        image_evidence_incomplete: false,
        image_evidence_ambiguous: false,
        registry_push_failure_reported: false,
        deployment_result_evidence: "Core 2.3.2 starts dependent redeployments asynchronously and returns no child update/result links in the Build update. Inspect the configured Deployment IDs and health separately; timing alone does not establish causality.",
        evidence_limit: "Images are narrow Docker Build output reports, not configured tags or independently verified registry contents. Missing/unparseable/ambiguous output stays unknown. A successful build or push is not proof of a deployment or healthy workload.",
    };
    let mut logs = u
        .logs
        .iter()
        .enumerate()
        .filter(|(_, l)| l.stage == "Docker Build");
    let Some((index, log)) = logs.next() else {
        return Some(result);
    };
    if logs.next().is_some() {
        result.image_evidence_ambiguous = true;
        return Some(result);
    }
    let mut events = Vec::new();
    for stream in [&log.stdout, &log.stderr] {
        let mut start = stream.len().saturating_sub(65536);
        while !stream.is_char_boundary(start) {
            start += 1;
        }
        result.image_evidence_incomplete |= start > 0;
        for line in stream[start..].lines() {
            let Some(line) = plain(line) else {
                result.image_evidence_incomplete = true;
                continue;
            };
            let line = line.trim();
            let text = if let Some(numbered) = line.strip_prefix('#') {
                let Some((number, text)) = numbered.split_once(' ') else {
                    continue;
                };
                if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                text.trim_start()
            } else if line.starts_with("Successfully tagged ") {
                line
            } else {
                continue;
            };
            result.registry_push_failure_reported |= text.starts_with("ERROR: failed to push ");
            let parsed = if let Some(s) = text.strip_prefix("Successfully tagged ") {
                Some((s, ImageEventKind::Tagged))
            } else if let Some(s) = text.strip_prefix("naming to ") {
                completed_reference(s).map(|s| (s, ImageEventKind::Tagged))
            } else if let Some(s) = text.strip_prefix("pushing manifest for ") {
                completed_reference(s).map(|s| (s, ImageEventKind::ManifestPushReported))
            } else {
                None
            };
            let Some((value, kind)) = parsed else {
                continue;
            };
            let Some((reference, digest)) = reference(client, value) else {
                result.image_evidence_incomplete = true;
                continue;
            };
            if kind == ImageEventKind::ManifestPushReported && digest.is_none() {
                result.image_evidence_incomplete = true;
                continue;
            }
            if !events.iter().any(|e: &ImageObservation| {
                e.reference == reference && e.digest == digest && e.kind == kind
            }) {
                events.push(ImageObservation {
                    reference,
                    digest,
                    kind,
                    stage_index: index,
                    stage_success: log.success,
                });
            }
            if events.len() > 32 {
                result.image_evidence_incomplete = true;
                return Some(result);
            }
        }
    }
    let ambiguous = events
        .iter()
        .filter(|e| {
            e.kind == ImageEventKind::ManifestPushReported
                && events.iter().any(|other| {
                    other.kind == e.kind
                        && other.reference == e.reference
                        && other.digest != e.digest
                })
        })
        .map(|e| e.reference.clone())
        .collect::<std::collections::BTreeSet<_>>();
    result.image_evidence_ambiguous = !ambiguous.is_empty();
    events.retain(|e| !ambiguous.contains(&e.reference));
    result.image_evidence_incomplete |= events.len() > 8;
    result.images = events.into_iter().take(8).collect();
    Some(result)
}
