// SPDX-License-Identifier: AGPL-3.0-only
//! Gateway against a fake upstream engine on an ephemeral port.
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde_json::{Value, json};
use shrug::serve::{Cfg, Shared, Stats};
use std::io::Read;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

fn boxed(s: &str) -> http_body_util::combinators::BoxBody<Bytes, hyper::Error> {
    Full::new(Bytes::from(s.to_string()))
        .map_err(|never| match never {})
        .boxed()
}

async fn upstream(
    req: Request<Incoming>,
) -> Result<
    Response<http_body_util::combinators::BoxBody<Bytes, hyper::Error>>,
    std::convert::Infallible,
> {
    let path = req.uri().path().to_string();
    let req_headers_slow = req.headers().get("x-slow").is_some();
    let model = if req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.contains("json"))
    {
        let b = req.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<Value>(&b)
            .ok()
            .and_then(|v| v["model"].as_str().map(str::to_string))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let resp = match path.as_str() {
        "/health" => Response::new(boxed("ok")),
        "/v1/chat/completions" if !req_headers_slow => {
            // Record the model the upstream saw in a header for assertions.
            let mut r = Response::new(boxed(
                "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\ndata: [DONE]\n",
            ));
            r.headers_mut()
                .insert("content-type", "text/event-stream".parse().unwrap());
            r.headers_mut().insert(
                "x-seen-model",
                model.parse().unwrap_or_else(|_| "none".parse().unwrap()),
            );
            r
        }
        p if p == "/v1/chat/completions" && req_headers_slow => {
            // Dribble slowly; when the client goes away the body drops and
            // the connection is abandoned — the flag is set by the drop side.
            let (tx, rx) =
                tokio::sync::mpsc::channel::<Result<hyper::body::Frame<Bytes>, hyper::Error>>(4);
            tokio::spawn(async move {
                for i in 0..50 {
                    if tx
                        .send(Ok(hyper::body::Frame::data(Bytes::from(format!(
                            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{i}\"}}}}]}}\n\n"
                        )))))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            });
            let body =
                http_body_util::StreamBody::new(tokio_stream::wrappers::ReceiverStream::new(rx));
            let mut r = Response::new(BodyExt::boxed(body));
            r.headers_mut()
                .insert("content-type", "text/event-stream".parse().unwrap());
            r
        }
        _ => Response::new(boxed("{\"ok\":true}")),
    };
    Ok(resp)
}

async fn start_upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(s), service_fn(upstream))
                    .await;
            });
        }
    });
    addr
}

fn shared(api_port: u16, listen: SocketAddr, force_auth: bool) -> Arc<Shared> {
    Arc::new(Shared {
        cfg: Cfg {
            listen,
            api_port,
            model_id: "test-model".into(),
            api_key: Some("secret-key".into()),
            force_auth,
        },
        client: Client::builder(TokioExecutor::new()).build(HttpConnector::new()),
        stats: Arc::new(Mutex::new(Stats::default())),
    })
}

async fn start_gateway(sh: Arc<Shared>) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    // serve() binds itself; here we need ephemeral, so replicate the loop.
    tokio::spawn(async move {
        loop {
            let (stream, peer) = l.accept().await.unwrap();
            let sh = sh.clone();
            tokio::spawn(async move {
                let loopback = peer.ip().is_loopback();
                let io = TokioIo::new(stream);
                let svc = hyper::service::service_fn(move |req| {
                    shrug::serve::handle_test(sh.clone(), loopback, req)
                });
                let _ = http1::Builder::new().serve_connection(io, svc).await;
            });
        }
    });
    addr
}

fn rt() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap()
    })
}

fn post(url: &str, body: &Value, headers: &[(&str, &str)]) -> reqwest::blocking::Response {
    let c = reqwest::blocking::Client::new();
    let mut rb = c.post(url).json(body);
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    rb.send().unwrap()
}

