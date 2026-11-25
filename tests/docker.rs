//! Production-executable acceptance of bounded, read-only Docker tools.
#[path = "support/docker.rs"]
mod fixture;
#[path = "support/http.rs"]
mod http;
#[path = "support/peer.rs"]
mod peer;
mod support;
use fixture::*;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use support::operations::{Process, Session, call};
const TOOLS: [&str; 9] = [
    "list_containers",
    "get_container",
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
    if tool == "get_container" {
        return json!({"identity":{"server_id":SERVER,"container_id":STOPPED}});
    }
    json!({"target":{"id":SERVER},"object":match tool { "get_image"|"get_image_history"=>json!({"id":IMAGE}),"get_network"=>json!({"id":NETWORK}),_=>json!({"name":"fixture-volume"}) }})
}
async fn finish(p: Process, s: Session, c: &Core) {
    c.assert_readonly();
    p.finish_excluding(s, &PRIVATE).await;
}

// Validate the structural JSON Schema keywords used by these generated output
// contracts. This is a protocol-envelope regression check, not a general-purpose
// JSON Schema implementation. No network or external validator is needed in CI.
fn matches_schema(value: &Value, schema: &Value, root: &Value) -> bool {
    if let Some(reference) = schema["$ref"].as_str() {
        return reference
            .strip_prefix('#')
            .and_then(|p| root.pointer(p))
            .is_some_and(|s| matches_schema(value, s, root));
    }
    if let Some(options) = schema["anyOf"].as_array() {
        return options.iter().any(|s| matches_schema(value, s, root));
    }
    let typed = |t: &str| match t {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        _ => false,
    };
    if let Some(t) = schema["type"].as_str()
        && !typed(t)
    {
        return false;
    }
    if let Some(types) = schema["type"].as_array()
        && !types.iter().any(|t| t.as_str().is_some_and(typed))
    {
        return false;
    }
    if let Some(options) = schema["enum"].as_array()
        && !options.contains(value)
    {
        return false;
    }
    if let Some(n) = value.as_f64()
        && (schema["minimum"].as_f64().is_some_and(|v| n < v)
            || schema["maximum"].as_f64().is_some_and(|v| n > v))
    {
        return false;
    }
    if let Some(object) = value.as_object() {
        if schema["required"].as_array().is_some_and(|keys| {
            keys.iter()
                .any(|k| !k.as_str().is_some_and(|k| object.contains_key(k)))
        }) {
            return false;
        }
        if let Some(properties) = schema["properties"].as_object() {
            for (key, v) in object {
                if let Some(s) = properties.get(key) {
                    if !matches_schema(v, s, root) {
                        return false;
                    }
                } else if schema["additionalProperties"] == false {
                    return false;
                }
            }
        }
    }
    if let Some(items) = value.as_array()
        && let Some(item_schema) = schema.get("items")
    {
        return items.iter().all(|v| matches_schema(v, item_schema, root));
    }
    true
}

#[tokio::test]
async fn successful_stdio_observations_match_the_advertised_output_contracts() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let tools = s.list_tools(None).await.unwrap().tools;
    for name in TOOLS {
        let tool = tools.iter().find(|t| t.name == name).unwrap();
        let schema = serde_json::to_value(tool.output_schema.as_ref().unwrap()).unwrap();
        let v = data(call(&s, name, args(name)).await);
        assert!(
            matches_schema(&v, &schema, &schema),
            "{name} output violates discovery schema: {schema}"
        );
        // With the payload at the root, a result must fail the envelope contract.
        assert!(
            !matches_schema(&v["data"], &schema, &schema),
            "{name} schema accepted an unwrapped payload"
        );
    }
    finish(p, s, &c).await;
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
            if tool == "get_network" && object == json!({"name":"-flag"}) {
                continue;
            }
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
        "ListContainers" => "list_containers",
        "InspectContainer" => "get_container",
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
        "ListContainers",
        "InspectContainer",
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
        "ListContainers",
        "InspectContainer",
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

