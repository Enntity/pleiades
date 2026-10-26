// SPDX-License-Identifier: AGPL-3.0-only
//! `shrug bench`: three quick workloads through the gateway, every rep
//! printed, median reported, a receipt written under SHRUG_HOME/bench/.
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WORKLOADS: [(&str, &str); 3] = [
    ("prose", "Write a 300-word story about two small computers."),
    (
        "code",
        "Write a Python function that parses an ISO 8601 duration string, with unit tests.",
    ),
    (
        "structured",
        "List 40 countries with their capital and population as a JSON array.",
    ),
];

/// Parse one streamed reply: first non-empty content/reasoning delta time,
/// usage from the last event that carries it, else count deltas.
pub fn parse_sse(sse: &str) -> Result<(Option<u64>, u64)> {
    let mut usage_out: Option<u64> = None;
    let mut deltas = 0u64;
    for line in sse.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(n) = v["usage"]["completion_tokens"].as_u64() {
            usage_out = Some(n);
        }
        for ch in v["choices"].as_array().into_iter().flatten() {
            let d = &ch["delta"];
            let has = ["content", "reasoning_content", "reasoning"]
                .iter()
                .any(|k| d[k].as_str().is_some_and(|s| !s.is_empty()));
            if has {
                deltas += 1;
            }
        }
    }
    Ok((usage_out, deltas))
}

#[derive(Serialize)]
struct Receipt {
    schema: String,
    release_id: String,
    model_id: String,
    image: String,
    atlas_commit: String,
    hosts: Vec<String>,
    settings: Value,
    reps: Vec<Value>,
}

pub fn run(reps: usize) -> Result<i32> {
    let s = crate::state::State::load()?.context("no state.json; run `shrug up --go` first")?;
    let cfg = crate::state::Config::load();
    let port = crate::gateway::listen_addr(&cfg)
        .map(|a| a.port())
        .unwrap_or(crate::gateway::DEFAULT_PORT);
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(900))
        .build()?;
    let cat = crate::catalog::embedded()?;
    let rel = cat.release(&s.release_id);
    let hosts: Vec<String> = s.ranks.iter().map(|r| r.host_label.clone()).collect();
    println!(
        "shrug bench: {} reps x {} workloads on {}",
        reps,
        WORKLOADS.len(),
        s.release_id
    );
    let mut by_name: std::collections::BTreeMap<&str, Vec<f64>> = Default::default();
    let mut receipt_reps = Vec::new();
    for rep in 1..=reps {
        for (name, prompt) in WORKLOADS {
            let t0 = Instant::now();
            let mut rb = client.post(format!("{base}/v1/chat/completions"));
            if let Some(k) = cfg.api_key.as_deref() {
                rb = rb.bearer_auth(k);
            }
            let resp = rb
                .json(&json!({
                    "model": s.model_id,
                    "messages": [{"role": "user", "content": prompt}],
                    "temperature": 0,
                    "max_tokens": 1024,
                    "reasoning_effort": "low",
                    "stream": true,
                    "stream_options": {"include_usage": true},
                }))
                .send()
                .and_then(|r| r.error_for_status())
                .with_context(|| format!("{name} rep {rep}"))?;
            let (first_ms, tokens, tok_s, used_usage) = measure(resp, t0)?;
            println!(
                "{name} rep {rep}: first token {first_ms} ms, {tokens} output tokens ({src}), {tok_s:.1} tok/s",
                src = if used_usage { "usage" } else { "deltas" }
            );
            by_name.entry(name).or_default().push(tok_s);
            receipt_reps.push(json!({
                "workload": name,
                "rep": rep,
                "first_token_ms": first_ms,
                "output_tokens": tokens,
                "tokens_from": if used_usage { "usage" } else { "deltas" },
                "tok_s": (tok_s * 10.0).round() / 10.0,
            }));
        }
    }
    let mut medians = json!({});
    for (name, _) in WORKLOADS {
        let v = by_name.get_mut(name).unwrap();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = v[v.len() / 2];
        medians[name] = json!((med * 10.0).round() / 10.0);
        println!("{name}: median {med:.1} tok/s");
    }
    if let Some(rel) = rel {
        if let Some(b) = &rel.bench_reference {
            println!(
                "reference ({}): prose {} / code {} / structured {} tok/s",
                b.harness, b.prose, b.code, b.structured
            );
            println!("yours:      {medians}");
        } else {
            println!("no Shrug reference measured for this release yet");
        }
    }
    let dir = crate::state::home().join("bench");
    std::fs::create_dir_all(&dir)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let receipt = Receipt {
        schema: "shrug-bench/v1".into(),
        release_id: s.release_id.clone(),
        model_id: s.model_id.clone(),
        image: rel.map(|r| r.engine.image.clone()).unwrap_or_default(),
        atlas_commit: rel
            .map(|r| r.engine.atlas_commit.clone())
            .unwrap_or_default(),
        hosts,
        settings: json!({"reps": reps, "temperature": 0, "reasoning_effort": "low",
            "max_tokens": 1024, "stream": true, "include_usage": true}),
        reps: receipt_reps,
    };
    let path = dir.join(format!("{now}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&receipt)?)?;
    println!("receipt: {}", path.display());
    Ok(0)
}

/// Measure one streamed response; returns (first_token_ms, tokens, tok/s, from_usage).
fn measure(resp: reqwest::blocking::Response, t0: Instant) -> Result<(u64, u64, f64, bool)> {
    let mut first_ms: Option<u64> = None;
    let mut usage_out: Option<u64> = None;
    let mut deltas = 0u64;
    use std::io::BufRead;
    for line in std::io::BufReader::new(resp).lines() {
        let line = line?;
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(n) = v["usage"]["completion_tokens"].as_u64() {
            usage_out = Some(n);
        }
        for ch in v["choices"].as_array().into_iter().flatten() {
            let d = &ch["delta"];
            let has = ["content", "reasoning_content", "reasoning"]
                .iter()
                .any(|k| d[k].as_str().is_some_and(|s| !s.is_empty()));
            if has {
                deltas += 1;
                if first_ms.is_none() {
                    first_ms = Some(t0.elapsed().as_millis() as u64);
                }
            }
        }
    }
    let first = first_ms.context("no content deltas in stream")?;
    let decode_s = t0.elapsed().as_secs_f64() - first as f64 / 1000.0;
    let tokens = usage_out.unwrap_or(deltas);
    let tok_s = if decode_s > 0.0 {
        tokens as f64 / decode_s
    } else {
        0.0
    };
    Ok((first, tokens, tok_s, usage_out.is_some()))
}
