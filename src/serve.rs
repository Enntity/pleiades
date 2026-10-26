// SPDX-License-Identifier: AGPL-3.0-only
//! `shrug serve`: the gateway. A thin authenticated proxy in front of the
//! engine on rank 0's loopback. Streams responses frame by frame, rewrites
//! the requested model to the release's, and records per-request stats.
use anyhow::{Context, Result};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Frame, Incoming};
use hyper::header::{
    CONNECTION, HOST, HeaderValue, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER,
    TRANSFER_ENCODING, UPGRADE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;

type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

const MAX_JSON_BODY: u64 = 64 * 1024 * 1024;
const MAX_FINAL_JSON: usize = 8 * 1024 * 1024;
const STATS_KEEP: usize = 200;

/// Where the engine listens on rank 0.
#[derive(Clone)]
pub struct Cfg {
    pub listen: SocketAddr,
    pub api_port: u16,
    pub model_id: String,
    pub api_key: Option<String>,
    /// Test hook: require auth even from loopback peers.
    pub force_auth: bool,
}

#[derive(Clone, Serialize)]
pub struct ReqEvent {
    pub path: String,
    pub model: Option<String>,
    pub status: u16,
    pub started_at: u64,
    pub first_byte_ms: Option<u64>,
    pub total_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Default)]
pub struct Stats {
    pub events: VecDeque<ReqEvent>,
    pub total: u64,
}

impl Stats {
    fn push(&mut self, e: ReqEvent) {
        self.total += 1;
        if self.events.len() >= STATS_KEEP {
            self.events.pop_front();
        }
        self.events.push_back(e);
    }
}

pub struct Shared {
    pub cfg: Cfg,
    pub client: Client<HttpConnector, BoxBody>,
    pub stats: Arc<Mutex<Stats>>,
}

/// Constant-time string compare.
fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Loopback exemption and the key check itself, factored for tests.
pub fn authorize(
    headers: &hyper::HeaderMap,
    key: Option<&str>,
    peer_loopback: bool,
    force_auth: bool,
) -> bool {
    let Some(key) = key else {
        return true;
    };
    if peer_loopback && !force_auth {
        return true;
    }
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| ct_eq(t, key));
    let xkey = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| ct_eq(t, key));
    bearer || xkey
}

fn json_error(status: StatusCode, msg: &str) -> Response<BoxBody> {
    let body = json!({"error": {"message": msg, "type": "invalid_request_error"}});
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(
            Full::new(Bytes::from(body.to_string()))
                .map_err(|never| match never {})
                .boxed(),
        )
        .unwrap()
}

const HOP_HEADERS: [hyper::header::HeaderName; 8] = [
    CONNECTION,
    PROXY_AUTHENTICATE,
    PROXY_AUTHORIZATION,
    TE,
    TRAILER,
    TRANSFER_ENCODING,
    UPGRADE,
    hyper::header::HeaderName::from_static("keep-alive"),
];

fn strip_hop(headers: &mut hyper::HeaderMap) {
    for h in &HOP_HEADERS {
        headers.remove(h);
    }
}

/// Usage parsed out of a passing byte stream: SSE events or one JSON body.
#[derive(Default)]
struct UsageTap {
    sse: bool,
    pending: Vec<u8>,
    buffered: Vec<u8>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

impl UsageTap {
    fn feed(&mut self, chunk: &[u8]) {
        if self.sse {
            self.pending.extend_from_slice(chunk);
            while let Some(pos) = self.pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=pos).collect();
                self.sse_line(&line);
            }
        } else if self.buffered.len() < MAX_FINAL_JSON {
            self.buffered.extend_from_slice(chunk);
        }
    }

    fn sse_line(&mut self, line: &[u8]) {
        let Ok(line) = std::str::from_utf8(line) else {
            return;
        };
        let Some(data) = line.trim().strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data == "[DONE]" {
            return;
        }
        if let Ok(v) = serde_json::from_str::<Value>(data) {
            self.take_usage(&v["usage"]);
        }
    }

    fn take_usage(&mut self, usage: &Value) {
        for k in ["prompt_tokens", "input_tokens"] {
            if let Some(n) = usage[k].as_u64() {
                self.input_tokens = Some(n);
            }
        }
        for k in ["completion_tokens", "output_tokens"] {
            if let Some(n) = usage[k].as_u64() {
                self.output_tokens = Some(n);
            }
        }
    }

    fn finish(&mut self) {
        if !self.sse
            && !self.buffered.is_empty()
            && let Ok(v) = serde_json::from_slice::<Value>(&self.buffered)
        {
            self.take_usage(&v["usage"]);
        }
    }
}

