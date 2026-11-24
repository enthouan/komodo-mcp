use axum::{
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use super::single_header;

fn allowed_request_header(name: &str) -> bool {
    let Ok(name) = HeaderName::from_bytes(name.trim().as_bytes()) else {
        return false;
    };
    matches!(
        name.as_str(),
        "accept"
            | "authorization"
            | "content-type"
            | "last-event-id"
            | "mcp-method"
            | "mcp-name"
            | "mcp-protocol-version"
            | "mcp-session-id"
    ) || name
        .as_str()
        .strip_prefix("mcp-param-")
        .is_some_and(|suffix| !suffix.is_empty())
}

pub(super) fn preflight(headers: &HeaderMap, allowed_origin: bool) -> Response {
    let method = single_header(headers, header::ACCESS_CONTROL_REQUEST_METHOD.as_str());
    if !allowed_origin || !matches!(method, Ok(Some("GET" | "POST" | "DELETE"))) {
        return (StatusCode::FORBIDDEN, "Preflight is not allowed").into_response();
    }
    let requested = single_header(headers, header::ACCESS_CONTROL_REQUEST_HEADERS.as_str());
    match requested {
        Ok(None) => (),
        Ok(Some(names)) if names.split(',').all(allowed_request_header) => (),
        _ => return (StatusCode::FORBIDDEN, "Preflight headers are not allowed").into_response(),
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    let reply = response.headers_mut();
    reply.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, DELETE"),
    );
    if let Some(names) = headers.get(header::ACCESS_CONTROL_REQUEST_HEADERS) {
        reply.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, names.clone());
    }
    response
}

pub(super) fn set_response_headers(headers: &mut HeaderMap, origin: Option<HeaderValue>) {
    headers.append(
        header::VARY,
        HeaderValue::from_static(
            "Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
        ),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(origin) = origin {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
        headers.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("Mcp-Session-Id, MCP-Protocol-Version, WWW-Authenticate"),
        );
    }
}
