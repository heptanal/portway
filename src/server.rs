use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{
        ConnectInfo, DefaultBodyLimit, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_server::tls_rustls::RustlsConfig;
use serde::{Deserialize, Serialize};
use tokio::{
    net::TcpListener,
    sync::{Mutex, broadcast},
};
use tracing::{info, warn};

use crate::{
    auth::Authenticator,
    config::Config,
    input::SharedBackend,
    protocol::{ClientEnvelope, MAX_MESSAGE_BYTES, PROTOCOL_VERSION, ServerMessage},
    session::{
        HEARTBEAT_TIMEOUT, HEARTBEAT_TIMEOUT_MS, RateLimiter, SessionError, SessionOutcome,
        SessionState,
    },
};

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE_CSS: &str = include_str!("../web/style.css");
const SESSION_COOKIE: &str = "portway_session";
const PAIR_REQUEST_LIMIT_BYTES: usize = 512;
const AUTH_ATTEMPT_LIMIT: u32 = 8;
const AUTH_ATTEMPT_WINDOW: Duration = Duration::from_mins(1);
const MAX_AUTH_ATTEMPT_ADDRESSES: usize = 1_024;

#[derive(Clone)]
pub struct AppState {
    backend: SharedBackend,
    auth: Authenticator,
    pointer_sensitivity: f64,
    max_clients: usize,
    active_clients: Arc<AtomicUsize>,
    allowed_origins: Arc<Vec<String>>,
    secure_transport: bool,
    auth_attempts: Arc<Mutex<HashMap<IpAddr, AuthAttemptWindow>>>,
    shutdown: broadcast::Sender<()>,
}

struct AuthAttemptWindow {
    started: Instant,
    attempts: u32,
}

impl AppState {
    #[must_use]
    pub fn new(config: &Config, backend: SharedBackend, auth: Authenticator) -> Self {
        let (shutdown, _) = broadcast::channel(8);
        Self {
            backend,
            auth,
            pointer_sensitivity: config.pointer_sensitivity,
            max_clients: config.max_clients,
            active_clients: Arc::new(AtomicUsize::new(0)),
            allowed_origins: Arc::new(config.allowed_origins.clone()),
            secure_transport: config.tls_enabled(),
            auth_attempts: Arc::new(Mutex::new(HashMap::new())),
            shutdown,
        }
    }

    async fn allow_auth_attempt(&self, remote: IpAddr) -> bool {
        let now = Instant::now();
        let mut attempts = self.auth_attempts.lock().await;
        attempts.retain(|_, window| now.duration_since(window.started) < AUTH_ATTEMPT_WINDOW);
        if !attempts.contains_key(&remote) && attempts.len() >= MAX_AUTH_ATTEMPT_ADDRESSES {
            return false;
        }
        let window = attempts.entry(remote).or_insert(AuthAttemptWindow {
            started: now,
            attempts: 0,
        });
        if window.attempts >= AUTH_ATTEMPT_LIMIT {
            return false;
        }
        window.attempts += 1;
        true
    }

    async fn clear_auth_attempts(&self, remote: IpAddr) {
        self.auth_attempts.lock().await.remove(&remote);
    }

    fn acquire_controller(&self) -> Option<ControllerPermit> {
        loop {
            let current = self.active_clients.load(Ordering::Acquire);
            if current >= self.max_clients {
                return None;
            }
            if self
                .active_clients
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(ControllerPermit {
                    active: Arc::clone(&self.active_clients),
                });
            }
        }
    }
}

struct ControllerPermit {
    active: Arc<AtomicUsize>,
}

impl Drop for ControllerPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/style.css", get(style_css))
        .route("/healthz", get(health))
        .route("/api/status", get(status))
        .route("/api/session", get(session_status))
        .route("/api/pair", post(pair))
        .route("/api/session/logout", post(logout))
        .route("/ws", get(websocket))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(PAIR_REQUEST_LIMIT_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_headers,
        ))
        .with_state(state)
}

pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let shutdown_sender = state.shutdown.clone();
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown.await;
        let _ = shutdown_sender.send(());
    })
    .await
}

pub async fn serve_tls(
    listener: TcpListener,
    state: AppState,
    tls_config: RustlsConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let listener = listener.into_std()?;
    let shutdown_sender = state.shutdown.clone();
    let handle = axum_server::Handle::<SocketAddr>::new();
    let shutdown_handle = handle.clone();
    let shutdown_task = tokio::spawn(async move {
        shutdown.await;
        let _ = shutdown_sender.send(());
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(5)));
    });
    let result = axum_server::from_tcp_rustls(listener, tls_config)?
        .handle(handle)
        .serve(router(state).into_make_service_with_connect_info::<SocketAddr>())
        .await;
    shutdown_task.abort();
    result
}

async fn index() -> impl IntoResponse {
    static_asset("text/html; charset=utf-8", INDEX_HTML)
}

async fn app_js() -> impl IntoResponse {
    static_asset("text/javascript; charset=utf-8", APP_JS)
}

async fn style_css() -> impl IntoResponse {
    static_asset("text/css; charset=utf-8", STYLE_CSS)
}

fn static_asset(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache, no-store, must-revalidate"),
            ),
        ],
        body,
    )
}

async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[derive(Serialize)]
struct PublicStatus {
    service: &'static str,
    version: &'static str,
    authentication_required: bool,
    controllers: usize,
    controller_limit: usize,
    input_backend: String,
    input_available: bool,
    secure_transport: bool,
}

async fn status(State(state): State<AppState>) -> impl IntoResponse {
    let backend = state.backend.lock().await.status();
    axum::Json(PublicStatus {
        service: "portway",
        version: env!("CARGO_PKG_VERSION"),
        authentication_required: state.auth.is_enabled(),
        controllers: state.active_clients.load(Ordering::Acquire),
        controller_limit: state.max_clients,
        input_backend: backend.name,
        input_available: backend.available,
        secure_transport: state.secure_transport,
    })
}

#[derive(Serialize)]
struct SessionStatus {
    authenticated: bool,
    authentication_required: bool,
}

async fn session_status(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    Json(SessionStatus {
        authenticated: state.auth.verify_session(session_cookie(&headers)).await,
        authentication_required: state.auth.is_enabled(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairRequest {
    code: String,
}

#[derive(Serialize)]
struct PairResponse {
    authenticated: bool,
    expires_in_seconds: u64,
}

async fn pair(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    uri: http::Uri,
    headers: HeaderMap,
    Json(request): Json<PairRequest>,
) -> Response {
    if !origin_allowed(
        &headers,
        &state.allowed_origins,
        &uri,
        state.secure_transport,
    ) {
        warn!(%remote, "pairing origin rejected");
        return (StatusCode::FORBIDDEN, "origin rejected").into_response();
    }
    if !state.auth.is_enabled() {
        return Json(PairResponse {
            authenticated: true,
            expires_in_seconds: 0,
        })
        .into_response();
    }
    if !state.allow_auth_attempt(remote.ip()).await {
        warn!(%remote, "pairing rate limit exceeded");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, HeaderValue::from_static("60"))],
            "too many pairing attempts",
        )
            .into_response();
    }
    let credential = request.code.trim();
    if !(8..=128).contains(&credential.len()) {
        warn!(%remote, "controller pairing failed");
        return (StatusCode::UNAUTHORIZED, "pairing failed").into_response();
    }
    let session = match state.auth.exchange(credential).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            warn!(%remote, "controller pairing failed");
            return (StatusCode::UNAUTHORIZED, "pairing failed").into_response();
        }
        Err(error) => {
            warn!(%remote, %error, "controller pairing failed internally");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    state.clear_auth_attempts(remote.ip()).await;
    info!(%remote, "controller paired");
    let mut response = Json(PairResponse {
        authenticated: true,
        expires_in_seconds: state.auth.session_ttl_seconds(),
    })
    .into_response();
    let cookie = session_cookie_header(
        &session,
        state.auth.session_ttl_seconds(),
        state.secure_transport,
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("session cookie contains only safe characters"),
    );
    response
}

async fn logout(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    uri: http::Uri,
    headers: HeaderMap,
) -> Response {
    if !origin_allowed(
        &headers,
        &state.allowed_origins,
        &uri,
        state.secure_transport,
    ) {
        warn!(%remote, "logout origin rejected");
        return (StatusCode::FORBIDDEN, "origin rejected").into_response();
    }
    state.auth.revoke_session(session_cookie(&headers)).await;
    let mut response = StatusCode::NO_CONTENT.into_response();
    let cookie = expired_session_cookie(state.secure_transport);
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("static cookie attributes are valid"),
    );
    info!(%remote, "controller session logged out");
    response
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(name, value)| (name == SESSION_COOKIE).then_some(value))
}

