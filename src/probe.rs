// SPDX-License-Identifier: AGPL-3.0-only
//! The embedded probe script and the parser for its output.
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::net::Ipv4Addr;
use std::str::FromStr;

pub const PROBE_SCRIPT: &str = include_str!("../assets/probe.sh");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Ipv4Net {
    pub ip: Ipv4Addr,
    pub prefix: u8,
}

impl Ipv4Net {
    pub fn network(&self) -> Ipv4Addr {
        let mask = if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - self.prefix)
        };
        Ipv4Addr::from(u32::from(self.ip) & mask)
    }
}

impl FromStr for Ipv4Net {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let (ip, prefix) = s.split_once('/').with_context(|| format!("bad CIDR {s}"))?;
        Ok(Self {
            ip: ip.parse().with_context(|| format!("bad IPv4 {s}"))?,
            prefix: prefix.parse().with_context(|| format!("bad prefix {s}"))?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FabricPort {
    pub hca: String,
    pub netdev: String,
    pub active: bool,
    pub addrs: Vec<Ipv4Net>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Container {
    pub name: String,
    pub image: String,
    pub status: String,
    pub holds_gpu: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuProc {
    pub pid: u32,
    pub name: String,
    pub used_memory: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Probe {
    pub hostname: String,
    pub machine_id: String,
    pub arch: String,
    pub user: String,
    pub home: String,
    pub gpu_name: Option<String>,
    pub driver_version: Option<String>,
    pub mem_total_kib: u64,
    pub mem_available_kib: u64,
    pub fabric: Vec<FabricPort>,
    pub lan_dev: Option<String>,
    pub lan_ip: Option<Ipv4Addr>,
    pub docker_server: Option<String>,
    pub docker_ok: bool,
    pub runtimes: Vec<String>,
    pub nvidia_ctk: bool,
    pub containers: Vec<Container>,
    pub gpu_procs: Vec<GpuProc>,
    pub clock_cap: String,
    pub home_free_kib: u64,
    pub tools: Vec<String>,
}

impl Probe {
    pub fn is_gb10(&self) -> bool {
        self.gpu_name.as_deref().is_some_and(|n| n.contains("GB10"))
    }
    pub fn has_nvidia_runtime(&self) -> bool {
        self.nvidia_ctk || self.runtimes.iter().any(|r| r == "nvidia")
    }
    pub fn has_tool(&self, tool: &str) -> bool {
        self.tools.iter().any(|t| t == tool)
    }
    /// Candidate peer IPv4 addresses for each active addressed fabric port.
    pub fn peer_candidates(&self) -> Vec<Ipv4Addr> {
        let mut out = Vec::new();
        for port in self.fabric.iter().filter(|p| p.active) {
            for net in &port.addrs {
                let peer = match net.prefix {
                    30 | 31 => {
                        let base = u32::from(net.network());
                        let peer = if u32::from(net.ip) == base + 1 {
                            base + 2
                        } else {
                            base + 1
                        };
                        Some(Ipv4Addr::from(peer))
                    }
                    24 => match net.ip.octets()[3] {
                        1 => Some(Ipv4Addr::new(
                            net.ip.octets()[0],
                            net.ip.octets()[1],
                            net.ip.octets()[2],
                            2,
                        )),
                        2 => Some(Ipv4Addr::new(
                            net.ip.octets()[0],
                            net.ip.octets()[1],
                            net.ip.octets()[2],
                            1,
                        )),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(p) = peer
                    && !out.contains(&p)
                {
                    out.push(p);
                }
            }
        }
        out
    }
}

/// Parse the probe output (the format emitted by `PROBE_SCRIPT`).
pub fn parse(text: &str) -> Result<Probe> {
    let mut p = Probe::default();
    let mut section = "";
    let mut saw_version = false;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix("## ") {
            section = name;
            if name == "shrug.probe/v1" {
                saw_version = true;
            }
            continue;
        }
        match section {
            "host" | "mem" | "lan" | "docker" | "disk" | "tools" => {
                let Some((k, v)) = line.split_once('=') else {
                    continue;
                };
                match (section, k) {
                    ("host", "hostname") => p.hostname = v.into(),
                    ("host", "machine_id") => p.machine_id = v.into(),
                    ("host", "arch") => p.arch = v.into(),
                    ("host", "user") => p.user = v.into(),
                    ("host", "home") => p.home = v.into(),
                    ("mem", "memtotal") => p.mem_total_kib = v.parse().unwrap_or(0),
                    ("mem", "memavailable") => p.mem_available_kib = v.parse().unwrap_or(0),
                    ("lan", "lan_dev") if !v.is_empty() => p.lan_dev = Some(v.into()),
                    ("lan", "lan_ip") => p.lan_ip = v.parse().ok(),
                    ("docker", "docker_server") if !v.is_empty() => {
                        p.docker_server = Some(v.into())
                    }
                    ("docker", "docker_ok") => p.docker_ok = v == "1",
                    ("docker", "runtimes") => {
                        p.runtimes = v
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect()
                    }
                    ("docker", "nvidia_ctk") => p.nvidia_ctk = v == "1",
                    ("disk", "home_free_kib") => p.home_free_kib = v.parse().unwrap_or(0),
                    ("tools", "tools") => {
                        p.tools = v
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect()
                    }
                    _ => {}
                }
            }
            "gpu" => {
                if line.starts_with("NVIDIA") || line.contains("GB10") || line.contains("GPU") {
                    let mut cols = line.splitn(2, ',');
                    p.gpu_name = cols.next().map(str::trim).map(str::to_string);
                    p.driver_version = cols.next().map(str::trim).map(str::to_string);
                }
            }
            "fabric" => {
                let mut parts = line.split_whitespace();
                let (Some(hca), Some(netdev)) = (parts.next(), parts.next()) else {
                    bail!("bad fabric line: {line}");
                };
                let mut port = FabricPort {
                    hca: hca.into(),
                    netdev: netdev.into(),
                    active: false,
                    addrs: Vec::new(),
                };
                for part in parts {
                    if let Some(s) = part.strip_prefix("state=") {
                        port.active = s.eq_ignore_ascii_case("active");
                    } else if let Some(a) = part.strip_prefix("addrs=") {
                        port.addrs = a
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(|s| s.parse::<Ipv4Net>())
                            .collect::<Result<_>>()?;
                    }
                }
                p.fabric.push(port);
            }
            "containers" => {
                let cols: Vec<&str> = line.split('|').collect();
                if cols.len() >= 3 {
                    let runtime = cols.get(3).copied().unwrap_or_default();
                    let devreq = cols.get(4).copied().unwrap_or_default();
                    p.containers.push(Container {
                        name: cols[0].into(),
                        image: cols[1].into(),
                        status: cols[2].into(),
                        holds_gpu: runtime == "nvidia"
                            || devreq.contains("gpu")
                            || devreq.contains("GPU"),
                    });
                }
            }
            "gpu-procs" => {
                let cols: Vec<&str> = line.split('|').collect();
                if cols.len() >= 2
                    && let Ok(pid) = cols[0].trim().parse()
                {
                    p.gpu_procs.push(GpuProc {
                        pid,
                        name: cols[1].trim().into(),
                        used_memory: cols
                            .get(2)
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default(),
                    });
                }
            }
            "clockcap" => p.clock_cap = line.to_string(),
            _ => {}
        }
    }
    if !saw_version {
        bail!("not shrug.probe/v1 output");
    }
    if p.machine_id.is_empty() {
        bail!("probe missing machine_id");
    }
    Ok(p)
}