#[tokio::test]
async fn mixed_local_and_swarm_network_ids_are_exact_and_do_not_poison_other_selection() {
    let c = Core::start().await;
    let overlay = "ovrly0123456789abcdefghij";
    {
        let mut d = c.data.lock().unwrap();
        let mut v = network(overlay, "ingress");
        v["driver"] = json!("overlay");
        v["scope"] = json!("swarm");
        d.replies
            .get_mut("ListNetworks")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(v);
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let v = data(call(&s, "list_networks", target()).await);
    assert_eq!(v["data"]["items"].as_array().unwrap().len(), 2);
    assert!(
        v["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == overlay && n["scope"] == "swarm")
    );
    // Either exact selector for the local network remains usable in this cache.
    data(call(&s, "get_network", object("get_network")).await);
    data(
        call(
            &s,
            "get_network",
            json!({"target":{"id":SERVER},"object":{"name":"fixture-net"}}),
        )
        .await,
    );
    {
        let mut d = c.data.lock().unwrap();
        d.replies.get_mut("InspectNetwork").unwrap()["Id"] = json!(overlay);
        d.replies.get_mut("InspectNetwork").unwrap()["Name"] = json!("ingress");
    }
    for selector in [json!({"id":overlay}), json!({"name":"ingress"})] {
        let v = data(
            call(
                &s,
                "get_network",
                json!({"target":{"id":SERVER},"object":selector}),
            )
            .await,
        );
        assert_eq!(v["data"]["id"], overlay);
        let q = c
            .requests("read")
            .into_iter()
            .rev()
            .find(|r| r.operation == "InspectNetwork")
            .unwrap();
        assert_eq!(q.params["network"], overlay);
    }
    let reads = c.requests("read").len();
    assert_eq!(
        error(
            call(
                &s,
                "get_network",
                json!({"target":{"id":SERVER},"object":{"id":&overlay[..12]}})
            )
            .await
        )["kind"],
        "invalid_arguments"
    );
    assert_eq!(c.requests("read").len(), reads);
    finish(p, s, &c).await;
}

#[tokio::test]
async fn broad_network_names_select_exactly_and_unsafe_display_never_poisons_inventory() {
    let c = Core::start().await;
    let names = [
        "_private".to_owned(),
        "app:backend".into(),
        "-flag".into(),
        "space separated".into(),
        format!("https://{}:{}@private.invalid", PRIVATE[4], PRIVATE[5]),
    ];
    {
        let mut d = c.data.lock().unwrap();
        let rows = names
            .iter()
            .enumerate()
            .map(|(n, name)| network(&format!("{:064x}", n + 1), name))
            .collect::<Vec<_>>();
        d.replies.insert("ListNetworks".into(), json!(rows));
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let v = data(call(&s, "list_networks", target()).await);
    let rows = v["data"]["items"].as_array().unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0]["name"], "_private");
    assert_eq!(rows[1]["name"], "app:backend");
    assert!(rows[3]["name"].is_null());
    assert_eq!(rows[3]["name_omitted"], true);
    assert!(rows[4]["name"].is_null());
    assert_eq!(rows[4]["name_omitted"], true);
    for (n, name) in names.iter().enumerate() {
        let id = format!("{:064x}", n + 1);
        {
            let mut d = c.data.lock().unwrap();
            d.replies.get_mut("InspectNetwork").unwrap()["Id"] = json!(id);
            d.replies.get_mut("InspectNetwork").unwrap()["Name"] = json!(name);
        }
        let value = data(
            call(
                &s,
                "get_network",
                json!({"target":{"id":SERVER},"object":{"id":id}}),
            )
            .await,
        );
        assert_eq!(value["data"]["id"], id);
        if n < 4 {
            let value = data(
                call(
                    &s,
                    "get_network",
                    json!({"target":{"id":SERVER},"object":{"name":name}}),
                )
                .await,
            );
            assert_eq!(value["data"]["id"], id);
        }
        assert_eq!(c.requests("read").last().unwrap().params["network"], id);
    }
    finish(p, s, &c).await;
}

#[tokio::test]
async fn ipv6_registry_tags_and_digests_are_preserved_and_bound_to_immutable_image_ids() {
    let c = Core::start().await;
    let tag = "[2001:db8::1]:5000/app:latest";
    let digest = format!("[2001:db8::1]:5000/app@{IMAGE}");
    {
        let mut d = c.data.lock().unwrap();
        d.replies.get_mut("ListImages").unwrap()[0]["name"] = json!(tag);
        d.replies.get_mut("ListImages").unwrap()[0]["tags"] = json!([tag]);
        d.replies.get_mut("ListImages").unwrap()[0]["digests"] = json!([digest]);
        d.replies.get_mut("InspectImage").unwrap()["RepoTags"] = json!([tag]);
        d.replies.get_mut("InspectImage").unwrap()["RepoDigests"] = json!([digest]);
    }
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let v = data(call(&s, "list_images", target()).await);
    assert_eq!(v["data"]["items"][0]["tags"], json!([tag]));
    assert_eq!(v["data"]["items"][0]["digests"], json!([digest]));
    assert_eq!(v["data"]["items"][0]["references_incomplete"], false);
    for name in [tag, &digest] {
        for tool in ["get_image", "get_image_history"] {
            data(
                call(
                    &s,
                    tool,
                    json!({"target":{"id":SERVER},"object":{"name":name}}),
                )
                .await,
            );
            assert!(
                c.requests("read")
                    .iter()
                    .filter(|r| r.operation == "InspectImage" || r.operation == "ListImageHistory")
                    .all(|r| r.params["image"] == IMAGE)
            );
        }
    }
    for name in [
        "[not-ipv6]:5000/app:latest",
        "[2001:db8::1]:/app",
        "[2001:db8::1]/app@user:password",
        "user:password@[2001:db8::1]/app",
    ] {
        let reads = c.requests("read").len();
        assert_eq!(
            error(
                call(
                    &s,
                    "get_image",
                    json!({"target":{"id":SERVER},"object":{"name":name}})
                )
                .await
            )["kind"],
            "invalid_arguments"
        );
        assert_eq!(c.requests("read").len(), reads);
    }
    finish(p, s, &c).await;
}

#[tokio::test]
async fn stopped_container_inspection_is_scoped_fresh_and_safe_on_both_transports() {
    for transport in peer::TRANSPORTS {
        let c = Core::start().await;
        let peer = transport.connect(&c.address, &[]).await;
        let catalog = peer.catalog().await;
        for tool in ["list_containers", "get_container"] {
            assert_eq!(
                catalog[tool].annotations.as_ref().unwrap().read_only_hint,
                Some(true)
            );
        }
        let list = |page, include_stopped| {
            json!({
                "target":{"name":"fixture-server"}, "page":page, "page_size":1,
                "include_stopped":include_stopped
            })
        };
        let running = data(peer.call("list_containers", target()).await.unwrap());
        assert_eq!(running["source"], "core_cache");
        assert_eq!(running["data"]["items"].as_array().unwrap().len(), 1);
        assert_eq!(
            running["data"]["items"][0]["identity"],
            json!({"server_id":SERVER,"container_id":RUNNING})
        );
        assert_eq!(running["data"]["items"][0]["state"], "running");
        assert!(running["data"]["next_page"].is_null());

        let first = data(peer.call("list_containers", list(0, true)).await.unwrap());
        assert_eq!(first["data"]["next_page"], 1);
        let second = data(peer.call("list_containers", list(1, true)).await.unwrap());
        assert!(second["data"]["next_page"].is_null());
        let first = &first["data"]["items"][0];
        let second = &second["data"]["items"][0];
        assert_ne!(first["identity"], second["identity"]);
        let stopped = [first, second]
            .into_iter()
            .find(|item| item["state"] == "exited")
            .unwrap();
        assert_eq!(stopped["name"], "failed-job");
        assert_eq!(stopped["image"], "fixture/web:latest");
        let inspect = json!({"identity":stopped["identity"]});
        assert_eq!(
            inspect["identity"],
            json!({"server_id":SERVER,"container_id":STOPPED})
        );

        // Drop the cache after discovery. A fresh inspect still addresses only the observed ID.
        c.data
            .lock()
            .unwrap()
            .replies
            .insert("ListContainers".into(), json!([]));
        let detail = data(peer.call("get_container", inspect.clone()).await.unwrap());
        assert_eq!(detail["source"], "fresh_inspect");
        assert_eq!(detail["server"]["id"], SERVER);
        assert_eq!(
            detail["data"],
            json!({
                "identity": inspect["identity"], "name":"failed-job", "state":"exited",
                "exit_code":17, "health_status":"unhealthy", "restart_count":3
            })
        );
        let empty = data(peer.call("list_containers", list(0, true)).await.unwrap());
        assert_eq!(empty["data"]["items"], json!([]));
        assert!(empty["evidence_limit"].as_str().unwrap().contains("cache"));

        // Core checks Read + Inspect on the fresh endpoint, while this identity can still list.
        let denied = "User does not have required permissions on this Server. Must have at least Read permissions, as well as these specific permissions: [Inspect]";
        c.data.lock().unwrap().errors.insert(
            "InspectContainer".into(),
            (
                500,
                json!({"error":denied,"trace":["sensitive-raw-trace-marker"]}),
            ),
        );
        assert_eq!(
            error(peer.call("get_container", inspect.clone()).await.unwrap())["kind"],
            "permission_denied"
        );
        data(peer.call("list_containers", target()).await.unwrap());

        let missing =
            format!("Docker responded with status code 404: No such container: {STOPPED}");
        c.data.lock().unwrap().errors.insert(
            "InspectContainer".into(),
            (
                500,
                json!({"error":"sensitive-description-marker","trace":[missing]}),
            ),
        );
        assert_eq!(
            error(peer.call("get_container", inspect.clone()).await.unwrap())["kind"],
            "not_found"
        );

        // A name can now point at a replacement, or Periphery can omit identity proof.
        c.data.lock().unwrap().errors.clear();
        for fresh_id in [json!(RUNNING), Value::Null] {
            c.data
                .lock()
                .unwrap()
                .replies
                .get_mut("InspectContainer")
                .unwrap()["Id"] = fresh_id;
            assert_eq!(
                error(peer.call("get_container", inspect.clone()).await.unwrap())["kind"],
                "invalid_response"
            );
        }
        let reads = c.requests("read");
        let inspected: Vec<_> = reads
            .iter()
            .filter(|r| r.operation == "InspectContainer")
            .collect();
        assert_eq!(inspected.len(), 5);
        assert!(
            inspected
                .iter()
                .all(|r| r.params == json!({"server":SERVER,"container":STOPPED}))
        );
        for request in reads {
            match request.operation.as_str() {
                "ListContainers" | "GetServer" => assert_eq!(request.params["server"], SERVER),
                "GetVersion" | "ListServers" | "InspectContainer" => (),
                unexpected => panic!("unexpected Core read {unexpected}"),
            }
        }
        c.assert_readonly();
        peer.finish().await;
    }
}

#[tokio::test]
async fn container_tool_selection_disallows_hidden_calls_before_core_on_both_transports() {
    for transport in peer::TRANSPORTS {
        let c = Core::start().await;
        for (visible, excluded) in [
            ("list_containers", "get_container"),
            ("get_container", "list_containers"),
        ] {
            let p = transport
                .connect(
                    &c.address,
                    &[
                        ("KOMODO_ENABLED_TOOLS", "list_containers,get_container"),
                        ("KOMODO_DISABLED_TOOLS", excluded),
                    ],
                )
                .await;
            assert_eq!(p.catalog().await.into_keys().collect::<Vec<_>>(), [visible]);
            data(p.call(visible, args(visible)).await.unwrap());
            let before = c.requests("read").len();
            let hidden = p.call(excluded, args(excluded)).await.unwrap_err();
            assert_eq!(
                hidden.code,
                p.call("unknown_fixture_tool", json!({}))
                    .await
                    .unwrap_err()
                    .code
            );
            assert_eq!(c.requests("read").len(), before);
            p.finish().await;
        }
        c.assert_readonly();
    }
}

#[tokio::test]
async fn incomplete_container_identity_never_becomes_a_name_lookup() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    c.data
        .lock()
        .unwrap()
        .replies
        .get_mut("ListContainers")
        .unwrap()[0]["id"] = Value::Null;
    let page = data(call(&s, "list_containers", target()).await);
    assert_eq!(page["data"]["items"][0]["name"], "active-web");
    assert!(page["data"]["items"][0]["identity"].is_null());
    for identity in [
        json!({"server_id":SERVER,"container_id":"failed-job"}),
        json!({"server_id":SERVER,"container_id":&STOPPED[..12]}),
        json!({"server_id":"fixture-server","container_id":STOPPED}),
        json!({"server_id":SERVER,"name":"failed-job"}),
        Value::Null,
    ] {
        let before = c.requests("read").len();
        assert_eq!(
            error(call(&s, "get_container", json!({"identity":identity})).await)["kind"],
            "invalid_arguments"
        );
        assert_eq!(c.requests("read").len(), before);
    }
    c.data
        .lock()
        .unwrap()
        .replies
        .get_mut("ListContainers")
        .unwrap()[0]["server_id"] = json!("000000000000000000000009");
    assert_eq!(
        error(call(&s, "list_containers", target()).await)["kind"],
        "invalid_response"
    );
    finish(p, s, &c).await;
}

