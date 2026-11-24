//! Buffer complete bounded UTF-8 before redaction; never display partial streams.
use crate::client::CoreClient;

const MARKER: &str = "__KOMODO_EXIT_CODE:";

#[derive(serde::Serialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(super) struct Observation {
    pub end: &'static str,
    pub reported_exit_code: Option<u8>,
    pub output: Option<String>,
    pub output_truncated: bool,
    pub error_kind: Option<crate::error::ErrorKind>,
}
impl Observation {
    pub fn unknown(end: &'static str) -> Self {
        Self {
            end,
            reported_exit_code: None,
            output: None,
            output_truncated: false,
            error_kind: None,
        }
    }
}

/// Drop CSI, OSC and other ECMA-48 control strings. Incomplete or unsupported
/// escape syntax is not safe display data and invalidates the complete output.
fn controls(s: &str) -> Option<String> {
    let mut chars = s.chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        let introducer = if c == '\u{1b}' {
            Some(chars.next()?)
        } else {
            match c {
                '\u{9b}' => Some('['),
                '\u{9d}' => Some(']'),
                '\u{90}' => Some('P'),
                '\u{98}' => Some('X'),
                '\u{9e}' => Some('^'),
                '\u{9f}' => Some('_'),
                _ => None,
            }
        };
        if let Some(i) = introducer {
            match i {
                '[' => loop {
                    let ch = chars.next()?;
                    if ('@'..='~').contains(&ch) {
                        break;
                    }
                    if !(' '..='?').contains(&ch) {
                        return None;
                    }
                },
                ']' | 'P' | 'X' | '^' | '_' => loop {
                    match chars.next()? {
                        '\u{7}' if i == ']' => break,
                        '\u{9c}' => break,
                        '\u{1b}' => {
                            if chars.next()? == '\\' {
                                break;
                            }
                            return None;
                        }
                        _ => {}
                    }
                },
                // Safe fixed two-byte terminal escapes (save/restore/reset).
                '7' | '8' | 'c' | '=' | '>' => {}
                _ => return None,
            }
        } else if matches!(c, '\n' | '\t') || !c.is_control() {
            out.push(c);
        } else if c != '\r' {
            // CRLF from the PTY is normalized, never interpreted.
            return None;
        }
    }
    Some(out)
}

pub(super) fn decode(client: &CoreClient, bytes: Vec<u8>, limit: usize) -> Observation {
    let Ok(raw) = String::from_utf8(bytes) else {
        return Observation::unknown("invalid_utf8");
    };
    // Known credentials may occur inside removed escape payloads or span a
    // removed sequence. Redact on both sides of control normalization.
    let first = client.redact(&raw);
    let Some(clean) = controls(&first) else {
        return Observation::unknown("malformed_controls");
    };
    let mut markers = clean
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(MARKER));
    let marker = markers.next();
    let Some((line_no, line)) = marker else {
        return Observation::unknown("eof_without_status");
    };
    let final_line = clean
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, _)| i)
        .last();
    let code = line
        .strip_prefix(MARKER)
        .filter(|s| !s.is_empty() && s.len() <= 3 && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u8>().ok());
    if markers.next().is_some() || Some(line_no) != final_line || code.is_none() {
        return Observation::unknown("malformed_status");
    }
    let output = clean.lines().take(line_no).collect::<Vec<_>>().join("\n");
    let mut output = crate::logs::redact_log(client, &client.redact(&output));
    let truncated = output.len() > limit;
    if truncated {
        let mut end = limit;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
    }
    Observation {
        end: "upstream_reported_exit",
        reported_exit_code: code,
        output: Some(output),
        output_truncated: truncated,
        error_kind: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{client::RequestPolicy, config::Config};
    fn client() -> CoreClient {
        CoreClient::new(
            Config::parse("http://127.0.0.1:1", "fixture-key", "fixture-secret").unwrap(),
            RequestPolicy::default(),
        )
        .unwrap()
    }
    #[test]
    fn only_one_exact_final_marker_is_reported_and_output_is_redacted_before_clipping() {
        let c = client();
        let r = decode(
            &c,
            b"\x1b[31mhello\x1b[0m fixture-secret password=private\r\n__KOMODO_EXIT_CODE:7\n"
                .to_vec(),
            1000,
        );
        assert_eq!(r.reported_exit_code, Some(7));
        let output = r.output.unwrap();
        assert!(output.contains("hello"));
        assert!(
            !output.contains("fixture-secret")
                && !output.contains("private")
                && !output.contains('\u{1b}')
        );
        for text in [
            "hello",
            "__KOMODO_EXIT_CODE:0\nlate",
            "__KOMODO_EXIT_CODE:0\n__KOMODO_EXIT_CODE:1",
            "__KOMODO_EXIT_CODE:256",
            "__KOMODO_EXIT_CODE:-1",
            "x__KOMODO_EXIT_CODE:0",
            "\x1b]unfinished\n__KOMODO_EXIT_CODE:0",
        ] {
            let r = decode(&c, text.as_bytes().to_vec(), 100);
            assert!(
                r.reported_exit_code.is_none() && r.output.is_none(),
                "{text:?}"
            );
        }
        assert!(decode(&c, vec![255], 100).output.is_none());
        assert_eq!(
            decode(&c, "éé\n__KOMODO_EXIT_CODE:0".as_bytes().to_vec(), 3)
                .output
                .as_deref(),
            Some("é")
        );
    }
    #[test]
    fn control_removal_cannot_join_an_unredacted_known_credential() {
        let c = client();
        let r = decode(
            &c,
            "fixture-\u{9b}31msecret\x1b]0;title\x07\n__KOMODO_EXIT_CODE:0\n"
                .as_bytes()
                .to_vec(),
            1000,
        );
        assert!(!r.output.unwrap().contains("fixture-secret"));
    }
}
