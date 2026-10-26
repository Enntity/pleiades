// SPDX-License-Identifier: AGPL-3.0-only
mod common;
use common::*;
use serde_json::json;
use shrug::catalog;
use shrug::executor::Up;
use shrug::host::FakeHost;
use shrug::plan::{self, Options};
use shrug::state::Config;
use shrug::verify;

#[test]
fn verify_math_reply() {
    let ok = json!({"choices":[{"message":{"content":" 391 "},"finish_reason":"stop"}]});
    assert!(verify::check_math_reply(&ok).is_ok());
    // Digit runs must be exactly ["391"]: "391." is fine, "1391" is not.
    let ok2 =
        json!({"choices":[{"message":{"content":"The answer is 391."},"finish_reason":"stop"}]});
    assert!(verify::check_math_reply(&ok2).is_ok());
    let bad = json!({"choices":[{"message":{"content":"1391"},"finish_reason":"stop"}]});
    assert!(verify::check_math_reply(&bad).is_err());
    let bad = json!({"choices":[{"message":{"content":"391"},"finish_reason":"length"}]});
    assert!(verify::check_math_reply(&bad).is_err());
}

#[test]
fn verify_stream_content() {
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\
               data: {\"choices\":[{\"delta\":{\"content\":\" there\"}}]}\n\
               data: [DONE]\n";
    assert!(verify::check_stream_content(sse).is_ok());
    assert!(verify::check_stream_content("data: [DONE]\n").is_err());
    assert!(
        verify::check_stream_content("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n")
            .is_err()
    );
}

#[test]
fn verify_tool_call() {
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"get_\",\"arguments\":\"\"}}]}}]}\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"weather\",\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"arguments\":\"\\\"Paris\\\"}\"}}]}}]}\n",
        "data: [DONE]\n"
    );
    assert!(verify::check_stream_tool_call(sse).is_ok());
    let wrong = sse
        .replace("get_weather", "get_weathe")
        .replace("\"name\":\"get_\"", "\"name\":\"get_x\"");
    // name mismatch
    let bad = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"other\"}}]}}]}\n",
        "data: [DONE]\n"
    );
    assert!(verify::check_stream_tool_call(bad).is_err());
    assert!(verify::check_stream_tool_call(&wrong).is_err());
}

#[test]
fn verify_anthropic() {
    let ok = json!({"content":[{"type":"text","text":"hi"}]});
    assert!(verify::check_anthropic_reply(&ok).is_ok());
    let bad = json!({"content":[]});
    assert!(verify::check_anthropic_reply(&bad).is_err());
}

#[test]
fn launch_order_workers_first_then_leader() {
    let d = ring();
    let cfg = Config::default();
    let cat = catalog::embedded().unwrap();
    let checks = FakeChecks::default();
    let p = plan::build(
        &d,
        &cat,
        "embedded",
        &Options {
            model: Some("glm-5.3-flash-4x"),
            accept_noncommercial: true,
            no_clock_cap: false,
            config: &cfg,
        },
        &checks,
    );
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    let rel = cat.release("glm-5.3-flash-4x").unwrap();
    let fakes: Vec<FakeHost> = (0..4).map(|i| FakeHost::named(&format!("h{i}"))).collect();
    let refs: Vec<&dyn shrug::host::Host> =
        fakes.iter().map(|f| f as &dyn shrug::host::Host).collect();
    let up = Up {
        plan: &p,
        discovery: &d,
        release: rel,
        hosts: &refs,
        old_state: None,
        known_hosts: "kh".into(),
        ssh_user: "spark".into(),
    };
    up.launch().unwrap();
    // Each fake sees exactly one script run; sequence is rank 3, 2, 1, 0.
    let names: Vec<String> = (0..4).map(|i| fakes[i].calls().join(" ")).collect();
    for (i, calls) in names.iter().enumerate() {
        assert!(calls.contains("--name shrug-rank"), "{i}: {calls}");
    }
    // Order assertion via a shared recorder: give each host a script that logs.
    // FakeHost records per host; instead assert each host got its own rank args.
    for (i, calls) in names.iter().enumerate() {
        assert!(calls.contains(&format!("--rank {i}")), "{i}: {calls}");
    }
}

#[test]
fn launch_order_sequence() {
    use std::sync::{Arc, Mutex};
    let d = ring();
    let cfg = Config::default();
    let cat = catalog::embedded().unwrap();
    let p = plan::build(
        &d,
        &cat,
        "embedded",
        &Options {
            model: Some("glm-5.3-flash-4x"),
            accept_noncommercial: true,
            no_clock_cap: false,
            config: &cfg,
        },
        &FakeChecks::default(),
    );
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    let rel = cat.release("glm-5.3-flash-4x").unwrap();
    let shared = Arc::new(Mutex::new(Vec::new()));
    let fakes: Vec<FakeHost> = (0..4)
        .map(|i| FakeHost {
            shared: Some(shared.clone()),
            ..FakeHost::named(&format!("h{i}"))
        })
        .collect();
    let refs: Vec<&dyn shrug::host::Host> =
        fakes.iter().map(|f| f as &dyn shrug::host::Host).collect();
    let up = Up {
        plan: &p,
        discovery: &d,
        release: rel,
        hosts: &refs,
        old_state: None,
        known_hosts: "kh".into(),
        ssh_user: "spark".into(),
    };
    up.launch().unwrap();
    let calls = shared.lock().unwrap();
    let ranks: Vec<String> = calls
        .iter()
        .filter_map(|(_, c)| {
            c.split_whitespace()
                .skip_while(|t| *t != "--rank")
                .nth(1)
                .map(str::to_string)
        })
        .collect();
    assert_eq!(ranks, ["3", "2", "1", "0"], "{calls:?}");
}
