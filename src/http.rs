//! Authenticated Streamable HTTP. rmcp owns protocol negotiation, dispatch and SSE replay.

use std::{
    error::Error,
    future::{Future, IntoFuture},
    io,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::any,
};
use http_body_util::BodyExt;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, InitializeRequestParams, InitializeResult,
        ListToolsResult, PaginatedRequestParams, ServerConfig, Tool,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio::{
    net::TcpListener,
    sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError, oneshot},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    client::{CoreClient, CoreVersion},
    config::HttpConfig,
    server::KomodoServer,
};

mod cors;

const SESSION_HEADER: &str = "mcp-session-id";
const MAX_SESSIONS: usize = 32;
// Independent pools keep monitoring streams from occupying the slots used for calls and cleanup.
const MAX_STREAMS: usize = 32;
const MAX_OTHER_REQUESTS: usize = 32;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_BODY_DEADLINE: Duration = Duration::from_secs(15);
const SHUTDOWN_DRAIN: Duration = Duration::from_secs(30);

struct Lifecycle {
    shutdown: CancellationToken,
    cleanup: TaskTracker,
}

struct LegacySession {
    server: KomodoServer,
    // Dropping the last handler wakes its bounded cleanup task, including on idle expiry.
    _alive: oneshot::Sender<()>,
}

impl Drop for LegacySession {
    fn drop(&mut self) {
        self.server.client.invalidate_session();
    }
}

/// The SDK constructs one handler per initialized session, or per request on
/// the discover-based protocol. Only a real legacy handshake creates private
/// session state. Stateless calls share the authenticated principal's state.
struct HttpHandler {
    principal: Arc<KomodoServer>,
    session_slots: Arc<Semaphore>,
    lifecycle: Arc<Lifecycle>,
    legacy: OnceLock<LegacySession>,
}

impl HttpHandler {
    fn server(&self) -> &KomodoServer {
        self.legacy
            .get()
            .map_or(&self.principal, |session| &session.server)
    }
}

#[derive(Clone, Default)]
struct Admission(Arc<AtomicBool>);

impl ServerHandler for HttpHandler {
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        let info = self.negotiate_initialize(&request)?;
        let slot = self.session_slots.clone().try_acquire_owned();
        let Ok(slot) = slot else {
            if let Some(admission) = context
                .extensions
                .get::<Parts>()
                .and_then(|parts| parts.extensions.get::<Admission>())
            {
                admission.0.store(true, Ordering::Relaxed);
            }
            return Err(ErrorData::internal_error("Session limit reached", None));
        };
        if self.lifecycle.shutdown.is_cancelled() {
            return Err(ErrorData::internal_error("Server shutting down", None));
        }
        let client = self.principal.client.isolated_session();
        let (alive, ended) = oneshot::channel();
        let owner = LegacySession {
            server: KomodoServer {
                client: client.clone(),
                core_version: self.principal.core_version.clone(),
            },
            _alive: alive,
        };
        self.legacy
            .set(owner)
            .map_err(|_| ErrorData::invalid_request("Session already initialized", None))?;
        // The SDK cancels the handshake context when its session worker ends.
        // A previously dispatched request may still hold the handler alive.
        let session_closed = context.ct.clone();
        let shutdown = self.lifecycle.shutdown.clone();
        self.lifecycle.cleanup.spawn(async move {
            tokio::select! {
                _ = ended => (),
                _ = session_closed.cancelled() => (),
                _ = shutdown.cancelled() => (),
            }
            client.close_session().await;
            drop(slot);
        });
        context.peer.set_peer_info(request);
        Ok(info)
    }

    fn get_info(&self) -> ServerConfig {
        self.server().get_info()
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.server().get_tool(name)
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.server().list_tools(request, context).await
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.server().call_tool(request, context).await
    }
}

struct Principal {
    server: Arc<KomodoServer>,
    sessions: Arc<LocalSessionManager>,
    requests: RequestCapacity,
    transport: StreamableHttpService<HttpHandler, LocalSessionManager>,
}

struct RequestCapacity {
    streams: Arc<Semaphore>,
    other_requests: Arc<Semaphore>,
}