fn session_cookie_header(session: &str, max_age: u64, secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}={session}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}

fn expired_session_cookie(secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    )
}

async fn websocket(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    uri: http::Uri,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(
        &headers,
        &state.allowed_origins,
        &uri,
        state.secure_transport,
    ) {
        warn!(%remote, "WebSocket origin rejected");
        return (StatusCode::FORBIDDEN, "origin rejected").into_response();
    }
    if !state.auth.verify_session(session_cookie(&headers)).await {
        warn!(%remote, "controller authentication failed");
        return (StatusCode::UNAUTHORIZED, "authentication failed").into_response();
    }
    let Some(permit) = state.acquire_controller() else {
        warn!(%remote, "controller limit reached");
        return (StatusCode::SERVICE_UNAVAILABLE, "controller limit reached").into_response();
    };

    info!(%remote, "controller authenticated");
    upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| controller(socket, state, permit, remote))
}

fn origin_allowed(
    headers: &HeaderMap,
    allowed_origins: &[String],
    request_uri: &http::Uri,
    secure_transport: bool,
) -> bool {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    if allowed_origins.iter().any(|allowed| allowed == origin) {
        return true;
    }
    let Ok(uri) = origin.parse::<http::Uri>() else {
        return false;
    };
    let request_authority = request_uri
        .authority()
        .map(http::uri::Authority::as_str)
        .or_else(|| {
            headers
                .get(header::HOST)
                .and_then(|value| value.to_str().ok())
        });
    let Some(request_authority) = request_authority else {
        return false;
    };
    uri.scheme_str() == Some(if secure_transport { "https" } else { "http" })
        && uri
            .authority()
            .is_some_and(|authority| authority.as_str().eq_ignore_ascii_case(request_authority))
        && uri.path_and_query().is_none_or(|path| path.as_str() == "/")
}

