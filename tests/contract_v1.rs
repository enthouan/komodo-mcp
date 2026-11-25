//! Freeze discovery and observable calls from production stdio and HTTP processes.
#[path = "support/contract_calls.rs"]
mod calls;
#[path = "support/docker.rs"]
mod fixture;
#[path = "support/http.rs"]
mod http;
#[path = "support/peer.rs"]
mod peer;
#[path = "../examples/support/schema_pool.rs"]
mod schema_pool;
mod support;

use peer::{Peer, Transport};
use rmcp::model::Tool;
use schema_pool::SchemaPool;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

const BASE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/contracts/v1.json");
const GRANTS: [&str; 4] = [
    "KOMODO_ALLOW_OPERATE",
    "KOMODO_ALLOW_CONFIGURATION_EDIT",
    "KOMODO_ALLOW_PRUNE",
    "KOMODO_ALLOW_TERMINAL",
];

fn settings(mask: usize) -> Vec<(&'static str, &'static str)> {
    GRANTS
        .iter()
        .enumerate()
        .filter(|(bit, _)| mask & (1 << bit) != 0)
        .map(|(_, key)| (*key, "true"))
        .collect()
}

// Return the first changed JSON pointer with bounded values so CI can identify
// the exact field even if a schema containing hundreds of definitions changes.
fn difference(path: &str, old: &Value, new: &Value) -> Option<String> {
    if old == new {
        return None;
    }
    if let (Some(a), Some(b)) = (old.as_object(), new.as_object()) {
        for key in a.keys().chain(b.keys()) {
            let field = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
            match (a.get(key), b.get(key)) {
                (Some(x), Some(y)) => {
                    if let Some(diff) = difference(&field, x, y) {
                        return Some(diff);
                    }
                }
                (Some(_), None) => return Some(format!("removed {field}")),
                (None, Some(_)) => return Some(format!("added {field}")),
                _ => unreachable!(),
            }
        }
    }
    if let (Some(a), Some(b)) = (old.as_array(), new.as_array()) {
        for (index, (x, y)) in a.iter().zip(b).enumerate() {
            if let Some(diff) = difference(&format!("{path}/{index}"), x, y) {
                return Some(diff);
            }
        }
        return Some(format!("{path} length {} -> {}", a.len(), b.len()));
    }
    let brief = |v: &Value| v.to_string().chars().take(160).collect::<String>();
    Some(format!("{path}: {} -> {}", brief(old), brief(new)))
}

fn compare(label: &str, old: &Value, new: &Value) {
    if let Some(change) = difference("", old, new) {
        panic!("{label}: {change}");
    }
}

fn expand(catalog: &Value, id: &str) -> Value {
    let mut schema = catalog["schemas"][id].clone();
    if let Some(names) = catalog["schemaDefinitions"][id].as_object() {
        schema["$defs"] = Value::Object(
            names
                .iter()
                .map(|(local, shared)| {
                    (
                        local.clone(),
                        catalog["definitions"][shared.as_str().unwrap()].clone(),
                    )
                })
                .collect(),
        );
    }
    schema
}

// Intern only the live result. The website's catalog is an independent cross-check
// and also documents execute_terminal's success type outside wire discovery.
fn discovery(tools: &BTreeMap<String, Tool>, catalog: &Value) -> Value {
    let mut pool = SchemaPool::default();
    let mut entries = BTreeMap::new();
    for (name, tool) in tools {
        let mut entry = serde_json::to_value(tool).unwrap();
        let docs = &catalog["tools"][name];
        assert!(docs.is_object(), "generated catalog missing {name}");
        for key in ["annotations", "description"] {
            compare(&format!("catalog {name}/{key}"), &docs[key], &entry[key]);
        }
        for (field, suffix) in [("inputSchema", "input"), ("outputSchema", "output")] {
            if field == "outputSchema" && docs["outputSource"] == "resultType" {
                assert!(
                    entry.get(field).is_none(),
                    "{name} now advertises an output schema"
                );
                continue;
            }
            let schema = entry
                .as_object_mut()
                .unwrap()
                .remove(field)
                .unwrap_or_else(|| panic!("{name} missing {field}"));
            compare(
                &format!("catalog {name}/{field}"),
                &expand(catalog, docs[field].as_str().unwrap()),
                &schema,
            );
            entry[field] = json!(pool.intern(format!("{name}.{suffix}"), schema).unwrap());
        }
        entries.insert(name, entry);
    }
    assert_eq!(
        entries.len(),
        catalog["tools"].as_object().unwrap().len(),
        "catalog tool count"
    );
    json!({"tools": entries, "schemaPool": pool})
}

