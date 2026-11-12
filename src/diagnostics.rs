use std::{fmt, io, io::Write, sync::Arc};

use aho_corasick::{AhoCorasick, AhoCorasickKind, MatchKind};
use tracing_subscriber::fmt::MakeWriter;

use crate::config::Config;

pub(crate) const MAX_REDACTION_INPUT_BYTES: usize = 1024 * 1024;

/// Defense in depth for selected upstream strings and verbose SDK diagnostics.
#[derive(Clone)]
pub(crate) struct Redactor {
    matcher: Option<Arc<AhoCorasick>>,
    marker: Arc<str>,
}

impl Redactor {
    pub(crate) fn new(key: String, secret: String) -> Self {
        let mut values = Vec::new();
        for credential in [key, secret] {
            // Include every repeated JSON encoding that can fit in a bounded
            // log input, plus the Rust Debug form used by dependency traces.
            // Stop at the input budget instead of growing variants without a cap.
            let debug = format!("{credential:?}");
            for mut current in [credential, debug[1..debug.len() - 1].to_owned()] {
                loop {
                    values.push(current.clone());
                    if current.len() >= MAX_REDACTION_INPUT_BYTES {
                        break;
                    }
                    let json =
                        serde_json::to_string(&current).expect("string is JSON serializable");
                    let next = &json[1..json.len() - 1];
                    if next == current || next.len() > MAX_REDACTION_INPUT_BYTES {
                        break;
                    }
                    current = next.to_owned();
                }
            }
        }
        values.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        values.dedup();
        // Config rejects empty credentials. Ignore empties defensively so a
        // future internal caller cannot create a zero-progress replacement.
        values.retain(|value| !value.is_empty());
        let marker = replacement_marker(&values);
        // Search all encodings together, retaining earliest/longest semantics.
        // A contiguous NFA avoids DFA expansion for long escaped prefixes.
        let matcher = AhoCorasick::builder()
            .kind(Some(AhoCorasickKind::ContiguousNFA))
            .match_kind(MatchKind::LeftmostLongest)
            .build(&values)
            .ok()
            .map(Arc::new);
        Self {
            matcher,
            marker: marker.into(),
        }
    }

    pub(crate) fn marker(&self) -> &str {
        &self.marker
    }

    pub(crate) fn redact(&self, value: &str) -> String {
        if value.is_empty() {
            return String::new();
        }
        let Some(matcher) = &self.matcher else {
            // An exceptional matcher build failure must never expose input.
            return self.marker.to_string();
        };
        let mut safe = String::new();
        if matcher
            .try_replace_all_with(value, &mut safe, |_, _, dst| {
                dst.push_str(&self.marker);
                true
            })
            .is_err()
        {
            return self.marker.to_string();
        }
        safe
    }
}

fn replacement_marker(values: &[String]) -> String {
    // Delimiters absent from every raw/escaped credential prevent a new match
    // crossing either boundary of the replacement. Its interior must not
    // contain a whole credential either. No special prefix-skipping is needed:
    // a credential containing a literal marker still gets matched in full.
    if !values.iter().any(|value| value.contains(['[', ']'])) {
        for marker in ["[redacted]", "[hidden]"] {
            if !values.iter().any(|value| marker.contains(value)) {
                return marker.into();
            }
        }
    }
    for character in ['*', '#', '~', '^', '|'] {
        if values.iter().all(|value| !value.contains(character)) {
            return character.to_string().repeat(3);
        }
    }
    // A repeated absent scalar has the same boundary guarantee. Private-use
    // Unicode supplies a sentinel in serialized JSON when punctuation is
    // exhausted. U+001F is a final, guaranteed-absent fallback because Config
    // validates HTTP credentials and disallows that control byte; escaped
    // credential forms do not contain the literal control character either.
    let character = (0xE000..=0xF8FF)
        .chain(std::iter::once(0x001F))
        .filter_map(char::from_u32)
        .find(|character| values.iter().all(|value| !value.contains(*character)))
        .expect("validated credential headers exclude U+001F");
    character.to_string().repeat(3)
}

impl fmt::Debug for Redactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Redactor([redacted])")
    }
}

/// Wrap a tracing writer so even trace fields and dependency logs are redacted.
/// Each event is buffered until Drop, including credentials split across writes.
pub struct SafeDiagnostics<W> {
    writer: W,
    redactor: Redactor,
}

impl<W> SafeDiagnostics<W> {
    pub fn new(config: &Config, writer: W) -> Self {
        Self {
            writer,
            redactor: config.redactor(),
        }
    }
}

impl<'a, W: MakeWriter<'a>> MakeWriter<'a> for SafeDiagnostics<W> {
    type Writer = RedactingWriter<W::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter {
            writer: self.writer.make_writer(),
            buffer: Vec::new(),
            redactor: self.redactor.clone(),
        }
    }
}

#[doc(hidden)]
pub struct RedactingWriter<W: Write> {
    writer: W,
    buffer: Vec<u8>,
    redactor: Redactor,
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<W: Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        let safe = self.redactor.redact(&String::from_utf8_lossy(&self.buffer));
        let _ = self.writer.write_all(safe.as_bytes());
        let _ = self.writer.flush();
    }
}
