//! Production-executable stdio acceptance of bounded read-only Docker tools.
#[path = "support/docker.rs"]
mod fixture;
mod support;
use fixture::*;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use support::operations::{Process, Session, call};
const TOOLS: [&str; 7] = [
    "list_images",
    "get_image",
    "get_image_history",
    "list_networks",
    "get_network",
    "list_volumes",
    "get_volume",
];
fn safe(r: &CallToolResult) {
    let text = serde_json::to_string(r).unwrap();
    support::assert_safe(&text);
    for value in PRIVATE {
        assert!(
            !text.contains(value),
            "unsafe Docker metadata leaked: {value}"
        );
    }
}
fn data(r: CallToolResult) -> Value {
    safe(&r);
    assert_eq!(r.is_error, Some(false), "{r:?}");
    r.structured_content.unwrap()
}
fn error(r: CallToolResult) -> Value {
    safe(&r);
    assert_eq!(r.is_error, Some(true), "{r:?}");
    r.structured_content.unwrap()
}
fn target() -> Value {
    json!({"target":{"id":SERVER}})
}
fn object(tool: &str) -> Value {
    json!({"target":{"id":SERVER},"object":match tool { "get_image"|"get_image_history"=>json!({"id":IMAGE}),"get_network"=>json!({"id":NETWORK}),_=>json!({"name":"fixture-volume"}) }})
}
async fn finish(p: Process, s: Session, c: &Core) {
    c.assert_readonly();
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn discovery_and_direct_calls_are_readonly_in_every_capability_profile() {
    for operate in [false, true] {
        for edit in [false, true] {
            let c = Core::start().await;
            let (p, s) = Process::start_profile(&c.address, operate, edit).await;
            let tools = s.list_tools(None).await.unwrap().tools;
            for name in TOOLS {
                let tool = tools.iter().find(|t| t.name == name).unwrap();
                assert_eq!(
                    tool.annotations.as_ref().unwrap().read_only_hint,
                    Some(true)
                );
                let reads = c.requests("read").len();
                error(call(&s, name, json!({"unexpected":true})).await);
                assert_eq!(c.requests("read").len(), reads);
                data(
                    call(
                        &s,
                        name,
                        if name.starts_with("list_") {
                            target()
                        } else {
                            object(name)
                        },
                    )
                    .await,
                );
            }
            finish(p, s, &c).await;
        }
    }
}
#[tokio::test]
async fn exact_cached_lists_and_fresh_details_project_only_safe_operational_metadata() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    for name in ["list_images", "list_networks", "list_volumes"] {
        let v = data(call(&s, name, target()).await);
        assert_eq!(v["source"], "core_cache");
        assert_eq!(v["server"]["id"], SERVER);
        assert_eq!(v["data"]["observed_count"], 1);
        assert_eq!(v["data"]["scan_incomplete"], false);
        assert!(
            v["evidence_limit"]
                .as_str()
                .unwrap()
                .contains("not an authoritative prune dry run")
        );
    }
    let image = data(
        call(
            &s,
            "get_image",
            json!({"target":{"name":"fixture-server"},"object":{"name":"fixture/web:latest"}}),
        )
        .await,
    );
    assert_eq!(image["source"], "fresh_inspect");
    assert_eq!(image["data"]["id"], IMAGE);
    assert_eq!(image["data"]["architecture"], "amd64");
    assert_eq!(image["data"]["references_incomplete"], true);
    assert_eq!(image["data"]["tags"], json!(["fixture/web:latest"]));
    let history = data(call(&s, "get_image_history", object("get_image_history")).await);
    assert_eq!(history["data"]["image_id"], IMAGE);
    assert_eq!(history["data"]["layers"]["items"][0]["size_bytes"], 4096);
    assert!(history["data"]["layers"]["items"][1]["id"].is_null());
    assert!(history["data"]["layers"]["items"][1]["size_bytes"].is_null());
    let network = data(
        call(
            &s,
            "get_network",
            json!({"target":{"id":SERVER},"object":{"name":"fixture-net"}}),
        )
        .await,
    );
    assert_eq!(network["data"]["attached_container_count"], 1);
    assert!(network["data"]["cached_in_use"].is_null());
    let volume = data(call(&s, "get_volume", object("get_volume")).await);
    assert_eq!(volume["data"]["reference_count"], 2);
    assert!(volume["data"]["size_bytes"].is_null());
    assert!(
        volume["evidence_limit"]
            .as_str()
            .unwrap()
            .contains("same-name replacement")
    );
    for q in c.requests("read") {
        match q.operation.as_str() {
            "InspectImage" | "ListImageHistory" => {
                assert_eq!(q.params["server"], SERVER);
                assert_eq!(q.params["image"], IMAGE);
            }
            "InspectNetwork" => {
                assert_eq!(q.params["server"], SERVER);
                assert_eq!(q.params["network"], NETWORK);
            }
            "InspectVolume" => {
                assert_eq!(q.params["server"], SERVER);
                assert_eq!(q.params["volume"], "fixture-volume");
            }
            _ => {}
        }
    }
    finish(p, s, &c).await;
}
#[tokio::test]
async fn invalid_null_prefix_and_credential_selectors_fail_before_any_core_io() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for tool in [
        "get_image",
        "get_image_history",
        "get_network",
        "get_volume",
    ] {
        for object in [
            json!({}),
            json!({"id":"abc"}),
            json!({"name":"*"}),
            json!({"name":"-flag"}),
            json!({"id":null,"name":"fixture-volume"}),
            json!({"id":IMAGE,"name":"fixture/web:latest"}),
            json!({"name":"user:password@registry.invalid/web"}),
            json!({"name":null}),
        ] {
            let reads = c.requests("read").len();
            assert_eq!(
                error(call(&s, tool, json!({"target":{"id":SERVER},"object":object})).await)["kind"],
                "invalid_arguments"
            );
            assert_eq!(c.requests("read").len(), reads);
        }
    }
    for tool in [
        "list_images",
        "list_networks",
        "list_volumes",
        "get_image_history",
    ] {
        let mut args = if tool.starts_with("list") {
            target()
        } else {
            object(tool)
        };
        args["page_size"] = json!(0);
        let reads = c.requests("read").len();
        assert_eq!(
            error(call(&s, tool, args).await)["kind"],
            "invalid_arguments"
        );
        assert_eq!(c.requests("read").len(), reads);
    }
    finish(p, s, &c).await;
}
#[tokio::test]
async fn missing_ambiguous_and_changed_objects_never_broaden_or_follow_replacements() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    assert_eq!(
        error(
            call(
                &s,
                "get_network",
                json!({"target":{"id":SERVER},"object":{"name":"missing"}})
            )
            .await
        )["kind"],
        "not_found"
    );
    assert!(
        !c.requests("read")
            .iter()
            .any(|q| q.operation == "InspectNetwork")
    );
    {
        let mut d = c.data.lock().unwrap();
        let mut duplicate = d.replies["ListImages"][0].clone();
        duplicate["id"] = json!(format!("sha256:{}", "c".repeat(64)));
        duplicate["name"] = json!(format!(
            "{}:{}@registry.invalid/web",
            PRIVATE[4], PRIVATE[5]
        ));
        d.replies
            .get_mut("ListImages")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(duplicate);
    }
    assert_eq!(
        error(
            call(
                &s,
                "get_image",
                json!({"target":{"id":SERVER},"object":{"name":"fixture/web:latest"}})
            )
            .await
        )["kind"],
        "ambiguous_target"
    );
    assert!(
        !c.requests("read")
            .iter()
            .any(|q| q.operation == "InspectImage")
    );
    // Stable ID remains usable despite name ambiguity.
    data(call(&s, "get_image", object("get_image")).await);
    for (tool, op, field, value) in [
        (
            "get_image",
            "InspectImage",
            "Id",
            json!(format!("sha256:{}", "d".repeat(64))),
        ),
        ("get_network", "InspectNetwork", "Id", json!("e".repeat(64))),
        ("get_volume", "InspectVolume", "Name", json!("replacement")),
    ] {
        c.data.lock().unwrap().replies.get_mut(op).unwrap()[field] = value;
        assert_eq!(
            error(call(&s, tool, object(tool)).await)["kind"],
            "invalid_response"
        );
    }
    // A removed tag may not silently read the old image under that name.
    {
        let mut d = c.data.lock().unwrap();
        d.replies
            .get_mut("ListImages")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .truncate(1);
        d.replies.get_mut("InspectImage").unwrap()["Id"] = json!(IMAGE);
        d.replies.get_mut("InspectImage").unwrap()["RepoTags"] = json!(["different:tag"]);
    }
    assert_eq!(
        error(
            call(
                &s,
                "get_image_history",
                json!({"target":{"id":SERVER},"object":{"name":"fixture/web:latest"}})
            )
            .await
        )["kind"],
        "invalid_response"
    );
    assert!(
        !c.requests("read")
            .iter()
            .any(|q| q.operation == "ListImageHistory")
    );
    finish(p, s, &c).await;
}
fn tool(op: &str) -> &'static str {
    match op {
        "ListImages" => "list_images",
        "InspectImage" => "get_image",
        "ListImageHistory" => "get_image_history",
        "ListNetworks" => "list_networks",
        "InspectNetwork" => "get_network",
        "ListVolumes" => "list_volumes",
        _ => "get_volume",
    }
}
fn args(tool: &str) -> Value {
    if tool.starts_with("list") {
        target()
    } else {
        object(tool)
    }
}
#[tokio::test]
async fn read_permissions_unsupported_and_unsafe_errors_never_use_another_api() {
    for op in [
        "ListServers",
        "GetServer",
        "ListImages",
        "InspectImage",
        "ListImageHistory",
        "ListNetworks",
        "InspectNetwork",
        "ListVolumes",
        "InspectVolume",
    ] {
        let c = Core::start().await;
        let (p, s) = Process::start_profile(&c.address, true, true).await;
        c.data.lock().unwrap().errors.insert(op.into(),(500,json!({"error":"User does not have required permissions on this Server. Must have at least Read permissions","trace":[PRIVATE[9]]})));
        let t = if op == "ListServers" || op == "GetServer" {
            "list_images"
        } else {
            tool(op)
        };
        assert_eq!(
            error(call(&s, t, args(t)).await)["kind"],
            "permission_denied"
        );
        assert_eq!(
            c.requests("read")
                .iter()
                .filter(|r| r.operation == op)
                .count(),
            1
        );
        c.data
            .lock()
            .unwrap()
            .errors
            .insert(op.into(), (501, json!({"error":PRIVATE[9]})));
        if op != "ListServers" && op != "GetServer" {
            assert_eq!(
                error(call(&s, t, args(t)).await)["kind"],
                "unsupported_target"
            );
        }
        finish(p, s, &c).await;
    }
}
#[tokio::test]
async fn unavailable_servers_and_exact_fresh_missing_errors_remain_distinct_from_cache_empty() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    for (op, action, id, detail) in [
        (
            "InspectImage",
            "inspect image",
            IMAGE,
            format!("No such image: {IMAGE}"),
        ),
        (
            "ListImageHistory",
            "get image history",
            IMAGE,
            format!("No such image: {IMAGE}"),
        ),
        (
            "InspectNetwork",
            "inspect network",
            NETWORK,
            format!("network {NETWORK} not found"),
        ),
        (
            "InspectVolume",
            "inspect volume",
            "fixture-volume",
            "get fixture-volume: no such volume".into(),
        ),
    ] {
        let t = tool(op);
        c.data.lock().unwrap().errors.insert(
            op.into(),
            (
                500,
                json!({"error":format!("Cannot {action}: server is NotOk"),"trace":[PRIVATE[9]]}),
            ),
        );
        assert_eq!(error(call(&s, t, args(t)).await)["kind"], "unreachable");
        c.data.lock().unwrap().errors.insert(op.into(),(500,json!({"error":PRIVATE[9],"trace":[format!("Docker responded with status code 404: {detail}")]})));
        assert_eq!(error(call(&s, t, args(t)).await)["kind"], "not_found");
        c.data.lock().unwrap().errors.insert(op.into(),(500,json!({"error":format!("Docker responded with status code 404: {}",detail.replace(id,"different")),"trace":[]})));
        assert_eq!(error(call(&s, t, args(t)).await)["kind"], "upstream");
        c.data.lock().unwrap().errors.remove(op);
    }
    {
        let mut d = c.data.lock().unwrap();
        d.replies.get_mut("ListServers").unwrap()[0]["info"]["state"] = json!("NotOk");
        d.replies.insert("ListImages".into(), json!([]));
    }
    let v = data(call(&s, "list_images", target()).await);
    assert_eq!(v["server"]["cached_state"], "not-ok");
    assert_eq!(v["data"]["items"], json!([]));
    assert!(
        v["evidence_limit"]
            .as_str()
            .unwrap()
            .contains("missing cache")
    );
    finish(p, s, &c).await;
}
#[tokio::test]
async fn large_caches_clip_lists_but_refuse_exact_resolution_and_bodies_are_bounded() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let original = c.data.lock().unwrap().replies.clone();
    for (op, t, get) in [
        ("ListImages", "list_images", "get_image"),
        ("ListNetworks", "list_networks", "get_network"),
        ("ListVolumes", "list_volumes", "get_volume"),
    ] {
        let values = (0..600)
            .map(|n| match op {
                "ListImages" => image(&format!("sha256:{n:064x}"), &format!("fixture/web:{n}")),
                "ListNetworks" => network(&format!("{n:064x}"), &format!("net-{n}")),
                _ => volume(&format!("vol-{n}")),
            })
            .collect::<Vec<_>>();
        c.data
            .lock()
            .unwrap()
            .replies
            .insert(op.into(), json!(values));
        let v = data(call(&s, t, json!({"target":{"id":SERVER},"page_size":100})).await);
        assert_eq!(v["data"]["items"].as_array().unwrap().len(), 100);
        assert_eq!(v["data"]["scan_incomplete"], true);
        assert_eq!(v["data"]["next_page"], 1);
        assert_eq!(
            error(call(&s, get, object(get)).await)["kind"],
            "incomplete_resolution"
        );
        c.data.lock().unwrap().replies = original.clone();
    }
    for op in [
        "ListImages",
        "InspectImage",
        "ListImageHistory",
        "ListNetworks",
        "InspectNetwork",
        "ListVolumes",
        "InspectVolume",
    ] {
        c.data.lock().unwrap().oversized = Some(op.into());
        let t = tool(op);
        assert_eq!(
            error(call(&s, t, args(t)).await)["kind"],
            "response_too_large"
        );
        c.data.lock().unwrap().oversized = None;
    }
    finish(p, s, &c).await;
}
#[tokio::test]
async fn shared_read_transport_cannot_bypass_docker_response_budget() {
    use komodo_client::api::read::ListImages;
    use komodo_mcp::{
        client::{CoreClient, RequestPolicy},
        config::Config,
        error::ErrorKind,
    };
    let c = Core::start().await;
    c.data.lock().unwrap().oversized = Some("ListImages".into());
    let client = CoreClient::new(
        Config::parse(&c.address, support::KEY, support::SECRET).unwrap(),
        RequestPolicy::default(),
    )
    .unwrap();
    assert!(
        matches!(client.read(ListImages{server:SERVER.into()}).await,Err(e) if e.kind==ErrorKind::ResponseTooLarge)
    );
    c.assert_readonly();
}

