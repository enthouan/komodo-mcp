//! Bounded pinned-wire faults, not an actual Periphery or shell.
#![allow(dead_code)]
use crate::support::{
    KEY, SECRET,
    inventory::Family,
    operations::{Request, Task},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
pub const SERVER: &str = "000000000000000000000001";
pub const STACK: &str = "000000000000000000000002";
pub const DEPLOYMENT: &str = "000000000000000000000003";
pub const USER: &str = "000000000000000000000077";
pub const PRIVATE: &str = "terminal-private-baseline-marker";
pub fn cid() -> String {
    "ab".repeat(32)
}
#[derive(Clone, Default)]
pub enum Stream {
    #[default]
    Valid,
    Nonzero,
    Bytes(Vec<u8>),
    Split,
    Slow,
    Interrupted,
    Oversized,
    Denied,
    Redirect,
}
#[derive(Clone, Copy, Default)]
pub enum Create {
    #[default]
    Valid,
    Lost,
    Malformed,
    WrongTarget,
    Denied,
    Slow,
}
pub struct Data {
    pub requests: Vec<Request>,
    pub server: Value,
    pub stack: Value,
    pub deployment: Value,
    pub caller: Value,
    pub permission: Value,
    pub container: Value,
    pub services: Value,
    pub records: BTreeMap<String, Value>,
    pub stream: Stream,
    pub create: Create,
    pub bad_credentials: bool,
    pub collision: bool,
    pub replace_after_command: bool,
    pub drift_after_create: Option<Value>,
    pub permission_after_create: Option<Value>,
    pub delete_status: u16,
    pub delete_lost: bool,
    pub list_status: u16,
    pub inspect_status: u16,
    pub reads: BTreeMap<String, Value>,
}
pub struct Core {
    pub address: String,
    pub data: Arc<Mutex<Data>>,
    _task: Task,
}
impl Core {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let mut server = Family::Server.detail();
        server["config"]["private_extension"] = json!(PRIVATE);
        let container = json!({"Id":cid(),"Name":"/fixture-web","Created":"2026-01-01T00:00:00Z","State":{"Running":true},"Config":{"Labels":{"com.docker.compose.service":"web","com.docker.compose.project":"fixture-project"}}});
        let services = json!([{"stack_id":STACK,"service":"web","container":{"id":cid(),"name":"fixture-web"},"swarm_service":null}]);
        let data = Arc::new(Mutex::new(Data {
            requests: vec![],
            server,
            stack: Family::Stack.detail(),
            deployment: Family::Deployment.detail(),
            caller: json!({"_id":{"$oid":USER},"username":"terminal-operator","enabled":true,"admin":false,"super_admin":false,"config":{"password":PRIVATE}}),
            permission: json!({"level":"Read","specific":["Terminal","Inspect"]}),
            container,
            services,
            records: BTreeMap::new(),
            stream: Stream::Valid,
            create: Create::Valid,
            bad_credentials: false,
            collision: false,
            replace_after_command: false,
            drift_after_create: None,
            permission_after_create: None,
            delete_status: 200,
            delete_lost: false,
            list_status: 200,
            inspect_status: 200,
            reads: BTreeMap::new(),
        }));
        let state = data.clone();
        let task = Task(tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let data = state.clone();
                tokio::spawn(async move {
                    let _ = serve(socket, data).await;
                });
            }
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
}
struct Reply {
    status: u16,
    body: Vec<u8>,
    split: bool,
    delay: Duration,
    interrupted: bool,
}
fn reply(status: u16, body: Vec<u8>) -> Reply {
    Reply {
        status,
        body,
        split: false,
        delay: Duration::ZERO,
        interrupted: false,
    }
}
fn value(v: Value) -> Reply {
    reply(200, v.to_string().into_bytes())
}
fn failure(status: u16) -> Reply {
    reply(status, format!("{PRIVATE} {KEY} {SECRET}").into_bytes())
}
fn respond(d: &mut Data, path: &str, v: Value) -> Reply {
    let (route, op, p) = if path.ends_with("/user") {
        ("user", "CurrentCaller".to_owned(), json!({}))
    } else if path.ends_with("/terminal/execute") {
        ("terminal", "ExecuteTerminal".into(), v.clone())
    } else if path.ends_with("/read") {
        (
            "read",
            v["type"].as_str().unwrap_or("bad").into(),
            v["params"].clone(),
        )
    } else if path.ends_with("/write") {
        (
            "write",
            v["type"].as_str().unwrap_or("bad").into(),
            v["params"].clone(),
        )
    } else {
        ("unexpected", "unexpected".into(), v)
    };
    d.requests.push(Request {
        path: route,
        operation: op.clone(),
        params: p.clone(),
    });
    if let Some(raw) = d.reads.get(&op) {
        return value(raw.clone());
    }
    match op.as_str() {
        "GetVersion" => value(json!({"version":"2.3.2"})),
        "CurrentCaller" => value(d.caller.clone()),
        "GetServer" => value(d.server.clone()),
        "GetStack" => value(d.stack.clone()),
        "GetDeployment" => value(d.deployment.clone()),
        "ListServers" => value(json!([Family::Server.summary()])),
        "ListStacks" => value(json!([Family::Stack.summary()])),
        "ListDeployments" => value(json!([Family::Deployment.summary()])),
        "GetPermission" => value(d.permission.clone()),
        "InspectContainer" | "InspectStackContainer" | "InspectDeploymentContainer" => {
            if d.inspect_status != 200 {
                failure(d.inspect_status)
            } else {
                value(d.container.clone())
            }
        }
        "ListStackServices" => value(d.services.clone()),
        "ListTerminals" => {
            if d.list_status != 200 {
                return failure(d.list_status);
            }
            let term = p["terms"][0].as_str().unwrap();
            if d.collision && !d.records.contains_key(term) {
                d.records.insert(term.into(),json!({"name":term,"target":p["target"],"command":"foreign application shell","created_at":10}));
            }
            let records = d
                .records
                .values()
                .filter(|r| r["name"].as_str().is_some_and(|n| n.contains(term)))
                .cloned()
                .map(|mut r| {
                    if r["target"]["type"] == "Server" {
                        r["target"]["params"]["server"] = Value::Null;
                    }
                    r
                })
                .collect::<Vec<_>>();
            value(json!(records))
        }
        "CreateTerminal" => {
            if matches!(d.create, Create::Denied) {
                return failure(403);
            }
            let name = p["name"].as_str().unwrap();
            let prefix = match p["target"]["type"].as_str().unwrap() {
                "Server" => String::new(),
                "Container" => format!(
                    "docker exec -it {} ",
                    p["target"]["params"]["container"].as_str().unwrap()
                ),
                _ => "docker exec -it fixture-web ".into(),
            };
            let command = format!("{prefix}{}", p["command"].as_str().unwrap());
            let mut record = json!({"name":name,"target":p["target"],"command":command,"created_at":1000,"stored_size_kb":0.0});
            if matches!(d.create, Create::WrongTarget) {
                record["target"] = json!({"type":"Deployment","params":{"deployment":DEPLOYMENT}});
            }
            d.records.insert(name.into(), record.clone());
            if let Some(v) = d.drift_after_create.take() {
                d.server["config"]["private_extension"] = v;
            }
            if let Some(v) = d.permission_after_create.take() {
                d.permission = v;
            }
            match d.create {
                Create::Lost => failure(502),
                Create::Malformed => reply(200, b"not JSON".to_vec()),
                Create::Slow => {
                    let mut r = value(record);
                    r.delay = Duration::from_secs(3);
                    r
                }
                _ => value(record),
            }
        }
        "DeleteTerminal" => {
            if d.delete_status != 200 {
                return failure(d.delete_status);
            }
            d.records.remove(p["terminal"].as_str().unwrap());
            if d.delete_lost {
                failure(502)
            } else {
                value(json!({}))
            }
        }
        "ExecuteTerminal" => {
            if d.replace_after_command {
                d.records.get_mut(p["terminal"].as_str().unwrap()).unwrap()["created_at"] =
                    json!(2000);
            }
            match &d.stream {
                Stream::Valid=>reply(200,format!("harmless-output\n{KEY} password=application-private\n__KOMODO_EXIT_CODE:0\n").into_bytes()),
                Stream::Nonzero=>reply(200,b"nonzero-output\n__KOMODO_EXIT_CODE:7\n".to_vec()),
                Stream::Bytes(b)=>reply(200,b.clone()),
                Stream::Split=>{let mut r=reply(200,"\x1b[31mé\x1b[0m\n__KOMODO_EXIT_CODE:0\n".as_bytes().to_vec());r.split=true;r},
                Stream::Slow=>{let mut r=reply(200,b"late\n__KOMODO_EXIT_CODE:0\n".to_vec());r.delay=Duration::from_secs(3);r},
                Stream::Interrupted=>{let mut r=reply(200,b"partial-private-output".to_vec());r.interrupted=true;r},
                Stream::Oversized=>reply(200,vec![b'x';65_537]),Stream::Denied=>failure(403),Stream::Redirect=>failure(307),
            }
        }
        _ => failure(500),
    }
}
async fn serve(mut socket: TcpStream, data: Arc<Mutex<Data>>) -> std::io::Result<()> {
    let mut received = Vec::new();
    let end = loop {
        let mut b = [0; 4096];
        let n = socket.read(&mut b).await?;
        if n == 0 {
            return Ok(());
        }
        received.extend_from_slice(&b[..n]);
        if received.len() > 65536 {
            return Ok(());
        }
        if let Some(i) = received.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let header = String::from_utf8_lossy(&received[..end]).into_owned();
    let path = header
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned();
    let length = header
        .lines()
        .find_map(|s| {
            s.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    if length > 32768 {
        return Ok(());
    }
    while received.len() - end < length {
        let mut b = [0; 4096];
        let n = socket.read(&mut b).await?;
        if n == 0 {
            return Ok(());
        }
        received.extend_from_slice(&b[..n]);
    }
    let parsed = if length == 0 {
        json!({})
    } else {
        serde_json::from_slice(&received[end..end + length]).unwrap()
    };
    let response = {
        let mut d = data.lock().unwrap();
        let credentials = header
            .lines()
            .any(|h| h.eq_ignore_ascii_case(&format!("x-api-key: {KEY}")))
            && header
                .lines()
                .any(|h| h.eq_ignore_ascii_case(&format!("x-api-secret: {SECRET}")));
        d.bad_credentials |= !credentials;
        if credentials {
            respond(&mut d, &path, parsed)
        } else {
            failure(401)
        }
    };
    tokio::time::sleep(response.delay).await;
    if response.split {
        socket
            .write_all(
                format!(
                    "HTTP/1.1 {} OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                    response.status
                )
                .as_bytes(),
            )
            .await?;
        for byte in response.body {
            socket
                .write_all(&[b'1', b'\r', b'\n', byte, b'\r', b'\n'])
                .await?;
            tokio::task::yield_now().await;
        }
        socket.write_all(b"0\r\n\r\n").await?;
    } else {
        let length = response.body.len() + usize::from(response.interrupted) * 100;
        socket
            .write_all(
                format!(
                    "HTTP/1.1 {} OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n",
                    response.status
                )
                .as_bytes(),
            )
            .await?;
        socket.write_all(&response.body).await?;
    }
    socket.shutdown().await
}
