//! Acceptance through the compiled production stdio executable, including trace diagnostics.
#[path = "support/export.rs"]
mod fixture;
mod support;

use fixture::{Core, GROUP, KINDS, PRIVATE, id, input};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use support::operations::{Process, call};

fn data(result: CallToolResult) -> Value {
    let succeeded = result.is_error == Some(false);
    let text = serde_json::to_string(&result).unwrap();
    support::assert_safe(&text);
    for secret in PRIVATE {
        assert!(!text.contains(secret), "private TOML leaked");
    }
    let value = result.structured_content.unwrap();
    if succeeded {
        let duplicate: Value =
            serde_json::from_str(result.content[0].as_text().unwrap().text.as_str()).unwrap();
        assert_eq!(duplicate, value);
    }
    value
}
fn success(result: CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    let value = data(result);
    assert_eq!(value["mutation_requests"], 0);
    assert_eq!(value["authoritative_apply_input"], false);
    let source = value["toml"].as_str().unwrap();
    source.parse::<toml::Table>().unwrap();
    assert!(source.len() <= komodo_mcp::export::MAX_TOML_BYTES);
    assert!(serde_json::to_vec(&value).unwrap().len() <= komodo_mcp::export::MAX_OUTPUT_BYTES);
    value
}
fn failure(result: CallToolResult, kind: &str) -> Value {
    assert_eq!(result.is_error, Some(true), "{result:?}");
    let value = data(result);
    assert_eq!(value["kind"], kind, "{value}");
    assert!(value.get("toml").is_none());
    value
}

#[tokio::test]
async fn all_supported_targets_are_exact_and_privileged_toml_is_structurally_sanitized() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let value=success(call(&s,"export_resources_toml",json!({"targets":(0..11).map(input).collect::<Vec<_>>(),"user_groups":[{"name":"selected-group"}],"include_variables":true})).await);
    assert_eq!(value["targets"].as_array().unwrap().len(), 11);
    assert_eq!(value["selection_complete"], true);
    assert_eq!(value["variables_returned"], 2);
    assert_eq!(value["variable_scope"], "all_core_variables");
    assert!(value["omitted_fields"].as_u64().unwrap() > 30);
    let document = value["toml"]
        .as_str()
        .unwrap()
        .parse::<toml::Table>()
        .unwrap();
    assert_eq!(document.len(), 13);
    for (kind, _, _) in KINDS {
        assert_eq!(document[kind].as_array().unwrap().len(), 1);
        let entry = &document[kind][0];
        assert_eq!(
            entry["name"].as_str(),
            Some(format!("selected-{kind}").as_str())
        );
        assert!(entry.get("description").is_none());
        assert!(entry["config"].get("environment").is_none());
        assert!(entry["config"].get("file_contents").is_none());
    }
    assert_eq!(
        document["stack"][0]["config"]["server"].as_str(),
        Some("outside-selection")
    );
    assert!(
        document["variable"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.get("value").is_none())
    );
    assert!(document["user_group"][0].get("users").is_none());
    assert!(!value.to_string().contains("private-user"));
    let export = core
        .requests()
        .into_iter()
        .filter(|r| r.operation == "ExportResourcesToToml")
        .collect::<Vec<_>>();
    assert_eq!(export.len(), 1);
    assert_eq!(export[0].params["targets"].as_array().unwrap().len(), 11);
    assert_eq!(export[0].params["user_groups"], json!([GROUP]));
    core.assert_reads_only();
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn single_target_excludes_unrelated_resources_and_works_in_every_local_profile() {
    for (operate, edit) in [(false, false), (true, false), (false, true), (true, true)] {
        let core = Core::start().await;
        let (p, s) = Process::start_profile(&core.address, operate, edit).await;
        let tools = s.list_all_tools().await.unwrap();
        let tool = tools
            .iter()
            .find(|t| t.name == "export_resources_toml")
            .unwrap();
        let tool = serde_json::to_value(tool).unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        let value = success(call(&s, "export_resources_toml", json!({"targets":[input(2)]})).await);
        let document = value["toml"]
            .as_str()
            .unwrap()
            .parse::<toml::Table>()
            .unwrap();
        assert_eq!(document.len(), 1);
        assert!(document.contains_key("stack"));
        assert_eq!(value["variable_scope"], "excluded");
        assert_eq!(value["user_groups"], json!([]));
        let requests = core.requests();
        assert_eq!(
            requests.last().unwrap().params,
            json!({"targets":[{"type":"Stack","id":id(2)}],"user_groups":[],"include_variables":false,"existing":null})
        );
        core.assert_reads_only();
        p.finish_excluding(s, &PRIVATE).await;
    }
}

