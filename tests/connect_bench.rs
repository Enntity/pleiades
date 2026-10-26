// SPDX-License-Identifier: AGPL-3.0-only
use serde_json::json;
use shrug::bench::parse_sse;
use shrug::connect::*;

fn t(key: Option<&str>) -> Target {
    Target {
        base: "http://127.0.0.1:8145".into(),
        api_key: key.map(str::to_string),
        model_id: "glm-5.3-flash-atlas".into(),
        context_tokens: 1048576,
    }
}

#[test]
fn claude_golden() {
    assert_eq!(
        render_claude(&t(Some("k1"))),
        "export ANTHROPIC_BASE_URL=http://127.0.0.1:8145\n\
         export ANTHROPIC_AUTH_TOKEN=k1\n\
         export ANTHROPIC_MODEL=glm-5.3-flash-atlas\n\
         export ANTHROPIC_SMALL_FAST_MODEL=glm-5.3-flash-atlas\n\
         export API_TIMEOUT_MS=3600000"
    );
    assert!(render_claude(&t(None)).contains("ANTHROPIC_AUTH_TOKEN=shrug"));
}

#[test]
fn codex_golden() {
    let s = render_codex(&t(Some("k")));
    assert!(s.contains("[model_providers.shrug]"));
    assert!(s.contains("base_url = \"http://127.0.0.1:8145/v1\""));
    assert!(s.contains("wire_api = \"responses\""));
    assert!(s.contains("env_key = \"SHRUG_API_KEY\""));
    assert!(s.contains("[profiles.shrug]"));
    assert!(s.contains("model = \"glm-5.3-flash-atlas\""));
    assert!(!render_codex(&t(None)).contains("env_key"));
}

#[test]
fn opencode_merge_preserves() {
    let mut doc = json!({
        "model": "other/x",
        "provider": {"other": {"npm": "y"}, "shrug": {"old": true}},
        "theme": "dark"
    });
    opencode_merge(&mut doc, &t(Some("k")));
    assert_eq!(doc["model"], "other/x", "existing model kept");
    assert_eq!(doc["theme"], "dark");
    assert_eq!(doc["provider"]["other"]["npm"], "y");
    assert_eq!(doc["provider"]["shrug"]["npm"], "@ai-sdk/openai-compatible");
    assert_eq!(
        doc["provider"]["shrug"]["options"]["baseURL"],
        "http://127.0.0.1:8145/v1"
    );
    assert_eq!(doc["provider"]["shrug"]["options"]["apiKey"], "k");
    assert_eq!(
        doc["provider"]["shrug"]["models"]["glm-5.3-flash-atlas"]["limit"]["context"],
        1048576
    );
    // No model key at all → set ours.
    let mut doc = json!({"provider": {}});
    opencode_merge(&mut doc, &t(None));
    assert_eq!(doc["model"], "shrug/glm-5.3-flash-atlas");
    assert!(doc["provider"]["shrug"]["options"].get("apiKey").is_none());
}

#[test]
fn env_golden() {
    let s = render_env(&t(None));
    assert!(s.contains("OPENAI_BASE_URL=http://127.0.0.1:8145/v1"));
    assert!(s.contains("OPENAI_MODEL=glm-5.3-flash-atlas"));
    assert!(s.contains("ANTHROPIC_BASE_URL=http://127.0.0.1:8145"));
}

#[test]
fn bench_parse_sse() {
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n",
        "data: {\"choices\":[{\"delta\":{}}],\"usage\":{\"completion_tokens\":41,\"prompt_tokens\":9}}\n",
        "data: [DONE]\n"
    );
    let (usage, deltas) = parse_sse(sse).unwrap();
    assert_eq!(usage, Some(41));
    assert_eq!(deltas, 3);
    let (usage, deltas) = parse_sse("data: [DONE]\n").unwrap();
    assert_eq!(usage, None);
    assert_eq!(deltas, 0);
}
