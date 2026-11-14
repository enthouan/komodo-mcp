//! Cross-family production-stdio acceptance. These are deterministic owned wire
//! fixtures; actual Core authorization/build/push/health evidence is opt-in live.
#[allow(dead_code)]
#[path = "support/actions.rs"]
mod actions;
#[allow(dead_code)]
#[path = "support/build_operations.rs"]
mod builds;
#[allow(dead_code)]
#[path = "support/build_definitions.rs"]
mod definitions;
#[allow(dead_code)]
#[path = "support/procedures.rs"]
mod procedures;
#[allow(dead_code)]
#[path = "support/repositories.rs"]
mod repos;
#[allow(dead_code)]
#[path = "support/schedules.rs"]
mod schedules;
mod support;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::operations::{self, Process, Request, Task, call};

const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

// Compose existing pinned-wire fixtures behind one authenticated Core endpoint.
// Namespace Mongo update IDs so history from each resource remains independent.
// Parent objects used by scheduling are the same objects used by execution.
struct Core {
    address: String,
    builds: builds::Core,
    repos: repos::Core,
    actions: actions::Core,
    procedures: procedures::Core,
    schedules: schedules::Core,
    workloads: operations::Core,
    requests: Mutex<Vec<Request>>,
    http: reqwest::Client,
    task: Mutex<Option<Task>>,
}
impl Core {
    async fn start() -> Arc<Self> {
        let builds = builds::Core::start().await;
        let repos = repos::Core::start().await;
        {
            let mut d = builds.definitions.data.lock().unwrap();
            d.repos.insert(
                repos::REPO.into(),
                repos.data.lock().unwrap().repos[repos::REPO].clone(),
            );
            d.builds.get_mut(definitions::BUILD).unwrap()["config"]["linked_repo"] =
                json!(repos::REPO);
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let c = Arc::new(Self {
            address: format!("http://{}", listener.local_addr().unwrap()),
            builds,
            repos,
            actions: actions::Core::start().await,
            procedures: procedures::Core::start().await,
            schedules: schedules::Core::start().await,
            workloads: operations::Core::start(operations::Options {
                outcome: operations::Outcome::Succeeded,
                state: Some("not_deployed".into()),
                ..Default::default()
            })
            .await,
            requests: Mutex::new(vec![]),
            http: reqwest::Client::new(),
            task: Mutex::new(None),
        });
        // Weak state prevents an Arc -> task -> router -> Arc lifecycle cycle.
        let app = Router::new()
            .route("/{path}", post(handle))
            .with_state(Arc::downgrade(&c));
        *c.task.lock().unwrap() = Some(Task(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        })));
        c
    }
    fn sync_schedules(&self) {
        let action = self.actions.data.lock().unwrap().actions[actions::ACTION].clone();
        let procedure =
            self.procedures.data.lock().unwrap().procedures[procedures::PROCEDURE].clone();
        let mut d = self.schedules.data.lock().unwrap();
        for (kind, id, parent) in [
            ("action", actions::ACTION, action),
            ("procedure", procedures::PROCEDURE, procedure),
        ] {
            d.parents.insert((kind.into(), id.into()), parent);
            d.refresh(kind, id);
        }
    }
    fn execute_count(&self) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == "execute")
            .count()
    }
}
fn namespace(id: &mut Value, prefix: char) {
    if let Some(s) = id.as_str() {
        assert_eq!(s.len(), 24);
        *id = json!(format!("{prefix}{}", &s[1..]));
    }
}
async fn handle(
    State(weak): State<std::sync::Weak<Core>>,
    Path(path): Path<String>,
    headers: HeaderMap,
    Json(mut v): Json<Value>,
) -> Response {
    let c = weak.upgrade().unwrap();
    assert_eq!(headers["x-api-key"], support::KEY);
    assert_eq!(headers["x-api-secret"], support::SECRET);
    let op = v["type"].as_str().unwrap().to_owned();
    let path = match path.as_str() {
        "read" => "read",
        "write" => "write",
        "execute" => "execute",
        other => panic!("unexpected endpoint {other}"),
    };
    c.requests.lock().unwrap().push(Request {
        path,
        operation: op.clone(),
        params: v["params"].clone(),
    });
    let prefix = if op == "GetUpdate" {
        let prefix = v["params"]["id"].as_str().unwrap().chars().next().unwrap();
        namespace(
            &mut v["params"]["id"],
            if matches!(prefix, 'd' | 'e') {
                '1'
            } else {
                '0'
            },
        );
        prefix
    } else if op == "ListUpdates" {
        match v["params"]["query"]["target.type"].as_str().unwrap() {
            "Action" => 'a',
            "Procedure" => 'c',
            "Repo" => 'd',
            "Deployment" | "Stack" => 'e',
            "Build" => 'b',
            other => panic!("unexpected target {other}"),
        }
    } else if op.contains("Action") {
        'a'
    } else if op.contains("Procedure") {
        'c'
    } else if op.contains("Repo") {
        'd'
    } else if op == "ListSchedules" {
        's'
    } else if (op.contains("Deployment") || op == "Deploy")
        && v["params"]["deployment"] != definitions::DEPLOYMENT
        && v["params"]["query"]["specific"]["build_ids"]
            .as_array()
            .is_none_or(|ids| ids.is_empty())
        || matches!(
            op.as_str(),
            "ListStacks" | "ListAllStackServices" | "GetSystemStats" | "ListAlerts"
        )
    {
        'e'
    } else {
        'b'
    };
    let address = match prefix {
        'a' => &c.actions.address,
        'b' => &c.builds.address,
        'c' => &c.procedures.address,
        'd' => &c.repos.address,
        'e' => &c.workloads.address,
        's' => {
            c.sync_schedules();
            &c.schedules.address
        }
        other => panic!("unknown update namespace {other}"),
    };
    let response = c
        .http
        .post(format!("{address}/{path}"))
        .headers(headers)
        .json(&v)
        .send()
        .await
        .unwrap();
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
    let mut reply: Value = response.json().await.unwrap();
    if path == "execute" || op == "GetUpdate" {
        namespace(&mut reply["_id"]["$oid"], prefix);
        if matches!(prefix, 'b' | 'd') && reply["status"] == "Complete" {
            // The fixture's common source revision is independent of selectors.
            reply["commit_hash"] = json!(REVISION);
        }
    } else if op == "ListUpdates" {
        for item in reply["updates"].as_array_mut().unwrap() {
            namespace(&mut item["id"], prefix);
        }
    } else if op == "GetDeployment" && prefix == 'e' {
        reply["config"]["image"] = json!({"type":"Build","params":{
            "build_id":definitions::BUILD,"version":{"major":1,"minor":2,"patch":3}}});
    }
    (status, Json(reply)).into_response()
}
fn data(result: CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    let serialized = serde_json::to_string(&result).unwrap();
    support::assert_safe(&serialized);
    for secret in definitions::PRIVATE
        .into_iter()
        .chain(repos::PRIVATE)
        .chain(actions::PRIVATE)
        .chain(procedures::PRIVATE)
    {
        assert!(!serialized.contains(secret), "private fixture value leaked");
    }
    result.structured_content.unwrap()
}
async fn observed(s: &operations::Session, receipt: &Value) -> Value {
    assert_eq!(receipt["submission"], "accepted");
    data(call(s, "get_update", json!({"id":receipt["update_id"]})).await)
}
async fn schedule(s: &operations::Session, kind: &str, id: &str, patch: Value) -> Value {
    let mut input = json!({"owner_kind":kind,"target":{"id":id},"patch":patch});
    let preview = data(call(s, "preview_schedule_update", input.clone()).await);
    input["preview_id"] = preview["preview_id"].clone();
    let saved = data(call(s, "apply_schedule_update", input).await);
    assert_eq!(saved["saved_configuration_verified"], true);
    data(
        call(
            s,
            "get_schedule",
            json!({"owner_kind":kind,"target":{"id":id}}),
        )
        .await,
    )
}

