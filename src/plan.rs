// SPDX-License-Identifier: AGPL-3.0-only
//! The plan: schema `shrug.plan/v1`, built by probe + discover, printed or
//! executed. Planning never changes anything.
use crate::catalog::Catalog;
use crate::discover::Discovery;
use crate::host::Host;
use crate::render::{self, Vars};
use crate::topo::{self, RankFabric};
use serde::Serialize;
use std::net::Ipv4Addr;

/// Runtime questions the plan asks the machines. Implemented by real hosts
/// in production and by fakes in tests.
pub trait Checks {
    fn ping(&self, host: usize, ip: Ipv4Addr) -> bool;
    fn route_dev(&self, host: usize, ip: Ipv4Addr) -> Option<String>;
    fn image_present(&self, host: usize, image: &str) -> bool;
    /// Marker `.shrug-revision` under dir contains rev.
    fn weight_present(&self, host: usize, dir: &str, rev: &str) -> bool;
}

pub struct HostChecks<'a> {
    pub hosts: &'a [&'a dyn Host],
}

impl Checks for HostChecks<'_> {
    fn ping(&self, host: usize, ip: Ipv4Addr) -> bool {
        self.hosts[host]
            .run(&["ping", "-c1", "-W2", &ip.to_string()])
            .is_ok_and(|o| o.ok())
    }
    fn route_dev(&self, host: usize, ip: Ipv4Addr) -> Option<String> {
        let out = self.hosts[host]
            .run(&["ip", "-o", "route", "get", &ip.to_string()])
            .ok()?;
        if !out.ok() {
            return None;
        }
        let mut it = out.stdout.split_whitespace();
        while let Some(t) = it.next() {
            if t == "dev" {
                return it.next().map(str::to_string);
            }
        }
        None
    }
    fn image_present(&self, host: usize, image: &str) -> bool {
        self.hosts[host]
            .run(&["docker", "image", "inspect", image])
            .is_ok_and(|o| o.ok())
    }
    fn weight_present(&self, host: usize, dir: &str, rev: &str) -> bool {
        self.hosts[host]
            .run(&["cat", &format!("{dir}/.shrug-revision")])
            .is_ok_and(|o| o.ok() && o.stdout.trim() == rev)
    }
}

#[derive(Serialize)]
pub struct RankPlan {
    pub rank: usize,
    #[serde(skip_serializing)]
    pub host_idx: usize,
    pub host: String,
    pub lan_ip: Option<String>,
    pub ssh: crate::host::SshRoute,
    pub model_root: String,
    pub fabric: RankFabric,
    pub container: String,
    pub args: Vec<String>,
}

#[derive(Serialize)]
pub struct Step {
    pub id: String,
    pub host: String,
    pub status: String, // "pending" or "done"
}

#[derive(Serialize)]
pub struct Plan {
    pub schema: String,
    pub catalog: serde_json::Value,
    pub cluster: serde_json::Value,
    pub release: Option<serde_json::Value>,
    pub release_id: Option<String>,
    pub ranks: Vec<RankPlan>,
    pub steps: Vec<Step>,
    pub disk: Vec<serde_json::Value>,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

impl Plan {
    pub fn blocked(&self) -> bool {
        !self.blockers.is_empty()
    }
}

pub struct Options<'a> {
    pub model: Option<&'a str>,
    pub accept_noncommercial: bool,
    pub no_clock_cap: bool,
    pub config: &'a crate::state::Config,
}

fn model_root(home: &str) -> String {
    format!("{home}/.shrug/models")
}

fn cache_dir(home: &str, name: &str) -> String {
    format!("{home}/.cache/{name}")
}

