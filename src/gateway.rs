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

/// Is this pid our own `serve` child? The pid file can outlive a reboot.
fn is_our_gateway(pid: u32) -> bool {
    let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let args: Vec<&str> = cmdline
        .split(|b| *b == 0)
        .filter_map(|b| std::str::from_utf8(b).ok())
        .filter(|s| !s.is_empty())
        .collect();
    let exe = std::env::current_exe().unwrap_or_default();
    args.len() >= 2 && args[0] == exe.to_string_lossy() && args[1] == "serve"
}

fn signal(pid: u32, sig: &str) {
    let _ = Command::new("kill").args([sig, &pid.to_string()]).status();
}

/// Terminate the gateway recorded in the pid file, but only if it is ours.
fn stop_pid(pid: u32) {
    if !is_our_gateway(pid) {
        eprintln!("gateway pid {pid} is not a shrug gateway; removing stale pid file");
        let _ = std::fs::remove_file(pid_file());
        return;
    }
    signal(pid, "-TERM");
    // The gateway runs in its own process group, so signal the group too.
    let _ = Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .status();
    // Wait for it to exit before the replacement binds the port.
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::path::Path::new(&format!("/proc/{pid}")).exists() {
        if Instant::now() > deadline {
            signal(pid, "-KILL");
            std::thread::sleep(Duration::from_millis(200));
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_file(pid_file());
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
    // setsid detaches the gateway from the ssh session that ran `up --go`;
    // process_group(0) is the fallback when setsid is absent.
    let mut child = if Command::new("setsid")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
    {
        let mut c = Command::new("setsid");
        c.arg(exe.as_os_str()).arg("serve");
        c
    } else {
        let mut c = Command::new(&exe);
        c.arg("serve");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            c.process_group(0);
        }
        c
    };
    child
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err));
    let child = child.spawn().context("spawn shrug serve")?;
    let child_pid = child.id();
    std::fs::write(pid_file(), format!("{child_pid}\n"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    // Always probe loopback: 0.0.0.0 is not a client address. The pid in the
    // reply must be the child we just spawned, not a dying predecessor.
    let url = format!("http://127.0.0.1:{}/health", listen.port());
    loop {
        if let Ok(r) = client.get(&url).send()
            && r.status().is_success()
            && let Ok(body) = r.json::<serde_json::Value>()
            && body["pid"].as_u64() == Some(child_pid as u64)
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