#[tokio::test]
async fn one_stdio_session_traces_revision_build_image_explicit_deployment_and_independent_health()
{
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for family in [
        "repos",
        "builders",
        "builds",
        "procedures",
        "actions",
        "schedules",
    ] {
        data(call(&s, &format!("list_{family}"), json!({"page_size":1})).await);
    }
    for tool in ["clone_repo", "pull_repo"] {
        let receipt = data(
            call(
                &s,
                tool,
                json!({"target":{"id":repos::REPO},
            "expected_hooks":{"on_clone":true,"on_pull":true}}),
            )
            .await,
        );
        let update = observed(&s, &receipt).await;
        assert_eq!(update["observed_revision"]["revision"], REVISION);
        assert_eq!(update["outcome"], "succeeded");
        assert_eq!(update["target"]["id"], repos::REPO);
    }
    let build = data(call(&s, "get_build", json!({"id":definitions::BUILD})).await);
    assert_eq!(build["definition"]["source"]["linked_repo_id"], repos::REPO);
    let receipt = data(
        call(
            &s,
            "run_build",
            json!({"target":{"id":definitions::BUILD},
        "expected_effects":build["configured_effects"]["acknowledgement"]}),
        )
        .await,
    );
    let built = observed(&s, &receipt).await;
    assert_eq!(built["observed_revision"]["revision"], REVISION);
    assert_eq!(built["observed_build_result"]["version"], "1.2.3");
    let images = &built["observed_build_result"]["images"];
    assert_eq!(
        images[1]["reference"],
        "registry.invalid:5000/owned-fixture/fixture-image:1.2.3"
    );
    assert_eq!(images[1]["digest"], format!("sha256:{}", "a".repeat(64)));
    assert!(
        built["observed_build_result"]["deployment_result_evidence"]
            .as_str()
            .unwrap()
            .contains("no child")
    );
    let deployed = data(
        call(
            &s,
            "deploy_deployment",
            json!({"id":operations::DEPLOYMENT}),
        )
        .await,
    );
    let deployment = observed(&s, &deployed).await;
    assert_eq!(deployment["outcome"], "succeeded");
    assert_ne!(deployed["update_id"], receipt["update_id"]);
    let fleet = data(call(&s, "fleet_health", json!({})).await);
    assert_eq!(fleet["deployments"][0]["health"], "unhealthy");
    assert_eq!(c.execute_count(), 4);
    p.finish(s).await;
}