/// Build the plan. Pure apart from what `checks` asks the machines.
pub fn build(
    d: &Discovery,
    catalog: &Catalog,
    source: &str,
    opts: &Options,
    checks: &dyn Checks,
) -> Plan {
    let mut warnings = Vec::new();
    let mut blockers: Vec<String> = Vec::new();

    // Cluster first: its shape decides which hosts the release uses.
    let cluster = match topo::cluster(d) {
        Ok(c) => c,
        Err(e) => {
            blockers.extend(e);
            topo::Cluster {
                shape: topo::Shape::Single,
                ranks: vec![0],
                found: d.hosts.len(),
            }
        }
    };

    // Release choice: --model, else the default for the usable count.
    let usable = cluster.ranks.len().min(d.hosts.len());
    let release_id = opts
        .model
        .map(str::to_string)
        .or_else(|| catalog.defaults.get(&usable.to_string()).cloned());
    let release = release_id.as_deref().and_then(|id| {
        let r = catalog.release(id);
        if r.is_none() {
            blockers.push(format!("catalog has no release {id:?}"));
        }
        r
    });

    // The hosts the chosen release actually runs on; problems elsewhere are
    // warnings, not blockers.
    let used: std::collections::BTreeSet<usize> = match release {
        Some(r) if (r.sparks as usize) <= usable => {
            cluster.ranks[..r.sparks as usize].iter().copied().collect()
        }
        _ => (0..d.hosts.len()).collect(),
    };

    // ssh/discovery failures: blockers only without an explicit --model —
    // the default release depends on how many hosts were found.
    for f in &d.failures {
        let mut s = format!("ssh to {} failed: {}", f.candidate, f.detail);
        if let Some(r) = &f.remedy {
            s.push_str(&format!("; run `{r}`"));
        }
        if opts.model.is_none() {
            blockers.push(s);
        } else {
            warnings.push(s);
        }
    }

    // Per-host checks.
    for (i, h) in d.hosts.iter().enumerate() {
        let p = &h.probe;
        let who = if p.hostname.is_empty() {
            format!("host {i}")
        } else {
            p.hostname.clone()
        };
        let used_host = used.contains(&i);
        let mut issues: Vec<String> = Vec::new();
        let unused = if used_host { "" } else { " (unused host)" };
        if !p.is_gb10() {
            issues.push(format!("{who}: no GB10 GPU found{unused}"));
        }
        if !p.docker_ok {
            issues.push(format!(
                "{who}: docker is not usable by {}{}",
                p.user, unused
            ));
        } else if !p.has_nvidia_runtime() {
            issues.push(format!(
                "{who}: nvidia container toolkit missing (no nvidia-ctk, no nvidia runtime){unused}"
            ));
        }
        if p.clock_cap != "active" {
            issues.push(format!(
                "{who}: gpu-clock-cap.service is {}; run `shrug clock-cap --go`{unused}",
                p.clock_cap
            ));
        }
        for c in &p.containers {
            if !c.name.starts_with("shrug-") && c.holds_gpu && c.status == "running" {
                issues.push(format!(
                    "{who}: container {} ({}) holds the GPU; Shrug did not start it{unused}",
                    c.name, c.image
                ));
            }
        }
        for proc in &p.gpu_procs {
            if proc
                .container
                .as_deref()
                .is_some_and(|c| c.starts_with("shrug-"))
            {
                continue;
            }
            issues.push(format!(
                "{who}: GPU process {} (pid {}) is not Shrug's{unused}",
                proc.name, proc.pid
            ));
        }
        if used_host && !opts.no_clock_cap {
            blockers.extend(issues);
        } else {
            warnings.extend(issues);
        }
    }

    let mut ranks: Vec<RankPlan> = Vec::new();
    let mut steps = Vec::new();
    let mut disk = Vec::new();
    if let Some(rel) = release {
        if rel.sparks as usize > usable {
            blockers.push(format!(
                "release {} needs {} Sparks; {} usable",
                rel.id, rel.sparks, usable
            ));
        } else {
            let used = &cluster.ranks[..rel.sparks as usize];
            let topology = match rel.sparks {
                1 => "single",
                2 => "pair",
                _ => cluster.shape.name(),
            };
            if !rel.topologies.iter().any(|t| t == topology) {
                blockers.push(format!(
                    "release {} does not support a {topology} cluster",
                    rel.id
                ));
            }
            // Non-commercial weights.
            for w in rel.weights.iter().filter(|w| w.noncommercial) {
                if !opts.accept_noncommercial && !opts.config.accepted(&w.dir_name(), &w.revision) {
                    blockers.push(format!(
                        "weight {} is licensed {} (non-commercial); pass --accept-noncommercial or accept during `up --go`",
                        w.repo, w.license
                    ));
                }
            }
            match topo::rank_fabrics(d, &cluster, used, &|h, ip| checks.ping(h, ip), &|h, ip| {
                checks.route_dev(h, ip)
            }) {
                Ok(fabrics) => {
                    for (ri, (&h, fabric)) in used.iter().zip(fabrics.iter()).enumerate() {
                        let p = &d.hosts[h].probe;
                        let mut vars: Vars = [
                            ("rank".into(), ri.to_string()),
                            ("topology".into(), fabric.topology.clone()),
                            ("leader_address".into(), fabric.leader_address.clone()),
                            ("fabric_interface".into(), fabric.fabric_interface.clone()),
                            ("fabric_hca".into(), fabric.fabric_hca.clone()),
                            ("left_hca".into(), fabric.left_hca.clone()),
                            ("right_hca".into(), fabric.right_hca.clone()),
                            ("model_root".into(), model_root(&p.home)),
                            ("image".into(), rel.engine.image.clone()),
                            ("cuda_cache".into(), cache_dir(&p.home, "atlas-cuda")),
                            ("ple_cache".into(), cache_dir(&p.home, "atlas-ple")),
                        ]
                        .into_iter()
                        .collect();
                        let container =
                            render::render(&rel.container_name, &vars).unwrap_or_else(|e| {
                                blockers.push(format!("container_name: {e}"));
                                String::new()
                            });
                        vars.insert("name".into(), container.clone());
                        let args = render::render_args(&rel.args, &vars).unwrap_or_else(|e| {
                            blockers.push(format!("release {} args: {e}", rel.id));
                            Vec::new()
                        });
                        ranks.push(RankPlan {
                            rank: ri,
                            host_idx: h,
                            host: p.hostname.clone(),
                            lan_ip: p.lan_ip.map(|i| i.to_string()),
                            ssh: d.hosts[h].route.clone(),
                            model_root: model_root(&p.home),
                            fabric: fabric.clone(),
                            container,
                            args,
                        });
                    }
                }
                Err(e) => blockers.extend(e),
            }
            // Steps and disk.
            let need_gib =
                (rel.weights.iter().map(|w| w.size_gib).sum::<f64>() + rel.extra_disk_gib) * 1.1;
            for rp in &ranks {
                let h = rp.host_idx;
                steps.push(Step {
                    id: format!("image-rank{}", rp.rank),
                    host: rp.host.clone(),
                    status: if checks.image_present(h, &rel.engine.image) {
                        "done".into()
                    } else {
                        "pending".into()
                    },
                });
                let mut host_need = 0.0;
                for w in &rel.weights {
                    let dir = format!("{}/{}", rp.model_root, w.dir_name());
                    let done = checks.weight_present(h, &dir, &w.revision);
                    if !done {
                        host_need += w.size_gib;
                    }
                    steps.push(Step {
                        id: format!("weights-{}-rank{}", w.dir_name(), rp.rank),
                        host: rp.host.clone(),
                        status: if done {
                            "done".into()
                        } else {
                            "pending".into()
                        },
                    });
                }
                if rel.extra_disk_gib > 0.0 {
                    host_need += rel.extra_disk_gib;
                }
                let free = d.hosts[h].probe.home_free_kib as f64 / 1048576.0;
                disk.push(serde_json::json!({
                    "host": rp.host,
                    "need_gib": (host_need * 1.1 * 10.0).round() / 10.0,
                    "budget_gib": (need_gib * 10.0).round() / 10.0,
                    "free_gib": (free * 10.0).round() / 10.0,
                }));
                if host_need * 1.1 > free {
                    blockers.push(format!(
                        "{}: needs {:.1} GiB free for weights, has {:.1} GiB",
                        rp.host,
                        host_need * 1.1,
                        free
                    ));
                }
                steps.push(Step {
                    id: format!("launch-rank{}", rp.rank),
                    host: rp.host.clone(),
                    status: "pending".into(),
                });
            }
            for id in ["wait-health", "verify", "gateway"] {
                steps.push(Step {
                    id: id.into(),
                    host: ranks.first().map(|r| r.host.clone()).unwrap_or_default(),
                    status: "pending".into(),
                });
            }
        }
    }

    Plan {
        schema: "shrug.plan/v1".into(),
        catalog: serde_json::json!({"serial": catalog.serial, "source": source}),
        cluster: serde_json::json!({
            "shape": cluster.shape.name(),
            "found": cluster.found,
            "usable": usable,
        }),
        release: release.map(|r| serde_json::to_value(r).unwrap_or_default()),
        release_id,
        ranks,
        steps,
        disk,
        warnings,
        blockers,
    }
}