impl RequestCapacity {
    fn new(count: usize, index: usize) -> Self {
        Self {
            streams: reserved_slots(MAX_STREAMS, count, index),
            other_requests: reserved_slots(MAX_OTHER_REQUESTS, count, index),
        }
    }

    fn acquire(&self, method: &Method) -> Result<OwnedSemaphorePermit, TryAcquireError> {
        let pool = if method == Method::GET {
            &self.streams
        } else {
            &self.other_requests
        };
        pool.clone().try_acquire_owned()
    }
}

// Assign the remainder once so every credential has a reservation and the total stays bounded.
fn reserved_slots(total: usize, count: usize, index: usize) -> Arc<Semaphore> {
    Arc::new(Semaphore::new(
        total / count + usize::from(index < total % count),
    ))
}

struct HttpState {
    config: HttpConfig,
    principals: Vec<Principal>,
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(HeaderValue::to_str)
        .transpose()
        .map_err(|_| ())?;
    if values.next().is_some() {
        Err(())
    } else {
        Ok(value)
    }
}

fn unauthorized() -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, "Bearer authentication required").into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

async fn mcp(State(state): State<Arc<HttpState>>, request: Request) -> Response {
    let origin = match single_header(request.headers(), header::ORIGIN.as_str()) {
        Ok(None) => None,
        Ok(Some(origin)) if state.config.allows_origin(origin) => {
            request.headers().get(header::ORIGIN).cloned()
        }
        _ => return (StatusCode::FORBIDDEN, "Origin is not allowed").into_response(),
    };
    let mut response = if request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
    {
        cors::preflight(request.headers(), origin.is_some())
    } else {
        authenticated(&state, request).await
    };
    cors::set_response_headers(response.headers_mut(), origin);
    response
}

async fn authenticated(state: &HttpState, mut request: Request) -> Response {
    let token = single_header(request.headers(), header::AUTHORIZATION.as_str())
        .ok()
        .flatten()
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map(|(_, token)| token.trim_start_matches(' '));
    let Some(index) = token.and_then(|token| state.config.authenticate(token)) else {
        return unauthorized();
    };
    // rmcp passes HTTP parts into request extensions. The MCP handler never needs the credential.
    request.headers_mut().remove(header::AUTHORIZATION);
    let principal = &state.principals[index];
    match single_header(request.headers(), SESSION_HEADER) {
        Ok(Some(id)) => {
            if !principal.sessions.sessions.read().await.contains_key(id) {
                return (StatusCode::NOT_FOUND, "Session not found").into_response();
            }
        }
        Ok(None) => (),
        Err(()) => return (StatusCode::BAD_REQUEST, "Invalid session header").into_response(),
    }
    if single_header(request.headers(), "last-event-id").is_err() {
        return (StatusCode::BAD_REQUEST, "Invalid resumption header").into_response();
    }
    let Ok(slot) = principal.requests.acquire(request.method()) else {
        return (StatusCode::TOO_MANY_REQUESTS, "Request limit reached").into_response();
    };
    // Apply a wall-clock limit to receiving input, without timing out a submitted Core operation.
    if request.method() == Method::POST {
        let body = std::mem::take(request.body_mut());
        let input = axum::body::to_bytes(body, MAX_BODY_BYTES);
        let bytes = match tokio::time::timeout(REQUEST_BODY_DEADLINE, input).await {
            Err(_) => {
                return (StatusCode::REQUEST_TIMEOUT, "Request body timed out").into_response();
            }
            Ok(Err(error))
                if error
                    .source()
                    .is_some_and(|e| e.is::<http_body_util::LengthLimitError>()) =>
            {
                return (StatusCode::PAYLOAD_TOO_LARGE, "Request body too large").into_response();
            }
            Ok(Err(_)) => return (StatusCode::BAD_REQUEST, "Invalid request body").into_response(),
            Ok(Ok(bytes)) => bytes,
        };
        *request.body_mut() = Body::from(bytes);
    }
    let admission = Admission::default();
    if request.method() == Method::POST && !request.headers().contains_key(SESSION_HEADER) {
        request.extensions_mut().insert(admission.clone());
    }
    let response = principal.transport.handle(request).await;
    if admission.0.load(Ordering::Relaxed) {
        return (StatusCode::TOO_MANY_REQUESTS, "Session limit reached").into_response();
    }
    // Count a streaming response until the body completes or the recipient disconnects.
    response.map(|body| {
        Body::new(body.with_trailers(async move {
            drop(slot);
            None
        }))
    })
}

