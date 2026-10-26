// SPDX-License-Identifier: AGPL-3.0-only
//! How Shrug runs a command on a host: locally, or over an ssh route.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Private keys under ~/.ssh (`id_*`, no .pub), offered to every hop's auth.
/// Cluster fabrics commonly use named keys rather than the default names.
fn identity_files() -> &'static [String] {
    static KEYS: OnceLock<Vec<String>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let Some(home) = std::env::home_dir() else {
            return Vec::new();
        };
        let dir = home.join(".ssh");
        let mut keys: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("id_") && !n.ends_with(".pub"))
                            && p.is_file()
                            && p.extension().is_none_or(|e| e != "old")
                    })
                    .map(|p| p.display().to_string())
                    .collect()
            })
            .unwrap_or_default();
        keys.sort();
        keys
    })
}

fn add_identity_args(cmd: &mut Command) {
    for k in identity_files() {
        cmd.arg("-i").arg(k);
    }
}

pub struct CmdOut {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOut {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

/// One hop: `user@dest`. `jumps` are the ProxyJump path to reach `dest`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshRoute {
    pub user: String,
    pub dest: String,
    pub jumps: Vec<String>,
}

impl SshRoute {
    pub fn local() -> Self {
        Self {
            user: String::new(),
            dest: "local".into(),
            jumps: Vec::new(),
        }
    }
    pub fn is_local(&self) -> bool {
        self.jumps.is_empty() && self.dest == "local"
    }
    pub fn target(&self) -> String {
        if self.user.is_empty() {
            self.dest.clone()
        } else {
            format!("{}@{}", self.user, self.dest)
        }
    }
    /// Route to `next` through this host. From the local machine that's a
    /// direct ssh; through a peer it gains a jump.
    pub fn via(&self, user: &str, next: &str) -> Self {
        let mut jumps = self.jumps.clone();
        if !self.is_local() {
            jumps.push(self.target());
        }
        Self {
            user: user.into(),
            dest: next.into(),
            jumps,
        }
    }
}

/// Runs commands on one machine. Commands are always non-interactive.
pub trait Host: Send + Sync {
    fn route(&self) -> &SshRoute;
    /// Run argv on the host.
    fn run(&self, argv: &[&str]) -> Result<CmdOut>;
    /// Pipe `script` to `bash -s -- args` on the host.
    fn run_script(&self, script: &str, args: &[&str]) -> Result<CmdOut>;
    /// Run argv with `stdin_text` on stdin.
    fn run_stdin(&self, argv: &[&str], stdin_text: &str) -> Result<CmdOut>;
    /// Run `remote_cmd` under bash with the terminal attached (for sudo).
    fn run_tty(&self, remote_cmd: &str) -> Result<i32>;

