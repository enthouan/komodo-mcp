//! Check the MCP listener without forwarding operator or Core credentials.
use std::{
    env,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use komodo_mcp::config::HttpConfig;
use reqwest::{Client, StatusCode, header::WWW_AUTHENTICATE, redirect::Policy};

pub async fn check() -> bool {
    let configured = match env::var("KOMODO_HTTP_BIND") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => return false,
    };
    let Ok(address) = HttpConfig::parse_bind(configured.as_deref()) else {
        return false;
    };
    probe(address).await
}

fn local_target(mut address: SocketAddr) -> SocketAddr {
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            IpAddr::V6(Ipv6Addr::LOCALHOST)
        });
    }
    address
}

async fn probe(address: SocketAddr) -> bool {
    let Ok(client) = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    let Ok(response) = client
        .get(format!("http://{}/mcp", local_target(address)))
        .send()
        .await
    else {
        return false;
    };
    response.status() == StatusCode::UNAUTHORIZED
        && response
            .headers()
            .get(WWW_AUTHENTICATE)
            .is_some_and(|h| h.as_bytes().eq_ignore_ascii_case(b"Bearer"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, http::header, routing::get};
    use tokio::net::TcpListener;

    #[test]
    fn wildcard_binds_use_the_same_family_loopback() {
        for (bind, target) in [
            ("0.0.0.0:3000", "127.0.0.1:3000"),
            ("[::]:8900", "[::1]:8900"),
            ("127.0.0.2:6", "127.0.0.2:6"),
        ] {
            assert_eq!(local_target(bind.parse().unwrap()), target.parse().unwrap());
        }
    }

    #[tokio::test]
    async fn accepts_only_the_local_bearer_challenge_and_rejects_redirects() {
        for (status, challenge, expected) in [
            (StatusCode::UNAUTHORIZED, "Bearer", true),
            (StatusCode::OK, "Bearer", false),
            (StatusCode::UNAUTHORIZED, "Basic", false),
            (StatusCode::TEMPORARY_REDIRECT, "Bearer", false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let app = Router::new()
                .route(
                    "/mcp",
                    get(move |headers: axum::http::HeaderMap| async move {
                        assert!(!headers.contains_key(header::AUTHORIZATION));
                        assert!(!headers.contains_key("x-api-key"));
                        (
                            status,
                            [
                                (header::WWW_AUTHENTICATE, challenge),
                                (header::LOCATION, "/other"),
                            ],
                        )
                    }),
                )
                .route(
                    "/other",
                    get(|| async {
                        (
                            StatusCode::UNAUTHORIZED,
                            [(header::WWW_AUTHENTICATE, "Bearer")],
                        )
                    }),
                );
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            assert_eq!(probe(address).await, expected);
            task.abort();
        }
    }
}