/// A body wrapper that taps each passing frame for stats and first-byte
/// timing, and finalizes the request event on end or drop (client disconnect
/// drops the upstream response, which cancels the engine's work).
struct TapBody {
    inner: BoxBody,
    done: bool,
    tap: UsageTap,
    ev: ReqEvent,
    started: Instant,
    stats: Arc<Mutex<Stats>>,
}

impl hyper::body::Body for TapBody {
    type Data = Bytes;
    type Error = hyper::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.as_mut().get_mut();
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    if this.ev.first_byte_ms.is_none() {
                        this.ev.first_byte_ms = Some(this.started.elapsed().as_millis() as u64);
                    }
                    this.tap.feed(data);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(None) => {
                this.finish();
                Poll::Ready(None)
            }
            other => other,
        }
    }
}

impl TapBody {
    fn finish(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        self.tap.finish();
        self.ev.total_ms = self.started.elapsed().as_millis() as u64;
        self.ev.input_tokens = self.tap.input_tokens;
        self.ev.output_tokens = self.tap.output_tokens;
        if let Ok(mut s) = self.stats.lock() {
            s.push(self.ev.clone());
        }
    }
}

impl Drop for TapBody {
    fn drop(&mut self) {
        self.finish();
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Handle one request end to end. Public for tests that run their own
/// accept loop on an ephemeral port.
pub async fn handle_test(
    sh: Arc<Shared>,
    peer_loopback: bool,
    req: Request<Incoming>,
) -> Result<Response<BoxBody>, std::convert::Infallible> {
    handle(sh, peer_loopback, req).await
}

async fn handle(
    sh: Arc<Shared>,
    peer_loopback: bool,
    req: Request<Incoming>,
) -> Result<Response<BoxBody>, std::convert::Infallible> {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    // Auth: on a non-loopback listen, non-loopback peers need the key.
    let health_probe = method == Method::GET && path == "/health";
    if !health_probe
        && !authorize(
            req.headers(),
            sh.cfg.api_key.as_deref(),
            peer_loopback,
            sh.cfg.force_auth,
        )
    {
        return Ok(json_error(
            StatusCode::UNAUTHORIZED,
            "invalid or missing API key",
        ));
    }
    match (method, path.as_str()) {
        (Method::GET, "/health") => Ok(health(&sh).await),
        (Method::GET, "/v1/models") => Ok(models(&sh)),
        (Method::GET, "/shrug/stats") => Ok(stats(&sh)),
        (
            Method::POST,
            "/v1/chat/completions"
            | "/v1/completions"
            | "/v1/responses"
            | "/v1/messages"
            | "/v1/messages/count_tokens",
        ) => Ok(proxy_passthrough(sh, req).await),
        _ => Ok(json_error(StatusCode::NOT_FOUND, "not found")),
    }
}

fn boxed(v: &Value) -> BoxBody {
    Full::new(Bytes::from(v.to_string()))
        .map_err(|never| match never {})
        .boxed()
}

async fn health(sh: &Arc<Shared>) -> Response<BoxBody> {
    let upstream = sh
        .client
        .request(
            Request::builder()
                .uri(format!("http://127.0.0.1:{}/health", sh.cfg.api_port))
                .body(
                    Full::new(Bytes::new())
                        .map_err(|never| match never {})
                        .boxed(),
                )
                .unwrap(),
        )
        .await
        .is_ok_and(|r| r.status().is_success());
    Response::builder()
        .header("content-type", "application/json")
        .body(boxed(&json!({"ok": true, "upstream": upstream})))
        .unwrap()
}

fn models(sh: &Arc<Shared>) -> Response<BoxBody> {
    let m = |id: &str| json!({"id": id, "object": "model", "created": 0, "owned_by": "shrug"});
    Response::builder()
        .header("content-type", "application/json")
        .body(boxed(&json!({
            "object": "list",
            "data": [m(&sh.cfg.model_id), m("default")],
        })))
        .unwrap()
}

fn stats(sh: &Arc<Shared>) -> Response<BoxBody> {
    let s = sh.stats.lock().unwrap();
    Response::builder()
        .header("content-type", "application/json")
        .body(boxed(&json!({
            "counters": {"total": s.total},
            "events": s.events.iter().collect::<Vec<_>>(),
        })))
        .unwrap()
}

/// Read, maybe rewrite, forward, and stream the response through a tap.
async fn proxy_passthrough(sh: Arc<Shared>, req: Request<Incoming>) -> Response<BoxBody> {
    let is_json = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.contains("json"));
    let small = req
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n <= MAX_JSON_BODY);
    let path = req.uri().path().to_string();
    let (mut parts, body) = req.into_parts();
    let (out_body, requested_model, new_len) = if is_json && small {
        match body.collect().await {
            Ok(collected) => {
                let bytes = collected.to_bytes();
                match serde_json::from_slice::<Value>(&bytes) {
                    Ok(Value::Object(mut obj)) => {
                        let requested = obj
                            .get("model")
                            .and_then(|m| m.as_str())
                            .map(str::to_string);
                        obj.insert("model".into(), Value::String(sh.cfg.model_id.clone()));
                        let body = serde_json::to_vec(&Value::Object(obj)).unwrap_or_default();
                        let len = body.len() as u64;
                        (
                            Full::new(Bytes::from(body))
                                .map_err(|never| match never {})
                                .boxed(),
                            requested,
                            Some(len),
                        )
                    }
                    _ => (
                        Full::new(bytes).map_err(|never| match never {}).boxed(),
                        None,
                        None,
                    ),
                }
            }
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "could not read body"),
        }
    } else {
        (body.boxed(), None, None)
    };
    let started = Instant::now();
    if let Some(len) = new_len {
        parts
            .headers
            .insert(hyper::header::CONTENT_LENGTH, HeaderValue::from(len));
    }
    strip_hop(&mut parts.headers);
    parts.headers.remove("authorization");
    parts.headers.remove("x-api-key");
    parts.headers.insert(
        HOST,
        HeaderValue::from_str(&format!("127.0.0.1:{}", sh.cfg.api_port)).unwrap(),
    );
    parts.uri = format!("http://127.0.0.1:{}{}", sh.cfg.api_port, {
        parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
    })
    .parse()
    .unwrap();
    let upstream_req = Request::from_parts(parts, out_body);
    let resp = match sh.client.request(upstream_req).await {
        Ok(r) => r,
        Err(e) => {
            return json_error(StatusCode::BAD_GATEWAY, &format!("upstream error: {e}"));
        }
    };
    let status = resp.status();
    let is_sse = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.contains("event-stream"));
    let (mut rparts, rbody) = resp.into_parts();
    strip_hop(&mut rparts.headers);
    let ev = ReqEvent {
        path,
        model: requested_model,
        status: status.as_u16(),
        started_at: now_unix(),
        first_byte_ms: None,
        total_ms: 0,
        input_tokens: None,
        output_tokens: None,
    };
    let tap = TapBody {
        inner: rbody.boxed(),
        done: false,
        tap: UsageTap {
            sse: is_sse,
            ..Default::default()
        },
        ev,
        started,
        stats: sh.stats.clone(),
    };
    Response::from_parts(rparts, tap.boxed())
}

