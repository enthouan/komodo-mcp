use std::{fmt, io, io::Write, sync::Arc};

use tracing_subscriber::fmt::MakeWriter;

use crate::config::Config;

/// Defense in depth for selected upstream strings and verbose SDK diagnostics.
#[derive(Clone)]
pub(crate) struct Redactor(Arc<[String]>);

impl Redactor {
    pub(crate) fn new(key: String, secret: String) -> Self {
        let mut values = Vec::new();
        for credential in [key, secret] {
            // Debug and JSON traces can escape quotes, backslashes, and tabs.
            let debug = format!("{credential:?}");
            values.push(debug[1..debug.len() - 1].to_owned());
            let json = serde_json::to_string(&credential).expect("string is JSON serializable");
            values.push(json[1..json.len() - 1].to_owned());
            values.push(credential);
        }
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values.dedup();
        Self(values.into())
    }

    pub(crate) fn redact(&self, value: &str) -> String {
        let mut safe = String::new();
        let mut rest = value;
        while !rest.is_empty() {
            if let Some(secret) = self
                .0
                .iter()
                .find(|secret| rest.starts_with(secret.as_str()))
            {
                safe.push_str("[redacted]");
                rest = &rest[secret.len()..];
            } else {
                let character = rest.chars().next().expect("nonempty string");
                safe.push(character);
                rest = &rest[character.len_utf8()..];
            }
        }
        safe
    }
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