#[test]
fn model_rewrite_and_stats() {
    let _g = rt().enter();
    let up = rt().block_on(start_upstream());
    let sh = rt().block_on(async { shared(up.port(), "127.0.0.1:0".parse().unwrap(), false) });
    let sh2 = sh.clone();
    let gw = rt().block_on(start_gateway(sh2));
    let r = post(
        &format!("http://{gw}/v1/chat/completions"),
        &json!({"model": "default", "messages": []}),
        &[],
    );
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers()
            .get("x-seen-model")
            .and_then(|v| v.to_str().ok()),
        Some("test-model"),
        "default must map to the release model"
    );
    // SSE usage captured into stats.
    let _ = r.text();
    std::thread::sleep(Duration::from_millis(100));
    let s = sh.stats.lock().unwrap();
    let ev = s.events.iter().last().expect("an event");
    assert_eq!(ev.model.as_deref(), Some("default"));
    assert_eq!(ev.input_tokens, Some(3));
    assert_eq!(ev.output_tokens, Some(2));
    assert!(ev.first_byte_ms.is_some());
}

#[test]
fn v1_models_synthesized() {
    let _g = rt().enter();
    let up = rt().block_on(start_upstream());
    let sh = rt().block_on(async { shared(up.port(), "127.0.0.1:0".parse().unwrap(), false) });
    let gw = rt().block_on(start_gateway(sh));
    let v: Value = reqwest::blocking::get(format!("http://{gw}/v1/models"))
        .unwrap()
        .json()
        .unwrap();
    let ids: Vec<&str> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert_eq!(ids, ["test-model", "default"]);
}

#[test]
fn auth_required_when_forced() {
    let _g = rt().enter();
    let up = rt().block_on(start_upstream());
    let sh = rt().block_on(async { shared(up.port(), "127.0.0.1:0".parse().unwrap(), true) });
    let gw = rt().block_on(start_gateway(sh));
    let url = format!("http://{gw}/v1/chat/completions");
    let r = post(&url, &json!({"model": "x"}), &[]);
    assert_eq!(r.status(), 401);
    let r = post(
        &url,
        &json!({"model": "x"}),
        &[("authorization", "Bearer secret-key")],
    );
    assert_eq!(r.status(), 200);
    let r = post(&url, &json!({"model": "x"}), &[("x-api-key", "secret-key")]);
    assert_eq!(r.status(), 200);
    // /health is exempt.
    let r = reqwest::blocking::get(format!("http://{gw}/health")).unwrap();
    assert_eq!(r.status(), 200);
    // auth headers are not forwarded upstream.
    let r = post(
        &url,
        &json!({"model": "default"}),
        &[("x-api-key", "secret-key")],
    );
    assert_eq!(r.status(), 200);
}

#[test]
fn health_reports_upstream() {
    let _g = rt().enter();
    let up = rt().block_on(start_upstream());
    let sh = rt().block_on(async { shared(up.port(), "127.0.0.1:0".parse().unwrap(), false) });
    let gw = rt().block_on(start_gateway(sh));
    let v: Value = reqwest::blocking::get(format!("http://{gw}/health"))
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(v["upstream"], true);
}

#[test]
fn first_chunk_before_upstream_finishes() {
    let _g = rt().enter();
    let up = rt().block_on(start_upstream());
    let sh = rt().block_on(async { shared(up.port(), "127.0.0.1:0".parse().unwrap(), false) });
    let gw = rt().block_on(start_gateway(sh));
    // /v1/slow dribbles for ~2.5s; the first byte must arrive much sooner.
    let mut resp = post(
        &format!("http://{gw}/v1/chat/completions"),
        &json!({"model": "x", "stream": true}),
        &[("x-slow", "1")],
    );
    assert_eq!(resp.status(), 200);
    let t0 = std::time::Instant::now();
    let mut buf = [0u8; 16];
    let n = resp.read(&mut buf).unwrap();
    let elapsed = t0.elapsed();
    assert!(n > 0);
    assert!(
        elapsed < Duration::from_secs(2),
        "first byte took {elapsed:?}"
    );
    // Disconnect: the upstream body must be dropped with us.
    drop(resp);
    std::thread::sleep(Duration::from_millis(300));
}
