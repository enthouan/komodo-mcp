//! Mutable pinned-wire definition fixture, not a replacement for live Core auth.
use crate::support::{
    KEY, SECRET,
    inventory::Family,
    operations::{Request, Task},
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use komodo_client::entities::{
    build::{Build, BuildConfig, ImageRegistryConfig},
    builder::{Builder, BuilderConfig, ServerBuilderConfig},
    repo::{Repo, RepoConfig, RepoListItemInfo},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::net::TcpListener;
pub const BUILDER: &str = "000000000000000000000010";
pub const BUILD: &str = "000000000000000000000011";
pub const SERVER: &str = "000000000000000000000001";
pub const SERVER2: &str = "000000000000000000000002";
pub const REPO: &str = "00000000000000000000000a";
pub const DEPLOYMENT: &str = "000000000000000000000020";
pub const PRIVATE: [&str; 7] = [
    "build-private-git-account",
    "build-private-registry-account",
    "build-private-secret-argument",
    "build-private-webhook-secret",
    "build-private-pre-hook",
    "build-private-dockerfile",
    "build-private-database-token",
];
#[derive(Clone, Copy, Default)]
pub enum Fault {
    #[default]
    None,
    Denied,
    Lost,
    MissingEcho,
    ChangeUnrelated,
    ReadBackDenied,
    Delayed,
}
pub struct Data {
    pub builders: BTreeMap<String, Value>,
    pub builds: BTreeMap<String, Value>,
    pub servers: BTreeMap<String, Value>,
    pub repos: BTreeMap<String, Value>,
    pub deployments: BTreeMap<String, Value>,
    pub git_accounts: Vec<Value>,
    pub registry_accounts: Vec<Value>,
    pub config_git: Vec<Value>,
    pub config_registry: Vec<Value>,
    pub requests: Vec<Request>,
    pub fault: Fault,
    pub deny_read: Option<String>,
    pub deny_account_reads: bool,
    pub deny_config_reads: bool,
    pub admin: bool,
    pub bad_credentials: bool,
    pub switch_builder_on_read: Option<usize>,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
fn normalize<T: serde::de::DeserializeOwned + serde::Serialize>(v: T) -> Value {
    serde_json::to_value(serde_json::from_value::<T>(serde_json::to_value(v).unwrap()).unwrap())
        .unwrap()
}
impl Core {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let builder = normalize(Builder {
            id: BUILDER.into(),
            name: "fixture-builder".into(),
            config: BuilderConfig::Server(ServerBuilderConfig {
                server_ids: vec![SERVER.into(), SERVER2.into()],
            }),
            ..Default::default()
        });
        let mut c = BuildConfig {
            builder_id: BUILDER.into(),
            repo: "fixture/workload".into(),
            branch: "main".into(),
            git_account: PRIVATE[0].into(),
            image_name: "fixture-image".into(),
            image_registry: vec![ImageRegistryConfig {
                domain: "registry.invalid:5000".into(),
                account: PRIVATE[1].into(),
                organization: "owned-fixture".into(),
            }],
            secret_args: format!("SECRET={}", PRIVATE[2]),
            webhook_secret: PRIVATE[3].into(),
            webhook_enabled: false,
            dockerfile: format!("# {}", PRIVATE[5]),
            links: vec!["https://private.invalid/link".into()],
            ..Default::default()
        };
        c.pre_build.command = format!("printf {}", PRIVATE[4]);
        let build = normalize(Build {
            id: BUILD.into(),
            name: "fixture-build".into(),
            config: c,
            ..Default::default()
        });
        let repo = normalize(Repo {
            id: REPO.into(),
            name: "fixture-repo".into(),
            config: RepoConfig {
                repo: "fixture/linked".into(),
                git_account: PRIVATE[0].into(),
                ..Default::default()
            },
            ..Default::default()
        });
        let server = Family::Server.detail();
        let mut server2 = server.clone();
        server2["_id"]["$oid"] = json!(SERVER2);
        server2["name"] = json!("second-server");
        let mut deployment = Family::Deployment.detail();
        deployment["_id"]["$oid"] = json!(DEPLOYMENT);
        deployment["name"] = json!("dependent-workload");
        deployment["config"]["image"] =
            json!({"type":"Build","params":{"build_id":BUILD,"version":"0.0.0"}});
        deployment["config"]["redeploy_on_build"] = json!(true);
        let data = Arc::new(Mutex::new(Data {
            builders: BTreeMap::from([(BUILDER.into(), builder)]),
            builds: BTreeMap::from([(BUILD.into(), build)]),
            servers: BTreeMap::from([(SERVER.into(), server), (SERVER2.into(), server2)]),
            repos: BTreeMap::from([(REPO.into(), repo)]),
            deployments: BTreeMap::from([(DEPLOYMENT.into(), deployment)]),
            git_accounts: vec![
                json!({"_id":{"$oid":"000000000000000000000031"},"domain":"github.com","username":PRIVATE[0],"token":PRIVATE[6],"https":true}),
            ],
            registry_accounts: vec![
                json!({"_id":{"$oid":"000000000000000000000032"},"domain":"registry.invalid:5000","username":PRIVATE[1],"token":PRIVATE[6]}),
            ],
            config_git: vec![
                json!({"domain":"github.com","https":true,"accounts":[{"username":PRIVATE[0]}]}),
            ],
            config_registry: vec![
                json!({"domain":"registry.invalid:5000","accounts":[{"username":PRIVATE[1]}],"organizations":["owned-fixture"]}),
            ],
            requests: vec![],
            fault: Fault::None,
            deny_read: None,
            deny_account_reads: false,
            deny_config_reads: false,
            admin: true,
            bad_credentials: false,
            switch_builder_on_read: None,
        }));
        let app = Router::new()
            .route("/read", post(read))
            .route("/write", post(write))
            .route("/execute", post(execute))
            .with_state(data.clone());
        let task = Task(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        Self {
            address,
            data,
            _task: task,
        }
    }
    pub fn requests(&self, path: &str) -> Vec<Request> {
        self.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.path == path)
            .cloned()
            .collect()
    }
    pub fn build(&self) -> Value {
        self.data.lock().unwrap().builds[BUILD].clone()
    }
    pub fn builder(&self) -> Value {
        self.data.lock().unwrap().builders[BUILDER].clone()
    }
}
fn record(d: &mut Data, h: &HeaderMap, path: &'static str, v: &Value) {
    d.bad_credentials |= h.get("x-api-key").and_then(|v| v.to_str().ok()) != Some(KEY)
        || h.get("x-api-secret").and_then(|v| v.to_str().ok()) != Some(SECRET);
    d.requests.push(Request {
        path,
        operation: v["type"].as_str().unwrap().into(),
        params: v["params"].clone(),
    });
}
fn denied() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error":format!("permission denied {KEY} {SECRET}")})),
    )
        .into_response()
}
fn filtered(mut items: Vec<Value>, p: &Value) -> Vec<Value> {
    let names = p["query"]["names"].as_array().unwrap();
    let terms = p["query"]["terms"].as_array().unwrap();
    items.retain(|v| {
        (names.is_empty() || names.iter().any(|n| n == &v["id"] || n == &v["name"]))
            && terms
                .iter()
                .all(|t| v["name"].as_str().unwrap().contains(t.as_str().unwrap()))
    });
    items.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let limit = p["limit"].as_u64().unwrap() as usize;
    assert!((1..=100).contains(&limit));
    assert_eq!(p["sort_desc"], false);
    items
        .into_iter()
        .skip(p["page"].as_u64().unwrap() as usize * limit)
        .take(limit)
        .collect()
}
fn listed(v: &Value, kind: &str) -> Value {
    let c = &v["config"];
    let info = match kind {
        "Builder" => json!({"builder_type":c["type"],"instance_type":null}),
        "Build" => {
            json!({"state":"Ok","last_built_at":0,"version":c["version"],"builder_id":c["builder_id"],"files_on_host":c["files_on_host"],"dockerfile_contents":false,"linked_repo":c["linked_repo"],"linked_repo_name":"","git_provider":c["git_provider"],"repo":c["repo"],"branch":c["branch"],"repo_link":"","built_hash":"abc1234","latest_hash":"def5678","image_registry_domain":"registry.invalid:5000"})
        }
        "Repo" => serde_json::to_value(RepoListItemInfo {
            server_id: c["server_id"].as_str().unwrap().into(),
            repo: c["repo"].as_str().unwrap().into(),
            branch: c["branch"].as_str().unwrap().into(),
            git_provider: c["git_provider"].as_str().unwrap().into(),
            ..Default::default()
        })
        .unwrap(),
        "Server" => {
            let mut s = Family::Server.summary();
            s["id"] = v["_id"]["$oid"].clone();
            s["name"] = v["name"].clone();
            return s;
        }
        "Deployment" => {
            let mut s = Family::Deployment.summary();
            s["id"] = v["_id"]["$oid"].clone();
            s["name"] = v["name"].clone();
            return s;
        }
        _ => panic!("unknown family"),
    };
    json!({"id":v["_id"]["$oid"],"name":v["name"],"type":kind,"template":v["template"],"tags":[],"info":info})
}
async fn read(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    record(&mut d, &headers, "read", &v);
    let op = v["type"].as_str().unwrap();
    let p = &v["params"];
    if op == "GetBuilder"
        && d.switch_builder_on_read.is_some_and(|n| {
            d.requests
                .iter()
                .filter(|r| r.operation == "GetBuilder")
                .count()
                == n
        })
    {
        d.builders.get_mut(BUILDER).unwrap()["config"] = json!({"type":"Aws","params":{}});
    }
    if d.deny_read.as_deref() == Some(op)
        || (matches!(d.fault, Fault::ReadBackDenied)
            && matches!(op, "GetBuild" | "GetBuilder")
            && d.requests.iter().any(|r| r.path == "write"))
    {
        return denied();
    }
    match op {
        "GetVersion" => Json(json!({"version":"2.3.2"})).into_response(),
        "ListBuilders" => Json(filtered(
            d.builders.values().map(|v| listed(v, "Builder")).collect(),
            p,
        ))
        .into_response(),
        "ListBuilds" => Json(filtered(
            d.builds.values().map(|v| listed(v, "Build")).collect(),
            p,
        ))
        .into_response(),
        "ListServers" => Json(filtered(
            d.servers.values().map(|v| listed(v, "Server")).collect(),
            p,
        ))
        .into_response(),
        "ListRepos" => Json(filtered(
            d.repos.values().map(|v| listed(v, "Repo")).collect(),
            p,
        ))
        .into_response(),
        "ListDeployments" => {
            let ids = p["query"]["specific"]["build_ids"].as_array().unwrap();
            let selected = d
                .deployments
                .values()
                .filter(|v| {
                    ids.is_empty()
                        || ids
                            .iter()
                            .any(|id| id == &v["config"]["image"]["params"]["build_id"])
                })
                .map(|v| listed(v, "Deployment"))
                .collect();
            Json(filtered(selected, p)).into_response()
        }
        "GetBuilder" => Json(d.builders[p["builder"].as_str().unwrap()].clone()).into_response(),
        "GetBuild" => Json(d.builds[p["build"].as_str().unwrap()].clone()).into_response(),
        "GetServer" => Json(d.servers[p["server"].as_str().unwrap()].clone()).into_response(),
        "GetRepo" => Json(d.repos[p["repo"].as_str().unwrap()].clone()).into_response(),
        "GetDeployment" => {
            Json(d.deployments[p["deployment"].as_str().unwrap()].clone()).into_response()
        }
        "ListGitProviderAccounts" | "ListImageRegistryAccounts" => {
            if d.deny_account_reads {
                return denied();
            }
            let source = if op == "ListGitProviderAccounts" {
                &d.git_accounts
            } else {
                &d.registry_accounts
            };
            Json(
                source
                    .iter()
                    .filter(|r| r["domain"] == p["domain"] && r["username"] == p["username"])
                    .cloned()
                    .collect::<Vec<_>>(),
            )
            .into_response()
        }
        "ListGitProvidersFromConfig" | "ListImageRegistriesFromConfig" => {
            if d.deny_config_reads {
                return denied();
            }
            assert_eq!(p["target"]["type"], "Server");
            assert!(matches!(p["target"]["id"].as_str(), Some(SERVER | SERVER2)));
            Json(if op == "ListGitProvidersFromConfig" {
                d.config_git.clone()
            } else {
                d.config_registry.clone()
            })
            .into_response()
        }
        _ => panic!("unexpected read {op}"),
    }
}
async fn write(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let (saved, fault) = {
        let mut d = data.lock().unwrap();
        record(&mut d, &headers, "write", &v);
        let op = v["type"].as_str().unwrap();
        let p = &v["params"];
        let c = &p["config"];
        if matches!(d.fault, Fault::Denied) || (op == "CreateBuilder" && !d.admin) {
            return denied();
        }
        assert!(c.as_object().unwrap().values().all(|v| !v.is_null()));
        let is_builder = op.ends_with("Builder");
        let creating = op.starts_with("Create");
        let mut saved = if creating {
            let id = format!("{:024x}", 100 + d.builders.len() + d.builds.len());
            if is_builder {
                serde_json::to_value(Builder {
                    id,
                    name: p["name"].as_str().unwrap().into(),
                    config: BuilderConfig::Server(ServerBuilderConfig { server_ids: vec![] }),
                    ..Default::default()
                })
                .unwrap()
            } else {
                serde_json::to_value(Build {
                    id,
                    name: p["name"].as_str().unwrap().into(),
                    ..Default::default()
                })
                .unwrap()
            }
        } else if is_builder {
            d.builders[p["id"].as_str().unwrap()].clone()
        } else {
            d.builds[p["id"].as_str().unwrap()].clone()
        };
        if is_builder {
            assert_eq!(c["type"], "Server");
            assert_eq!(saved["config"]["type"], "Server", "unexpected conversion");
            saved["config"]["params"]
                .as_object_mut()
                .unwrap()
                .extend(c["params"].as_object().unwrap().clone());
            saved = normalize(serde_json::from_value::<Builder>(saved).unwrap());
        } else {
            saved["config"]
                .as_object_mut()
                .unwrap()
                .extend(c.as_object().unwrap().clone());
            saved = normalize(serde_json::from_value::<Build>(saved).unwrap());
        }
        let mut stored = saved.clone();
        if matches!(d.fault, Fault::ChangeUnrelated) {
            if is_builder {
                stored["config"]["params"]["server_ids"] = json!([SERVER2]);
            } else {
                stored["config"]["webhook_secret"] = json!("concurrent-private-change");
            }
        }
        let id = saved["_id"]["$oid"].as_str().unwrap().to_string();
        if is_builder {
            d.builders.insert(id, stored);
        } else {
            d.builds.insert(id, stored);
        }
        if matches!(d.fault, Fault::MissingEcho) {
            let key = c.as_object().unwrap().keys().next().unwrap();
            saved["config"].as_object_mut().unwrap().remove(key);
        }
        (saved, d.fault)
    };
    if matches!(fault, Fault::Delayed) {
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    if matches!(fault, Fault::Lost) {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "acknowledgement lost after save",
        )
            .into_response()
    } else {
        Json(saved).into_response()
    }
}
async fn execute(
    State(data): State<Arc<Mutex<Data>>>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> Response {
    let mut d = data.lock().unwrap();
    record(&mut d, &headers, "execute", &v);
    denied()
}