async fn controller(
    mut socket: WebSocket,
    state: AppState,
    _permit: ControllerPermit,
    remote: SocketAddr,
) {
    let backend_status = state.backend.lock().await.status();
    let ready = ServerMessage::Ready {
        v: PROTOCOL_VERSION,
        backend: backend_status.name,
        input_available: backend_status.available,
        pointer_sensitivity: state.pointer_sensitivity,
        heartbeat_timeout_ms: HEARTBEAT_TIMEOUT_MS,
    };
    if send_json(&mut socket, &ready).await.is_err() {
        return;
    }

    let mut session = SessionState::new(state.pointer_sensitivity);
    let mut limiter = RateLimiter::standard();
    let mut last_activity = Instant::now();
    let mut timeout_tick = tokio::time::interval(Duration::from_secs(1));
    let mut shutdown = state.shutdown.subscribe();

    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            _ = timeout_tick.tick() => {
                if last_activity.elapsed() > HEARTBEAT_TIMEOUT {
                    let _ = send_error(&mut socket, "heartbeat_timeout", "controller timed out").await;
                    break;
                }
            }
            incoming = socket.recv() => {
                let Some(incoming) = incoming else { break };
                let Ok(message) = incoming else { break };
                match message {
                    Message::Text(text) => {
                        last_activity = Instant::now();
                        if !limiter.allow_at(last_activity) {
                            warn!(%remote, "controller rate limit exceeded");
                            let _ = send_error(&mut socket, "rate_limited", "message rate limit exceeded").await;
                            break;
                        }
                        let Ok(envelope) = serde_json::from_str::<ClientEnvelope>(&text) else {
                            let _ = send_error(&mut socket, "malformed_message", "message does not match protocol v1").await;
                            break;
                        };
                        match session.process(envelope, &state.backend).await {
                            Ok(SessionOutcome::Heartbeat(sequence)) => {
                                let pong = ServerMessage::Pong { v: PROTOCOL_VERSION, seq: sequence };
                                if send_json(&mut socket, &pong).await.is_err() { break; }
                            }
                            Ok(SessionOutcome::Applied) => {}
                            Err(SessionError::Input(error)) => {
                                warn!(%remote, %error, "input command failed");
                                if send_error(&mut socket, "input_unavailable", &error.to_string()).await.is_err() { break; }
                            }
                            Err(error) => {
                                let _ = send_error(&mut socket, "invalid_message", &error.to_string()).await;
                                break;
                            }
                        }
                    }
                    Message::Ping(payload) => {
                        if socket.send(Message::Pong(payload)).await.is_err() { break; }
                    }
                    Message::Pong(_) => last_activity = Instant::now(),
                    Message::Close(_) => break,
                    Message::Binary(_) => {
                        let _ = send_error(&mut socket, "binary_unsupported", "binary messages are not accepted").await;
                        break;
                    }
                }
            }
        }
    }

    if let Err(error) = session.cleanup(&state.backend).await {
        warn!(%remote, %error, "controller cleanup failed");
    }
    let _ = socket.send(Message::Close(None)).await;
    info!(%remote, "controller disconnected and input state released");
}

async fn send_json(socket: &mut WebSocket, message: &ServerMessage) -> Result<(), ()> {
    let json = serde_json::to_string(message).map_err(|_| ())?;
    socket
        .send(Message::Text(json.into()))
        .await
        .map_err(|_| ())
}

async fn send_error(socket: &mut WebSocket, code: &'static str, message: &str) -> Result<(), ()> {
    send_json(
        socket,
        &ServerMessage::Error {
            v: PROTOCOL_VERSION,
            code,
            message: message.to_owned(),
        },
    )
    .await
}

async fn security_headers(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; connect-src 'self'; img-src 'self' data:; script-src 'self'; style-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=()"),
    );
    headers.insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    if state.secure_transport {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    response
}

async fn not_found() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "not found")
}

#[cfg(test)]
mod tests {
    use std::{net::IpAddr, path::PathBuf, sync::Arc};

