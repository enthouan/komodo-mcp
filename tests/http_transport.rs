#[path = "support/http.rs"]
mod http;
mod support;
#[path = "support/variables.rs"]
mod variables;

use std::{
    net::SocketAddr,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use http::{
    DEADLINE, FIRST, Http, INVALID, ORIGIN, PRIVATE_ARGUMENT, SECOND, Server, assert_redacted,
    initialize, post, rejected,
};
use komodo_mcp::{
    capabilities::CapabilityPolicy,
    client::{CoreClient, RequestPolicy},
    config::Config,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    KEY, SECRET,
    operations::{Core, DEPLOYMENT, Options, Outcome, STACK},
    recovery::{FaultProxy, LostReply},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    time::timeout,
};

fn client(address: &str, operate: bool, edit: bool) -> CoreClient {
    CoreClient::new(
        Config::parse(address, KEY, SECRET)
            .unwrap()
            .with_capabilities(CapabilityPolicy::new(operate, edit)),
        RequestPolicy {
            connect_timeout: Duration::from_secs(1),
            operation_timeout: Duration::from_secs(3),
        },
    )
    .unwrap()
}

fn data(response: &Value) -> &Value {
    assert_eq!(response["result"]["isError"], false);
    &response["result"]["structuredContent"]
}

#[tokio::test]
async fn browser_preflight_and_session_headers_are_available_only_to_an_allowed_origin() {
    let core = Core::start(Options::default()).await;
    let server = Server::start(core.client(false, DEADLINE)).await;
    let http = &server.http;
    let baseline = core.requests().len();
    let allowed = http.preflight(ORIGIN).send().await.unwrap();
    assert!(allowed.status().is_success());
    assert_eq!(allowed.headers()["access-control-allow-origin"], ORIGIN);
    let includes = |headers: &reqwest::header::HeaderMap, name: &str, item: &str| {
        headers
            .get_all(name)
            .iter()
            .flat_map(|v| v.to_str().unwrap().split(','))
            .any(|v| v.trim().eq_ignore_ascii_case(item))
    };
    assert!(includes(
        allowed.headers(),
        "access-control-allow-methods",
        "POST"
    ));
    for header in [
        "authorization",
        "content-type",
        "mcp-protocol-version",
        "mcp-session-id",
    ] {
        assert!(includes(
            allowed.headers(),
            "access-control-allow-headers",
            header
        ));
    }
    let forbidden = http
        .preflight("https://other.example.test")
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    assert!(
        !forbidden
            .headers()
            .contains_key("access-control-allow-origin")
    );
    assert_eq!(core.requests().len(), baseline);
    let start = post(
        http.request(Method::POST, Some(FIRST), None)
            .header("origin", ORIGIN),
        initialize(),
    )
    .send()
    .await
    .unwrap();
    assert!(start.status().is_success());
    assert_eq!(start.headers()["access-control-allow-origin"], ORIGIN);
    assert!(includes(
        start.headers(),
        "access-control-expose-headers",
        "mcp-session-id"
    ));
    assert!(start.headers().contains_key("mcp-session-id"));
    drop(start);
    server.finish().await;
}

#[tokio::test]
async fn authenticated_discovery_read_and_both_permission_boundaries() {
    let core = Core::start(Options {
        history_status: Some(403),
        ..Default::default()
    })
    .await;
    let server = Server::start(core.client(false, DEADLINE)).await;
    let http = &server.http;
    let id = http.session(FIRST, None).await;
    let catalog = http.rpc(FIRST, &id, "tools/list", json!({})).await;
    let tools = catalog["result"]["tools"].as_array().unwrap();
    let listed = tools
        .iter()
        .find(|tool| tool["name"] == "list_stacks")
        .unwrap();
    assert_eq!(listed["annotations"]["readOnlyHint"], true);
    assert!(listed["inputSchema"].is_object() && listed["outputSchema"].is_object());
    assert!(tools.iter().all(|tool| tool["name"] != "deploy_stack"));

    let stacks = http.call(FIRST, &id, "list_stacks", json!({})).await;
    assert_eq!(data(&stacks)["items"][0]["id"], STACK);
    let before = core.requests().len();
    let disabled = http
        .call(
            FIRST,
            &id,
            "deploy_stack",
            json!({"target":{"id":STACK},"scope":{"kind":"whole_stack"}}),
        )
        .await;
    assert!(
        disabled["error"]["code"].is_number(),
        "a locally disabled call must fail at dispatch"
    );
    assert_eq!(core.requests().len(), before);
    let denied = http.call(FIRST, &id, "list_updates", json!({})).await;
    assert_eq!(denied["result"]["isError"], true);
    assert_eq!(
        denied["result"]["structuredContent"]["kind"],
        "permission_denied"
    );
    let requests = core.requests();
    assert_eq!(requests.len(), before + 1);
    assert_eq!(requests.last().unwrap().operation, "ListUpdates");
    assert_eq!(core.executions(), 0);
    core.assert_credentials();
    server.finish().await;
}

#[tokio::test]
async fn all_session_routes_enforce_credentials_origins_and_session_ownership_before_core() {
    let core = Core::start(Options::default()).await;
    let server = Server::start(core.client(false, DEADLINE)).await;
    let http = &server.http;
    let baseline = core.requests().len();
    for token in [None, Some(INVALID), Some(KEY)] {
        rejected(
            post(http.request(Method::POST, token, None), initialize()),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    let init = || http.request(Method::POST, Some(FIRST), None);
    let bearer = |value: String| {
        post(
            http.request(Method::POST, None, None)
                .header("authorization", value),
            initialize(),
        )
    };
    rejected(bearer(format!("Bearer\t{FIRST}")), StatusCode::UNAUTHORIZED).await;
    let accepted = bearer(format!("bEaReR   {FIRST}")).send().await.unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    assert!(accepted.headers().contains_key("mcp-session-id"));
    drop(accepted);
    rejected(
        post(init().bearer_auth(SECOND), initialize()),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    rejected(
        post(
            init().header("origin", "https://other.example.test"),
            initialize(),
        ),
        StatusCode::FORBIDDEN,
    )
    .await;
    rejected(
        post(
            init().header("origin", ORIGIN).header("origin", ORIGIN),
            initialize(),
        ),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(core.requests().len(), baseline);
    let owner = http.session(FIRST, Some(ORIGIN)).await;
    let visitor = http.session(SECOND, None).await;
    assert_ne!(owner, visitor);
    let after_init = core.requests().len();
    for (method, resume) in [
        (Method::POST, false),
        (Method::GET, false),
        (Method::GET, true),
        (Method::DELETE, false),
    ] {
        let route = |token: Option<&str>| {
            let mut request = http.request(method.clone(), token, Some(&owner));
            if resume {
                request = request.header("last-event-id", "0");
            }
            if method == Method::POST {
                request = post(
                    request,
                    json!({"jsonrpc":"2.0","id":44,"method":"tools/call","params":{"name":"list_stacks","arguments":{}}}),
                );
            }
            request
        };
        rejected(route(None), StatusCode::UNAUTHORIZED).await;
        rejected(route(Some(INVALID)), StatusCode::UNAUTHORIZED).await;
        rejected(
            route(Some(FIRST)).header("origin", "https://other.example.test"),
            StatusCode::FORBIDDEN,
        )
        .await;
        rejected(route(Some(SECOND)), StatusCode::NOT_FOUND).await;
    }
    assert_eq!(core.requests().len(), after_init);
    // Foreign DELETE, rejected streaming and rejected resumption must leave the owner's session usable.
    let result = http.call(FIRST, &owner, "list_stacks", json!({})).await;
    assert_eq!(data(&result)["items"][0]["id"], STACK);
    let mut stream = http
        .request(Method::GET, Some(FIRST), Some(&owner))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    assert!(
        stream.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    // The SDK emits a priming event carrying the initial cursor on the standalone stream.
    let mut priming = Vec::new();
    while !priming.ends_with(b"\n\n") && !priming.ends_with(b"\r\n\r\n") {
        priming.extend(
            stream
                .chunk()
                .await
                .unwrap()
                .expect("standalone SSE priming event"),
        );
        assert!(priming.len() < 4096);
    }
    let cursor = std::str::from_utf8(&priming)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("id:"))
        .expect("SSE event ID")
        .trim();
    drop(stream);
    let resumed = http
        .request(Method::GET, Some(FIRST), Some(&owner))
        .header("last-event-id", cursor)
        .send()
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    drop(resumed);
    let del = http
        .request(Method::DELETE, Some(FIRST), Some(&owner))
        .send()
        .await
        .unwrap();
    assert!(del.status().is_success());
    let count = core.requests().len();
    rejected(
        http.request(Method::GET, Some(FIRST), Some(&owner)),
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(core.requests().len(), count);
    assert!(
        http.rpc(SECOND, &visitor, "tools/list", json!({})).await["result"]["tools"].is_array()
    );
    core.assert_credentials();
    server.finish().await;
}

#[tokio::test]
async fn session_capacity_is_reserved_per_credential_and_released_on_owned_delete() {
    let core = Core::start(Options::default()).await;
    let server = Server::start(core.client(false, DEADLINE)).await;
    let http = &server.http;
    let mut first = None;
    let baseline = core.requests().len();
    for _ in 0..16 {
        let id = http.session(FIRST, None).await;
        first.get_or_insert(id);
    }
    let initialize_next = || post(http.request(Method::POST, Some(FIRST), None), initialize());
    rejected(initialize_next(), StatusCode::TOO_MANY_REQUESTS).await;
    http.session(SECOND, None).await;
    let id = first.as_deref().unwrap();
    let removed = http
        .request(Method::DELETE, Some(FIRST), Some(id))
        .send()
        .await
        .unwrap();
    assert!(removed.status().is_success());
    timeout(DEADLINE, async {
        loop {
            let response = initialize_next().send().await.unwrap();
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers().contains_key("mcp-session-id"));
            break;
        }
    })
    .await
    .expect("an owned DELETE must release its session slot");
    assert_eq!(core.requests().len(), baseline);
    server.finish().await;
}

#[tokio::test]
async fn legacy_sessions_keep_private_previews_separate() {
    let core = variables::Core::start().await;
    let server = Server::start(client(&core.address, false, true)).await;
    let http = &server.http;
    let owner = http.session(FIRST, None).await;
    let other_session = http.session(FIRST, None).await;
    let other_caller = http.session(SECOND, None).await;
    let change = json!({"kind":"set_description","name":"APP_MODE","description":"reviewed fixture metadata"});
    let preview = http
        .call(
            FIRST,
            &owner,
            "preview_variable_change",
            json!({"change":change}),
        )
        .await;
    let args =
        json!({"change":change, "preview_id":data(&preview)["preview_id"].as_str().unwrap()});
    for (token, id) in [(SECOND, other_caller), (FIRST, other_session)] {
        let rejected = http
            .call(token, &id, "apply_variable_change", args.clone())
            .await;
        assert_eq!(
            rejected["result"]["structuredContent"]["kind"],
            "invalid_preview"
        );
    }
    assert!(core.writes().is_empty());
    let saved = http
        .call(FIRST, &owner, "apply_variable_change", args)
        .await;
    assert_eq!(data(&saved)["outcome"], "saved");
    assert_eq!(core.writes().len(), 1);
    assert!(!core.data.lock().unwrap().bad_credentials);
    server.finish().await;
}

#[tokio::test]
async fn stateless_requests_can_apply_only_their_own_bearers_review() {
    let core = variables::Core::start().await;
    let server = Server::start(client(&core.address, false, true)).await;
    let http = &server.http;
    let change = json!({"kind":"set_description","name":"APP_MODE","description":"stateless reviewed metadata"});
    let preview = http
        .stateless_call(FIRST, "preview_variable_change", json!({"change":change}))
        .await;
    let args = json!({"change":change,"preview_id":data(&preview)["preview_id"].as_str().unwrap()});
    let foreign = http
        .stateless_call(SECOND, "apply_variable_change", args.clone())
        .await;
    assert_eq!(
        foreign["result"]["structuredContent"]["kind"],
        "invalid_preview"
    );
    assert!(core.writes().is_empty());
    let saved = http
        .stateless_call(FIRST, "apply_variable_change", args)
        .await;
    assert_eq!(data(&saved)["outcome"], "saved");
    assert_eq!(core.writes().len(), 1);
    assert!(!core.data.lock().unwrap().bad_credentials);
    server.finish().await;
}

#[tokio::test]
async fn reconnect_after_disconnect_reconciles_the_accepted_operation_without_replay() {
    let core = Core::start(Options {
        outcome: Outcome::Succeeded,
        ..Default::default()
    })
    .await;
    let proxy = FaultProxy::start(&core, LostReply::Timeout).await;
    let server = Server::start(client(&proxy.address, true, false)).await;
    let http = &server.http;
    let session = http.session(FIRST, None).await;
    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    };
    let started = now();
    let task = tokio::spawn({
        let http = http.clone();
        async move {
            http.call(
                FIRST,
                &session,
                "start_deployment",
                json!({"id":DEPLOYMENT}),
            )
            .await
        }
    });
    proxy.accepted(1).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(core.executions(), 1);
    let fresh = http.session(FIRST, None).await;
    let recovery = http.call(FIRST, &fresh, "reconcile_operation", json!({
        "target":{"kind":"deployment","id":DEPLOYMENT}, "operation":"start",
        "requested_scope":{"kind":"whole_deployment"}, "submission_window":{"started_ts":started,"ended_ts":now()}
    })).await;
    let recovered = data(&recovery);
    assert_eq!(recovered["attribution"], "unconfirmed");
    assert_eq!(recovered["automatic_resubmission"], false);
    let candidates = recovered["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    let observed = http
        .call(
            FIRST,
            &fresh,
            "get_update",
            json!({"id":candidates[0]["update"]["id"]}),
        )
        .await;
    assert_eq!(data(&observed)["outcome"], "succeeded");
    assert_eq!(core.executions(), 1);
    core.assert_credentials();
    server.finish().await;
}

#[tokio::test]
async fn actual_http_binary_accepts_native_client_and_redacts_diagnostics() {
    let core = Core::start(Options::default()).await;
    let mut child = Command::new(env!("CARGO_BIN_EXE_komodo-mcp"))
        .env_clear()
        .env("KOMODO_ADDRESS", &core.address)
        .env("KOMODO_API_KEY", KEY)
        .env("KOMODO_API_SECRET", SECRET)
        .env("KOMODO_TRANSPORT", "http")
        .env("KOMODO_HTTP_BIND", "127.0.0.1:0")
        .env("KOMODO_HTTP_TOKENS", format!("{FIRST},{SECOND}"))
        .env("KOMODO_HTTP_ALLOWED_ORIGINS", ORIGIN)
        .env("RUST_LOG", "trace")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let address = timeout(DEADLINE, async {
        while let Some(line) = lines.next_line().await.unwrap() {
            assert_redacted(&line);
            if line.contains("starting komodo-mcp over HTTP") {
                return line
                    .split_whitespace()
                    .find_map(|part| part.strip_prefix("address="))
                    .and_then(|addr| addr.parse::<SocketAddr>().ok())
                    .expect("bound address in readiness log");
            }
        }
        panic!("binary exited before listening");
    })
    .await
    .unwrap();
    let diagnostics = tokio::spawn(async move {
        while let Some(line) = lines.next_line().await.unwrap() {
            assert_redacted(&line);
        }
    });
    let http = Http::new(address);
    rejected(
        post(
            http.request(Method::POST, Some(INVALID), None),
            initialize(),
        ),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let session = http.session(FIRST, None).await;
    let invalid = http
        .call(
            FIRST,
            &session,
            "list_stacks",
            json!({"unexpected":PRIVATE_ARGUMENT}),
        )
        .await;
    assert_eq!(
        invalid["result"]["structuredContent"]["kind"],
        "invalid_arguments"
    );
    let read = http.call(FIRST, &session, "list_stacks", json!({})).await;
    assert_eq!(data(&read)["items"][0]["id"], STACK);
    core.assert_credentials();
    child.start_kill().unwrap();
    let output = timeout(DEADLINE, child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.stdout.is_empty());
    timeout(DEADLINE, diagnostics).await.unwrap().unwrap();
}