#[tokio::test]
async fn absent_empty_invalid_or_unbounded_selection_never_falls_back_to_export_all() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, true, true).await;
    let before = core.requests().len();
    for value in [
        json!({}),
        json!({"targets":[]}),
        json!({"targets":[],"include_variables":false}),
        json!({"targets":[{"kind":"system","target":{"name":"all"}}]}),
        json!({"targets":[{"kind":"stack","target":{}}]}),
        json!({"targets":[{"kind":"stack","target":{"id":id(2),"name":"selected-stack"}}]}),
        json!({"targets":[{"kind":"stack","target":{"name":"x".repeat(129)}}]}),
        json!({"targets":vec![input(2);17]}),
        json!({"targets":[],"user_groups":vec![json!({"name":"selected-group"});17]}),
        json!({"targets":null,"include_variables":true}),
        json!({"targets":[],"include_variables":true,"existing":{"variable":[{"value":PRIVATE[2]}]}}),
        json!({"targets":[],"user_groups":[null]}),
        json!({"targets":[input(2)],"include_variables":null}),
    ] {
        failure(
            call(&s, "export_resources_toml", value).await,
            "invalid_arguments",
        );
    }
    assert_eq!(core.requests().len(), before);
    core.assert_reads_only();
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn groups_and_all_variables_are_independent_explicit_scopes() {
    let core = Core::start().await;
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let vars = success(
        call(
            &s,
            "export_resources_toml",
            json!({"targets":[],"include_variables":true}),
        )
        .await,
    );
    assert_eq!(vars["variables_returned"], 2);
    assert_eq!(vars["targets"], json!([]));
    assert!(
        core.requests()
            .iter()
            .all(|r| !r.operation.starts_with("List") && !r.operation.starts_with("GetS"))
    );
    let groups = success(
        call(
            &s,
            "export_resources_toml",
            json!({"targets":[],"user_groups":[{"id":GROUP}]}),
        )
        .await,
    );
    assert_eq!(groups["variables_returned"], 0);
    assert_eq!(groups["user_groups"][0]["id"], GROUP);
    assert_eq!(groups["selection_complete"], true);
    let document = groups["toml"]
        .as_str()
        .unwrap()
        .parse::<toml::Table>()
        .unwrap();
    assert_eq!(document.len(), 1);
    assert!(document.contains_key("user_group"));
    failure(
        call(
            &s,
            "export_resources_toml",
            json!({"targets":[],"user_groups":[{"name":"invisible-group"}]}),
        )
        .await,
        "not_found",
    );
    core.assert_reads_only();
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn missing_and_ambiguous_targets_and_upstream_denials_remain_errors() {
    for mode in [
        "missing",
        "ambiguous",
        "list-denied",
        "get-denied",
        "export-denied",
        "groups-denied",
    ] {
        let core = Core::start().await;
        {
            let mut d = core.data.lock().unwrap();
            match mode {
                "missing" => d.missing = true,
                "ambiguous" => d.ambiguous = true,
                "list-denied" => d.denied = Some("ListStacks".into()),
                "get-denied" => d.denied = Some("GetStack".into()),
                "export-denied" => d.denied = Some("ExportResourcesToToml".into()),
                "groups-denied" => d.denied = Some("ListUserGroups".into()),
                _ => unreachable!(),
            }
        }
        let (p, s) = Process::start_profile(&core.address, true, true).await;
        let args = if mode == "ambiguous" {
            json!({"targets":[{"kind":"stack","target":{"name":"selected-stack"}}]})
        } else if mode == "groups-denied" {
            json!({"targets":[],"user_groups":[{"id":GROUP}]})
        } else {
            json!({"targets":[input(2)]})
        };
        failure(
            call(&s, "export_resources_toml", args).await,
            match mode {
                "missing" => "not_found",
                "ambiguous" => "ambiguous_target",
                _ => "permission_denied",
            },
        );
        assert_eq!(
            core.requests()
                .iter()
                .filter(|r| r.operation == "ExportResourcesToToml")
                .count(),
            usize::from(mode == "export-denied")
        );
        core.assert_reads_only();
        p.finish_excluding(s, &PRIVATE).await;
    }
}

#[tokio::test]
async fn exact_object_id_shaped_names_use_name_resolution_and_duplicates_are_rejected() {
    let core = Core::start().await;
    let name = "abcdefabcdefabcdefabcdef";
    core.data
        .lock()
        .unwrap()
        .resources
        .get_mut("Stack")
        .unwrap()["name"] = json!(name);
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let target = json!({"kind":"stack","target":{"name":name}});
    let value = success(
        call(
            &s,
            "export_resources_toml",
            json!({"targets":[target.clone()]}),
        )
        .await,
    );
    assert_eq!(value["targets"][0]["id"], id(2));
    let query = &core
        .requests()
        .into_iter()
        .find(|r| r.operation == "ListStacks")
        .unwrap()
        .params["query"];
    assert_eq!(query["names"], json!([]));
    assert_eq!(query["terms"], json!([name]));
    failure(
        call(
            &s,
            "export_resources_toml",
            json!({"targets":[target,input(2)]}),
        )
        .await,
        "invalid_arguments",
    );
    failure(
        call(
            &s,
            "export_resources_toml",
            json!({"targets":[],"user_groups":[{"name":"selected-group"},{"id":GROUP}]}),
        )
        .await,
        "invalid_arguments",
    );
    assert_eq!(
        core.requests()
            .iter()
            .filter(|r| r.operation == "ExportResourcesToToml")
            .count(),
        1
    );
    core.assert_reads_only();
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn core_omissions_are_explicit_and_never_claim_a_complete_round_trip() {
    let core = Core::start().await;
    core.data.lock().unwrap().omit_sync = true;
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let value = success(
        call(
            &s,
            "export_resources_toml",
            json!({"targets":[input(0),input(10)]}),
        )
        .await,
    );
    assert_eq!(value["selection_complete"], false);
    assert_eq!(value["targets"][0]["returned_by_core"], true);
    assert_eq!(value["targets"][1]["returned_by_core"], false);
    let document = value["toml"]
        .as_str()
        .unwrap()
        .parse::<toml::Table>()
        .unwrap();
    assert!(!document.contains_key("resource_sync"));
    assert!(
        value["warnings"]
            .to_string()
            .contains("inline or unconfigured ResourceSyncs")
    );
    core.assert_reads_only();
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn malformed_duplicate_or_out_of_scope_toml_is_rejected_without_echoing_it() {
    for source in [
        format!("[[stack]\nname={}\n", json!(PRIVATE[0])),
        "[[server]]\nname='unrelated'\n".into(),
        "[[stack]]\nname='selected-stack'\n[[stack]]\nname='selected-stack'\n".into(),
        "[[variable]]\nname='unrequested-variable'\n".into(),
        "[[unknown]]\nname='selected-stack'\n".into(),
        "[stack]\nname='selected-stack'\n".into(),
        "[[stack]]\nname='selected-stack'\nconfig='invalid'\n".into(),
        format!("[[stack]]\nname={}\n", json!(support::SECRET)),
    ] {
        let core = Core::start().await;
        core.data.lock().unwrap().source = Some(source);
        let (p, s) = Process::start_profile(&core.address, false, false).await;
        failure(
            call(&s, "export_resources_toml", json!({"targets":[input(2)]})).await,
            "invalid_response",
        );
        core.assert_reads_only();
        p.finish_excluding(s, &PRIVATE).await;
    }
}

#[tokio::test]
async fn quoted_keys_comments_multiline_strings_and_nested_secrets_are_removed_structurally() {
    let core = Core::start().await;
    core.data.lock().unwrap().source = Some(format!(
        r#"# {secret}
[[stack]]
name = 'selected-stack'
description = '''{private}'''
[stack.config]
"environment" = '''API_TOKEN={private}
PASSWORD={secret}
'''
file_contents = '''services:
  app:
    environment:
      - PASSWORD={private}
'''
auto_pull = true
[stack.config.unexpected]
"{private}" = {{ nested = ["{secret}", "{private}"] }}
"#,
        secret = support::SECRET,
        private = PRIVATE[0]
    ));
    let (p, s) = Process::start_profile(&core.address, false, false).await;
    let value = success(call(&s, "export_resources_toml", json!({"targets":[input(2)]})).await);
    let parsed: komodo_client::entities::toml::ResourcesToml =
        toml::from_str(value["toml"].as_str().unwrap()).unwrap();
    assert_eq!(parsed.stacks.len(), 1);
    assert_eq!(parsed.stacks[0].config.auto_pull, Some(true));
    assert!(value["omitted_fields"].as_u64().unwrap() >= 4);
    core.assert_reads_only();
    p.finish_excluding(s, &PRIVATE).await;
}

#[tokio::test]
async fn body_source_entry_and_serialized_output_budgets_fail_without_partial_documents() {
    for mode in ["json", "body", "source", "entries", "output"] {
        let core = Core::start().await;
        {
            let mut d = core.data.lock().unwrap();
            match mode {
                "json" => d.body = Some(format!("{{{}", PRIVATE[0])),
                "body" => {
                    d.body = Some("x".repeat(komodo_mcp::client::MAX_EVIDENCE_RESPONSE_BYTES + 1))
                }
                "source" => {
                    d.source = Some(format!(
                        "#{}",
                        "x".repeat(komodo_mcp::export::MAX_SOURCE_BYTES)
                    ))
                }
                "entries" => {
                    d.source = Some(
                        (0..129)
                            .map(|i| format!("[[variable]]\nname='v{i}'\n"))
                            .collect(),
                    )
                }
                "output" => {
                    let refs = (0..32)
                        .map(|i| format!("tag{i}{}", "x".repeat(115)))
                        .collect::<Vec<_>>();
                    let mut source = String::new();
                    for (kind, _, _) in KINDS {
                        source += &format!(
                            "[[{kind}]]\nname='selected-{kind}'\ntags={}\nafter={}\n",
                            json!(refs),
                            json!(refs)
                        );
                    }
                    assert!(source.len() < komodo_mcp::export::MAX_SOURCE_BYTES);
                    d.source = Some(source);
                }
                _ => unreachable!(),
            }
        }
        let (p, s) = Process::start_profile(&core.address, false, false).await;
        let args = if mode == "entries" {
            json!({"targets":[],"include_variables":true})
        } else if mode == "output" {
            json!({"targets":(0..11).map(input).collect::<Vec<_>>()})
        } else {
            json!({"targets":[input(2)]})
        };
        failure(
            call(&s, "export_resources_toml", args).await,
            if mode == "json" {
                "invalid_response"
            } else {
                "response_too_large"
            },
        );
        core.assert_reads_only();
        p.finish_excluding(s, &PRIVATE).await;
    }
}
