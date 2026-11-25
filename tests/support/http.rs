#![allow(dead_code)] // The transport and selection integration suites exercise different helpers.
use std::{
    net::SocketAddr,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use komodo_mcp::{client::CoreClient, config::HttpConfig};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::TcpListener,
    process::Child,
    sync::oneshot,
    task::JoinHandle,
    time::timeout,
};

pub const FIRST: &str = "fixture-first-inbound-bearer-token-0001";
pub const SECOND: &str = "fixture-second-inbound-bearer-token-0002";
pub const INVALID: &str = "fixture-unrecognized-inbound-bearer-0003";
pub const PRIVATE_ARGUMENT: &str = "fixture-private-input-should-not-be-logged";
pub const ORIGIN: &str = "https://operator.example.test";
pub const VERSION: &str = "2025-11-25";
pub const DEADLINE: Duration = Duration::from_secs(10);

pub fn assert_redacted(text: &str) {
    super::support::assert_safe(text);
    for token in [FIRST, SECOND, INVALID, PRIVATE_ARGUMENT] {
        assert!(
            !text.contains(token),
            "an inbound fixture credential was disclosed"
        );
    }
}

pub fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "protocolVersion":VERSION, "capabilities":{},
        "clientInfo":{"name":"http-fixture","version":"1"}
    }})
}

#[derive(Clone)]
pub struct Http {
    client: Client,
    url: String,
    next_id: Arc<AtomicU64>,
}

impl Http {
    pub fn new(address: SocketAddr) -> Self {
        Self {
            client: Client::builder()
                .no_proxy()
                .timeout(DEADLINE)
                .build()
                .unwrap(),
            url: format!("http://{address}/mcp"),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn request(
        &self,
        method: Method,
        token: Option<&str>,
        session: Option<&str>,
    ) -> RequestBuilder {
        let mut request = self
            .client
            .request(method, &self.url)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", VERSION);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        request
    }

    pub fn preflight(&self, origin: &str) -> RequestBuilder {
        self.client
            .request(Method::OPTIONS, &self.url)
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization, content-type, mcp-protocol-version, mcp-session-id",
            )
    }

    pub async fn session(&self, token: &str, origin: Option<&str>) -> String {
        self.initialize_session(token, origin).await.0
    }

    pub async fn initialize_session(&self, token: &str, origin: Option<&str>) -> (String, Value) {
        let mut request = self.request(Method::POST, Some(token), None);
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let init = post(request, initialize()).send().await.unwrap();
        let session = init
            .headers()
            .get("mcp-session-id")
            .expect("legacy initialization must issue a session")
            .to_str()
            .unwrap()
            .to_owned();
        assert!(!session.is_empty());
        let response = rpc_response(init, 0).await;
        assert_eq!(response["result"]["protocolVersion"], VERSION);
        assert_eq!(
            response["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        let notice = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let request = self.request(Method::POST, Some(token), Some(&session));
        assert_eq!(
            post(request, notice).send().await.unwrap().status(),
            StatusCode::ACCEPTED
        );
        (session, response["result"].clone())
    }

    pub async fn rpc(&self, token: &str, session: &str, method: &str, params: Value) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = self.request(Method::POST, Some(token), Some(session));
        let frame = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        rpc_response(post(request, frame).send().await.unwrap(), id).await
    }

    pub async fn call(&self, token: &str, session: &str, name: &str, args: Value) -> Value {
        self.rpc(
            token,
            session,
            "tools/call",
            json!({"name":name,"arguments":args}),
        )
        .await
    }

    pub async fn stateless_call(&self, token: &str, name: &str, args: Value) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = self
            .client
            .post(&self.url)
            .bearer_auth(token)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", name);
        let frame = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{
            "name":name,"arguments":args,
            "_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientInfo":{"name":"http-fixture","version":"1"},
                "io.modelcontextprotocol/clientCapabilities":{}
            }
        }});
        let response = post(request, frame).send().await.unwrap();
        assert!(response.headers().get("mcp-session-id").is_none());
        rpc_response(response, id).await
    }
}

pub fn post(request: RequestBuilder, body: Value) -> RequestBuilder {
    request
        .header("content-type", "application/json")
        .body(body.to_string())
}

async fn rpc_response(response: Response, id: u64) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    let sse = response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream");
    // An individual request stream closes when its response is delivered.
    let body = response.text().await.unwrap();
    assert_redacted(&body);
    let frame: Value = if sse {
        body.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
            .find(|frame| frame["id"] == id)
            .expect("an SSE message must answer the request")
    } else {
        serde_json::from_str(&body).unwrap()
    };
    assert_eq!(frame["jsonrpc"], "2.0");
    assert_eq!(frame["id"], id);
    frame
}

pub async fn rejected(request: RequestBuilder, status: StatusCode) {
    let response = request.send().await.unwrap();
    assert_eq!(response.status(), status);
    assert!(response.headers().get("mcp-session-id").is_none());
    assert_redacted(&response.text().await.unwrap());
}

pub struct Server {
    pub http: Http,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Server {
    pub async fn start(client: CoreClient) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = Http::new(listener.local_addr().unwrap());
        let version = client.detect_version().await.unwrap();
        let config = HttpConfig::parse(None, &format!("{FIRST},{SECOND}"), Some(ORIGIN)).unwrap();
        let (stop, recv) = oneshot::channel();
        let task = tokio::spawn(komodo_mcp::http::serve(
            listener,
            config,
            client,
            version,
            async {
                let _ = recv.await;
            },
        ));
        Self { http, stop, task }
    }

    pub async fn finish(self) {
        self.stop.send(()).unwrap();
        timeout(DEADLINE, self.task)
            .await
            .expect("HTTP shutdown exceeded its bound")
            .expect("HTTP service panicked")
            .expect("HTTP service failed");
    }
}

pub struct Binary {
    pub http: Http,
    child: Child,
    diagnostics: JoinHandle<()>,
}
impl Binary {
    pub async fn start(address: &str, settings: &[(&str, &str)]) -> Self {
        let mut child = super::support::binary(address, settings)
            .env("KOMODO_TRANSPORT", "http")
            .env("KOMODO_HTTP_BIND", "127.0.0.1:0")
            .env("KOMODO_HTTP_TOKENS", format!("{FIRST},{SECOND}"))
            .env("KOMODO_HTTP_ALLOWED_ORIGINS", ORIGIN)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let address = timeout(DEADLINE, async {
            while let Some(line) = lines.next_line().await.unwrap() {
                assert_redacted(&line);
                if line.contains("starting komodo-mcp over HTTP") {
                    return line
                        .split_whitespace()
                        .find_map(|part| part.strip_prefix("address="))
                        .and_then(|addr| addr.parse::<SocketAddr>().ok())
                        .expect("bound address in readiness log");
                }
            }
            panic!("binary exited before listening");
        })
        .await
        .unwrap();
        let diagnostics = tokio::spawn(async move {
            while let Some(line) = lines.next_line().await.unwrap() {
                assert_redacted(&line);
            }
        });
        Self {
            http: Http::new(address),
            child,
            diagnostics,
        }
    }

    pub async fn finish(mut self) {
        self.child.start_kill().unwrap();
        let output = timeout(DEADLINE, self.child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(output.stdout.is_empty());
        timeout(DEADLINE, self.diagnostics).await.unwrap().unwrap();
    }
}