async fn handshake(peer: &Peer) -> (Value, Option<Value>) {
    let (mut init, transport) = match peer {
        Peer::Stdio(_, session) => (
            serde_json::to_value(&*session.peer_info().unwrap()).unwrap(),
            None,
        ),
        Peer::Http(binary, owner) => {
            use reqwest::Method;
            let http = &binary.http;
            let (session, init) = http.initialize_session(http::FIRST, None).await;
            assert_ne!(
                &session, owner,
                "each successful initialization must issue a separate session"
            );
            let unauthorized =
                http::post(http.request(Method::POST, None, None), http::initialize())
                    .send()
                    .await
                    .unwrap();
            let foreign = http
                .request(Method::DELETE, Some(http::SECOND), Some(&session))
                .send()
                .await
                .unwrap();
            let deleted = http
                .request(Method::DELETE, Some(http::FIRST), Some(&session))
                .send()
                .await
                .unwrap();
            for response in [&unauthorized, &foreign] {
                assert!(!response.headers().contains_key("mcp-session-id"));
            }
            assert_eq!(unauthorized.status(), 401);
            assert_eq!(foreign.status(), 404);
            assert!(deleted.status().is_success());
            (
                init,
                Some(
                    json!({"uniqueNonemptySessionIds":true,"unauthenticatedInitializeStatus":unauthorized.status().as_u16(),
                "foreignSessionDeleteStatus":foreign.status().as_u16(),"ownedSessionDeleteStatus":deleted.status().as_u16()}),
                ),
            )
        }
    };
    assert_eq!(init["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    init["serverInfo"]["version"] = json!("<Cargo package version>");
    (init, transport)
}

async fn observe(transport: Transport) -> (Value, Option<Value>) {
    let fixture = fixture::Core::start().await;
    let address = &fixture.address;
    let catalog: Value = serde_json::from_str(include_str!("../docs/tool-catalog.json")).unwrap();
    let full = transport.connect(address, &settings(15)).await;
    let (init, evidence) = handshake(&full).await;
    let all = full.catalog().await;
    let schemas = discovery(&all, &catalog);
    full.finish().await;
    let mut profiles = BTreeMap::new();
    for mask in 0..16 {
        let grants = settings(mask);
        let enabled: Vec<_> = grants.iter().map(|(grant, _)| *grant).collect();
        let tools = if mask == 15 {
            all.clone()
        } else {
            let peer = transport.connect(address, &grants).await;
            let selected = peer.catalog().await;
            peer.finish().await;
            selected
        };
        for (name, full_tool) in &all {
            let expected = catalog["tools"][name]["grantSets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|set| {
                    set.as_array()
                        .unwrap()
                        .iter()
                        .all(|grant| enabled.contains(&grant.as_str().unwrap()))
                });
            assert_eq!(
                tools.contains_key(name),
                expected,
                "catalog visibility: {enabled:?}/{name}"
            );
            if let Some(visible) = tools.get(name) {
                assert!(
                    visible == full_tool,
                    "profile changed {name} metadata: {enabled:?}"
                );
            }
        }
        assert!(
            tools.keys().all(|name| all.contains_key(name)),
            "unexpected tool under {enabled:?}"
        );
        profiles.insert(
            if enabled.is_empty() {
                "default".to_string()
            } else {
                enabled.join("+")
            },
            tools.into_keys().collect::<Vec<_>>(),
        );
    }
    let calls = calls::capture(transport, &fixture).await;
    fixture.assert_readonly();
    (
        json!({"initialize":init,"discovery":schemas,"profiles":profiles,"calls":calls}),
        evidence,
    )
}

async fn snapshot() -> Value {
    let (shared, _) = observe(Transport::Stdio).await;
    let (other, http) = observe(Transport::Http).await;
    compare(
        "stdio / authenticated HTTP application contract",
        &shared,
        &other,
    );
    let mut result = json!({"formatVersion":1,"application":shared,"streamableHttp":http.unwrap()});
    result.sort_all_objects();
    result
}

#[tokio::test]
async fn running_server_matches_v1() {
    let current = snapshot().await;
    let baseline = fs::read_to_string(BASE).expect("missing reviewed docs/contracts/v1.json");
    let baseline: Value = serde_json::from_str(&baseline).unwrap();
    if let Some(change) = difference("", &baseline, &current) {
        let actual = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/contract-v1.actual.json");
        fs::create_dir_all(actual.parent().unwrap()).unwrap();
        fs::write(
            &actual,
            serde_json::to_string_pretty(&current).unwrap() + "\n",
        )
        .unwrap();
        panic!(
            "v1 contract drift: {change}\nInspect: git diff --no-index docs/contracts/v1.json target/contract-v1.actual.json"
        );
    }
}

#[tokio::test]
#[ignore = "explicitly regenerate from both running transports after reviewing a contract change"]
async fn regenerate_v1() {
    let actual = snapshot().await;
    fs::create_dir_all(Path::new(BASE).parent().unwrap()).unwrap();
    fs::write(BASE, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
}

#[test]
fn one_changed_annotation_identifies_the_public_field() {
    let reviewed = json!({"application":{"discovery":{"tools":{
        "list_containers":{"annotations":{"idempotentHint":true}}
    }}}});
    let mut changed = reviewed.clone();
    *changed
        .pointer_mut("/application/discovery/tools/list_containers/annotations/idempotentHint")
        .unwrap() = json!(false);
    assert_eq!(
        difference("", &reviewed, &changed).as_deref(),
        Some(
            "/application/discovery/tools/list_containers/annotations/idempotentHint: true -> false"
        )
    );
}
