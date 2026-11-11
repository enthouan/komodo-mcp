use komodo_mcp::{config::Config, error::ErrorKind};

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
