// SPDX-License-Identifier: AGPL-3.0-only
//! Gateway process management: `up` starts `shrug serve` detached after the
//! engine is healthy; `down` stops it. The server itself is src/serve.rs.
use crate::state::{Config, State};
use anyhow::{Context, Result, bail};
use std::io::Write;
use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn pid_file() -> std::path::PathBuf {
    crate::state::home().join("gateway.pid")
}

fn log_file() -> std::path::PathBuf {
    crate::state::home().join("gateway.log")
}

pub const DEFAULT_LISTEN: &str = "127.0.0.1:8145";
pub const DEFAULT_PORT: u16 = 8145;

/// The listen address from config.json (default loopback).
pub fn listen_addr(cfg: &Config) -> Result<SocketAddr> {
    cfg.listen
        .as_deref()
        .unwrap_or(DEFAULT_LISTEN)
        .parse()
        .context("invalid listen address in config.json")
}

fn new_api_key() -> Result<String> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).context("read OS randomness")?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Ensure config.json carries an api key when the listen isn't loopback.
/// Rewrites the file mode 0600.
pub fn ensure_config(cfg: &mut Config) -> Result<()> {
    let listen = listen_addr(cfg)?;
    if !listen.ip().is_loopback() && cfg.api_key.is_none() {
        cfg.api_key = Some(new_api_key()?);
    }
    let dir = crate::state::home();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config.json");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    f.write_all(serde_json::to_string_pretty(cfg)?.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn stop_pid(pid: u32) {
    // Negative pid: the gateway runs in its own process group.
    let _ = Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .status();
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
}

/// Stop any live gateway, spawn `shrug serve` detached, wait for /health.
pub fn start(_state: &State) -> Result<()> {
    let mut cfg = Config::load();
    ensure_config(&mut cfg)?;
    let listen = listen_addr(&cfg)?;
    if let Some(pid) = std::fs::read_to_string(pid_file())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    {
        stop_pid(pid);
    }
    let exe = std::env::current_exe()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file())?;
    let err = log.try_clone()?;
    let mut child = Command::new(exe);
    child
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        child.process_group(0);
    }
    let child = child.spawn().context("spawn shrug serve")?;
    std::fs::write(pid_file(), format!("{}\n", child.id()))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let url = format!("http://{listen}/health");
    loop {
        if client
            .get(&url)
            .send()
            .is_ok_and(|r| r.status().is_success())
        {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!(
                "gateway did not come up within 10 s (see {})",
                log_file().display()
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub fn stop() -> Result<()> {
    if let Some(pid) = std::fs::read_to_string(pid_file())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    {
        stop_pid(pid);
        let _ = std::fs::remove_file(pid_file());
    }
    Ok(())
}