/// Human-readable plan.
pub fn print_human(plan: &Plan) {
    println!("¯\\_(ツ)_/¯ shrug plan");
    println!(
        "catalog: serial {} ({})",
        plan.catalog["serial"], plan.catalog["source"]
    );
    println!(
        "cluster: {} ({} found, {} usable)",
        plan.cluster["shape"], plan.cluster["found"], plan.cluster["usable"]
    );
    for r in &plan.ranks {
        println!(
            "  rank {} {} lan={} leader={} iface={} left={} right={}",
            r.rank,
            r.host,
            r.lan_ip.as_deref().unwrap_or("-"),
            r.fabric.leader_address,
            r.fabric.fabric_interface,
            r.fabric.left_hca,
            r.fabric.right_hca
        );
    }
    if let Some(rel) = &plan.release {
        println!(
            "release: {} — {}",
            rel["id"].as_str().unwrap_or(""),
            rel["name"].as_str().unwrap_or("")
        );
        println!("  image: {}", rel["engine"]["image"].as_str().unwrap_or(""));
        println!(
            "  atlas: {}@{}",
            rel["engine"]["atlas_repo"].as_str().unwrap_or(""),
            &rel["engine"]["atlas_commit"].as_str().unwrap_or("")
                [..12.min(rel["engine"]["atlas_commit"].as_str().unwrap_or("").len())]
        );
        for w in rel["weights"].as_array().into_iter().flatten() {
            println!(
                "  weight: {} @ {} ({:.1} GiB) license: {}{}",
                w["repo"].as_str().unwrap_or(""),
                &w["revision"].as_str().unwrap_or("")
                    [..12.min(w["revision"].as_str().unwrap_or("").len())],
                w["size_gib"].as_f64().unwrap_or(0.0),
                w["license"].as_str().unwrap_or(""),
                if w["noncommercial"].as_bool().unwrap_or(false) {
                    " [non-commercial]"
                } else {
                    ""
                }
            );
        }
    }
    if !plan.steps.is_empty() {
        println!("steps:");
        for s in &plan.steps {
            if s.status == "pending" {
                println!("  {} ({})", s.id, s.host);
            } else {
                println!("  {} ({}) — already done", s.id, s.host);
            }
        }
    }
    for dsk in &plan.disk {
        println!(
            "disk {}: needs {} GiB free, has {} GiB",
            dsk["host"], dsk["need_gib"], dsk["free_gib"]
        );
    }
    for w in &plan.warnings {
        println!("warning: {w}");
    }
    if plan.blockers.is_empty() {
        println!("no blockers; `shrug up --go` would run the pending steps");
    } else {
        println!("blockers:");
        for b in &plan.blockers {
            println!("  - {b}");
        }
    }
}
