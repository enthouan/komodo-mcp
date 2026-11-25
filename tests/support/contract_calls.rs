//! Read-only public call scenarios. Store the arguments and the complete MCP
//! outcome to retain both text and structured content, plus the dispatch error
//! envelope for unknown and locally hidden tools.
use crate::{
    fixture::{self, Core, SERVER, STOPPED},
    peer::{Peer, Transport},
};
use serde_json::{Value, json};

type Cases = std::collections::BTreeMap<&'static str, Value>;

async fn record(
    cases: &mut Cases,
    case: &'static str,
    peer: &Peer,
    tool: &str,
    args: Value,
) -> Value {
    let response = match peer.call(tool, args.clone()).await {
        Ok(result) => json!({"result": result}),
        Err(error) => json!({"error": error}),
    };
    let wire = response.to_string();
    crate::http::assert_redacted(&wire);
    for marker in fixture::PRIVATE {
        assert!(!wire.contains(marker), "fixture disclosure in {case}");
    }
    assert!(
        cases
            .insert(
                case,
                json!({"tool":tool,"arguments": args,"response":response})
            )
            .is_none()
    );
    response
}

fn success(response: &Value) -> &Value {
    assert_eq!(response["result"]["isError"], false, "{response}");
    &response["result"]["structuredContent"]
}

fn category(response: &Value, expected: &str) {
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(response["result"]["structuredContent"]["kind"], expected);
}

async fn hidden(cases: &mut Cases, core: &Core, peer: &Peer, case: &'static str, tool: &str) {
    let reads = core.requests("read").len();
    let response = record(cases, case, peer, tool, json!({})).await;
    assert!(response["error"]["code"].is_number());
    assert_eq!(
        reads,
        core.requests("read").len(),
        "hidden calls reached Core"
    );
}

pub async fn capture(transport: Transport, core: &Core) -> Value {
    let mut cases = Cases::new();
    let peer = transport.connect(&core.address, &[]).await;
    hidden(&mut cases, core, &peer, "capabilityHidden", "deploy_stack").await;
    hidden(&mut cases, core, &peer, "unknown", "unknown_contract_tool").await;
    containers(&mut cases, core, &peer).await;

    core.data.lock().unwrap().replies.insert("GetContainerLog".into(), json!({
        "stdout":"earlier line\nselected line\n", "stderr":"last diagnostic line", "success":true,
        "command":fixture::PRIVATE[2], "stage":"contract", "start_ts":10, "end_ts":20
    }));
    let log = record(
        &mut cases,
        "truncatedLogTail",
        &peer,
        "get_container_logs",
        json!({"server_id":SERVER,"container":"active-web","tail":2,"max_bytes":8}),
    )
    .await;
    let truncation = &success(&log)["truncation"];
    assert_eq!(truncation["lines"], true);
    assert_eq!(truncation["bytes"], true);

    // Exercise the bound itself, retaining one item of the clipped inventory
    // and the reported scan limit and pagination metadata.
    core.data.lock().unwrap().replies.insert(
        "ListVolumes".into(),
        json!(
            (0..513)
                .map(|n| fixture::volume(&format!("volume-{n:03}")))
                .collect::<Vec<_>>()
        ),
    );
    let clipped = record(
        &mut cases,
        "boundedScan",
        &peer,
        "list_volumes",
        json!({"target":{"id":SERVER}, "page_size":1}),
    )
    .await;
    assert_eq!(success(&clipped)["data"]["scan_incomplete"], true);
    for (code, case, expected) in [
        (401, "coreAuthentication", "authentication"),
        (502, "coreFailure", "upstream"),
    ] {
        core.data.lock().unwrap().errors.insert(
            "ListVolumes".into(),
            (code, json!({"error":"docker-error-private"})),
        );
        category(
            &record(
                &mut cases,
                case,
                &peer,
                "list_volumes",
                json!({"target":{"id":SERVER}}),
            )
            .await,
            expected,
        );
    }
    peer.finish().await;

    let mut selections = std::collections::BTreeMap::new();
    for (name, allowed, disabled, expected, hidden_tool) in [
        (
            "selected",
            "list_containers,get_container,execute_terminal",
            "get_container",
            vec!["list_containers"],
            "get_container",
        ),
        (
            "empty",
            "list_containers",
            "list_containers",
            vec![],
            "list_containers",
        ),
    ] {
        let peer = transport
            .connect(
                &core.address,
                &[
                    ("KOMODO_ENABLED_TOOLS", allowed),
                    ("KOMODO_DISABLED_TOOLS", disabled),
                ],
            )
            .await;
        let names = peer.catalog().await.into_keys().collect::<Vec<_>>();
        assert_eq!(names, expected);
        selections.insert(name, names);
        hidden(&mut cases, core, &peer, name, hidden_tool).await;
        peer.finish().await;
    }
    json!({"examples":cases,"perToolVisibility":selections})
}

