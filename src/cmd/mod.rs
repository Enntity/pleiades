// SPDX-License-Identifier: AGPL-3.0-only
pub mod catalog_dev;
pub mod cluster;
pub mod inspect;

use crate::discover::{self, Discovery};
use crate::host::{Host, LocalHost, SshHost};
use crate::plan::{self, HostChecks, Plan};
use crate::{catalog, state};
use anyhow::Result;
use std::path::PathBuf;

/// Global options shared by every command.
pub struct Globals {
    pub json: bool,
    pub hosts: Option<Vec<String>>,
    pub ssh_user: String,
    pub model: Option<String>,
    pub accept_noncommercial: bool,
    pub offline: bool,
    pub no_clock_cap: bool,
}

/// Probe the local machine and peers; return hosts aligned with d.hosts.
pub fn discover_hosts(g: &Globals) -> Result<(Discovery, Vec<Box<dyn Host>>)> {
    let local = LocalHost::new();
    let d = discover::discover(
        &local,
        &state::known_hosts(),
        &g.ssh_user,
        g.hosts.as_deref(),
    )?;
    let _ = std::fs::create_dir_all(state::home());
    let mut hosts: Vec<Box<dyn Host>> = vec![Box::new(LocalHost::new())];
    for h in &d.hosts[1..] {
        hosts.push(Box::new(SshHost::new(
            h.route.clone(),
            state::known_hosts(),
        )));
    }
    Ok((d, hosts))
}

/// Build the plan against real hosts.
pub fn make_plan(
    d: &Discovery,
    hosts: &[Box<dyn Host>],
    g: &Globals,
    cfg: &state::Config,
) -> Result<(Plan, catalog::Catalog)> {
    let loaded = catalog::load(&state::home(), g.offline);
    let refs: Vec<&dyn Host> = hosts.iter().map(|h| h.as_ref()).collect();
    let checks = HostChecks { hosts: &refs };
    let plan = plan::build(
        d,
        &loaded.catalog,
        loaded.source.name(),
        &plan::Options {
            model: g.model.as_deref(),
            accept_noncommercial: g.accept_noncommercial,
            no_clock_cap: g.no_clock_cap,
            config: cfg,
        },
        &checks,
    );
    Ok((plan, loaded.catalog))
}

/// Print a plan human-readable or as JSON, and its exit code (3 if blocked).
pub fn emit_plan(plan: &Plan, json: bool) -> Result<i32> {
    if json {
        println!("{}", serde_json::to_string_pretty(plan)?);
    } else {
        plan::print_human(plan);
    }
    Ok(if plan.blocked() { 3 } else { 0 })
}

pub fn known_hosts_display() -> PathBuf {
    state::known_hosts()
}
