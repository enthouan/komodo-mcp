use komodo_mcp::{config::Config, error::ErrorKind};

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
