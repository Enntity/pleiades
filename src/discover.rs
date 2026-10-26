// SPDX-License-Identifier: AGPL-3.0-only
//! Finding the peer Sparks: probe the local host, then walk the cables.
use crate::host::{Host, SshHost, SshRoute};
use crate::probe::{self, Probe};
use anyhow::Result;
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::net::Ipv4Addr;
use std::path::Path;

const MAX_SPARKS: usize = 4;

#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub probe: Probe,
    pub route: SshRoute,
}

#[derive(Debug, Clone, Serialize)]
pub struct SshFailure {
    pub candidate: String,
    /// "refused" (pinged but ssh failed) or "key-mismatch".
    pub kind: String,
    pub detail: String,
    /// The command a human runs to fix it, when known.
    pub remedy: Option<String>,
}

#[derive(Default, Serialize)]
pub struct Discovery {
    /// Host 0 is always the local machine.
    pub hosts: Vec<Found>,
    pub failures: Vec<SshFailure>,
}

fn probe_route(route: &SshRoute, known_hosts: &Path) -> Result<Probe, (String, String)> {
    let host = SshHost::new(route.clone(), known_hosts.to_path_buf());
    match host.run_script(probe::PROBE_SCRIPT, &[]) {
        Ok(out) if out.ok() => probe::parse(&out.stdout).map_err(|e| (e.to_string(), out.stderr)),
        Ok(out) => Err((format!("ssh exit {}", out.status), out.stderr)),
        Err(e) => Err((e.to_string(), String::new())),
    }
}

fn pings(candidate: Ipv4Addr) -> bool {
    std::process::Command::new("ping")
        .args(["-c1", "-W2", &candidate.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn classify(candidate: Ipv4Addr, user: &str, err: &str, stderr: &str) -> Option<SshFailure> {
    let all = format!("{err}\n{stderr}");
    if all.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || all.contains("Host key verification failed")
        || all.contains("host key") && all.contains("mismatch")
    {
        return Some(SshFailure {
            candidate: candidate.to_string(),
            kind: "key-mismatch".into(),
            detail: "ssh host key does not match known_hosts; verify the host and fix the entry"
                .into(),
            remedy: None,
        });
    }
    if pings(candidate) {
        return Some(SshFailure {
            candidate: candidate.to_string(),
            kind: "refused".into(),
            detail: format!("{err} {stderr}").trim().chars().take(300).collect(),
            remedy: Some(format!("ssh-copy-id {user}@{candidate}")),
        });
    }
    None // silent: nothing there
}

/// Probe the local machine and every reachable peer, breadth-first.
/// `explicit` lists peer ssh destinations for `--hosts` (skips discovery).
pub fn discover(
    local: &dyn Host,
    known_hosts: &Path,
    ssh_user: &str,
    explicit: Option<&[String]>,
) -> Result<Discovery> {
    let mut d = Discovery::default();
    let out = local.run_script(probe::PROBE_SCRIPT, &[])?;
    if !out.ok() {
        anyhow::bail!("local probe failed: {}", out.stderr.trim());
    }
    let local_probe = probe::parse(&out.stdout)?;
    d.hosts.push(Found {
        probe: local_probe,
        route: SshRoute::local(),
    });

    if let Some(dests) = explicit {
        for dest in dests {
            if d.hosts.len() >= MAX_SPARKS {
                break;
            }
            let (user, target) = dest.split_once('@').unwrap_or((ssh_user, dest.as_str()));
            let route = SshRoute {
                user: user.into(),
                dest: target.into(),
                jumps: Vec::new(),
            };
            match probe_route(&route, known_hosts) {
                Ok(p) => {
                    if d.hosts.iter().all(|h| h.probe.machine_id != p.machine_id) {
                        d.hosts.push(Found { probe: p, route });
                    }
                }
                Err((err, stderr)) => {
                    d.failures.push(SshFailure {
                        candidate: dest.clone(),
                        kind: "refused".into(),
                        detail: format!("{err} {stderr}").trim().chars().take(300).collect(),
                        remedy: Some(format!("ssh-copy-id {dest}")),
                    });
                }
            }
        }
        return Ok(d);
    }

    let mut seen_ids: HashSet<String> = HashSet::new();
    seen_ids.insert(d.hosts[0].probe.machine_id.clone());
    let mut seen_dests: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    queue.push_back(0);

    while let Some(idx) = queue.pop_front() {
        if d.hosts.len() >= MAX_SPARKS {
            break;
        }
        let host_route = d.hosts[idx].route.clone();
        for candidate in d.hosts[idx].probe.peer_candidates() {
            let dest = candidate.to_string();
            if !seen_dests.insert(dest.clone()) {
                continue;
            }
            let route = host_route.via(ssh_user, &dest);
            match probe_route(&route, known_hosts) {
                Ok(p) => {
                    if seen_ids.insert(p.machine_id.clone()) {
                        d.hosts.push(Found { probe: p, route });
                        queue.push_back(d.hosts.len() - 1);
                    }
                }
                Err((err, stderr)) => {
                    if let Some(f) = classify(candidate, ssh_user, &err, &stderr) {
                        d.failures.push(f);
                    }
                }
            }
            if d.hosts.len() >= MAX_SPARKS {
                break;
            }
        }
    }
    Ok(d)
}
