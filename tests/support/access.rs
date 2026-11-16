//! Authenticated Core 2.3.2 administration wire fixture; all credentials are fake.
#![allow(dead_code)]
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
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;
pub const ACTOR: &str = "000000000000000000000077";
pub const USER: &str = "000000000000000000000081";
pub const OTHER: &str = "000000000000000000000082";
pub const GROUP: &str = "000000000000000000000091";
pub const SERVER: &str = "000000000000000000000001";
pub const PRIVATE: [&str; 5] = [
    "private-admin-password-hash-88a",
    "private-linked-oauth-token-88b",
    "private-totp-seed-88c",
    "private-passkey-88d",
    "private-opaque-user-extension-88e",
];
#[derive(Clone, Copy, Default)]
pub enum Reply {
    #[default]
    Valid,
    Denied,
    Lost,
    Malformed,
    WrongId,
    MissingRole,
    PrivateNoData,
    Oversized,
    Redirect,
    BeforeError,
    PartialCleanup,
}
pub struct Data {
    pub users: BTreeMap<String, Value>,
    pub groups: BTreeMap<String, Value>,
    pub permissions: Vec<Value>,
    pub role: Value,
    pub requests: Vec<Request>,
    pub reply: Reply,
    pub bad_credentials: bool,
    pub read_status: Option<u16>,
    pub user_status: Option<u16>,
    pub after_write_read_status: Option<u16>,
    pub current_reads: usize,
    pub find_reads: usize,
    pub group_reads: usize,
    pub permission_reads: usize,
    pub drift_user: Option<(usize, Value)>,
    pub drift_group: Option<(usize, Value)>,
    pub drift_permissions: Option<(usize, Vec<Value>)>,
    pub drift_role: Option<(usize, Value)>,
    pub after_write_user: Option<Value>,
    pub after_write_group: Option<Value>,
    pub after_write_read_override: Option<(String, Value)>,
    pub resource: Value,
    pub resource_list: Value,
    pub effective: Value,
    pub read_override: BTreeMap<String, Value>,
    pub inherited: bool,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
pub fn user(id: &str, name: &str, service: bool) -> Value {
    json!({"_id":{"$oid":id},"username":name,"enabled":true,"admin":false,"super_admin":false,"create_server_permissions":false,"create_build_permissions":false,"all":{},"config":if service{json!({"type":"Service","data":{"description":"owned metadata"}})}else{json!({"type":"Local","data":{"password":PRIVATE[0]}})},"linked_logins":{"Github":{"type":"Github","data":{"github_id":PRIVATE[1],"avatar":PRIVATE[1]}}},"totp":{"secret":PRIVATE[2],"confirmed_at":1,"recovery_codes":[PRIVATE[2]]},"passkey":{"passkey":{"private":PRIVATE[3]},"created_at":1},"extra_private":PRIVATE[4],"updated_at":1})
}
pub fn group(id: &str, name: &str) -> Value {
    json!({"_id":{"$oid":id},"name":name,"everyone":false,"users":[USER,OTHER],"all":{"Server":{"level":"Read","specific":["Logs","Inspect"]}},"updated_at":1,"private_extension":PRIVATE[4]})
}
pub fn permission(
    subject_kind: &str,
    subject_id: &str,
    resource_kind: &str,
    resource_id: &str,
) -> Value {
    json!({"_id":{"$oid":"0000000000000000000000a1"},"user_target":{"type":subject_kind,"id":subject_id},"resource_target":{"type":resource_kind,"id":resource_id},"level":"Read","specific":["Inspect","Logs"],"private_extension":PRIVATE[4]})
}
impl Core {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let mut admin = user(ACTOR, "fixture-admin", false);
        admin["admin"] = json!(true);
        let data = Arc::new(Mutex::new(Data {
            users: BTreeMap::from([
                (ACTOR.into(), admin.clone()),
                (USER.into(), user(USER, "owned-service", true)),
                (OTHER.into(), user(OTHER, "ordinary-user", false)),
            ]),
            groups: BTreeMap::from([(GROUP.into(), group(GROUP, "owned-group"))]),
            permissions: vec![
                permission("User", USER, "Server", SERVER),
                permission("UserGroup", GROUP, "Server", SERVER),
            ],
            role: admin,
            requests: vec![],
            reply: Reply::Valid,
            bad_credentials: false,
            read_status: None,
            user_status: None,
            after_write_read_status: None,
            current_reads: 0,
            find_reads: 0,
            group_reads: 0,
            permission_reads: 0,
            drift_user: None,
            drift_group: None,
            drift_permissions: None,
            drift_role: None,
            after_write_user: None,
            after_write_group: None,
            after_write_read_override: None,
            resource: Family::Server.detail(),
            resource_list: Family::Server.summary(),
            effective: json!({"level":"Read","specific":["Logs","Inspect"]}),
            read_override: BTreeMap::new(),
            inherited: false,
        }));
        let app = Router::new()
            .route("/komodo/read", post(read))
            .route("/komodo/write", post(write))
            .route("/komodo/user", get(current))
            .route("/komodo/execute", post(unexpected))
            .route("/komodo/terminal/execute", post(unexpected))
            .fallback(fallback)
            .with_state(data.clone());
        Self {
            address,
            data,
            _task: Task(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            })),
        }
    }
    pub fn writes(&self) -> Vec<Request> {
        self.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.path == "write")
            .cloned()
            .collect()
    }
}
fn fail(status: u16) -> Response {
    (
        StatusCode::from_u16(status).unwrap(),
        format!("{} {KEY} {SECRET}", PRIVATE[0]),
    )
        .into_response()
}
fn record(d: &mut Data, h: &HeaderMap, path: &'static str, op: &str, p: Value) -> bool {
    let ok = h.get("x-api-key").and_then(|x| x.to_str().ok()) == Some(KEY)
        && h.get("x-api-secret").and_then(|x| x.to_str().ok()) == Some(SECRET);
    d.bad_credentials |= !ok;
    d.requests.push(Request {
        path,
        operation: op.into(),
        params: p,
    });
    ok
}
async fn fallback(State(d): State<Arc<Mutex<Data>>>) -> Response {
    d.lock().unwrap().requests.push(Request {
        path: "unexpected",
        operation: "unexpected".into(),
        params: Value::Null,
    });
    fail(404)
}
async fn unexpected(
    State(d): State<Arc<Mutex<Data>>>,
    h: HeaderMap,
    Json(b): Json<Value>,
) -> Response {
    let mut d = d.lock().unwrap();
    record(
        &mut d,
        &h,
        "unexpected",
        b["type"].as_str().unwrap_or("terminal"),
        b["params"].clone(),
    );
    fail(403)
}
async fn current(State(d): State<Arc<Mutex<Data>>>, h: HeaderMap) -> Response {
    let mut d = d.lock().unwrap();
    if !record(&mut d, &h, "user", "CurrentCaller", json!({})) {
        return fail(401);
    }
    d.current_reads += 1;
    let at = d.current_reads;
    if let Some((n, v)) = d.drift_role.clone()
        && n == at
    {
        d.role = v;
    }
    if let Some(s) = d.user_status {
        return fail(s);
    }
    Json(d.role.clone()).into_response()
}
fn effective(d: &Data) -> Value {
    if !d.inherited {
        return d.effective.clone();
    }
    let uid = d.role["_id"]["$oid"].as_str().unwrap();
    let member = d
        .groups
        .values()
        .any(|g| g["everyone"] == true || g["users"].as_array().unwrap().iter().any(|v| v == uid));
    if member {
        json!({"level":"Read","specific":["Inspect","Logs"]})
    } else {
        json!("None")
    }
}
async fn read(State(d): State<Arc<Mutex<Data>>>, h: HeaderMap, Json(b): Json<Value>) -> Response {
    let mut d = d.lock().unwrap();
    let op = b["type"].as_str().unwrap();
    let p = &b["params"];
    if !record(&mut d, &h, "read", op, p.clone()) {
        return fail(401);
    }
    if op == "GetVersion" {
        return Json(json!({"version":"2.3.2"})).into_response();
    }
    if let Some(s) = d.read_status {
        return fail(s);
    }
    if let Some(v) = d.read_override.get(op) {
        return Json(v.clone()).into_response();
    }
    if d.role["admin"] != true
        && matches!(op, "ListUsers" | "FindUser" | "ListUserTargetPermissions")
    {
        return fail(403);
    }
    match op {
        "ListUsers" => {
            let a = d
                .users
                .values()
                .filter(|u| match p["service_users"].as_str().unwrap() {
                    "Include" => true,
                    "Exclude" => u["config"]["type"] != "Service",
                    "Only" => u["config"]["type"] == "Service",
                    _ => panic!("bad filter"),
                })
                .cloned()
                .collect::<Vec<_>>();
            Json(json!(a)).into_response()
        }
        "FindUser" => {
            d.find_reads += 1;
            let at = d.find_reads;
            if let Some((n, v)) = d.drift_user.clone()
                && n == at
            {
                let id = v["_id"]["$oid"].as_str().unwrap().to_owned();
                d.users.insert(id, v);
            }
            match d.users.get(p["user"].as_str().unwrap()) {
                Some(u) => Json(u.clone()).into_response(),
                None => fail(404),
            }
        }
        "ListUserGroups" => {
            let a = d
                .groups
                .values()
                .filter(|g| {
                    d.role["admin"] == true
                        || g["users"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|v| v == &d.role["_id"]["$oid"])
                })
                .cloned()
                .collect::<Vec<_>>();
            Json(json!(a)).into_response()
        }
        "GetUserGroup" => {
            d.group_reads += 1;
            let at = d.group_reads;
            if let Some((n, v)) = d.drift_group.clone()
                && n == at
            {
                let id = v["_id"]["$oid"].as_str().unwrap().to_owned();
                d.groups.insert(id, v);
            }
            match d.groups.get(p["user_group"].as_str().unwrap()) {
                Some(g)
                    if d.role["admin"] == true
                        || g["users"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|v| v == &d.role["_id"]["$oid"]) =>
                {
                    Json(g.clone()).into_response()
                }
                _ => fail(404),
            }
        }
        "ListPermissions" => Json(json!(
            d.permissions
                .iter()
                .filter(|v| v["user_target"] == json!({"type":"User","id":d.role["_id"]["$oid"]}))
                .collect::<Vec<_>>()
        ))
        .into_response(),
        "ListUserTargetPermissions" => {
            d.permission_reads += 1;
            if let Some((at, records)) = d.drift_permissions.clone()
                && d.permission_reads == at
            {
                d.permissions = records;
            }
            Json(json!(
                d.permissions
                    .iter()
                    .filter(|v| v["user_target"] == p["user_target"])
                    .collect::<Vec<_>>()
            ))
            .into_response()
        }
        "GetPermission" => {
            assert_eq!(p["target"]["type"], "Server");
            if p["target"]["id"] != SERVER {
                return if d.role["admin"] == true {
                    Json(json!("Write")).into_response()
                } else {
                    fail(404)
                };
            }
            Json(effective(&d)).into_response()
        }
        "ListServers" => {
            if d.inherited && effective(&d) == json!("None") {
                return fail(403);
            }
            Json(json!([d.resource_list])).into_response()
        }
        "GetServer" => Json(d.resource.clone()).into_response(),
        _ => fail(404),
    }
}
fn response(kind: Reply, mut ack: Value) -> Response {
    match kind {
        Reply::Valid => Json(ack).into_response(),
        Reply::Denied => fail(403),
        Reply::BeforeError => fail(500),
        Reply::Lost => fail(502),
        Reply::Malformed => (StatusCode::OK, "{").into_response(),
        Reply::PartialCleanup => fail(500),
        Reply::WrongId => {
            ack["_id"] = json!({"$oid":"0000000000000000000000ff"});
            Json(ack).into_response()
        }
        Reply::MissingRole => {
            ack.as_object_mut().unwrap().remove("enabled");
            Json(ack).into_response()
        }
        Reply::PrivateNoData => Json(json!({"private":PRIVATE[0]})).into_response(),
        Reply::Oversized => (StatusCode::OK, "x".repeat(2 * 1024 * 1024 + 1)).into_response(),
        Reply::Redirect => (
            StatusCode::TEMPORARY_REDIRECT,
            [("Location", "/komodo/write")],
            PRIVATE[0],
        )
            .into_response(),
    }
}
async fn write(State(d): State<Arc<Mutex<Data>>>, h: HeaderMap, Json(b): Json<Value>) -> Response {
    let mut d = d.lock().unwrap();
    let op = b["type"].as_str().unwrap();
    let p = &b["params"];
    if !record(&mut d, &h, "write", op, p.clone()) {
        return fail(401);
    }
    let reply = d.reply;
    if matches!(reply, Reply::Denied | Reply::BeforeError) {
        return response(reply, Value::Null);
    }
    if d.role["admin"] != true {
        return fail(403);
    }
    let ack = match op {
        "CreateServiceUser" => {
            let id = format!("{:024x}", 0x100 + d.users.len());
            let mut u = user(&id, p["username"].as_str().unwrap(), true);
            u["config"]["data"]["description"] = p["description"].clone();
            d.users.insert(id, u.clone());
            u
        }
        "UpdateServiceUserDescription" => {
            let u = d
                .users
                .values_mut()
                .find(|u| u["username"] == p["username"])
                .unwrap();
            assert_eq!(u["config"]["type"], "Service");
            u["config"]["data"]["description"] = p["description"].clone();
            u.clone()
        }
        "UpdateUserBasePermissions" => {
            let u = d.users.get_mut(p["user_id"].as_str().unwrap()).unwrap();
            for (param, field) in [
                ("enabled", "enabled"),
                ("create_servers", "create_server_permissions"),
                ("create_builds", "create_build_permissions"),
            ] {
                if p[param].is_boolean() {
                    u[field] = p[param].clone();
                }
            }
            json!({})
        }
        "UpdateUserAdmin" => {
            if d.role["super_admin"] != true {
                return fail(403);
            }
            d.users.get_mut(p["user_id"].as_str().unwrap()).unwrap()["admin"] = p["admin"].clone();
            json!({})
        }
        "DeleteUser" => {
            let uid = p["user"].as_str().unwrap();
            let u = d.users.remove(uid).unwrap();
            if !matches!(reply, Reply::PartialCleanup) {
                for g in d.groups.values_mut() {
                    g["users"].as_array_mut().unwrap().retain(|v| v != uid);
                }
            }
            u
        }
        "CreateUserGroup" => {
            let gid = format!("{:024x}", 0x200 + d.groups.len());
            let mut g = group(&gid, p["name"].as_str().unwrap());
            g["users"] = json!([]);
            g["all"] = json!({});
            d.groups.insert(gid, g.clone());
            g
        }
        "RenameUserGroup" => {
            let g = d.groups.get_mut(p["id"].as_str().unwrap()).unwrap();
            g["name"] = p["name"].clone();
            g.clone()
        }
        "DeleteUserGroup" => {
            let gid = p["id"].as_str().unwrap();
            let g = d.groups.remove(gid).unwrap();
            if !matches!(reply, Reply::PartialCleanup) {
                d.permissions
                    .retain(|v| v["user_target"] != json!({"type":"UserGroup","id":gid}));
            }
            g
        }
        "AddUserToUserGroup" | "RemoveUserFromUserGroup" => {
            let g = d.groups.get_mut(p["user_group"].as_str().unwrap()).unwrap();
            let a = g["users"].as_array_mut().unwrap();
            if op == "AddUserToUserGroup" {
                if !a.contains(&p["user"]) {
                    a.push(p["user"].clone());
                }
            } else {
                a.retain(|v| v != &p["user"]);
            }
            g.clone()
        }
        "SetEveryoneUserGroup" => {
            let g = d.groups.get_mut(p["user_group"].as_str().unwrap()).unwrap();
            g["everyone"] = p["everyone"].clone();
            g.clone()
        }
        "UpdatePermissionOnTarget" => {
            let mut v = d
                .permissions
                .iter()
                .find(|v| {
                    v["user_target"] == p["user_target"]
                        && v["resource_target"] == p["resource_target"]
                })
                .cloned()
                .unwrap_or_else(|| {
                    permission(
                        p["user_target"]["type"].as_str().unwrap(),
                        p["user_target"]["id"].as_str().unwrap(),
                        p["resource_target"]["type"].as_str().unwrap(),
                        p["resource_target"]["id"].as_str().unwrap(),
                    )
                });
            let grant = &p["permission"];
            v["level"] = if grant.is_string() {
                grant.clone()
            } else {
                grant["level"].clone()
            };
            v["specific"] = if grant.is_string() {
                json!([])
            } else {
                grant["specific"].clone()
            };
            d.permissions.retain(|v| {
                v["user_target"] != p["user_target"] || v["resource_target"] != p["resource_target"]
            });
            d.permissions.push(v);
            json!({})
        }
        "UpdatePermissionOnResourceType" => {
            let m = if p["user_target"]["type"] == "User" {
                d.users
                    .get_mut(p["user_target"]["id"].as_str().unwrap())
                    .unwrap()
            } else {
                d.groups
                    .get_mut(p["user_target"]["id"].as_str().unwrap())
                    .unwrap()
            };
            m["all"][p["resource_type"].as_str().unwrap()] = p["permission"].clone();
            json!({})
        }
        _ => panic!("unexpected administration write {op}"),
    };
    if let Some(u) = d.after_write_user.clone() {
        let uid = u["_id"]["$oid"].as_str().unwrap().to_owned();
        d.users.insert(uid, u);
    }
    if let Some(g) = d.after_write_group.clone() {
        let gid = g["_id"]["$oid"].as_str().unwrap().to_owned();
        d.groups.insert(gid, g);
    }
    d.read_status = d.after_write_read_status;
    if let Some((operation, raw)) = d.after_write_read_override.clone() {
        d.read_override.insert(operation, raw);
    }
    response(reply, ack)
}