async fn containers(cases: &mut Cases, core: &Core, peer: &Peer) {
    let args = json!({"target":{"id":SERVER}});
    let running = record(
        cases,
        "runningContainers",
        peer,
        "list_containers",
        args.clone(),
    )
    .await;
    assert_eq!(
        success(&running)["data"]["items"].as_array().unwrap().len(),
        1
    );
    let mut page = json!({"target":{"name":"fixture-server"},"include_stopped":true,"page_size":1});
    let first = record(
        cases,
        "containerFirstPage",
        peer,
        "list_containers",
        page.clone(),
    )
    .await;
    page["page"] = success(&first)["data"]["next_page"].clone();
    assert!(page["page"].is_u64());
    let next = record(cases, "containerNextPage", peer, "list_containers", page).await;
    let identity = success(&next)["data"]["items"][0]["identity"].clone();
    assert_eq!(identity["container_id"], STOPPED);
    let inspect = json!({"identity":identity});
    core.data
        .lock()
        .unwrap()
        .replies
        .insert("ListContainers".into(), json!([]));
    let empty = record(
        cases,
        "emptyContainerCache",
        peer,
        "list_containers",
        args.clone(),
    )
    .await;
    assert_eq!(success(&empty)["data"]["items"], json!([]));
    let fresh = record(
        cases,
        "freshStoppedContainer",
        peer,
        "get_container",
        inspect.clone(),
    )
    .await;
    assert_eq!(success(&fresh)["source"], "fresh_inspect");
    assert_eq!(success(&fresh)["data"]["identity"], inspect["identity"]);
    let reads = core.requests("read").len();
    category(
        &record(
            cases,
            "invalidPageSize",
            peer,
            "list_containers",
            json!({"target":{"id":SERVER},"page_size":0}),
        )
        .await,
        "invalid_arguments",
    );
    assert_eq!(reads, core.requests("read").len());
    let failure = "User does not have required permissions on this Server. Must have at least Read permissions, as well as these specific permissions: [Inspect]";
    core.data.lock().unwrap().errors.insert(
        "InspectContainer".into(),
        (
            500,
            json!({"error":failure,"trace":["docker-error-private"]}),
        ),
    );
    success(&record(cases, "listOnlyReader", peer, "list_containers", args).await);
    category(
        &record(
            cases,
            "inspectDenied",
            peer,
            "get_container",
            inspect.clone(),
        )
        .await,
        "permission_denied",
    );
    core.data.lock().unwrap().errors.insert("InspectContainer".into(), (500, json!({"error":format!("Docker responded with status code 404: No such container: {STOPPED}")})));
    category(
        &record(
            cases,
            "disappearedContainer",
            peer,
            "get_container",
            inspect.clone(),
        )
        .await,
        "not_found",
    );
    core.data.lock().unwrap().errors.remove("InspectContainer");
    for (case, id) in [
        ("replacedContainer", json!(fixture::RUNNING)),
        ("missingFreshIdentity", Value::Null),
    ] {
        core.data
            .lock()
            .unwrap()
            .replies
            .get_mut("InspectContainer")
            .unwrap()["Id"] = id;
        category(
            &record(cases, case, peer, "get_container", inspect.clone()).await,
            "invalid_response",
        );
    }
    let inspected = core
        .requests("read")
        .into_iter()
        .filter(|r| r.operation == "InspectContainer")
        .collect::<Vec<_>>();
    assert_eq!(inspected.len(), 5);
    assert!(
        inspected
            .iter()
            .all(|r| r.params == json!({"server":SERVER,"container":STOPPED}))
    );
}
