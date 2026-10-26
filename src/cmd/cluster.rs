// SPDX-License-Identifier: AGPL-3.0-only
//! plan, up, rollback, down, clock-cap.
use super::Globals;
use crate::executor::Up;
use crate::state::{Config, State};
use anyhow::{Context, Result, bail};
use std::io::IsTerminal;

pub fn plan_cmd(g: &Globals) -> Result<i32> {
    let (d, hosts) = super::discover_hosts(g)?;
    let cfg = Config::load();
    let (plan, _cat) = super::make_plan(&d, &hosts, g, &cfg)?;
    super::emit_plan(&plan, g.json)
}

pub fn up(g: &Globals, go: bool) -> Result<i32> {
    let (d, hosts) = super::discover_hosts(g)?;
    let mut cfg = Config::load();
    let (mut plan, cat) = super::make_plan(&d, &hosts, g, &cfg)?;
    if !go {
        return super::emit_plan(&plan, g.json);
    }
    // Non-commercial weights: the last gate before any download.
    if let Some(rel) = plan.release_id.as_deref().and_then(|id| cat.release(id)) {
        for w in rel.weights.iter().filter(|w| w.noncommercial) {
            if g.accept_noncommercial || cfg.accepted(&w.dir_name(), &w.revision) {
                continue;
            }
            if std::io::stdin().is_terminal() {
                eprintln!(
                    "{} is licensed {} ({}). Non-commercial use only. Accept? [y/N]",
                    w.repo, w.license, w.license_url
                );
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if line.trim().eq_ignore_ascii_case("y") {
                    cfg.accept(&w.dir_name(), &w.revision);
                    cfg.save()?;
                }
            }
        }
        // Rebuild the plan with the updated config.
        plan = super::make_plan(&d, &hosts, g, &cfg)?.0;
    }
    if plan.blocked() {
        return super::emit_plan(&plan, g.json);
    }
    let release = cat
        .release(plan.release_id.as_deref().context("no release chosen")?)
        .context("release vanished")?;
    let refs: Vec<&dyn crate::host::Host> = hosts.iter().map(|h| h.as_ref()).collect();
    let up = Up {
        plan: &plan,
        discovery: &d,
        release,
        hosts: &refs,
        old_state: State::load()?,
        known_hosts: state_known_hosts(),
        ssh_user: g.ssh_user.clone(),
    };
    up.run()?;
    Ok(0)
}

fn state_known_hosts() -> String {
    crate::state::known_hosts().display().to_string()
}

pub fn rollback(g: &Globals, go: bool) -> Result<i32> {
    let Some(state) = State::load()? else {
        bail!("no state.json; nothing to roll back from");
    };
    let Some(prev) = &state.previous_release_id else {
        bail!("state has no previous release to roll back to");
    };
    let g = Globals {
        json: g.json,
        hosts: g.hosts.clone(),
        ssh_user: g.ssh_user.clone(),
        model: Some(prev.clone()),
        accept_noncommercial: g.accept_noncommercial,
        offline: g.offline,
        no_clock_cap: g.no_clock_cap,
    };
    up(&g, go)
}

pub fn down(g: &Globals) -> Result<i32> {
    let Some(state) = State::load()? else {
        eprintln!("no state.json; nothing is recorded as running");
        return Ok(1);
    };
    let local = crate::host::LocalHost::new();
    for r in &state.ranks {
        if r.route.is_local() {
            let _ = crate::host::Host::run(&local, &["docker", "rm", "-f", &r.container]);
        } else {
            let host = crate::host::SshHost::new(r.route.clone(), crate::state::known_hosts());
            let out = crate::host::Host::run(&host, &["docker", "rm", "-f", &r.container])?;
            if !out.ok() {
                eprintln!("{}: {}", r.host_label, out.stderr.trim());
            }
        }
        println!("removed {}", r.container);
    }
    crate::gateway::stop()?;
    std::fs::remove_file(crate::state::home().join("state.json")).ok();
    if g.json {
        println!("{}", serde_json::json!({"down": true}));
    }
    Ok(0)
}

const GPU_CLOCK_CAP: &str = include_str!("../../assets/gpu-clock-cap.sh");
const CLOCK_MHZ: &str = "2200";

pub fn clock_cap(g: &Globals, go: bool) -> Result<i32> {
    let (d, hosts) = super::discover_hosts(g)?;
    if !go {
        println!("would cap GPU clocks at 300,{CLOCK_MHZ} MHz on each host (sudo, needs a TTY):");
        for h in &d.hosts {
            println!("  {}: sudo gpu-clock-cap {CLOCK_MHZ}", h.probe.hostname);
        }
        println!("run `shrug clock-cap --go` to do it");
        return Ok(0);
    }
    for (i, h) in d.hosts.iter().enumerate() {
        let remote = format!(
            "sudo bash -c {} gpu-clock-cap {}",
            shlex::try_quote(GPU_CLOCK_CAP)?,
            CLOCK_MHZ
        );
        let status = hosts[i].run_tty(&remote)?;
        if status != 0 {
            bail!("clock-cap failed on {} (exit {status})", h.probe.hostname);
        }
    }
    Ok(0)
}
