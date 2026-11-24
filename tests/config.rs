use komodo_mcp::{config::Config, error::ErrorKind};

#[test]
fn http_startup_rejects_absent_or_reused_credentials_before_the_core_probe() {
    const CORE_SECRET: &str = "synthetic-core-secret-000000000000001";
    const CORE_KEY: &str = "synthetic-core-key-000000000000000001";
    for tokens in [None, Some(CORE_KEY), Some(CORE_SECRET)] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_komodo-mcp"));
        command
            .env_clear()
            .env("KOMODO_TRANSPORT", "http")
            .env("KOMODO_ADDRESS", "http://127.0.0.1:1")
            .env("KOMODO_API_KEY", CORE_KEY)
            .env("KOMODO_API_SECRET", CORE_SECRET);
        if let Some(tokens) = tokens {
            command.env("KOMODO_HTTP_TOKENS", tokens);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostic.contains("KOMODO_HTTP_TOKENS"));
        assert!(!diagnostic.contains(CORE_SECRET) && !diagnostic.contains(CORE_KEY));
        assert!(!diagnostic.contains("GetVersion"));
    }
}

#[test]
fn capability_configuration_is_independent_strict_and_read_only_by_default() {
    use komodo_mcp::capabilities::{Capability, CapabilityPolicy};

    assert_eq!(
        CapabilityPolicy::parse(None, None).unwrap(),
        CapabilityPolicy::default()
    );
    assert_eq!(
        Config::parse("http://localhost", "key", "secret")
            .unwrap()
            .capabilities(),
        CapabilityPolicy::default()
    );
    for operate in [false, true] {
        for edit in [false, true] {
            let policy = CapabilityPolicy::parse(
                Some(if operate { "true" } else { "false" }),
                Some(if edit { "true" } else { "false" }),
            )
            .unwrap();
            assert!(policy.allows(Capability::Read));
            assert_eq!(policy.allows(Capability::Operate), operate);
            assert_eq!(policy.allows(Capability::ConfigurationEdit), edit);
        }
    }
    for value in [
        "",
        "TRUE",
        "1",
        "yes",
        " true",
        "false ",
        "secret-configuration-marker",
    ] {
        for (operate, edit, field) in [
            (Some(value), None, "KOMODO_ALLOW_OPERATE"),
            (None, Some(value), "KOMODO_ALLOW_CONFIGURATION_EDIT"),
        ] {
            let error = CapabilityPolicy::parse(operate, edit).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Configuration);
            assert_eq!(error.field, Some(field));
            assert!(!format!("{error:?}").contains("secret-configuration-marker"));
        }
    }
}

#[test]
fn addresses_and_credentials_are_validated_without_values_in_errors() {
    for address in [
        "",
        "relative",
        "http:core.test",
        "http:///core.test",
        " https://core.test",
        "https://core.test\n",
        "https://core.test\\@other.test",
        "ftp://core.test",
        "https://key:secret@core.test",
        "https://@core.test",
        "https://core.test?query",
        "https://core.test#fragment",
    ] {
        let error = Config::parse(address, "synthetic-key", "synthetic-secret").unwrap_err();
        assert_eq!(error.kind, ErrorKind::Configuration);
        assert_eq!(error.field, Some("KOMODO_ADDRESS"));
        assert!(!format!("{error:#?}").contains("synthetic"));
    }
    for (key, secret, field) in [
        ("", "synthetic-secret", "KOMODO_API_KEY"),
        ("   ", "secret", "KOMODO_API_KEY"),
        ("synthetic-key", "", "KOMODO_API_SECRET"),
        ("bad\r\nkey", "secret", "KOMODO_API_KEY"),
        ("key", "bad\0secret", "KOMODO_API_SECRET"),
    ] {
        let error = Config::parse("http://localhost:9120/prefix/", key, secret).unwrap_err();
        assert_eq!(error.field, Some(field));
        assert!(!format!("{error:#}").contains("synthetic"));
    }
    for address in [
        "http://localhost:9120/prefix/",
        "https://core.test",
        "https://core.test/komodo",
    ] {
        let config = Config::parse(address, "synthetic-key", "synthetic-secret").unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("synthetic"));
        assert!(!debug.contains(address));
    }
}
