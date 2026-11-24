use std::{fmt, net::SocketAddr};

use reqwest::Url;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::AppError;

const TOKENS: &str = "KOMODO_HTTP_TOKENS";
const ORIGINS: &str = "KOMODO_HTTP_ALLOWED_ORIGINS";

#[derive(Clone)]
pub struct HttpConfig {
    bind: SocketAddr,
    token_digests: Vec<[u8; 32]>,
    origins: Vec<String>,
}

impl HttpConfig {
    pub fn parse(
        bind: Option<&str>,
        tokens: &str,
        origins: Option<&str>,
    ) -> Result<Self, AppError> {
        let bind = bind.unwrap_or("127.0.0.1:3000").parse().map_err(|_| {
            AppError::configuration("KOMODO_HTTP_BIND", "use an IP address and port")
        })?;
        let mut token_digests = Vec::new();
        for token in tokens.split(',').map(str::trim) {
            if !valid_bearer(token) {
                return Err(AppError::configuration(
                    TOKENS,
                    "supply distinct 32–512 character bearer tokens, separated by commas",
                ));
            }
            let digest = Sha256::digest(token.as_bytes()).into();
            if token_digests.contains(&digest) || token_digests.len() >= 32 {
                return Err(AppError::configuration(
                    TOKENS,
                    "use at most 32 distinct bearer tokens",
                ));
            }
            token_digests.push(digest);
        }
        let mut allowed = Vec::new();
        if let Some(origins) = origins {
            for entry in origins.split(',').map(str::trim) {
                let origin = canonical_origin(entry).ok_or_else(|| {
                    AppError::configuration(
                        ORIGINS,
                        "use exact HTTP or HTTPS origins with an optional port and no path",
                    )
                })?;
                if !allowed.contains(&origin) {
                    allowed.push(origin);
                }
            }
        }
        Ok(Self {
            bind,
            token_digests,
            origins: allowed,
        })
    }

    pub fn bind(&self) -> SocketAddr {
        self.bind
    }

    pub(crate) fn principal_count(&self) -> usize {
        self.token_digests.len()
    }

    /// A matching index identifies a configured transport principal for this process lifetime.
    pub(crate) fn authenticate(&self, bearer: &str) -> Option<usize> {
        if !valid_bearer(bearer) {
            return None;
        }
        let digest: [u8; 32] = Sha256::digest(bearer.as_bytes()).into();
        self.token_digests
            .iter()
            .position(|candidate| bool::from(candidate.ct_eq(&digest)))
    }

    pub(crate) fn allows_origin(&self, raw: &str) -> bool {
        canonical_origin(raw).is_some_and(|origin| self.origins.contains(&origin))
    }

    pub(crate) fn allowed_origins(&self) -> &[String] {
        &self.origins
    }
}

// RFC 6750 b64token. Padding is allowed only at the end.
fn valid_bearer(token: &str) -> bool {
    if !(32..=512).contains(&token.len()) {
        return false;
    }
    let data = token.trim_end_matches('=');
    data.len() >= 32
        && data
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/".contains(&b))
}

// Reject malformed wire origins before URL normalization can turn a path or userinfo
// into an apparently trusted origin. Url handles IPv6, host case and default ports.
fn canonical_origin(raw: &str) -> Option<String> {
    let (_, authority) = raw.split_once("://")?;
    if !raw.is_ascii()
        || raw
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        || authority.is_empty()
        || authority.ends_with(':')
        || authority.contains(['/', '?', '#', '@', '\\', '*', '%'])
    {
        return None;
    }
    let parsed = Url::parse(raw).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return None;
    }
    Some(parsed.origin().ascii_serialization())
}

impl fmt::Debug for HttpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpConfig")
            .field("bind", &self.bind)
            .field("principals", &self.principal_count())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = "synthetic-operator-token-00000001";
    const SECOND: &str = "synthetic-operator-token-00000002";

    #[test]
    fn separately_configured_tokens_identify_distinct_principals() {
        let cfg = HttpConfig::parse(None, &format!("{FIRST}, {SECOND}"), None).unwrap();
        assert!(cfg.bind().ip().is_loopback());
        assert_eq!(cfg.authenticate(FIRST), Some(0));
        assert_eq!(cfg.authenticate(SECOND), Some(1));
        assert_eq!(cfg.authenticate("synthetic-operator-token-00000003"), None);
        assert!(!format!("{cfg:?}").contains("synthetic"));
        for invalid in [
            "",
            "short",
            &format!("{FIRST},"),
            &format!("{FIRST},{FIRST}"),
            &format!("{FIRST}=a"),
        ] {
            assert!(HttpConfig::parse(None, invalid, None).is_err());
        }
    }

    #[test]
    fn origins_match_scheme_host_and_effective_port() {
        let cfg = HttpConfig::parse(
            None,
            FIRST,
            Some("https://client.example:443, http://[::1]:9000"),
        )
        .unwrap();
        assert!(cfg.allows_origin("https://client.example"));
        assert!(cfg.allows_origin("http://[::1]:9000"));
        for bad in [
            "http://client.example",
            "https://client.example:444",
            "https://client.example.evil.test",
            "https://client.example/",
            "https://@client.example",
            "https://client.example?q",
            "null",
            "*",
            "https://client.example https://client.example",
        ] {
            assert!(!cfg.allows_origin(bad));
        }
        assert!(
            !HttpConfig::parse(None, FIRST, None)
                .unwrap()
                .allows_origin("https://client.example")
        );
        assert!(HttpConfig::parse(None, FIRST, Some("")).is_err());
        assert!(HttpConfig::parse(None, FIRST, Some("https://*.example")).is_err());
    }
}
