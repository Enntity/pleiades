// SPDX-License-Identifier: AGPL-3.0-only
//! `up --go`: idempotent install, start and verify. Never touches a container
//! Shrug did not start.
use crate::catalog::Release;
use crate::discover::Discovery;
use crate::gateway;
use crate::host::{Host, ssh_shell};
use crate::plan::Plan;
use crate::state::{RankState, State};
use crate::verify;
use anyhow::{Context, Result, bail};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Throwaway downloader image and library, both pinned.
pub const PYTHON_IMAGE: &str =
    "python:3.12.12-slim@sha256:f3fa41d74a768c2fce8016b98c191ae8c1bacd8f1152870a3f9f87d350920b7c";
pub const HF_HUB_VERSION: &str = "2.2.0";

pub struct Up<'a> {
    pub plan: &'a Plan,
    pub discovery: &'a Discovery,
    pub release: &'a Release,
    pub hosts: &'a [&'a dyn Host],
    pub old_state: Option<State>,
    pub known_hosts: String,
    pub ssh_user: String,
}

fn say(msg: &str) {
    println!("{msg}");
}

impl Up<'_> {
    /// Step 2: image on every rank, arm64, GPU inside.
    pub fn ensure_images(&self) -> Result<()> {
        let image = &self.release.engine.image;
        thread::scope(|s| {
            let mut jobs = Vec::new();
            for rp in &self.plan.ranks {
                let host = self.hosts[rp.host_idx];
                jobs.push(s.spawn(move || -> Result<()> {
                    if host
                        .run(&["docker", "image", "inspect", image])
                        .is_ok_and(|o| o.ok())
                    {
                        return Ok(());
                    }
                    let pull = host.run_watch(&["docker", "pull", image])?;
                    if !pull.ok() {
                        if rp.rank == 0 {
                            bail!("docker pull failed on rank 0: {}", pull.stderr.trim());
                        }
                        // Fall back to shipping the image over ssh from rank 0.
                        let ssh = ssh_shell(
                            host.route(),
                            std::path::Path::new(&self.known_hosts),
                            "docker load",
                        )?;
                        let script = format!(
                            "docker save {} | {}",
                            shlex::try_quote(image.as_str())?,
                            ssh
                        );
                        say(&format!("image: copying to {} over ssh", rp.host));
                        let load = self.hosts[0].run_watch(&["bash", "-c", &script])?;
                        if !load.ok() {
                            bail!("docker load failed on {}: {}", rp.host, load.stderr.trim());
                        }
                    }
                    Ok(())
                }));
            }
            for j in jobs {
                j.join()
                    .map_err(|_| anyhow::anyhow!("image thread panicked"))??;
            }
            Ok::<(), anyhow::Error>(())
        })?;
        for rp in &self.plan.ranks {
            let host = self.hosts[rp.host_idx];
            let arch = host.run(&[
                "docker",
                "image",
                "inspect",
                "-f",
                "{{.Architecture}}",
                image,
            ])?;
            if arch.stdout.trim() != "arm64" {
                bail!("image on {} is {}, not arm64", rp.host, arch.stdout.trim());
            }
            let gpu = host.run(&[
                "docker",
                "run",
                "--rm",
                "--gpus",
                "all",
                "--entrypoint",
                "nvidia-smi",
                image,
                "-L",
            ])?;
            if !gpu.ok() || !gpu.stdout.contains("GB10") {
                bail!("GPU check failed on {}: {}", rp.host, gpu.stderr.trim());
            }
        }
        say("image: ok on every rank");
        Ok(())
    }

    fn hf_download(
        host: &dyn Host,
        has_hf: bool,
        uid: &str,
        model_root: &str,
        repo: &str,
        rev: &str,
        dir: &str,
    ) -> Result<()> {
        let out = if has_hf {
            host.run_watch(&[
                "hf",
                "download",
                repo,
                "--revision",
                rev,
                "--local-dir",
                dir,
            ])?
        } else {
            host.run_watch(&[
                "docker", "run", "--rm",
                "--user", uid,
                "-e", "HOME=/tmp",
                "-e", "HF_TOKEN",
                "-v", &format!("{model_root}:{model_root}"),
                PYTHON_IMAGE,
                "sh", "-c",
                &format!(
                    "pip install -q --disable-pip-version-check --target /tmp/hf 'huggingface_hub=={HF_HUB_VERSION}' && PYTHONPATH=/tmp/hf /tmp/hf/bin/hf download \"$0\" --revision \"$1\" --local-dir \"$2\""
                ),
                repo, rev, dir,
            ])?
        };
        if !out.ok() {
            bail!("hf download {repo} failed: {}", out.stderr.trim());
        }
        Ok(())
    }

    /// Step 3: weights on every rank, marker last. Rank 0 and the ranks not
    /// cabled to it download in parallel; copies to the cabled ranks start
    /// once rank 0's copy is complete, in parallel with each other.
    pub fn ensure_weights(&self) -> Result<()> {
        let ranks = &self.plan.ranks;
        // Ranks directly cabled to rank 0: rank 1 always; rank 3 on a ring.
        let mut cabled: Vec<usize> = vec![];
        if ranks.len() > 1 {
            cabled.push(1);
        }
        if self.plan.cluster["shape"] == "ring" && ranks.len() == 4 {
            cabled.push(3);
        }
        for w in &self.release.weights {
            let dir_name = std::sync::Arc::new(w.dir_name());
            thread::scope(|s| {
                // Downloaders: rank 0 plus every rank not cabled to it.
                let mut jobs = Vec::new();
                for rp in ranks.iter() {
                    if rp.rank != 0 && cabled.contains(&rp.rank) {
                        continue;
                    }
                    let dir = format!("{}/{}", rp.model_root, &dir_name);
                    jobs.push(s.spawn(move || {
                        let host = self.hosts[rp.host_idx];
                        if self.marker(host, &dir, &w.revision) {
                            return Ok(());
                        }
                        say(&format!("weights: {} on {}", w.repo, rp.host));
                        let has_hf = self.discovery.hosts[rp.host_idx].probe.has_tool("hf");
                        let uid = host.run(&["id", "-u"])?.stdout.trim().to_string()
                            + ":"
                            + host.run(&["id", "-g"])?.stdout.trim();
                        let _ = host.run(&["mkdir", "-p", &dir]);
                        Self::hf_download(
                            host,
                            has_hf,
                            &uid,
                            &rp.model_root,
                            &w.repo,
                            &w.revision,
                            &dir,
                        )?;
                        self.write_marker(host, &dir, &w.revision)
                    }));
                }
                for j in jobs {
                    j.join()
                        .map_err(|_| anyhow::anyhow!("download thread panicked"))??;
                }
                // Cable copies now that rank 0's tree is complete.
                let mut copies = Vec::new();
                for &ri in &cabled {
                    let rp = &ranks[ri];
                    let dir = format!("{}/{}", rp.model_root, &dir_name);
                    let dir_name = dir_name.clone();
                    copies.push(s.spawn(move || {
                        let host = self.hosts[rp.host_idx];
                        if self.marker(host, &dir, &w.revision) {
                            return Ok(());
                        }
                        let target = self.fabric_target(rp)?;
                        say(&format!(
                            "weights: copy {} to {} over the cable",
                            dir_name, rp.host
                        ));
                        self.copy_over_cable(
                            &ranks[0].model_root,
                            &dir_name,
                            &rp.model_root,
                            &target,
                        )?;
                        self.write_marker(host, &dir, &w.revision)
                    }));
                }
                for j in copies {
                    j.join()
                        .map_err(|_| anyhow::anyhow!("copy thread panicked"))??;
                }
                Ok::<(), anyhow::Error>(())
            })?;
        }
        say("weights: ok on every rank");
        Ok(())
    }

    fn marker(&self, host: &dyn Host, dir: &str, rev: &str) -> bool {
        host.run(&["cat", &format!("{dir}/.shrug-revision")])
            .is_ok_and(|o| o.ok() && o.stdout.trim() == rev)
    }

    fn write_marker(&self, host: &dyn Host, dir: &str, rev: &str) -> Result<()> {
        let script = format!(
            "mkdir -p {dir} && printf %s {rev} > {dir}/.shrug-revision
",
            dir = shlex::try_quote(dir)?,
            rev = shlex::try_quote(rev)?,
        );
        let out = host.run_script(&script, &[])?;
        if !out.ok() {
            bail!("write marker {dir}: {}", out.stderr.trim());
        }
        Ok(())
    }

    /// Peer's fabric IP on its edge with rank 0 (lowest-subnet rail).
    fn fabric_target(&self, rp: &crate::plan::RankPlan) -> Result<String> {
        // Find the peer probe's fabric addr sharing a subnet with rank 0.
        let h0 = &self.discovery.hosts[0].probe;
        let hp = &self.discovery.hosts[rp.host_idx].probe;
        let mut best: Option<(std::net::Ipv4Addr, std::net::Ipv4Addr)> = None;
        for pa in h0.fabric.iter().filter(|p| p.active) {
            for na in &pa.addrs {
                for pb in hp.fabric.iter().filter(|p| p.active) {
                    for nb in &pb.addrs {
                        if na.network() == nb.network() && na.ip != nb.ip {
                            let cand = (na.network(), nb.ip);
                            if best.is_none_or(|b| cand.0 < b.0) {
                                best = Some(cand);
                            }
                        }
                    }
                }
            }
        }
        best.map(|(_, ip)| ip.to_string())
            .context("no cabled fabric address toward rank 0")
    }

    /// SparkGLM's copy_to_worker: 8 parallel streams for big files, then rsync.
    fn copy_over_cable(
        &self,
        src_root: &str,
        dir_name: &str,
        dst_root: &str,
        target_ip: &str,
    ) -> Result<()> {
        let target = format!("{}@{target_ip}", self.ssh_user);
        let ssh = format!(
            "ssh -c aes128-gcm@openssh.com -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o 'UserKnownHostsFile={} ~/.ssh/known_hosts'",
            self.known_hosts
        );
        let script = format!(
            r#"set -euo pipefail
src={src} dst={dst} dir={dir} target={target}
ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o 'UserKnownHostsFile={kh} ~/.ssh/known_hosts' "$target" mkdir -p "$dst"
(cd "$src" && find "$dir" -type f -size +64M ! -name '*.incomplete' -print0) |
  xargs -0 -P8 -I{{}} rsync -aR -e '{ssh}' "$src/./{{}}" "$target:$dst/"
rsync -a --exclude '*.incomplete' -e '{ssh}' "$src/$dir" "$target:$dst/"
"#,
            src = shlex::try_quote(src_root)?,
            dst = shlex::try_quote(dst_root)?,
            dir = shlex::try_quote(dir_name)?,
            target = shlex::try_quote(&target)?,
            kh = self.known_hosts,
            ssh = ssh,
        );
        let out = self.hosts[0].run_script(&script, &[])?;
        if !out.ok() {
            bail!("cable copy to {target}: {}", out.stderr.trim());
        }
        Ok(())
    }

    /// Step 4: stop old shrug containers from state, launch workers then rank 0.
    pub fn launch(&self) -> Result<()> {
        if let Some(old) = &self.old_state {
            for r in &old.ranks {
                for host in self.hosts {
                    if host.route() == &r.route {
                        let _ = host.run(&["docker", "rm", "-f", &r.container]);
                    }
                }
            }
        }
        for rp in self.plan.ranks[1..].iter().rev() {
            self.launch_rank(rp)?;
        }
        self.launch_rank(&self.plan.ranks[0])
    }

    fn launch_rank(&self, rp: &crate::plan::RankPlan) -> Result<()> {
        say(&format!("launch: {} on {}", rp.container, rp.host));
        let host = self.hosts[rp.host_idx];
        let args: Vec<&str> = rp.args.iter().map(String::as_str).collect();
        let out = host.run_script(&self.release.launcher.script, &args)?;
        if !out.ok() {
            bail!(
                "launch {} on {}: {}",
                rp.container,
                rp.host,
                out.stderr.trim()
            );
        }
        Ok(())
    }

    /// Step 5: wait for health, watching every rank's container.
    pub fn wait_health(&self) -> Result<()> {
        let port = self.release.api_port;
        let deadline = Instant::now() + Duration::from_secs(20 * 60);
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;
        loop {
            for rp in &self.plan.ranks {
                let host = self.hosts[rp.host_idx];
                let st = host
                    .run(&[
                        "docker",
                        "inspect",
                        "-f",
                        "{{.State.Status}}",
                        &rp.container,
                    ])
                    .map(|o| o.stdout.trim().to_string())
                    .unwrap_or_else(|_| "unknown".into());
                if st != "running" && st != "created" {
                    let logs = host
                        .run(&["docker", "logs", "--tail", "40", &rp.container])
                        .map(|o| format!("{}{}", o.stdout, o.stderr))
                        .unwrap_or_default();
                    bail!(
                        "container {} on {} is {st}; last logs:\n{}",
                        rp.container,
                        rp.host,
                        logs
                    );
                }
            }
            if client
                .get(format!("http://127.0.0.1:{port}/health"))
                .send()
                .is_ok_and(|r| r.status().is_success())
            {
                say("health: ok");
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("engine did not become healthy within 20 minutes");
            }
            thread::sleep(Duration::from_secs(10));
        }
    }

    /// Steps 6-7: write state, start the gateway, verify through it.
    pub fn finish(&self) -> Result<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let state = State {
            schema: "shrug.state/v1".into(),
            release_id: self.release.id.clone(),
            catalog_serial: self.plan.catalog["serial"].as_u64().unwrap_or(0),
            model_id: self.release.model_id.clone(),
            api_port: self.release.api_port,
            previous_release_id: self.old_state.as_ref().map(|s| s.release_id.clone()),
            ranks: self
                .plan
                .ranks
                .iter()
                .map(|rp| RankState {
                    rank: rp.rank,
                    host_label: rp.host.clone(),
                    lan_ip: rp.lan_ip.clone(),
                    route: rp.ssh.clone(),
                    container: rp.container.clone(),
                })
                .collect(),
            started_at_unix: now,
        };
        state.save()?;
        gateway::start(&state)?;
        let cfg = crate::state::Config::load();
        let gw_port = gateway::listen_addr(&cfg)
            .map(|a| a.port())
            .unwrap_or(gateway::DEFAULT_PORT);
        verify::verify_engine(
            &format!("http://127.0.0.1:{gw_port}"),
            &self.release.model_id,
            cfg.api_key.as_deref(),
        )?;
        say("verify: math, stream, tool call and /v1/messages all pass through the gateway");
        Ok(())
    }

    pub fn run(&self) -> Result<()> {
        self.ensure_images()?;
        self.ensure_weights()?;
        self.launch()?;
        self.wait_health()?;
        self.finish()?;
        say("ready");
        Ok(())
    }
}