#[tokio::test]
async fn shared_automation_definitions_survive_runs_and_schedule_enable_change_disable() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    let action_before = c.actions.data.lock().unwrap().actions[actions::ACTION].clone();
    let procedure_before =
        c.procedures.data.lock().unwrap().procedures[procedures::PROCEDURE].clone();
    for (kind, id) in [
        ("action", actions::ACTION),
        ("procedure", procedures::PROCEDURE),
    ] {
        let detail = data(call(&s, &format!("get_{kind}"), json!({"id":id})).await);
        let mut input =
            json!({"target":{"id":id},"expected_effects":detail["effects"]["acknowledgement"]});
        if kind == "action" {
            input["args"] = json!({"per_run":actions::PRIVATE[10]});
        }
        let receipt = data(call(&s, &format!("run_{kind}"), input).await);
        assert_eq!(observed(&s, &receipt).await["outcome"], "succeeded");
        let d = schedule(
            &s,
            kind,
            id,
            json!({"expression":"0 0 9 * * *","format":"cron",
            "timezone":"America/New_York","enabled":true}),
        )
        .await;
        assert_eq!(
            d["observation"]["reported"]["next_scheduled_run"],
            1794578400000_i64
        );
        let d = schedule(
            &s,
            kind,
            id,
            json!({"expression":"every day at 9:00 am","format":"english","timezone":""}),
        )
        .await;
        assert_eq!(
            d["observation"]["reported"]["next_scheduled_run"],
            1794614400000_i64
        );
        schedule(&s, kind, id, json!({"enabled":false})).await;
    }
    let mut a = c.actions.data.lock().unwrap().actions[actions::ACTION].clone();
    let mut b = action_before;
    let mut p_after = c.procedures.data.lock().unwrap().procedures[procedures::PROCEDURE].clone();
    let mut p_before = procedure_before;
    for v in [&mut a, &mut b, &mut p_after, &mut p_before] {
        for field in [
            "schedule",
            "schedule_format",
            "schedule_timezone",
            "schedule_enabled",
        ] {
            v["config"].as_object_mut().unwrap().remove(field);
        }
    }
    assert_eq!(a["config"], b["config"]);
    assert_eq!(p_after["config"], p_before["config"]);
    assert_eq!(
        c.actions.data.lock().unwrap().merged_arguments[0]["per_run"],
        actions::PRIVATE[10]
    );
    assert_eq!(c.execute_count(), 2);
    p.finish(s).await;
}

#[tokio::test]
async fn failed_build_stages_and_partial_procedure_never_turn_into_deployment_or_health_claims() {
    let c = Core::start().await;
    let (p, s) = Process::start_profile(&c.address, true, true).await;
    for outcome in [
        builds::Outcome::HookFailed,
        builds::Outcome::BuildFailed,
        builds::Outcome::PushFailed,
    ] {
        c.builds.data.lock().unwrap().outcome = outcome;
        let b = data(call(&s, "get_build", json!({"id":definitions::BUILD})).await);
        let receipt = data(
            call(
                &s,
                "run_build",
                json!({"target":{"id":definitions::BUILD},
            "expected_effects":b["configured_effects"]["acknowledgement"]}),
            )
            .await,
        );
        let u = observed(&s, &receipt).await;
        assert_eq!(u["outcome"], "failed");
        assert!(
            u["stages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["success"] == false)
        );
    }
    c.procedures.data.lock().unwrap().outcome = procedures::Outcome::PartialFailure;
    let d = data(call(&s, "get_procedure", json!({"id":procedures::PROCEDURE})).await);
    let receipt = data(
        call(
            &s,
            "run_procedure",
            json!({"target":{"id":procedures::PROCEDURE},
        "expected_effects":d["effects"]["acknowledgement"]}),
        )
        .await,
    );
    assert_eq!(observed(&s, &receipt).await["outcome"], "failed");
    assert_eq!(c.execute_count(), 4);
    assert!(
        c.requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.operation != "Deploy")
    );
    p.finish(s).await;
}