pub async fn serve(
    listener: TcpListener,
    config: HttpConfig,
    client: CoreClient,
    version: CoreVersion,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    let lifecycle = Arc::new(Lifecycle {
        shutdown: CancellationToken::new(),
        cleanup: TaskTracker::new(),
    });
    let mut sdk_config = StreamableHttpServerConfig::default()
        .with_allowed_origins(config.allowed_origins().iter().cloned())
        .enforce_origin_validation()
        .with_json_response(true)
        .with_max_request_body_bytes(MAX_BODY_BYTES)
        .with_cancellation_token(lifecycle.shutdown.clone());
    let bound_ip = listener.local_addr()?.ip();
    let host = bound_ip.to_string();
    if !bound_ip.is_unspecified() && !sdk_config.allowed_hosts.contains(&host) {
        sdk_config.allowed_hosts.push(host);
    }
    let count = config.principal_count();
    let mut principals = Vec::with_capacity(count);
    for index in 0..count {
        let server = Arc::new(KomodoServer {
            client: client.isolated_session(),
            core_version: version.clone(),
        });
        let sessions = Arc::new(LocalSessionManager::default());
        let transport = StreamableHttpService::new(
            {
                let principal = server.clone();
                let lifecycle = lifecycle.clone();
                let session_slots = reserved_slots(MAX_SESSIONS, count, index);
                move || {
                    Ok(HttpHandler {
                        principal: principal.clone(),
                        session_slots: session_slots.clone(),
                        lifecycle: lifecycle.clone(),
                        legacy: OnceLock::new(),
                    })
                }
            },
            sessions.clone(),
            sdk_config.clone(),
        );
        principals.push(Principal {
            server,
            sessions,
            requests: RequestCapacity::new(count, index),
            transport,
        });
    }
    let state = Arc::new(HttpState { config, principals });
    let app = Router::new()
        .route("/mcp", any(mcp))
        .with_state(state.clone());
    let serving = axum::serve(listener, app)
        .with_graceful_shutdown(lifecycle.shutdown.clone().cancelled_owned())
        .into_future();
    tokio::pin!(serving);
    let finished = tokio::select! {
        result = &mut serving => Some(result),
        _ = shutdown => None,
    };
    lifecycle.shutdown.cancel();
    for principal in &state.principals {
        principal.requests.streams.close();
        principal.requests.other_requests.close();
        let owner = principal.server.client.clone();
        owner.invalidate_session();
        lifecycle.cleanup.spawn(async move {
            owner.close_session().await;
        });
    }
    tokio::time::timeout(SHUTDOWN_DRAIN, async {
        let result = match finished {
            Some(result) => result,
            None => serving.await,
        };
        lifecycle.cleanup.close();
        lifecycle.cleanup.wait().await;
        result
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "HTTP session shutdown timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscriptions_leave_room_for_control_and_other_principals() {
        // With 32 configured bearers, each has one slot in each pool.
        let first = RequestCapacity::new(32, 0);
        let second = RequestCapacity::new(32, 1);
        let subscription = first.acquire(&Method::GET).unwrap();
        assert!(first.acquire(&Method::GET).is_err());
        let _cleanup = first.acquire(&Method::DELETE).unwrap();
        let _independent_subscription = second.acquire(&Method::GET).unwrap();
        let _independent_call = second.acquire(&Method::POST).unwrap();
        drop(subscription);
        assert!(first.acquire(&Method::GET).is_ok());
    }

    #[test]
    fn reservations_distribute_the_process_limit() {
        for count in 1..=32 {
            for limit in [MAX_SESSIONS, MAX_STREAMS, MAX_OTHER_REQUESTS] {
                let slots: Vec<_> = (0..count)
                    .map(|index| reserved_slots(limit, count, index).available_permits())
                    .collect();
                assert!(slots.iter().all(|value| *value >= 1));
                assert_eq!(slots.iter().sum::<usize>(), limit);
                assert!(slots[0] - slots[count - 1] <= 1);
            }
        }
    }
}