#[tokio::test]
async fn container_scan_bounds_and_template_servers_preserve_scope() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, false, false).await;
    let mut rows = (0..512)
        .map(|i| container(&format!("{i:064x}"), &format!("finished-{i}"), "exited"))
        .collect::<Vec<_>>();
    rows.push(container(RUNNING, "running-beyond-scan", "running"));
    c.data
        .lock()
        .unwrap()
        .replies
        .insert("ListContainers".into(), json!(rows));
    let list = data(call(&s, "list_containers", target()).await);
    assert_eq!(list["data"]["observed_count"], 513);
    assert_eq!(list["data"]["scan_incomplete"], true);
    assert_eq!(list["data"]["items"], json!([]));
    assert!(list["data"]["next_page"].is_null());

    {
        let mut d = c.data.lock().unwrap();
        d.replies.get_mut("ListServers").unwrap()[0]["template"] = json!(true);
        d.replies.get_mut("GetServer").unwrap()["template"] = json!(true);
    }
    let before = c.requests("read").len();
    for tool in ["list_containers", "get_container"] {
        assert_eq!(
            error(call(&s, tool, args(tool)).await)["kind"],
            "unsupported_target"
        );
    }
    assert!(
        c.requests("read")
            .iter()
            .skip(before)
            .all(|r| matches!(r.operation.as_str(), "ListServers" | "GetServer"))
    );
    finish(p, s, &c).await;
}
