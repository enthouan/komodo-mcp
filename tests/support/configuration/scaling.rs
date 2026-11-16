//! Scale-only extension of the existing authenticated definition fixture.
#![allow(dead_code)]
use super::{Core, Data, SERVER, SWARM};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use komodo_client::entities::{
    Operation, ResourceTarget,
    update::{Update, UpdateStatus},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const DEPLOYMENT: &str = "000000000000000000000003";
pub const UPDATE: &str = "100000000000000000000033";
pub const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaa";
pub const SERVICE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbb";
pub const PRIVATE: [&str; 4] = [
    "scale-environment-private",
    "scale-argument-private",
    "scale-docker-private",
    "scale-reference-private",
];
#[derive(Clone, Copy, Default)]
pub enum ExecuteReply {
    #[default]
    Valid,
    Denied,
    Lost,
    Failed,
    Queued,
}
pub struct ScaleData {
    pub services: Vec<Value>,
    pub service: Value,
    pub tasks: Vec<Value>,
    pub details: BTreeMap<String, Value>,
    pub updates: BTreeMap<String, Value>,
    pub denied: BTreeSet<String>,
    pub execute_reply: ExecuteReply,
    pub apply_runtime: bool,
    pub drift_after_runtime: bool,
    pub drift_next_definition: bool,
    pub oversized: Option<String>,
    pub swap_after_tasks: Option<i32>,
}
pub fn task(n: usize, service: &str, state: &str, desired: &str) -> Value {
    json!({"ID":format!("t{n:024}"),"ServiceID":service,"NodeID":"ccccccccccccccccccccccccc","State":state,"DesiredState":desired,"Configs":[],"Secrets":[]})
}
pub fn set_runtime(d: &mut ScaleData, replicas: i32, running: i32, healthy: bool) {
    set_runtime_id(d, replicas, running, healthy, SERVICE);
}
pub fn set_runtime_id(
    d: &mut ScaleData,
    replicas: i32,
    running: i32,
    healthy: bool,
    service: &str,
) {
    d.services = vec![
        json!({"ID":service,"Name":"fixture-web","Mode":"Replicated","Replicas":replicas,"DesiredTasks":replicas,"RunningTasks":running,"Configs":[],"Secrets":[],"State":if healthy{"Healthy"}else{"Unhealthy"}}),
    ];
    d.service = json!({"ID":service,"Mode":"Replicated","Replicas":replicas,"State":if healthy{"Healthy"}else{"Unhealthy"},"Version":{"Index":9},"Spec":{"Name":"fixture-web","Mode":{"Replicated":{"Replicas":replicas}},"Labels":{"secret":PRIVATE[2]},"TaskTemplate":{"ContainerSpec":{"Env":[format!("SECRET={}",PRIVATE[2])]}}}});
    d.tasks = (0..running)
        .map(|n| task(n as usize, service, "running", "running"))
        .collect();
    d.tasks.push(task(
        998,
        if service == OLD {
            "zzzzzzzzzzzzzzzzzzzzzzzzz"
        } else {
            OLD
        },
        "running",
        "running",
    ));
    d.tasks.push(task(999, service, "shutdown", "shutdown"));
    d.details.clear();
    for t in &d.tasks {
        d.details.insert(t["ID"].as_str().unwrap().into(), json!({"ID":t["ID"],"ServiceID":t["ServiceID"],"NodeID":t["NodeID"],"DesiredState":t["DesiredState"],"Spec":{"ContainerSpec":{"Env":[PRIVATE[2]]}},"Status":{"State":t["State"],"Message":"","Err":"","ContainerStatus":{"ExitCode":0}}}));
    }
}
impl Core {
    pub async fn start_scaling() -> Self {
        let c = Self::start().await;
        {
            let mut data = c.data.lock().unwrap();
            data.deployment["config"]["swarm_id"] = json!(SWARM);
            data.deployment["config"]["custom_name"] = json!("fixture-web");
            data.deployment["config"]["replicas"] = json!(1);
            data.deployment["config"]["environment"] = json!(format!("PRIVATE={}\n", PRIVATE[0]));
            data.deployment["config"]["extra_args"] = json!([
                format!("--env=PRIVATE={}", PRIVATE[1]),
                "--replicas=1",
                "--constraint=node.role==worker"
            ]);
            data.swarm["config"]["server_ids"] = json!([SERVER]);
            data.swarm["config"]["labels"] = json!({"private":PRIVATE[3]});
            let mut s = ScaleData {
                services: vec![],
                service: Value::Null,
                tasks: vec![],
                details: BTreeMap::new(),
                updates: BTreeMap::new(),
                denied: BTreeSet::new(),
                execute_reply: ExecuteReply::Valid,
                apply_runtime: true,
                drift_after_runtime: false,
                drift_next_definition: false,
                oversized: None,
                swap_after_tasks: None,
            };
            set_runtime_id(&mut s, 1, 1, true, OLD);
            data.scaling = Some(s);
        }
        c
    }
}
pub fn read(data: &mut Data, request: &Value) -> Option<Response> {
    let s = data.scaling.as_mut()?;
    let op = request["type"].as_str().unwrap();
    let p = &request["params"];
    if s.denied.contains(op) {
        let error = if op.starts_with("InspectSwarm") {
            "User does not have required permissions on this Swarm. Must have at least Read permissions, as well as these specific permissions: [Inspect]"
        } else if op.contains("Deployment") {
            "User does not have required permissions on this Deployment. Must have at least Read permissions"
        } else {
            "User does not have required permissions on this Swarm. Must have at least Read permissions"
        };
        return Some(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":error,"trace":[PRIVATE[2]]})),
            )
                .into_response(),
        );
    }
    if s.oversized.as_deref() == Some(op) {
        return Some(Json(json!({"private":PRIVATE[2].repeat(200_000)})).into_response());
    }
    if op == "GetDeployment" && s.drift_next_definition {
        s.drift_next_definition = false;
        data.deployment["config"]["environment"] = json!(format!("DRIFT={}\n", PRIVATE[0]));
    }
    let v = match op {
        "ListSwarmServices" => s.services.clone().into(),
        "InspectSwarmService" => {
            assert_eq!(p["service"], s.service["ID"]);
            if s.drift_after_runtime {
                s.drift_after_runtime = false;
                s.drift_next_definition = true;
            }
            s.service.clone()
        }
        "ListSwarmTasks" => {
            let tasks = s.tasks.clone();
            if let Some(n) = s.swap_after_tasks.take() {
                set_runtime(s, n, n, true);
            }
            tasks.into()
        }
        "InspectSwarmTask" => s.details.get(p["task"].as_str().unwrap()).unwrap().clone(),
        "GetUpdate" => s.updates.get(p["id"].as_str().unwrap()).unwrap().clone(),
        "ListUpdates" => {
            json!({"updates":s.updates.values().map(|u|json!({"id":u["_id"]["$oid"],"target":u["target"],"operation":u["operation"],"start_ts":u["start_ts"],"status":u["status"],"success":u["success"],"operator":u["operator"],"username":"fixture"})).collect::<Vec<_>>(),"next_page":null})
        }
        _ => return None,
    };
    Some(Json(v).into_response())
}
pub fn execute(data: &mut Data, request: &Value) -> Response {
    assert_eq!(request["type"], "Deploy");
    assert_eq!(request["params"]["deployment"], DEPLOYMENT);
    let s = data.scaling.as_mut().unwrap();
    if matches!(s.execute_reply, ExecuteReply::Denied) {
        return (StatusCode::FORBIDDEN, Json(json!({"error":PRIVATE[2]}))).into_response();
    }
    let update = serde_json::to_value(Update {
        id: UPDATE.into(),
        operation: Operation::Deploy,
        target: ResourceTarget::Deployment(DEPLOYMENT.into()),
        status: if matches!(s.execute_reply, ExecuteReply::Queued) {
            UpdateStatus::Queued
        } else {
            UpdateStatus::Complete
        },
        success: !matches!(s.execute_reply, ExecuteReply::Failed),
        start_ts: 100,
        end_ts: Some(110),
        ..Default::default()
    })
    .unwrap();
    s.updates.insert(UPDATE.into(), update.clone());
    if s.apply_runtime && matches!(s.execute_reply, ExecuteReply::Valid | ExecuteReply::Lost) {
        let replicas = data.deployment["config"]["replicas"].as_i64().unwrap() as i32;
        set_runtime(s, replicas, replicas, true);
    }
    if matches!(s.execute_reply, ExecuteReply::Lost) {
        return (StatusCode::OK, "{lost accepted response").into_response();
    }
    Json(update).into_response()
}
