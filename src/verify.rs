// SPDX-License-Identifier: AGPL-3.0-only
//! Post-start verification against the engine on 127.0.0.1, and the pure
//! parsers behind each check.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

/// Check 1: non-stream chat math. content trimmed == "391", finish "stop".
pub fn check_math_reply(body: &Value) -> Result<()> {
    let choice = &body["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("");
    let finish = choice["finish_reason"].as_str().unwrap_or("");
    if content.trim() == "391" && finish == "stop" {
        Ok(())
    } else {
        bail!("math check failed: content={content:?} finish_reason={finish:?}")
    }
}

/// Checks 2/3: SSE body produced at least one non-empty content delta
/// and terminated with [DONE].
pub fn check_stream_content(sse: &str) -> Result<()> {
    let mut deltas = 0;
    let mut done = false;
    for data in sse_data(sse) {
        if data == "[DONE]" {
            done = true;
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&data) {
            for ch in v["choices"].as_array().into_iter().flatten() {
                if let Some(c) = ch["delta"]["content"].as_str()
                    && !c.is_empty()
                {
                    deltas += 1;
                }
            }
        }
    }
    if done && deltas > 0 {
        Ok(())
    } else {
        bail!("stream check failed: {deltas} content deltas, done={done}")
    }
}

/// Check 3: a streamed tool call assembles to get_weather({...Paris...}).
pub fn check_stream_tool_call(sse: &str) -> Result<()> {
    let mut name = String::new();
    let mut args = String::new();
    let mut done = false;
    for data in sse_data(sse) {
        if data == "[DONE]" {
            done = true;
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&data) {
            for ch in v["choices"].as_array().into_iter().flatten() {
                for tc in ch["delta"]["tool_calls"].as_array().into_iter().flatten() {
                    if let Some(n) = tc["function"]["name"].as_str() {
                        name.push_str(n);
                    }
                    if let Some(a) = tc["function"]["arguments"].as_str() {
                        args.push_str(a);
                    }
                }
            }
        }
    }
    if !done {
        bail!("tool-call stream missing [DONE]");
    }
    if name != "get_weather" {
        bail!("tool call name {name:?}, expected get_weather");
    }
    let parsed: Value = serde_json::from_str(&args).context("tool args are not JSON")?;
    if !parsed.to_string().contains("Paris") {
        bail!("tool args {args:?} do not mention Paris");
    }
    Ok(())
}

/// Check 4: an Anthropic /v1/messages reply contains text.
pub fn check_anthropic_reply(body: &Value) -> Result<()> {
    let has_text = body["content"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|b| b["type"] == "text" && b["text"].as_str().is_some_and(|t| !t.is_empty()));
    if has_text {
        Ok(())
    } else {
        bail!("anthropic check failed: no text block in reply")
    }
}

fn sse_data(sse: &str) -> Vec<String> {
    sse.lines()
        .filter_map(|l| l.strip_prefix("data:").map(|d| d.trim().to_string()))
        .collect()
}

fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?)
}

/// Run all four checks against the engine at http://127.0.0.1:{port}.
pub fn verify_engine(port: u16, model_id: &str) -> Result<()> {
    let client = http()?;
    let base = format!("http://127.0.0.1:{port}");

    let body = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({
            "model": model_id,
            "messages": [{"role": "user", "content": "What is 17*23? Answer with the number."}],
            "temperature": 0,
            "max_tokens": 1024,
            "reasoning_effort": "low",
            "stream": false,
        }))
        .send()
        .and_then(|r| r.error_for_status())
        .context("math chat request")?
        .json::<Value>()
        .context("math chat reply")?;
    check_math_reply(&body)?;

    let sse = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({
            "model": model_id,
            "messages": [{"role": "user", "content": "Say hello in one sentence."}],
            "temperature": 0,
            "max_tokens": 128,
            "stream": true,
        }))
        .send()
        .and_then(|r| r.error_for_status())
        .context("stream chat request")?
        .text()
        .context("stream chat body")?;
    check_stream_content(&sse)?;

    let sse = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({
            "model": model_id,
            "messages": [{"role": "user", "content": "What's the weather in Paris? Use the tool."}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get the weather for a city.",
                    "parameters": {
                        "type": "object",
                        "properties": {"city": {"type": "string"}},
                        "required": ["city"]
                    }
                }
            }],
            "temperature": 0,
            "max_tokens": 256,
            "stream": true,
        }))
        .send()
        .and_then(|r| r.error_for_status())
        .context("tool-call request")?
        .text()
        .context("tool-call body")?;
    check_stream_tool_call(&sse)?;

    let body = client
        .post(format!("{base}/v1/messages"))
        .json(&json!({
            "model": model_id,
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "Say hi."}],
        }))
        .send()
        .and_then(|r| r.error_for_status())
        .context("anthropic request")?
        .json::<Value>()
        .context("anthropic reply")?;
    check_anthropic_reply(&body)
}