    use axum::{body::Body, http::Request};
    use futures_util::{SinkExt, StreamExt};
    use tokio::sync::oneshot;
    use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};
    use tower::ServiceExt;

    use crate::{
        config::{AuthMode, BackendKind},
        input::{RecordedEvent, RecordingBackend},
        protocol::{ClientEvent, KeyState, MouseButton},
    };

    use super::*;

    fn test_config() -> Config {
        Config {
            config_path: PathBuf::from("test.toml"),
            listen: IpAddr::from([127, 0, 0, 1]),
            port: 2721,
            auth_mode: AuthMode::Token,
            token_file: PathBuf::from("token"),
            tls_cert: None,
            tls_key: None,
            pairing_code_ttl_seconds: 300,
            session_ttl_seconds: 43_200,
            backend: BackendKind::Mock,
            max_clients: 1,
            pointer_sensitivity: 1.0,
            log_level: "info".into(),
            allowed_origins: Vec::new(),
            mouse_name: "test mouse".into(),
            keyboard_name: "test keyboard".into(),
        }
    }

    fn pair_request(code: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/pair")
            .header(header::HOST, "127.0.0.1:2721")
            .header(header::ORIGIN, "http://127.0.0.1:2721")
            .header(header::CONTENT_TYPE, "application/json")
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_000))))
            .body(Body::from(format!(r#"{{"code":"{code}"}}"#)))
            .unwrap()
    }

    #[test]
    fn origin_requires_matching_host_or_allowlist() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("192.0.2.10:2721"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://192.0.2.10:2721"),
        );
        let uri = http::Uri::from_static("/");
        assert!(origin_allowed(&headers, &[], &uri, false));
        assert!(!origin_allowed(&headers, &[], &uri, true));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(!origin_allowed(&headers, &[], &uri, false));
        assert!(origin_allowed(
            &headers,
            &["https://evil.example".into()],
            &uri,
            false
        ));

        headers.remove(header::HOST);
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://192.0.2.10:2721"),
        );
        let h2_uri = "https://192.0.2.10:2721/api/pair".parse().unwrap();
        assert!(origin_allowed(&headers, &[], &h2_uri, true));
    }

    #[tokio::test]
    async fn pairing_code_sets_hardened_cookie_and_cannot_be_replayed() {
        let (recording, _) = RecordingBackend::new();
        let backend: SharedBackend = Arc::new(tokio::sync::Mutex::new(Box::new(recording)));
        let auth = Authenticator::from_token(b"integration-token".to_vec());
        let code = auth.issue_pairing_code().unwrap().unwrap();
        let app = router(AppState::new(&test_config(), backend, auth));

        let response = app.clone().oneshot(pair_request(&code)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.starts_with("portway_session="));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(!cookie.contains("integration-token"));

        let replay = app.oneshot(pair_request(&code)).await.unwrap();
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pairing_attempts_are_rate_limited_per_address() {
        let (recording, _) = RecordingBackend::new();
        let backend: SharedBackend = Arc::new(tokio::sync::Mutex::new(Box::new(recording)));
        let state = AppState::new(
            &test_config(),
            backend,
            Authenticator::from_token(b"integration-token".to_vec()),
        );
        let remote = IpAddr::from([192, 0, 2, 10]);

        for _ in 0..AUTH_ATTEMPT_LIMIT {
            assert!(state.allow_auth_attempt(remote).await);
        }
        assert!(!state.allow_auth_attempt(remote).await);
        state.clear_auth_attempts(remote).await;
        assert!(state.allow_auth_attempt(remote).await);
    }

    #[tokio::test]
    async fn websocket_authenticates_records_and_cleans_up() {
        let (recording, handle) = RecordingBackend::new();
        let backend: SharedBackend = Arc::new(tokio::sync::Mutex::new(Box::new(recording)));
        let auth = Authenticator::from_token(b"integration-token".to_vec());
        let session = auth.exchange("integration-token").await.unwrap().unwrap();
        let state = AppState::new(&test_config(), backend, auth);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve(listener, state, async move {
            let _ = shutdown_rx.await;
        }));

        let mut request = format!("ws://{address}/ws").into_client_request().unwrap();
        request.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_str(&format!("http://{address}")).unwrap(),
        );
        request.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}={session}")).unwrap(),
        );
        let (mut websocket, _) = connect_async(request).await.unwrap();
        let ready = websocket.next().await.unwrap().unwrap();
        assert!(ready.to_text().unwrap().contains("\"type\":\"ready\""));

        let down = ClientEnvelope {
            v: PROTOCOL_VERSION,
            seq: 1,
            event: ClientEvent::PointerButton {
                button: MouseButton::Left,
                state: KeyState::Down,
            },
        };
        websocket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&down).unwrap().into(),
            ))
            .await
            .unwrap();
        websocket.close(None).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if handle.events().contains(&RecordedEvent::ButtonUp {
                    button: MouseButton::Left,
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(handle.events().contains(&RecordedEvent::ButtonDown {
            button: MouseButton::Left,
        }));
        let _ = shutdown_tx.send(());
        server.await.unwrap().unwrap();
    }
}
