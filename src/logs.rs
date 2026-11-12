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
/// Joint decoded stdout/stderr input budget, before sanitizer allocations.
pub const MAX_INPUT_BYTES: usize = crate::diagnostics::MAX_REDACTION_INPUT_BYTES;

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
    /// Exact services; at most 16. Compose: empty includes all. Swarm: pass one.
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
    if log.stdout.len().saturating_add(log.stderr.len()) > MAX_INPUT_BYTES {
        return Err(AppError::new(ErrorKind::ResponseTooLarge, operation, None));
    }
    // Use all context Core supplied before taking the local tail: a quoted
    // sensitive key can be on an earlier physical line than its value. Sanitize
    // streams independently without another request or decoding JSON escapes.
    let safe_stdout = redact_log(client, &log.stdout);
    let safe_stderr = redact_log(client, &log.stderr);
    // Core returns separate streams without a cross-stream ordering. Keep their
    // order and take the joint tail in stdout-then-stderr order, without claiming
    // timestamp ordering. The queue itself never holds more than `tail` slices.
    let mut selected = VecDeque::new();
    let mut count = 0usize;
    for (is_stderr, text) in [(false, safe_stdout.as_str()), (true, safe_stderr.as_str())] {
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
        // Context was redacted before line selection. Clip only at a UTF-8
        // boundary so neither a local tail nor a byte cut can expose a secret.
        let mut start = line.len().saturating_sub(remaining);
        while !line.is_char_boundary(start) {
            start += 1;
        }
        byte_truncated |= start != 0;
        let kept = &line[start..];
        if !kept.is_empty() {
            remaining -= kept.len();
            returned_lines += 1;
            if is_stderr {
                stderr.push(kept.to_owned());
            } else {
                stdout.push(kept.to_owned());
            }
        }
        // A UTF-8 adjustment can leave unused budget. Older fragments would
        // fill that gap with discontiguous content instead of a latest suffix.
        if start != 0 {
            break;
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
/// JSON values), full Authorization values and standalone Bearer tokens. This
/// deliberately does not promise to detect arbitrary unlabeled application secrets.
pub(crate) fn redact_log(client: &CoreClient, line: &str) -> String {
    if line.len() > MAX_INPUT_BYTES {
        return client.redaction_marker().to_owned();
    }
    // Recognize assignment labels before configured credentials can redact a
    // label itself (for example, an API key equal to `token`). Then sanitize
    // configured values in the remaining text and in any adjacent context.
    let configured = line;
    let lower = configured.to_ascii_lowercase();
    let mut result = String::new();
    let mut offset = 0;
    // Bound complete-fragment validation across this received stream. Exhausted
    // ambiguity checks conservatively hide the remaining received stream.
    let mut json_checks = 128usize;
    let mut checked_json = None;
    while offset < configured.len() {
        let rest = &lower[offset..];
        let previous = configured[..offset].bytes().next_back();
        let boundary =
            previous.is_none_or(|b| !b.is_ascii_alphanumeric() && b != b'_' && b != b'-');
        if boundary && rest.starts_with("bearer") {
            let prefix = offset + "bearer".len();
            let mut start = prefix;
            while configured
                .as_bytes()
                .get(start)
                .is_some_and(|b| matches!(b, b' ' | b'\t'))
            {
                start += 1;
            }
            if start > prefix {
                let end = value_end(configured, start, None);
                if end > start {
                    result.push_str(&configured[offset..start]);
                    result.push_str(client.redaction_marker());
                    offset = end;
                    continue;
                }
            }
        }
        // Consume and classify the entire assignment key once. Qualified
        // snake/hyphen/camel names share a classifier; unrelated right-hand
        // suffixes such as PASSWORD_FILE and secretKeySuffix do not match.
        let key_end = offset
            + configured.as_bytes()[offset..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || b"_-".contains(b))
                .count();
        let assignment = if key_end > offset {
            sensitive_key(&configured[offset..key_end]).and_then(|header| {
                sensitive_value(
                    configured,
                    offset,
                    key_end,
                    header,
                    &mut json_checks,
                    &mut checked_json,
                )
            })
        } else {
            None
        };
        if let Some((start, end, structured)) = assignment {
            result.push_str(&configured[offset..start]);
            if let Some(quote) = structured {
                let mut marker =
                    serde_json::to_string(client.redaction_marker()).expect("marker serializes");
                for _ in 0..quote.escapes.saturating_add(1).trailing_zeros() {
                    let encoded = serde_json::to_string(&marker).expect("marker serializes");
                    marker = encoded[1..encoded.len() - 1].to_owned();
                }
                result.push_str(&marker);
            } else {
                result.push_str(client.redaction_marker());
            }
            // Structured values can span physical lines. Retain their line
            // separators so local tail selection and line counts stay stable.
            result.extend(
                configured[start..end]
                    .chars()
                    .filter(|c| matches!(c, '\r' | '\n')),
            );
            offset = end;
        } else if key_end > offset {
            result.push_str(&configured[offset..key_end]);
            offset = key_end;
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

// Return whether the complete key denotes an Authorization header. Other
// sensitive keys use the same value parser without header whitespace rules.
fn sensitive_key(key: &str) -> Option<bool> {
    if !key.as_bytes().last()?.is_ascii_alphanumeric() {
        return None;
    }
    let compact: String = key
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|b| char::from(b.to_ascii_lowercase()))
        .collect();
    for label in [
        "password",
        "passwd",
        "apikey",
        "apisecret",
        "secretaccesskey",
        "secretkey",
        "accesskey",
        "secret",
        "token",
        "databaseurl",
        "connectionstring",
        "privatekey",
        "authorization",
    ] {
        if !compact.ends_with(label) {
            continue;
        }
        let (start, _) = key
            .bytes()
            .enumerate()
            .rev()
            .filter(|(_, b)| b.is_ascii_alphanumeric())
            .nth(label.len() - 1)?;
        let previous = key[..start].bytes().next_back();
        let camel_boundary = key.as_bytes()[start].is_ascii_uppercase()
            && previous.is_some_and(|b| {
                b.is_ascii_lowercase()
                    || (b.is_ascii_uppercase()
                        && key
                            .as_bytes()
                            .get(start + 1)
                            .is_some_and(u8::is_ascii_lowercase))
            });
        if previous.is_none_or(|b| !b.is_ascii_alphanumeric()) || camel_boundary {
            return Some(label == "authorization");
        }
    }
    None
}

fn sensitive_value(
    text: &str,
    label: usize,
    mut at: usize,
    header: bool,
    checks: &mut usize,
    checked: &mut Option<(usize, usize, usize)>,
) -> Option<(usize, usize, Option<Quote>)> {
    let key_quote = encoded_quote(text, at).map(|(quote, end)| {
        at = end;
        quote
    });
    at = skip_whitespace(text, at, key_quote, header, false);
    if !matches!(text.as_bytes().get(at), Some(b'=' | b':')) {
        return None;
    }
    at += 1;
    let value_space = at;
    at = skip_whitespace(text, at, key_quote, header, false);
    let quote = encoded_quote(text, at).map(|(quote, end)| {
        at = end;
        quote
    });
    let malformed_quote = quote.is_none()
        && text[at..]
            .trim_start_matches('\\')
            .starts_with(['\'', '\"']);
    let inconsistent_quotes = malformed_quote
        || key_quote.is_some_and(|key| quote.is_none_or(|value| key.escapes != value.escapes));
    // Plain YAML/config values can start after comments, tags, anchors, or blank
    // lines, and any sensitive assignment may continue on indented lines.
    // Quoted JSON keys keep their existing validated structured context.
    let plain_multiline =
        key_quote.is_none() && !header && text[value_space..at].contains(['\r', '\n']);
    let continuation =
        key_quote.is_none() && indented_continuation(text, line_end(text, at), label);
    let yaml_prefix = key_quote.is_none()
        && !header
        && quote.is_none()
        && matches!(text.as_bytes().get(at), Some(b'#' | b'!' | b'&'));
    if plain_multiline || continuation || yaml_prefix {
        return (text.len() > at).then_some((at, text.len(), None));
    }
    if quote.is_none() && matches!(text.as_bytes().get(at), Some(b'{' | b'[')) {
        let context = Quote {
            character: b'"',
            escapes: key_quote.map_or(0, |q| q.escapes),
        };
        let end = container_end(text, at, context)
            .filter(|end| valid_json_fragment(text, at, *end, context))
            .filter(|end| context_end_clear(text, label, *end, context, key_quote, checks, checked))
            .unwrap_or(text.len());
        return Some((at, end, Some(context)));
    }
    // Unquoted credentials can contain spaces, punctuation, or quoted
    // subfields (URLs, passwords, Authorization, ...). Mask through the line.
    // Clear quoted values retain delimiters at their original JSON depth.
    // Unclear quotes and YAML block values may span physical lines: hide the
    // remaining received stream, preserving CR/LF without an uncapped fetch.
    let multiline_or_unclear = inconsistent_quotes
        || quote.is_some_and(|q| quoted_end(text, at, q).is_none())
        || (quote.is_none() && matches!(text.as_bytes().get(at), Some(b'|' | b'>')));
    let mut end = if multiline_or_unclear {
        text.len()
    } else if quote.is_none() {
        line_end(text, at)
    } else {
        value_end(text, at, quote)
    };
    if !multiline_or_unclear
        && let Some(quote) = quote
        && !quoted_end_clear(text, label, end, quote, key_quote, checks, checked)
    {
        end = text.len();
    }
    (end > at).then_some((at, end, None))
}

fn indented_continuation(text: &str, mut at: usize, label: usize) -> bool {
    let bytes = text.as_bytes();
    let line_start = text[..label].rfind(['\r', '\n']).map_or(0, |end| end + 1);
    let prefix = &text[line_start..label];
    let literal_prefix = !prefix.trim().is_empty();
    let compose_prefix = prefix.contains('|');
    let formatted_prefix = if compose_prefix {
        compose_payload(prefix).unwrap_or(prefix)
    } else {
        prefix
    };
    let timestamp_prefix = timestamp_payload(formatted_prefix).is_some();
    loop {
        if !matches!(bytes.get(at), Some(b'\r' | b'\n')) {
            return false;
        }
        if bytes.get(at) == Some(&b'\r') {
            at += 1;
        }
        if bytes.get(at) == Some(&b'\n') {
            at += 1;
        }
        let end = line_end(text, at);
        let payload = continuation_payload(
            &text[at..end],
            prefix,
            literal_prefix,
            compose_prefix,
            timestamp_prefix,
        );
        let content = payload.trim_start_matches([' ', '\t']);
        if content.is_empty() || content.starts_with('#') {
            at = end;
            continue;
        }
        return content.len() < payload.len();
    }
}

fn continuation_payload<'a>(
    line: &'a str,
    prefix: &str,
    literal_prefix: bool,
    compose_prefix: bool,
    timestamp_prefix: bool,
) -> &'a str {
    if literal_prefix && let Some(payload) = line.strip_prefix(prefix) {
        return payload;
    }
    // Core Compose prefixes and optional Docker timestamps can otherwise hide
    // indentation/comment boundaries. Remove their formatting, not payload spaces.
    let payload = if compose_prefix {
        compose_payload(line).unwrap_or(line)
    } else {
        line
    };
    if timestamp_prefix {
        timestamp_payload(payload).unwrap_or(payload)
    } else {
        payload
    }
}

fn compose_payload(line: &str) -> Option<&str> {
    let (_, payload) = line.split_once('|')?;
    Some(payload.strip_prefix(' ').unwrap_or(payload))
}

fn timestamp_payload(line: &str) -> Option<&str> {
    let (timestamp, payload) = line.split_once(' ')?;
    let bytes = timestamp.as_bytes();
    (bytes.len() >= 19
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':'))
    .then_some(payload)
}

#[derive(Clone, Copy)]
struct Quote {
    character: u8,
    /// Delimiter backslashes after zero or more additional JSON encodings:
    /// 0, 1, 3, 7, ... . Embedded quotes have a different parity at that depth.
    escapes: usize,
}

fn encoded_quote(text: &str, start: usize) -> Option<(Quote, usize)> {
    let bytes = text.as_bytes();
    let mut at = start;
    while bytes.get(at) == Some(&b'\\') {
        at += 1;
    }
    let escapes = at - start;
    let character = bytes.get(at).copied()?;
    if !matches!(character, b'\'' | b'"') || !escapes.checked_add(1)?.is_power_of_two() {
        return None;
    }
    Some((Quote { character, escapes }, at + 1))
}

fn skip_whitespace(
    text: &str,
    mut at: usize,
    key_quote: Option<Quote>,
    header: bool,
    physical_line: bool,
) -> usize {
    let bytes = text.as_bytes();
    loop {
        if bytes.get(at).is_some_and(|b| {
            b.is_ascii_whitespace()
                && (!physical_line || !matches!(b, b'\r' | b'\n'))
                && (!header || key_quote.is_some() || matches!(b, b' ' | b'\t'))
        }) {
            at += 1;
            continue;
        }
        // An encoded physical LF/CR/tab has half the quote delimiter's escape
        // width: 1, 2, 4, ... backslashes at JSON encoding depths 1, 2, 3, ... .
        // Advance over its original bytes; never turn \n escapes into new lines.
        let Some(width) = key_quote.and_then(|quote| quote.escapes.checked_add(1)) else {
            break;
        };
        if width < 2 {
            break;
        }
        let end = at + bytes[at..].iter().take_while(|b| **b == b'\\').count();
        if end - at != width / 2 {
            break;
        }
        let escaped = &bytes[end..];
        let length = if matches!(escaped.first(), Some(b'n' | b'r' | b't')) {
            1
        } else if escaped.get(..5).is_some_and(|value| {
            [b"u0009", b"u000a", b"u000d", b"u0020"]
                .iter()
                .any(|space| value.eq_ignore_ascii_case(*space))
        }) {
            5
        } else {
            break;
        };
        at = end + length;
    }
    at
}

fn line_end(text: &str, start: usize) -> usize {
    text[start..]
        .find(['\r', '\n'])
        .map_or(text.len(), |end| start + end)
}

fn clear_trailer(text: &str, mut at: usize, mut context: Quote, containers: bool) -> bool {
    loop {
        // Inspect the actual trailer without first rescanning the rest of the
        // line for every field in a dense object.
        at = skip_whitespace(text, at, Some(context), false, true);
        if matches!(text.as_bytes().get(at), None | Some(b'\r' | b'\n')) {
            return true;
        }
        if containers && matches!(text.as_bytes()[at], b'}' | b']') {
            at += 1;
            continue;
        }
        if let Some((carrier, next)) = encoded_quote(text, at)
            && carrier.character == b'"'
            && carrier.escapes < context.escapes
        {
            at = next;
            context = carrier;
            continue;
        }
        return false;
    }
}

fn quoted_end_clear(
    text: &str,
    label: usize,
    end: usize,
    quote: Quote,
    key: Option<Quote>,
    checks: &mut usize,
    checked: &mut Option<(usize, usize, usize)>,
) -> bool {
    let Some((closer, after)) = encoded_quote(text, end) else {
        return false;
    };
    if closer.character != quote.character || closer.escapes != quote.escapes {
        return false;
    }
    context_end_clear(text, label, after, quote, key, checks, checked)
}

fn context_end_clear(
    text: &str,
    label: usize,
    after: usize,
    quote: Quote,
    key: Option<Quote>,
    checks: &mut usize,
    checked: &mut Option<(usize, usize, usize)>,
) -> bool {
    if clear_trailer(text, after, quote, false) {
        return true;
    }
    // A comma/field-like trailer can also be unescaped Digest parameters. Trust
    // adjacent fields or container trailers only inside a complete valid JSON object,
    // not merely because the next bytes resemble a key and colon.
    if key.is_none_or(|key| key.character != b'"' || key.escapes != quote.escapes) {
        return false;
    }
    if checked.is_some_and(|(start, close, depth)| {
        start < label && after <= close && depth == quote.escapes
    }) {
        return true;
    }
    for (start, _) in text[..label].rmatch_indices('{').take(*checks) {
        *checks -= 1;
        if let Some(close) = container_end(text, start, quote)
            && close >= after
            && valid_json_fragment(text, start, close, quote)
            && clear_trailer(text, close, quote, true)
        {
            *checked = Some((start, close, quote.escapes));
            return true;
        }
    }
    false
}

fn valid_json_fragment(text: &str, start: usize, end: usize, context: Quote) -> bool {
    let mut source = text[start..end].to_owned();
    for _ in 0..context.escapes.saturating_add(1).trailing_zeros() {
        let Ok(decoded) = serde_json::from_str::<String>(&format!("\"{source}\"")) else {
            return false;
        };
        source = decoded;
    }
    serde_json::from_str::<serde_json::Value>(&source).is_ok()
}

fn container_end(text: &str, start: usize, context: Quote) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut pending = vec![match bytes.get(start)? {
        b'{' => b'}',
        b'[' => b']',
        _ => return None,
    }];
    let mut at = start + 1;
    while at < bytes.len() {
        // Consume each backslash run once, even if it is malformed. Calling
        // encoded_quote at each byte of the same run would rescan it O(n²).
        let run_start = at;
        while bytes.get(at) == Some(&b'\\') {
            at += 1;
        }
        if bytes.get(at) == Some(&b'"') && at - run_start == context.escapes {
            at = quoted_end(text, at + 1, context)?.checked_add(context.escapes + 1)?;
            continue;
        }
        match *bytes.get(at)? {
            b'{' | b'[' => {
                if pending.len() == 128 {
                    return None;
                }
                pending.push(if bytes[at] == b'{' { b'}' } else { b']' });
            }
            b'}' | b']' => {
                if pending.pop()? != bytes[at] {
                    return None;
                }
                if pending.is_empty() {
                    return Some(at + 1);
                }
            }
            _ => (),
        }
        at += 1;
    }
    None
}