#[tokio::test]
async fn history_coverage_output_budgets_and_malformed_identities_fail_safely() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let original = c.data.lock().unwrap().replies.clone();
    let layer = original["ListImageHistory"][0].clone();
    c.data
        .lock()
        .unwrap()
        .replies
        .insert("ListImageHistory".into(), json!(vec![layer; 600]));
    let v = data(
        call(
            &s,
            "get_image_history",
            json!({"target":{"id":SERVER},"object":{"id":IMAGE},"page_size":100}),
        )
        .await,
    );
    assert_eq!(v["data"]["layers"]["scan_incomplete"], true);
    assert_eq!(v["data"]["layers"]["observed_count"], 600);
    assert_eq!(v["data"]["layers"]["items"].as_array().unwrap().len(), 100);
    assert_eq!(v["data"]["layers"]["next_page"], 1);
    // Long but syntactically valid references can exceed the *projected*
    // output budget even though the upstream body is under its separate cap.
    let images = (0..100)
        .map(|n| {
            let mut v = image(&format!("sha256:{n:064x}"), "fixture/web:latest");
            v["tags"] = json!(
                (0..4)
                    .map(|i| format!("fixture/{}:{i}", "a".repeat(240)))
                    .collect::<Vec<_>>()
            );
            v
        })
        .collect::<Vec<_>>();
    c.data
        .lock()
        .unwrap()
        .replies
        .insert("ListImages".into(), json!(images));
    assert_eq!(
        error(
            call(
                &s,
                "list_images",
                json!({"target":{"id":SERVER},"page_size":100})
            )
            .await
        )["kind"],
        "response_too_large"
    );
    // Small pages remain available, and malformed identities never reach inspect.
    data(
        call(
            &s,
            "list_images",
            json!({"target":{"id":SERVER},"page_size":1}),
        )
        .await,
    );
    {
        let mut d = c.data.lock().unwrap();
        d.replies = original;
        d.replies.get_mut("ListImages").unwrap()[0]["id"] = json!(PRIVATE[0]);
    }
    let reads = c
        .requests("read")
        .iter()
        .filter(|r| r.operation == "InspectImage")
        .count();
    assert_eq!(
        error(call(&s, "get_image", object("get_image")).await)["kind"],
        "invalid_response"
    );
    assert_eq!(
        c.requests("read")
            .iter()
            .filter(|r| r.operation == "InspectImage")
            .count(),
        reads
    );
    finish(p, s, &c).await;
}
