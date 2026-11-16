mod support;

use std::{collections::BTreeSet, time::Duration};

use anyhow::Result;
use komodo_client::entities::{deployment::Deployment, server::Server, stack::Stack};
use serde_json::{Value, json};
use support::{inventory::*, *};

fn reply(value: &Value) -> Reply {
    Reply::new(200, value.to_string())
}
fn expected(steps: &[Step]) -> Vec<&'static str> {
    steps.iter().map(|step| step.operation).collect()
}

#[tokio::test]
async fn production_discovery_and_all_six_tools_use_allowlisted_projections() -> Result<()> {
    let mut steps = vec![Step::version()];
    for family in FAMILIES {
        let summary = family.summary();
        steps.push(family.list(0, 30, std::slice::from_ref(&summary)));
        for field in ["id", "name"] {
            steps.push(family.lookup(
                summary[field].as_str().unwrap(),
                std::slice::from_ref(&summary),
            ));
            steps.push(family.get(summary["id"].as_str().unwrap(), reply(&family.detail())));
        }
    }
    let operations = expected(&steps);
    let core = MockCore::start(steps).await?;
    let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
    let discovery = session.request("tools/list", json!({})).await?;
    let tools = discovery["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 56);
    assert!(discovery.get("nextCursor").is_none());
    for family in FAMILIES {
        for (tool, detail) in [(family.list_tool(), false), (family.get_tool(), true)] {
            let metadata = tools.iter().find(|t| t["name"] == tool).unwrap();
            assert_eq!(metadata["annotations"]["readOnlyHint"], true);
            assert_eq!(metadata["annotations"]["idempotentHint"], true);
            assert_eq!(metadata["annotations"]["destructiveHint"], false);
            assert!(
                metadata["description"]
                    .as_str()
                    .unwrap()
                    .contains(if detail {
                        "exact case-sensitive name"
                    } else {
                        "next_page"
                    })
            );
            let input = &metadata["inputSchema"];
            let output = &metadata["outputSchema"];
            assert_eq!(output["type"], "object");
            let branches = output["anyOf"].as_array().unwrap();
            assert_eq!(branches.len(), 2);
            let success_schema = output
                .pointer(
                    branches[0]["$ref"]
                        .as_str()
                        .unwrap()
                        .strip_prefix('#')
                        .unwrap(),
                )
                .unwrap();
            let error_schema = output
                .pointer(
                    branches[1]["$ref"]
                        .as_str()
                        .unwrap()
                        .strip_prefix('#')
                        .unwrap(),
                )
                .unwrap();
            assert_keys(
                &error_schema["properties"],
                &[
                    "kind",
                    "message",
                    "operation",
                    "status",
                    "field",
                    "correction",
                    "candidates",
                ],
            );
            assert_eq!(error_schema["additionalProperties"], false);
            assert_eq!(input["type"], "object");
            assert_eq!(input["additionalProperties"], false);
            if detail {
                assert_keys(&input["properties"], &["id", "name"]);
                assert_eq!(
                    input["oneOf"],
                    json!([{"required":["id"]},{"required":["name"]}])
                );
                assert_eq!(input["properties"]["name"]["minLength"], 1);
                assert_eq!(input["properties"]["id"]["pattern"], "^[0-9a-fA-F]{24}$");
                assert_keys(&success_schema["properties"], &family.keys(true));
            } else {
                assert_keys(&input["properties"], &["page", "page_size"]);
                assert_eq!(input["properties"]["page"]["default"], 0);
                assert_eq!(input["properties"]["page_size"]["default"], 30);
                assert_eq!(input["properties"]["page_size"]["minimum"], 1);
                assert_eq!(input["properties"]["page_size"]["maximum"], 100);
                assert_keys(
                    &success_schema["properties"],
                    &["items", "page", "page_size", "has_more", "next_page"],
                );
            }
            assert_eq!(metadata["outputSchema"]["type"], "object");
        }
        let response = session.call(family.list_tool(), json!({})).await?;
        let data = assert_success(&response);
        assert_keys(
            data,
            &["items", "page", "page_size", "has_more", "next_page"],
        );
        assert_eq!(data["page"], 0);
        assert_eq!(data["page_size"], 30);
        assert_eq!(data["has_more"], false);
        assert!(data["next_page"].is_null());
        assert_keys(&data["items"][0], &family.keys(false));
        for field in ["id", "name"] {
            let response = session
                .call(family.get_tool(), json!({field:family.summary()[field]}))
                .await?;
            let data = assert_success(&response);
            assert_keys(data, &family.keys(true));
            assert_eq!(data["id"], family.summary()["id"]);
            assert_eq!(data["name"], family.summary()["name"]);
            match family {
                Family::Server => {
                    assert_eq!(data["state"], "not_ok");
                    assert_eq!(data["enabled"], true);
                    assert!(data["periphery_version"].is_null());
                }
                Family::Stack => {
                    assert_eq!(data["state"], "running");
                    assert_eq!(data["project_name"], "fixture-project");
                    assert_eq!(data["service_count"], 2);
                }
                Family::Deployment => {
                    assert_eq!(data["state"], "running");
                    assert_eq!(data["runtime_name"], "fixture-web");
                    assert_eq!(data["image"], "nginx:stable");
                    assert_eq!(data["update_available"], true);
                }
            }
        }
    }
    session.finish().await?;
    core.assert_finished(&operations);
    Ok(())
}

#[test]
fn sensitive_fixtures_populate_actual_typed_sdk_fields() -> Result<()> {
    let server: Server = serde_json::from_value(Family::Server.detail())?;
    assert_eq!(server.config.passkey, SENSITIVE_MARKERS[0]);
    assert_eq!(server.info.public_key, SENSITIVE_MARKERS[3]);
    let stack: Stack = serde_json::from_value(Family::Stack.detail())?;
    assert!(stack.config.environment.contains(SENSITIVE_MARKERS[1]));
    assert!(stack.config.file_contents.contains(SENSITIVE_MARKERS[2]));
    assert_eq!(stack.config.git_account, SENSITIVE_MARKERS[0]);
    assert_eq!(
        stack.info.deployed_contents.unwrap()[0].contents,
        SENSITIVE_MARKERS[3]
    );
    assert_eq!(
        stack.info.remote_errors.unwrap()[0].contents,
        SENSITIVE_MARKERS[4]
    );
    let deployment: Deployment = serde_json::from_value(Family::Deployment.detail())?;
    assert!(deployment.config.environment.contains(SENSITIVE_MARKERS[1]));
    assert_eq!(
        deployment.config.image_registry_account,
        SENSITIVE_MARKERS[0]
    );
    assert!(deployment.config.command.contains(SENSITIVE_MARKERS[3]));
    Ok(())
}

#[tokio::test]
async fn every_family_pages_to_exhaustion_without_skips_or_duplicates() -> Result<()> {
    for family in FAMILIES {
        for length in [0usize, 4, 5] {
            let items: Vec<_> = (1..=length)
                .map(|i| {
                    let mut item = family.summary();
                    item["id"] = json!(format!("{i:024x}"));
                    item["name"] = json!(format!("fixture-{i:03}"));
                    item
                })
                .collect();
            let mut steps = vec![Step::version()];
            let pages = if length == 0 { 1 } else { length.div_ceil(2) };
            for page in 0..pages {
                let start = page * 2;
                let end = (start + 2).min(length);
                steps.push(family.list(page as u64, 2, &items[start..end]));
                if end - start == 2 {
                    steps.push(family.list(
                        (page + 1) as u64,
                        2,
                        &items[end..(end + 2).min(length)],
                    ));
                }
            }
            let operations = expected(&steps);
            let core = MockCore::start(steps).await?;
            let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
            let mut ids = Vec::new();
            let mut page = 0;
            loop {
                let response = session
                    .call(family.list_tool(), json!({"page":page,"page_size":2}))
                    .await?;
                let data = assert_success(&response);
                assert_eq!(data["page"], page);
                assert_eq!(data["page_size"], 2);
                ids.extend(
                    data["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|i| i["id"].as_str().unwrap().to_owned()),
                );
                if !data["has_more"].as_bool().unwrap() {
                    assert!(data["next_page"].is_null());
                    break;
                }
                assert_eq!(data["next_page"], page + 1);
                page = data["next_page"].as_u64().unwrap();
                assert!(page < 4, "continuation must terminate");
            }
            assert_eq!(
                ids,
                items
                    .iter()
                    .map(|i| i["id"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            );
            assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
            session.finish().await?;
            core.assert_finished(&operations);
        }
    }
    Ok(())
}

#[tokio::test]
async fn invalid_arguments_are_safe_and_make_no_core_requests() -> Result<()> {
    let core = MockCore::start(vec![Step::version()]).await?;
    let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
    for family in FAMILIES {
        for input in [
            json!({"page_size":0}),
            json!({"page_size":101}),
            json!({"page_size":-1}),
            json!({"page_size":null}),
            json!({"page":u64::MAX,"page_size":1}),
            json!({"page":u64::MAX/100,"page_size":100}),
            json!({"page":-1}),
            json!({"page":0.5}),
            json!({"page_size":format!("{KEY}{SECRET}")}),
            json!({KEY:SECRET}),
        ] {
            let result = session.call(family.list_tool(), input).await?;
            assert_failure(&result, "invalid_arguments");
        }
        for input in [
            json!({}),
            json!({"id":""}),
            json!({"name":""}),
            json!({"name":" \t"}),
            json!({"id":"not-an-id"}),
            json!({"id":family.summary()["id"],"name":family.summary()["name"]}),
            json!({"id":null,"name":"fixture"}),
            json!({"id":family.summary()["id"],"name":null}),
            json!({"name":{"secret":SECRET}}),
            json!({"name":true}),
            json!({KEY:SECRET}),
        ] {
            let result = session.call(family.get_tool(), input).await?;
            assert_failure(&result, "invalid_arguments");
        }
    }
    session.finish().await?;
    core.assert_finished(&["GetVersion"]);
    Ok(())
}

#[tokio::test]
async fn names_are_exact_and_id_shaped_names_never_select_another_id() -> Result<()> {
    for family in FAMILIES {
        let mut summary = family.summary();
        let id = summary["id"].as_str().unwrap().to_owned();
        let mut detail = family.detail();
        summary["name"] = json!("shared-name");
        detail["name"] = json!("shared-name");
        let mut neighbor = summary.clone();
        neighbor["id"] = json!("000000000000000000000005");
        neighbor["name"] = json!("shared-name-next");
        let id_shaped_name = "000000000000000000000004";
        let mut named = summary.clone();
        named["name"] = json!(id_shaped_name);
        let mut named_detail = detail.clone();
        named_detail["name"] = json!(id_shaped_name);
        let mut by_id_trap = neighbor.clone();
        by_id_trap["id"] = json!(id_shaped_name);
        by_id_trap["name"] = json!("wrong-resource");
        let mut term_neighbor = neighbor.clone();
        term_neighbor["name"] = json!(format!("{id_shaped_name}-neighbor"));
        let steps = vec![
            Step::version(),
            family.lookup("shared-name", &[neighbor, summary]),
            family.get(&id, reply(&detail)),
            family.list_step(
                0,
                100,
                &[],
                &[id_shaped_name],
                reply(&json!([by_id_trap, term_neighbor, named])),
            ),
            family.get(&id, reply(&named_detail)),
            family.list_step(0, 100, &[], &[id_shaped_name], reply(&json!([]))),
        ];
        let operations = expected(&steps);
        let core = MockCore::start(steps).await?;
        let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
        let result = session
            .call(family.get_tool(), json!({"name":"shared-name"}))
            .await?;
        assert_eq!(assert_success(&result)["id"], id);
        let result = session
            .call(family.get_tool(), json!({"name":id_shaped_name}))
            .await?;
        assert_eq!(assert_success(&result)["id"], id);
        let result = session
            .call(family.get_tool(), json!({"name":id_shaped_name}))
            .await?;
        assert_failure(&result, "not_found");
        session.finish().await?;
        core.assert_finished(&operations);
    }
    Ok(())
}

#[tokio::test]
async fn ambiguity_is_actionable_bounded_and_redacts_candidate_names() -> Result<()> {
    for family in FAMILIES {
        let name = format!("duplicate-{KEY}-{SECRET}");
        let matches: Vec<_> = (1..=8)
            .map(|i| {
                let mut item = family.summary();
                item["id"] = json!(format!("{i:024x}"));
                item["name"] = json!(name);
                item
            })
            .collect();
        let steps = vec![Step::version(), family.lookup(&name, &matches)];
        let operations = expected(&steps);
        let core = MockCore::start(steps).await?;
        let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
        let result = session
            .call(family.get_tool(), json!({"name":name}))
            .await?;
        let error = assert_failure(&result, "ambiguous_target");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("explicit candidate ID")
        );
        let candidates = error["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 5);
        for candidate in candidates {
            assert_keys(candidate, &["id", "name"]);
            assert!(candidate["name"].as_str().unwrap().contains("[redacted]"));
        }
        session.finish().await?;
        core.assert_finished(&operations);
    }
    Ok(())
}

#[tokio::test]
async fn fallback_search_is_finite_and_reports_incomplete_resolution() -> Result<()> {
    for family in FAMILIES {
        for exhaust in [true, false] {
            let name = "000000000000000000000004";
            let mut steps = vec![Step::version()];
            let full_page: Vec<_> = (0..100)
                .map(|i| {
                    let mut item = family.summary();
                    item["name"] = json!(format!("{name}-neighbor-{i}"));
                    item
                })
                .collect();
            let count = if exhaust { 3 } else { 4 };
            for page in 0..count {
                steps.push(family.list_step(page, 100, &[], &[name], reply(&json!(full_page))));
            }
            let mut matching = family.summary();
            matching["name"] = json!(name);
            let mut detail = family.detail();
            detail["name"] = json!(name);
            if exhaust {
                steps.push(family.list_step(
                    3,
                    100,
                    &[],
                    &[name],
                    reply(&json!([matching.clone()])),
                ));
                steps.push(family.get(matching["id"].as_str().unwrap(), reply(&detail)));
            }
            let operations = expected(&steps);
            let core = MockCore::start(steps).await?;
            let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
            let result = session
                .call(family.get_tool(), json!({"name":name}))
                .await?;
            if exhaust {
                assert_success(&result);
            } else {
                assert_failure(&result, "incomplete_resolution");
            }
            session.finish().await?;
            core.assert_finished(&operations);
        }
    }
    Ok(())
}

#[tokio::test]
async fn targets_respect_swarm_precedence_and_missing_display_names() -> Result<()> {
    for family in [Family::Stack, Family::Deployment] {
        for mode in ["server", "swarm", "unassigned"] {
            let mut summary = family.summary();
            let mut detail = family.detail();
            summary["info"]["server_name"] = json!("");
            if mode == "swarm" {
                summary["info"]["swarm_id"] = json!("000000000000000000000004");
                summary["info"]["swarm_name"] = json!("");
                detail["config"]["swarm_id"] = summary["info"]["swarm_id"].clone();
            }
            if mode == "unassigned" {
                summary["info"]["server_id"] = json!("");
                detail["config"]["server_id"] = json!("");
                summary["info"]["state"] = json!("unknown");
            }
            let id = summary["id"].as_str().unwrap();
            let steps = vec![
                Step::version(),
                family.list(0, 30, std::slice::from_ref(&summary)),
                family.lookup(id, std::slice::from_ref(&summary)),
                family.get(id, reply(&detail)),
            ];
            let operations = expected(&steps);
            let core = MockCore::start(steps).await?;
            let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
            for (tool, args) in [
                (family.list_tool(), json!({})),
                (family.get_tool(), json!({"id":id})),
            ] {
                let result = session.call(tool, args).await?;
                let data = assert_success(&result);
                let item = if tool == family.list_tool() {
                    &data["items"][0]
                } else {
                    data
                };
                if mode == "unassigned" {
                    assert!(item["target"].is_null());
                    assert_eq!(item["state"], "unknown");
                } else {
                    assert_keys(&item["target"], &["kind", "id", "name"]);
                    assert_eq!(item["target"]["kind"], mode);
                    assert_eq!(item["target"]["id"], summary["info"][format!("{mode}_id")]);
                    assert!(item["target"]["name"].is_null());
                }
            }
            session.finish().await?;
            core.assert_finished(&operations);
        }
    }
    Ok(())
}

#[tokio::test]
async fn missing_permission_failures_and_identity_races_stay_distinct() -> Result<()> {
    for family in FAMILIES {
        let summary = family.summary();
        let id = summary["id"].as_str().unwrap();
        let missing = json!({"error":format!("Did not find any {} matching {id}",match family{Family::Server=>"Server",Family::Stack=>"Stack",Family::Deployment=>"Deployment"}),"trace":[KEY,SECRET,SENSITIVE_MARKERS[4]]});
        let permission = json!({"error":format!("User does not have required permissions on this {}. Must have at least Read permissions",match family{Family::Server=>"Server",Family::Stack=>"Stack",Family::Deployment=>"Deployment"}),"trace":[KEY,SECRET,SENSITIVE_MARKERS[4]]});
        let mut changed = family.detail();
        changed["_id"]["$oid"] = json!("000000000000000000000005");
        let mut renamed = family.detail();
        renamed["name"] = json!("replacement");
        let failures=[(Reply::new(500,missing.to_string()),"not_found"),(Reply::new(500,permission.to_string()),"permission_denied"),(Reply::new(401,AUTHENTICATION),"authentication"),(Reply::new(403,permission.to_string()),"permission_denied"),(Reply::new(500,json!({"error":format!("Did not find any Server matching unrelated database {KEY}"),"trace":[SECRET,SENSITIVE_MARKERS[4]]}).to_string()),"upstream"),(reply(&changed),"invalid_response"),(reply(&renamed),"invalid_response"),(Reply::new(200,format!("invalid {KEY}{SECRET}")),"invalid_response")];
        let mut steps = vec![Step::version(), family.lookup("missing-name", &[])];
        for (failure, _) in &failures {
            steps.push(family.lookup(id, std::slice::from_ref(&summary)));
            steps.push(family.get(id, failure.clone()));
        }
        steps.push(family.list(0, 30, &[]));
        let operations = expected(&steps);
        let core = MockCore::start(steps).await?;
        let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
        let result = session
            .call(family.get_tool(), json!({"name":"missing-name"}))
            .await?;
        assert_failure(&result, "not_found");
        for (_, kind) in failures {
            let result = session.call(family.get_tool(), json!({"id":id})).await?;
            let error = assert_failure(&result, kind);
            assert_eq!(error["operation"], family.get_operation());
        }
        assert_success(&session.call(family.list_tool(), json!({})).await?);
        session.finish().await?;
        core.assert_finished(&operations);
    }
    Ok(())
}

#[tokio::test]
async fn production_list_and_probe_failures_are_typed_and_recoverable() -> Result<()> {
    for family in FAMILIES {
        let mut stalled = Reply::new(200, "[]");
        stalled.body_delay = Duration::from_secs(3);
        let failures = [
            (Reply::echoed(401, AUTHENTICATION), "authentication"),
            (Reply::new(403, PERMISSION), "permission_denied"),
            (
                Reply::new(502, format!("proxy {KEY}{SECRET} {}", SENSITIVE_MARKERS[4])),
                "upstream",
            ),
            (Reply::new(200, "{}"), "invalid_response"),
            (stalled, "timeout"),
        ];
        for probe in [false, true] {
            let mut steps = vec![Step::version()];
            for (failure, _) in &failures {
                if probe {
                    steps.push(family.list(0, 1, &[family.summary()]));
                }
                steps.push(family.list_step(
                    if probe { 1 } else { 0 },
                    1,
                    &[],
                    &[],
                    failure.clone(),
                ));
            }
            steps.push(family.list(0, 30, &[]));
            let operations = expected(&steps);
            let core = MockCore::start(steps).await?;
            let mut session = McpSession::start(&core, Duration::from_millis(300)).await?;
            for (_, kind) in &failures {
                let result = session
                    .call(family.list_tool(), json!({"page_size":1}))
                    .await?;
                let error = assert_failure(&result, kind);
                assert_eq!(error["operation"], family.list_operation());
            }
            assert_success(&session.call(family.list_tool(), json!({})).await?);
            session.finish().await?;
            core.assert_finished(&operations);
        }
    }
    Ok(())
}

#[tokio::test]
async fn selected_display_fields_are_redacted_and_unavailable_fields_are_null() -> Result<()> {
    for family in FAMILIES {
        let mut summary = family.summary();
        let mut detail = family.detail();
        let display = format!("display-{KEY}-{SECRET}");
        summary["name"] = json!(display);
        detail["name"] = json!(display);
        match family {
            Family::Server => {
                summary["info"]["region"] = json!(display);
                summary["info"]["version"] = json!(display);
            }
            Family::Stack => {
                summary["info"]["server_name"] = json!(display);
                detail["info"]["deployed_project_name"] = json!(display);
            }
            Family::Deployment => {
                summary["info"]["server_name"] = json!(display);
                summary["info"]["image"] = json!(display);
                summary["info"]["custom_name"] = json!(display);
            }
        }
        let id = summary["id"].as_str().unwrap();
        let steps = vec![
            Step::version(),
            family.lookup(id, std::slice::from_ref(&summary)),
            family.get(id, reply(&detail)),
        ];
        let operations = expected(&steps);
        let core = MockCore::start(steps).await?;
        let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
        let result = session.call(family.get_tool(), json!({"id":id})).await?;
        assert!(
            assert_success(&result)["name"]
                .as_str()
                .unwrap()
                .contains("[redacted]")
        );
        session.finish().await?;
        core.assert_finished(&operations);
    }
    Ok(())
}

#[tokio::test]
async fn observed_states_and_unavailable_fields_remain_distinct() -> Result<()> {
    for family in FAMILIES {
        let states: &[&str] = match family {
            Family::Server => &["Ok", "NotOk", "Disabled"],
            Family::Stack => &["unknown", "running", "stopped", "unhealthy", "down"],
            Family::Deployment => &["unknown", "running", "exited", "unhealthy", "not_deployed"],
        };
        let mut steps = vec![Step::version()];
        for state in states {
            let mut summary = family.summary();
            summary["info"]["state"] = json!(state);
            if matches!(family, Family::Server) {
                summary["info"]["region"] = json!("");
                summary["info"]["version"] = Value::Null;
            }
            if matches!(family, Family::Deployment) {
                summary["info"]["image"] = json!("");
                summary["info"]["custom_name"] = json!("");
            }
            steps.push(family.list(0, 100, &[summary]));
        }
        let operations = expected(&steps);
        let core = MockCore::start(steps).await?;
        let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
        for state in states {
            let result = session
                .call(family.list_tool(), json!({"page_size":100}))
                .await?;
            let item = &assert_success(&result)["items"][0];
            let state = match *state {
                "Ok" => "ok",
                "NotOk" => "not_ok",
                "Disabled" => "disabled",
                other => other,
            };
            assert_eq!(item["state"], state);
            match family {
                Family::Server => {
                    assert!(item["region"].is_null());
                    assert!(item["periphery_version"].is_null());
                }
                Family::Deployment => {
                    assert!(item["image"].is_null());
                    assert_eq!(item["runtime_name"], family.summary()["name"]);
                }
                Family::Stack => {}
            }
        }
        session.finish().await?;
        core.assert_finished(&operations);
    }
    Ok(())
}

#[tokio::test]
async fn detail_reads_obey_deadlines_and_detect_target_reassignment() -> Result<()> {
    for family in FAMILIES {
        let summary = family.summary();
        let id = summary["id"].as_str().unwrap();
        let mut stalled = reply(&family.detail());
        stalled.body_delay = Duration::from_secs(3);
        let mut steps = vec![
            Step::version(),
            family.lookup(id, std::slice::from_ref(&summary)),
            family.get(id, stalled),
        ];
        if !matches!(family, Family::Server) {
            let mut moved = family.detail();
            moved["config"]["swarm_id"] = json!("000000000000000000000004");
            steps.push(family.lookup(id, std::slice::from_ref(&summary)));
            steps.push(family.get(id, reply(&moved)));
        }
        steps.push(family.list(0, 30, &[]));
        let operations = expected(&steps);
        let core = MockCore::start(steps).await?;
        let mut session = McpSession::start(&core, Duration::from_millis(300)).await?;
        let result = session.call(family.get_tool(), json!({"id":id})).await?;
        assert_eq!(
            assert_failure(&result, "timeout")["operation"],
            family.get_operation()
        );
        if !matches!(family, Family::Server) {
            let result = session.call(family.get_tool(), json!({"id":id})).await?;
            let error = assert_failure(&result, "invalid_response");
            assert!(
                error["correction"]
                    .as_str()
                    .unwrap()
                    .contains("resource changed")
            );
        }
        assert_success(&session.call(family.list_tool(), json!({})).await?);
        session.finish().await?;
        core.assert_finished(&operations);
    }
    Ok(())
}

#[tokio::test]
async fn incompatible_list_responses_and_selector_mismatches_never_return_wrong_targets()
-> Result<()> {
    for family in FAMILIES {
        let summary = family.summary();
        let id = summary["id"].as_str().unwrap();
        let mut no_name = summary.clone();
        no_name.as_object_mut().unwrap().remove("name");
        let mut no_id = summary.clone();
        no_id["id"] = json!("");
        let mut wrong_family = summary.clone();
        wrong_family["type"] = json!(if matches!(family, Family::Server) {
            "Stack"
        } else {
            "Server"
        });
        let steps = vec![
            Step::version(),
            family.list(0, 1, &[summary.clone(), summary.clone()]),
            family.list(0, 30, &[no_name]),
            family.list(0, 30, &[no_id]),
            family.list(0, 30, &[wrong_family]),
            family.lookup("000000000000000000000008", std::slice::from_ref(&summary)),
            family.lookup("FIXTURE-NAME", std::slice::from_ref(&summary)),
            family.list(0, 30, &[]),
        ];
        let operations = expected(&steps);
        let core = MockCore::start(steps).await?;
        let mut session = McpSession::start(&core, TEST_TIMEOUT).await?;
        for args in [json!({"page_size":1}), json!({}), json!({}), json!({})] {
            let result = session.call(family.list_tool(), args).await?;
            assert_failure(&result, "invalid_response");
        }
        for selector in [
            json!({"id":"000000000000000000000008"}),
            json!({"name":"FIXTURE-NAME"}),
        ] {
            let result = session.call(family.get_tool(), selector).await?;
            assert_failure(&result, "not_found");
        }
        let result = session.call(family.list_tool(), json!({})).await?;
        assert_success(&result);
        assert!(
            !result.to_string().contains(id),
            "empty result contains a discarded identity"
        );
        session.finish().await?;
        core.assert_finished(&operations);
    }
    Ok(())
}
