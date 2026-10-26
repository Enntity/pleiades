// SPDX-License-Identifier: AGPL-3.0-only
//! models, status, logs.
use super::Globals;
use crate::catalog;
use crate::host::{Host, LocalHost, SshHost};
use crate::state::{self, State};
use anyhow::Result;

pub fn models(g: &Globals) -> Result<i32> {
    let loaded = catalog::load(&state::home(), g.offline);
    let (d, _hosts) = super::discover_hosts(g)?;
    let usable = crate::topo::cluster(&d).map(|c| c.ranks.len()).unwrap_or(0);
    if g.json {
        let releases: Vec<_> = loaded
            .catalog
            .releases
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "name": r.name,
                    "summary": r.summary,
                    "sparks": r.sparks,
                    "topologies": r.topologies,
                    "model_id": r.model_id,
                    "context_tokens": r.context_tokens,
                    "qualified_by": r.qualified_by,
                    "fits": (r.sparks as usize) <= usable && (usable > 0),
                    "weights": r.weights.iter().map(|w| serde_json::json!({
                        "repo": w.repo, "size_gib": w.size_gib,
                        "license": w.license, "noncommercial": w.noncommercial,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "catalog": {"serial": loaded.catalog.serial, "source": loaded.source.name()},
                "usable_sparks": usable,
                "releases": releases,
            }))?
        );
        return Ok(0);
    }
    println!(
        "catalog serial {} ({})",
        loaded.catalog.serial,
        loaded.source.name()
    );
    println!("usable Sparks here: {usable}");
    for r in &loaded.catalog.releases {
        let fits = (r.sparks as usize) <= usable && usable > 0;
        println!(
            "\n{} — {} [{}]",
            r.id,
            r.name,
            if fits { "fits" } else { "does not fit" }
        );
        println!("  {}", r.summary);
        for w in &r.weights {
            println!(
                "  weight {} ({:.1} GiB) license: {}{}",
                w.repo,
                w.size_gib,
                w.license,
                if w.noncommercial {
                    " [non-commercial]"
                } else {
                    ""
                }
            );
        }
    }
    Ok(0)
}

fn host_for(state: &crate::state::RankState) -> Box<dyn Host> {
    if state.route.is_local() {
        Box::new(LocalHost::new())
    } else {
        Box::new(SshHost::new(state.route.clone(), state::known_hosts()))
    }
}

pub fn status(g: &Globals) -> Result<i32> {
    let Some(s) = State::load()? else {
        eprintln!("no state.json; shrug has not started a release here");
        return Ok(1);
    };
    let mut doc = serde_json::json!({
        "release_id": s.release_id,
        "catalog_serial": s.catalog_serial,
        "model_id": s.model_id,
        "api_port": s.api_port,
        "previous_release_id": s.previous_release_id,
        "started_at_unix": s.started_at_unix,
    });
    let mut ranks = Vec::new();
    for r in &s.ranks {
        let host = host_for(r);
        let container = host
            .run(&["docker", "inspect", "-f", "{{.State.Status}}", &r.container])
            .map(|o| o.stdout.trim().to_string())
            .unwrap_or_else(|_| "unreachable".into());
        let mem = host
            .run(&["awk", "/MemAvailable/{print $2}", "/proc/meminfo"])
            .map(|o| o.stdout.trim().parse::<u64>().unwrap_or(0))
            .unwrap_or(0);
        let clock = host
            .run(&["systemctl", "is-active", "gpu-clock-cap.service"])
            .map(|o| o.stdout.trim().to_string())
            .unwrap_or_else(|_| "unknown".into());
        ranks.push(serde_json::json!({
            "rank": r.rank,
            "host": r.host_label,
            "container": r.container,
            "container_status": container,
            "mem_available_kib": mem,
            "clock_cap": clock,
        }));
    }
    doc["ranks"] = ranks.into();
    // Engine health and KV metrics on rank 0.
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let base = format!("http://127.0.0.1:{}", s.api_port);
    let health = client
        .get(format!("{base}/health"))
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    doc["engine_healthy"] = health.into();
    let metrics: Vec<String> = client
        .get(format!("{base}/metrics"))
        .send()
        .and_then(|r| r.text())
        .map(|t| {
            t.lines()
                .filter(|l| {
                    let name = l.split([' ', '{']).next().unwrap_or("");
                    name.contains("prefix_cache") || name.contains("kv")
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    doc["kv_metrics"] = metrics.clone().into();
    if g.json {
        println!("{}", serde_json::to_string_pretty(&doc)?);
    } else {
        println!(
            "release {} ({}), api :{}, healthy: {}",
            s.release_id, s.model_id, s.api_port, health
        );
        for r in doc["ranks"].as_array().into_iter().flatten() {
            println!(
                "  rank {} {}: {} {} mem_free={:.1} GiB clock_cap={}",
                r["rank"],
                r["host"],
                r["container"],
                r["container_status"],
                r["mem_available_kib"].as_f64().unwrap_or(0.0) / 1048576.0,
                r["clock_cap"],
            );
        }
        for m in &metrics {
            println!("  {m}");
        }
    }
    Ok(0)
}

pub fn logs(_g: &Globals, rank: usize, follow: bool) -> Result<i32> {
    let Some(s) = State::load()? else {
        anyhow::bail!("no state.json; nothing is recorded as running");
    };
    let Some(r) = s.ranks.iter().find(|r| r.rank == rank) else {
        anyhow::bail!("no rank {rank} in state");
    };
    let host = host_for(r);
    let mut argv = vec!["docker", "logs", "--tail", "200"];
    if follow {
        argv.push("-f");
    }
    argv.push(&r.container);
    let out = host.run(&argv)?;
    print!("{}{}", out.stdout, out.stderr);
    Ok(if out.ok() { 0 } else { 1 })
}
