mod support;

use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use komodo_mcp::{config::Config, diagnostics::SafeDiagnostics};
use support::{KEY, SECRET, assert_safe};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn verbose_fields_and_fragmented_writes_are_redacted() {
    let config = Config::parse("http://localhost:9120", KEY, SECRET).unwrap();
    let capture = Capture::default();
    let writer = SafeDiagnostics::new(&config, capture.clone());
    {
        let mut event = writer.make_writer();
        event.write_all(&KEY.as_bytes()[..8]).unwrap();
        event.flush().unwrap();
        event.write_all(&KEY.as_bytes()[8..]).unwrap();
        event.write_all(SECRET.as_bytes()).unwrap();
    }
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(writer)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        tracing::trace!(
            upstream_error = KEY,
            trace = SECRET,
            "synthetic upstream trace fields"
        );
    });
    let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert_safe(&output);
    assert!(output.contains("synthetic upstream trace fields"));
    assert!(output.contains("[redacted]"));
}

#[test]
fn escaped_and_overlapping_credentials_are_redacted() {
    let key = "synthetic-key";
    let secret = "synthetic-key-\"quoted\\secret\tend";
    let config = Config::parse("http://localhost:9120", key, secret).unwrap();
    let capture = Capture::default();
    let writer = SafeDiagnostics::new(&config, capture.clone());
    {
        let mut event = writer.make_writer();
        write!(
            event,
            "{key} {secret} {secret:?} {}",
            serde_json::to_string(secret).unwrap()
        )
        .unwrap();
    }
    let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(!output.contains(key));
    assert!(!output.contains("quoted"));
    assert!(!output.contains("secret"));
}