/// Load config and serve until killed.
pub fn run() -> Result<()> {
    let s = crate::state::State::load()?.context("no state.json; run `shrug up --go` first")?;
    let cfg = crate::state::Config::load();
    let listen: SocketAddr = cfg
        .listen
        .as_deref()
        .unwrap_or("127.0.0.1:8145")
        .parse()
        .context("invalid listen address in config.json")?;
    let shared = Arc::new(Shared {
        cfg: Cfg {
            listen,
            api_port: s.api_port,
            model_id: s.model_id.clone(),
            api_key: cfg.api_key.clone(),
            force_auth: false,
        },
        client: Client::builder(TokioExecutor::new()).build(HttpConnector::new()),
        stats: Arc::new(Mutex::new(Stats::default())),
    });
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(serve(shared))
}

/// Serve an already-built Shared on its listen address.
pub async fn serve(sh: Arc<Shared>) -> Result<()> {
    let listener = TcpListener::bind(sh.cfg.listen).await?;
    eprintln!("shrug: gateway on {}", sh.cfg.listen);
    loop {
        let (stream, peer) = listener.accept().await?;
        let sh = sh.clone();
        tokio::spawn(async move {
            let loopback = peer.ip().is_loopback();
            let io = TokioIo::new(stream);
            let svc = service_fn(move |req| handle(sh.clone(), loopback, req));
            let _ = http1::Builder::new().serve_connection(io, svc).await;
        });
    }
}
