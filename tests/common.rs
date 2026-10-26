// SPDX-License-Identifier: AGPL-3.0-only
//! Shared test helpers: sanitized fixtures and fake checks.
use shrug::discover::{Discovery, Found};
use shrug::host::SshRoute;
use shrug::plan::Checks;

#[allow(dead_code)]
use shrug::probe;
use std::net::Ipv4Addr;
use std::path::PathBuf;

#[allow(dead_code)]
pub fn fixture(name: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(name);
    std::fs::read_to_string(p).unwrap()
}

#[allow(dead_code)]
pub fn found(name: &str, dest: &str) -> Found {
    Found {
        probe: probe::parse(&fixture(name)).unwrap(),
        route: SshRoute {
            user: "spark".into(),
            dest: dest.into(),
            jumps: vec![],
        },
    }
}

/// The four sanitized Sparks: ring a-b-c-d-a, host 0 = spark-a.
#[allow(dead_code)]
pub fn ring() -> Discovery {
    Discovery {
        hosts: vec![
            found("spark-a.txt", "local"),
            found("spark-b.txt", "10.0.12.1"),
            found("spark-c.txt", "10.0.14.2"),
            found("spark-d.txt", "10.0.16.2"),
        ],
        failures: vec![],
    }
}

/// The same four without the d-a cable: chain a-b-c-d.
#[allow(dead_code)]
pub fn chain() -> Discovery {
    let mut d = ring();
    // a drops its d-a addresses (10.0.18/19), d drops 10.0.18/19 too.
    for idx in [0usize, 3usize] {
        for port in &mut d.hosts[idx].probe.fabric {
            port.addrs
                .retain(|a| a.network().octets()[2] != 18 && a.network().octets()[2] != 19);
        }
    }
    d
}

#[allow(dead_code)]
pub fn pair() -> Discovery {
    let mut d = ring();
    d.hosts.truncate(2);
    d
}

#[allow(dead_code)]
pub fn single() -> Discovery {
    let mut d = ring();
    d.hosts.truncate(1);
    d
}

/// Three hosts found: a-b-c (uses 2).
#[allow(dead_code)]
pub fn three() -> Discovery {
    let mut d = ring();
    d.hosts.truncate(3);
    d
}

/// Middle-of-chain: local host is b (host 0), so ends are a and d.
#[allow(dead_code)]
pub fn middle_chain() -> Discovery {
    let mut d = chain();
    // Put b first; ranks[0]=host 0 must be an end — b is not.
    d.hosts.swap(0, 1);
    d
}

/// Checks that say everything is reachable and nothing is installed.
pub struct FakeChecks {
    pub image: bool,
    pub weight: bool,
    pub route_ok: bool,
}

impl Default for FakeChecks {
    fn default() -> Self {
        Self {
            image: false,
            weight: false,
            route_ok: true,
        }
    }
}

impl Checks for FakeChecks {
    fn ping(&self, _h: usize, _ip: Ipv4Addr) -> bool {
        true
    }
    fn route_dev(&self, _h: usize, ip: Ipv4Addr) -> Option<String> {
        if self.route_ok {
            // The netdev facing the given subnet's rail.
            Some(
                match ip.octets()[2] {
                    12 => "enp1s0f0np0",
                    13 => "enP2p1s0f0np0",
                    _ => "enp1s0f0np0",
                }
                .to_string(),
            )
        } else {
            None
        }
    }
    fn image_present(&self, _h: usize, _image: &str) -> bool {
        self.image
    }
    fn weight_present(&self, _h: usize, _dir: &str, _rev: &str) -> bool {
        self.weight
    }
}
