//! Read-only workload log tails. Limits apply jointly to stdout and stderr after
//! credential redaction, in UTF-8 bytes (JSON escaping adds transport overhead).

use std::collections::{BTreeSet, VecDeque};

use komodo_client::{
    api::read::{GetContainerLog, GetDeploymentLog, GetStackLog},
    entities::update::Log,
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
};

pub const MAX_TAIL: u64 = 500;
pub const MAX_BYTES: usize = 65_536;
pub const MAX_SERVICES: usize = 16;

fn default_tail() -> u64 {
    100
}
fn default_bytes() -> usize {
    16_384
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContainerLogsInput {
    /// Stable Server ID returned by inventory.
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub server_id: String,
    /// Exact Docker container name or ID; at most 128 ASCII name characters.
    #[schemars(length(min = 1, max = 128))]
    pub container: String,
    /// Positive upstream tail; also the joint stdout/stderr output line limit.
    #[serde(default = "default_tail")]
    #[schemars(range(min = 1, max = 500))]
    pub tail: u64,
    /// Joint stdout/stderr UTF-8 byte budget after redaction.
    #[serde(default = "default_bytes")]
    #[schemars(range(min = 1, max = 65536))]
    pub max_bytes: usize,
    #[serde(default)]
    pub timestamps: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeploymentLogsInput {
    /// Stable Deployment ID returned by inventory.
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub id: String,
    #[serde(default = "default_tail")]
    #[schemars(range(min = 1, max = 500))]
    pub tail: u64,
    #[serde(default = "default_bytes")]
    #[schemars(range(min = 1, max = 65536))]
    pub max_bytes: usize,
    #[serde(default)]
    pub timestamps: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StackLogsInput {
    /// Stable Stack ID returned by inventory.
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub id: String,
    /// Exact Compose services. Empty includes all services; at most 16 filters.
    #[serde(default)]
    #[schemars(length(max = 16))]
    pub services: Vec<String>,
    #[serde(default = "default_tail")]
    #[schemars(range(min = 1, max = 500))]
    pub tail: u64,
    #[serde(default = "default_bytes")]
    #[schemars(range(min = 1, max = 65536))]
    pub max_bytes: usize,
    #[serde(default)]
    pub timestamps: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogTargetKind {
    Container,
    Deployment,
    Stack,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogTarget {
    pub kind: LogTargetKind,
    /// Server ID for a container, otherwise the workload's stable ID.
    pub id: String,
    pub container: Option<String>,
    pub services: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogTruncation {
    /// More than the requested joint line limit was returned by Core.
    pub lines: bool,
    /// Redacted selected lines exceeded the joint UTF-8 byte budget.
    pub bytes: bool,
    /// Core provides no complete-log count. A full tail may omit earlier logs.
    pub upstream_may_have_more: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkloadLogs {
    pub target: LogTarget,
    pub requested_tail: u64,
    pub max_bytes: usize,
    pub timestamps: bool,
    pub stdout: String,
    pub stderr: String,
    pub returned_lines: usize,
    pub returned_bytes: usize,
    pub truncation: LogTruncation,
    pub start_ts: i64,
    pub end_ts: i64,
}

fn valid_id(value: &str) -> bool {
    value.len() == 24 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0] != b'-'
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn validate(id: &str, tail: u64, bytes: usize, operation: &'static str) -> Result<(), AppError> {
    if !valid_id(id) || !(1..=MAX_TAIL).contains(&tail) || !(1..=MAX_BYTES).contains(&bytes) {
        return Err(AppError::arguments(
            operation,
            "Use a 24-character hexadecimal resource ID, tail from 1 through 500, and max_bytes from 1 through 65536",
        ));
    }
    Ok(())
}

pub async fn container_logs(
    client: &CoreClient,
    input: ContainerLogsInput,
) -> Result<WorkloadLogs, AppError> {
    const OP: &str = "GetContainerLog";
    validate(&input.server_id, input.tail, input.max_bytes, OP)?;
    if !valid_name(&input.container) {
        return Err(AppError::arguments(
            OP,
            "Use an exact container name or ID with at most 128 ASCII letters, digits, underscores, dots or hyphens, without a leading hyphen",
        ));
    }
    let log = client
        .read(GetContainerLog {
            server: input.server_id.clone(),
            container: input.container.clone(),
            tail: input.tail,
            timestamps: input.timestamps,
        })
        .await?;
    project(
        client,
        log,
        LogTarget {
            kind: LogTargetKind::Container,
            id: input.server_id,
            container: Some(client.redact(&input.container)),
            services: vec![],
        },
        input.tail,
        input.max_bytes,
        input.timestamps,
        OP,
    )
}

pub async fn deployment_logs(
    client: &CoreClient,
    input: DeploymentLogsInput,
) -> Result<WorkloadLogs, AppError> {
    const OP: &str = "GetDeploymentLog";
    validate(&input.id, input.tail, input.max_bytes, OP)?;
    let log = client
        .read(GetDeploymentLog {
            deployment: input.id.clone(),
            tail: input.tail,
            timestamps: input.timestamps,
        })
        .await?;
    project(
        client,
        log,
        LogTarget {
            kind: LogTargetKind::Deployment,
            id: input.id,
            container: None,
            services: vec![],
        },
        input.tail,
        input.max_bytes,
        input.timestamps,
        OP,
    )
}

pub async fn stack_logs(
    client: &CoreClient,
    input: StackLogsInput,
) -> Result<WorkloadLogs, AppError> {
    const OP: &str = "GetStackLog";
    validate(&input.id, input.tail, input.max_bytes, OP)?;
    if input.services.len() > MAX_SERVICES
        || input.services.iter().any(|service| !valid_name(service))
        || input.services.iter().collect::<BTreeSet<_>>().len() != input.services.len()
    {
        return Err(AppError::arguments(
            OP,
            "Use at most 16 distinct exact service names, each with at most 128 ASCII letters, digits, underscores, dots or hyphens, without a leading hyphen",
        ));
    }
    let log = client
        .read(GetStackLog {
            stack: input.id.clone(),
            services: input.services.clone(),
            tail: input.tail,
            timestamps: input.timestamps,
        })
        .await?;
    project(
        client,
        log,
        LogTarget {
            kind: LogTargetKind::Stack,
            id: input.id,
            container: None,
            services: input.services.iter().map(|s| client.redact(s)).collect(),
        },
        input.tail,
        input.max_bytes,
        input.timestamps,
        OP,
    )
}

fn project(
    client: &CoreClient,
    log: Log,
    target: LogTarget,
    tail: u64,
    max_bytes: usize,
    timestamps: bool,
    operation: &'static str,
) -> Result<WorkloadLogs, AppError> {
    // A failed log command is an unavailable source. Never expose its command or
    // failure payload, and never retry with an unlimited/fallback request.
    if !log.success {
        return Err(AppError::new(ErrorKind::Upstream, operation, None));
    }
    // Core returns separate streams without a cross-stream ordering. Keep their
    // order and take the joint tail in stdout-then-stderr order, without claiming
    // timestamp ordering. The queue itself never holds more than `tail` slices.
    let mut selected = VecDeque::new();
    let mut count = 0usize;
    for (is_stderr, text) in [(false, log.stdout.as_str()), (true, log.stderr.as_str())] {
        for line in text.split_inclusive('\n') {
            count = count.saturating_add(1);
            if selected.len() == tail as usize {
                selected.pop_front();
            }
            selected.push_back((is_stderr, line));
        }
    }
    let mut remaining = max_bytes;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut byte_truncated = false;
    let mut returned_lines = 0usize;
    for (is_stderr, line) in selected.into_iter().rev() {
        if remaining == 0 {
            byte_truncated = true;
            break;
        }
        // Redact whole retained lines before clipping so clipping cannot reveal
        // a credential prefix that was cut before the full secret was matched.
        let safe = redact_log(client, line);
        let mut start = safe.len().saturating_sub(remaining);
        while !safe.is_char_boundary(start) {
            start += 1;
        }
        byte_truncated |= start != 0;
        let kept = &safe[start..];
        if !kept.is_empty() {
            remaining -= kept.len();
            returned_lines += 1;
            if is_stderr {
                stderr.push(kept.to_owned());
            } else {
                stdout.push(kept.to_owned());
            }
        }
    }
    stdout.reverse();
    stderr.reverse();
    let stdout = stdout.concat();
    let stderr = stderr.concat();
    Ok(WorkloadLogs {
        target,
        requested_tail: tail,
        max_bytes,
        timestamps,
        returned_bytes: stdout.len() + stderr.len(),
        returned_lines,
        stdout,
        stderr,
        truncation: LogTruncation {
            lines: count > tail as usize,
            bytes: byte_truncated,
            upstream_may_have_more: count >= tail as usize,
        },
        start_ts: log.start_ts,
        end_ts: log.end_ts,
    })
}

/// Configured API credentials, common sensitive assignments (including quoted
/// JSON values) and Bearer tokens. This deliberately does not promise to detect
/// arbitrary application secrets without a recognizable label.
pub(crate) fn redact_log(client: &CoreClient, line: &str) -> String {
    // Recognize assignment labels before configured credentials can redact a
    // label itself (for example, an API key equal to `token`). Then sanitize
    // configured values in the remaining text and in any adjacent context.
    let configured = line;
    let lower = configured.to_ascii_lowercase();
    const LABELS: &[&str] = &[
        "password",
        "passwd",
        "api_key",
        "api-key",
        "apikey",
        "api_secret",
        "api-secret",
        "secret",
        "token",
        "access_token",
        "refresh_token",
        "client_secret",
        "database_url",
        "connection_string",
        "private_key",
        "authorization",
    ];
    let mut result = String::new();
    let mut offset = 0;
    while offset < configured.len() {
        let rest = &lower[offset..];
        let previous = configured[..offset].bytes().next_back();
        let boundary =
            previous.is_none_or(|b| !b.is_ascii_alphanumeric() && b != b'_' && b != b'-');
        if boundary && rest.starts_with("bearer ") {
            let start = offset + "bearer ".len();
            let end = value_end(configured, start, None);
            if end > start {
                result.push_str(&configured[offset..start]);
                result.push_str(client.redaction_marker());
                offset = end;
                continue;
            }
        }
        let mut assignment = None;
        if boundary {
            for label in LABELS {
                if !rest.starts_with(label) {
                    continue;
                }
                let mut at = offset + label.len();
                if configured
                    .as_bytes()
                    .get(at)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
                {
                    continue;
                }
                if matches!(configured.as_bytes().get(at), Some(b'\'' | b'"')) {
                    at += 1;
                }
                while configured
                    .as_bytes()
                    .get(at)
                    .is_some_and(u8::is_ascii_whitespace)
                {
                    at += 1;
                }
                if !matches!(configured.as_bytes().get(at), Some(b'=' | b':')) {
                    continue;
                }
                at += 1;
                while configured
                    .as_bytes()
                    .get(at)
                    .is_some_and(u8::is_ascii_whitespace)
                {
                    at += 1;
                }
                let quote = configured
                    .as_bytes()
                    .get(at)
                    .copied()
                    .filter(|b| matches!(b, b'\'' | b'"'));
                if quote.is_some() {
                    at += 1;
                }
                let end = if label == &"authorization"
                    && configured[at..].to_ascii_lowercase().starts_with("bearer ")
                {
                    value_end(configured, at + "bearer ".len(), quote)
                } else {
                    value_end(configured, at, quote)
                };
                if end > at {
                    assignment = Some((at, end));
                    break;
                }
            }
        }
        if let Some((start, end)) = assignment {
            result.push_str(&configured[offset..start]);
            result.push_str(client.redaction_marker());
            offset = end;
        } else {
            let character = configured[offset..]
                .chars()
                .next()
                .expect("nonempty remainder");
            result.push(character);
            offset += character.len_utf8();
        }
    }
    client.redact(&result)
}

fn value_end(text: &str, start: usize, quote: Option<u8>) -> usize {
    let mut escaped = false;
    for (index, character) in text[start..].char_indices() {
        if !escaped
            && (quote == Some(character as u8) && character.is_ascii()
                || quote.is_none() && (character.is_whitespace() || ",;\"'".contains(character)))
        {
            return start + index;
        }
        escaped = !escaped && character == '\\';
    }
    text.len()
}