fn quoted_end(text: &str, start: usize, quote: Quote) -> Option<usize> {
    let period = quote.escapes.checked_add(1)?.checked_mul(2)?;
    let bytes = text.as_bytes();
    let mut at = start;
    while at < bytes.len() {
        let run_start = at;
        while bytes.get(at) == Some(&b'\\') {
            at += 1;
        }
        if matches!(bytes.get(at), None | Some(b'\r' | b'\n')) {
            return None;
        }
        let run = at - run_start;
        if bytes[at] == quote.character && run % period == quote.escapes {
            return Some(at - quote.escapes);
        }
        at += 1;
    }
    None
}

fn value_end(text: &str, start: usize, quote: Option<Quote>) -> usize {
    if let Some(quote) = quote {
        return quoted_end(text, start, quote).unwrap_or_else(|| line_end(text, start));
    }
    let mut escaped = false;
    for (index, character) in text[start..].char_indices() {
        // Do not scan for the physical line ending before finding a nearby
        // delimiter: many short Bearer tokens would repeatedly scan the suffix.
        if matches!(character, '\r' | '\n')
            || (!escaped && (character.is_whitespace() || ",;\"'".contains(character)))
        {
            return start + index;
        }
        escaped = !escaped && character == '\\';
    }
    text.len()
}