    fn label(&self) -> String {
        self.route().target()
    }
}

pub struct LocalHost {
    route: SshRoute,
}

impl Default for LocalHost {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalHost {
    pub fn new() -> Self {
        Self {
            route: SshRoute::local(),
        }
    }
}

fn spawn(mut cmd: Command, stdin_text: Option<&str>) -> Result<CmdOut> {
    if stdin_text.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn {:?}", cmd.get_program()))?;
    if let Some(text) = stdin_text {
        child
            .stdin
            .take()
            .expect("stdin piped")
            .write_all(text.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    Ok(CmdOut {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into(),
        stderr: String::from_utf8_lossy(&out.stderr).into(),
    })
}

impl Host for LocalHost {
    fn route(&self) -> &SshRoute {
        &self.route
    }
    fn run(&self, argv: &[&str]) -> Result<CmdOut> {
        let (prog, args) = argv.split_first().context("empty argv")?;
        let mut cmd = Command::new(prog);
        cmd.args(args);
        spawn(cmd, None)
    }
    fn run_script(&self, script: &str, args: &[&str]) -> Result<CmdOut> {
        let mut cmd = Command::new("bash");
        cmd.arg("-s").arg("--").args(args);
        spawn(cmd, Some(script))
    }
    fn run_stdin(&self, argv: &[&str], stdin_text: &str) -> Result<CmdOut> {
        let (prog, args) = argv.split_first().context("empty argv")?;
        let mut cmd = Command::new(prog);
        cmd.args(args);
        spawn(cmd, Some(stdin_text))
    }
    fn run_tty(&self, remote_cmd: &str) -> Result<i32> {
        Ok(Command::new("bash")
            .arg("-c")
            .arg(remote_cmd)
            .status()?
            .code()
            .unwrap_or(-1))
    }
}

/// A host reached by `ssh [-J jumps] user@dest`. Never writes ~/.ssh/known_hosts.
pub struct SshHost {
    route: SshRoute,
    known_hosts: PathBuf,
}

impl SshHost {
    pub fn new(route: SshRoute, known_hosts: PathBuf) -> Self {
        Self { route, known_hosts }
    }

    fn ssh(&self, remote_cmd: &str, stdin_text: Option<&str>, extra: &[&str]) -> Result<CmdOut> {
        let mut cmd = Command::new("ssh");
        cmd.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=6",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            &format!(
                "UserKnownHostsFile={} ~/.ssh/known_hosts",
                self.known_hosts.display()
            ),
        ]);
        if !self.route.jumps.is_empty() {
            cmd.arg("-J").arg(self.route.jumps.join(","));
        }
        add_identity_args(&mut cmd);
        cmd.args(extra).arg(self.route.target()).arg(remote_cmd);
        spawn(cmd, stdin_text)
    }
}

impl Host for SshHost {
    fn route(&self) -> &SshRoute {
        &self.route
    }
    fn run(&self, argv: &[&str]) -> Result<CmdOut> {
        self.ssh(&shlex::try_join(argv.iter().copied())?, None, &[])
    }
    fn run_script(&self, script: &str, args: &[&str]) -> Result<CmdOut> {
        let remote = format!("bash -s -- {}", shlex::try_join(args.iter().copied())?);
        self.ssh(&remote, Some(script), &[])
    }
    fn run_stdin(&self, argv: &[&str], stdin_text: &str) -> Result<CmdOut> {
        self.ssh(
            &shlex::try_join(argv.iter().copied())?,
            Some(stdin_text),
            &[],
        )
    }
    fn run_tty(&self, remote_cmd: &str) -> Result<i32> {
        let mut cmd = Command::new("ssh");
        cmd.arg("-t");
        if !self.route.jumps.is_empty() {
            cmd.arg("-J").arg(self.route.jumps.join(","));
        }
        add_identity_args(&mut cmd);
        cmd.arg(self.route.target()).arg(remote_cmd);
        Ok(cmd.status()?.code().unwrap_or(-1))
    }
}

/// Shared cross-host call log: (dest, command line) pairs.
#[doc(hidden)]
pub type SharedCalls = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// Records invocations and replies from a script; for tests, no network.
#[doc(hidden)]
#[derive(Default)]
pub struct FakeHost {
    pub route: SshRoute,
    pub calls: std::sync::Mutex<Vec<String>>,
    /// Optional cross-host call log, to assert ordering between hosts.
    pub shared: Option<SharedCalls>,
    /// (substring of command line, output) checked in order.
    pub replies: Vec<(String, CmdOut)>,
}

impl FakeHost {
    pub fn named(dest: &str) -> Self {
        Self {
            route: SshRoute {
                user: "tester".into(),
                dest: dest.into(),
                jumps: Vec::new(),
            },
            ..Default::default()
        }
    }
    pub fn on(mut self, pat: &str, status: i32, stdout: &str) -> Self {
        self.replies.push((
            pat.into(),
            CmdOut {
                status,
                stdout: stdout.into(),
                stderr: String::new(),
            },
        ));
        self
    }
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn dispatch(&self, cmdline: &str) -> Result<CmdOut> {
        self.calls.lock().unwrap().push(cmdline.to_string());
        if let Some(s) = &self.shared {
            s.lock()
                .unwrap()
                .push((self.route.dest.clone(), cmdline.to_string()));
        }
        for (pat, out) in &self.replies {
            if cmdline.contains(pat.as_str()) {
                return Ok(CmdOut {
                    status: out.status,
                    stdout: out.stdout.clone(),
                    stderr: out.stderr.clone(),
                });
            }
        }
        Ok(CmdOut {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

impl Host for FakeHost {
    fn route(&self) -> &SshRoute {
        &self.route
    }
    fn run(&self, argv: &[&str]) -> Result<CmdOut> {
        self.dispatch(&argv.join(" "))
    }
    fn run_script(&self, script: &str, args: &[&str]) -> Result<CmdOut> {
        // Tests match on a marker inside the script or on the args.
        self.dispatch(&format!(
            "bash -s -- {} <<<{}",
            args.join(" "),
            &script[..script.len().min(64)]
        ))
    }
    fn run_stdin(&self, argv: &[&str], stdin_text: &str) -> Result<CmdOut> {
        self.dispatch(&format!(
            "{} <<<{}",
            argv.join(" "),
            &stdin_text[..stdin_text.len().min(64)]
        ))
    }
    fn run_tty(&self, remote_cmd: &str) -> Result<i32> {
        let _ = self.dispatch(&format!("tty: {remote_cmd}"))?;
        Ok(0)
    }
}
