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

fn redact_fragments(config: &Config, value: &str) -> String {
    let capture = Capture::default();
    let writer = SafeDiagnostics::new(config, capture.clone());
    {
        let mut event = writer.make_writer();
        for fragment in value.as_bytes().chunks(3) {
            event.write_all(fragment).unwrap();
            event.flush().unwrap();
        }
    }
    String::from_utf8(capture.0.lock().unwrap().clone()).unwrap()
}

#[test]
fn repeated_redaction_is_stable_for_short_and_marker_containing_credentials() {
    for (key, secret) in [
        ("red", "another-credential"),
        ("[redacted]suffix", "another-credential"),
        ("[redacted]", "[hidden]"),
        ("d", "e"),
        ("[*#~^|]", "prefix[redacted]suffix"),
    ] {
        let config = Config::parse("http://localhost:9120", key, secret).unwrap();
        let raw = format!("key={key} secret={secret} {secret:?}");
        let once = redact_fragments(&config, &raw);
        let twice = redact_fragments(&config, &once);
        assert!(!once.contains(key), "raw key survived first projection");
        assert!(
            !once.contains(secret),
            "raw secret survived first projection"
        );
        assert_eq!(once, twice, "second projection changed byte bounds");
    }
}

#[test]
fn replacement_cannot_synthesize_a_credential_across_its_boundaries() {
    // A naive replacement of x with [redacted] would create the other complete
    // credential. It would leak on the first pass and change on the second.
    let key = "x";
    let secret = "before[redacted]after";
    let config = Config::parse("http://localhost:9120", key, secret).unwrap();
    let once = redact_fragments(&config, "beforexafter");
    assert!(!once.contains(key));
    assert!(!once.contains(secret));
    assert_eq!(once, redact_fragments(&config, &once));
}

#[test]
fn exhausted_ascii_markers_use_a_nonempty_stable_unicode_sentinel() {
    let key = "[*#~^|]";
    let secret = "prefix[redacted]#[hidden]~^|suffix";
    let config = Config::parse("http://localhost:9120", key, secret).unwrap();
    let once = redact_fragments(&config, &format!("{key}{secret}"));
    assert!(!once.is_empty());
    assert!(!once.is_ascii());
    assert!(!once.contains(key));
    assert!(!once.contains(secret));
    assert_eq!(once, redact_fragments(&config, &once));
}

#[test]
fn repeatedly_encoded_configured_credentials_are_redacted_without_losing_context() {
    let key = "diagnostic-\"key\\suffix";
    let secret = "diagnostic-\\\"secret\\\\finish";
    let config = Config::parse("http://localhost:9120", key, secret).unwrap();
    let mut source = format!("before {key} {secret} after");
    for _ in 0..=12 {
        let output = redact_fragments(&config, &source);
        assert!(!output.contains("diagnostic-"));
        assert!(output.contains("before") && output.contains("after"));
        assert_eq!(output, redact_fragments(&config, &output));
        let encoded = serde_json::to_string(&source).unwrap();
        source = encoded[1..encoded.len() - 1].to_owned();
    }
}
