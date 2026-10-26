// SPDX-License-Identifier: AGPL-3.0-only
//! `shrug connect`: client config snippets for claude-code, codex, opencode
//! and env. Without --go print; with --go write.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::PathBuf;

pub struct Target {
    pub base: String, // http://127.0.0.1:8145 or http://lan:port
    pub api_key: Option<String>,
    pub model_id: String,
    pub context_tokens: u64,
}

/// Resolve the target from state+config; --remote errors when loopback-only.
pub fn target(remote: bool) -> Result<Target> {
    let s = crate::state::State::load()?.context("no state.json; run `shrug up --go` first")?;
    let cfg = crate::state::Config::load();
    let listen = crate::gateway::listen_addr(&cfg)?;
    let cat = crate::catalog::embedded()?;
    let ctx = cat
        .release(&s.release_id)
        .map(|r| r.context_tokens)
        .unwrap_or(0);
    if !remote {
        return Ok(Target {
            base: format!("http://127.0.0.1:{}", listen.port()),
            api_key: cfg.api_key,
            model_id: s.model_id,
            context_tokens: ctx,
        });
    }
    if listen.ip().is_loopback() {
        bail!(
            "the gateway listens on {listen} (loopback only); run `shrug up --go --listen 0.0.0.0:{}` first",
            listen.port()
        );
    }
    let lan = s
        .ranks
        .first()
        .and_then(|r| r.lan_ip.clone())
        .context("no LAN IP known for rank 0")?;
    Ok(Target {
        base: format!("http://{lan}:{}", listen.port()),
        api_key: cfg.api_key,
        model_id: s.model_id,
        context_tokens: ctx,
    })
}

pub fn render_claude(t: &Target) -> String {
    format!(
        "export ANTHROPIC_BASE_URL={}\nexport ANTHROPIC_AUTH_TOKEN={}\nexport ANTHROPIC_MODEL={}\nexport ANTHROPIC_SMALL_FAST_MODEL={}\nexport API_TIMEOUT_MS=3600000",
        t.base,
        t.api_key.as_deref().unwrap_or("shrug"),
        t.model_id,
        t.model_id
    )
}

pub fn render_codex(t: &Target) -> String {
    let env = t
        .api_key
        .as_ref()
        .map(|_| "env_key = \"SHRUG_API_KEY\"\n")
        .unwrap_or_default();
    format!(
        "[model_providers.shrug]\nname = \"Shrug\"\nbase_url = \"{}/v1\"\nwire_api = \"responses\"\n{}[profiles.shrug]\nmodel_provider = \"shrug\"\nmodel = \"{}\"",
        t.base, env, t.model_id
    )
}

/// The opencode provider block merged into opencode.json.
pub fn opencode_provider(t: &Target) -> Value {
    let mut opts = json!({"baseURL": format!("{}/v1", t.base), "timeout": false});
    if let Some(k) = &t.api_key {
        opts["apiKey"] = Value::String(k.clone());
    }
    json!({
        "npm": "@ai-sdk/openai-compatible",
        "options": opts,
        "models": {
            t.model_id.clone(): {
                "name": t.model_id,
                "tool_call": true,
                "limit": {"context": t.context_tokens, "output": 32768}
            }
        }
    })
}

/// Merge the shrug provider into an opencode.json document: replaces only
/// `provider.shrug`, preserves every other key, sets top-level `model` only
/// when the file has none.
pub fn opencode_merge(doc: &mut Value, t: &Target) {
    if !doc.is_object() {
        *doc = json!({});
    }
    doc["provider"]["shrug"] = opencode_provider(t);
    if doc.get("model").is_none() || doc["model"].is_null() {
        doc["model"] = Value::String(format!("shrug/{}", t.model_id));
    }
}

pub fn render_opencode(t: &Target) -> String {
    let mut doc = json!({});
    opencode_merge(&mut doc, t);
    serde_json::to_string_pretty(&doc).unwrap()
}

pub fn render_env(t: &Target) -> String {
    format!(
        "export OPENAI_BASE_URL={}/v1\nexport OPENAI_API_KEY={}\nexport OPENAI_MODEL={}\nexport ANTHROPIC_BASE_URL={}\nexport ANTHROPIC_AUTH_TOKEN={}\nexport ANTHROPIC_MODEL={}",
        t.base,
        t.api_key.as_deref().unwrap_or("shrug"),
        t.model_id,
        t.base,
        t.api_key.as_deref().unwrap_or("shrug"),
        t.model_id
    )
}

fn home_bin() -> PathBuf {
    std::env::home_dir().unwrap_or_default().join(".local/bin")
}

pub fn write(client: &str, t: &Target) -> Result<String> {
    match client {
        "claude-code" => {
            let path = home_bin().join("shrug-claude");
            std::fs::create_dir_all(path.parent().unwrap())?;
            let body = format!(
                "#!/usr/bin/env bash\n{}\nexec claude \"$@\"\n",
                render_claude(t)
            );
            std::fs::write(&path, body)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            }
            Ok(format!("wrote {}", path.display()))
        }
        "codex" => {
            let path = std::env::home_dir()
                .unwrap_or_default()
                .join(".codex/config.toml");
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if existing.contains("[model_providers.shrug]") {
                return Ok(format!(
                    "already configured in {}; use `codex --profile shrug`\n{}",
                    path.display(),
                    render_codex(t)
                ));
            }
            std::fs::create_dir_all(path.parent().unwrap())?;
            let mut f = std::fs::OpenOptions::new().append(true).open(&path)?;
            use std::io::Write;
            writeln!(f, "\n{}", render_codex(t))?;
            Ok(format!(
                "appended to {}; use `codex --profile shrug`",
                path.display()
            ))
        }
        "opencode" => {
            let path = std::env::home_dir()
                .unwrap_or_default()
                .join(".config/opencode/opencode.json");
            let mut doc: Value = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_else(|| json!({}));
            opencode_merge(&mut doc, t);
            std::fs::create_dir_all(path.parent().unwrap())?;
            if path.exists() {
                std::fs::copy(&path, path.with_extension("json.bak"))?;
            }
            std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
            Ok(format!("wrote {}", path.display()))
        }
        "env" => Ok("nothing to write; eval the output".into()),
        other => bail!("unknown client {other}"),
    }
}

pub fn run(client: &str, remote: bool, go: bool) -> Result<i32> {
    let t = target(remote)?;
    let snippet = match client {
        "claude-code" => render_claude(&t),
        "codex" => render_codex(&t),
        "opencode" => render_opencode(&t),
        "env" => render_env(&t),
        other => bail!("unknown client {other}; pick claude-code|codex|opencode|env"),
    };
    println!("{snippet}");
    if go {
        println!("{}", write(client, &t)?);
    } else {
        match client {
            "claude-code" => println!("# --go writes ~/.local/bin/shrug-claude"),
            "codex" => println!("# --go appends to ~/.codex/config.toml"),
            "opencode" => println!("# --go merges into ~/.config/opencode/opencode.json"),
            _ => {}
        }
    }
    Ok(0)
}
