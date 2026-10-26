// SPDX-License-Identifier: AGPL-3.0-only
//! Cluster cabling: edges, shapes, rank order and per-rank fabric values.
use crate::discover::Discovery;
use crate::probe::Ipv4Net;
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

#[derive(Debug, Clone)]
pub struct PortEnd {
    pub host: usize,
    pub hca: String,
    pub netdev: String,
    pub net: Ipv4Net,
}

#[derive(Debug, Clone)]
pub struct Rail {
    pub ends: [PortEnd; 2],
}

impl Rail {
    pub fn subnet(&self) -> Ipv4Addr {
        self.ends[0].net.network()
    }
    pub fn end(&self, host: usize) -> &PortEnd {
        self.ends.iter().find(|e| e.host == host).expect("rail end")
    }
}

/// All rails on one cable between two hosts, sorted by subnet ascending.
#[derive(Debug, Clone)]
pub struct Edge {
    pub hosts: (usize, usize),
    pub rails: Vec<Rail>,
}

impl Edge {
    pub fn other(&self, host: usize) -> usize {
        if self.hosts.0 == host {
            self.hosts.1
        } else {
            self.hosts.0
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Shape {
    Single,
    Pair,
    Chain,
    Ring,
}

impl Shape {
    pub fn name(&self) -> &'static str {
        match self {
            Shape::Single => "single",
            Shape::Pair => "pair",
            Shape::Chain => "chain",
            Shape::Ring => "ring",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Cluster {
    pub shape: Shape,
    /// host index per rank; ranks[0] is always host 0 (the local machine).
    pub ranks: Vec<usize>,
    /// how many hosts were found (may exceed ranks.len() when a subset is used).
    pub found: usize,
}

/// Cabled ports: same IPv4 network, different addresses (SparkGLM's rule).
pub fn edges(d: &Discovery) -> Vec<Edge> {
    let mut map: BTreeMap<(usize, usize), Vec<Rail>> = BTreeMap::new();
    for (i, a) in d.hosts.iter().enumerate() {
        for (j, b) in d.hosts.iter().enumerate().skip(i + 1) {
            for pa in a.probe.fabric.iter().filter(|p| p.active) {
                for pb in b.probe.fabric.iter().filter(|p| p.active) {
                    for na in &pa.addrs {
                        for nb in &pb.addrs {
                            if na.network() == nb.network() && na.ip != nb.ip {
                                map.entry((i, j)).or_default().push(Rail {
                                    ends: [
                                        PortEnd {
                                            host: i,
                                            hca: pa.hca.clone(),
                                            netdev: pa.netdev.clone(),
                                            net: *na,
                                        },
                                        PortEnd {
                                            host: j,
                                            hca: pb.hca.clone(),
                                            netdev: pb.netdev.clone(),
                                            net: *nb,
                                        },
                                    ],
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    map.into_iter()
        .map(|(hosts, mut rails)| {
            rails.sort_by_key(|r| r.subnet());
            Edge { hosts, rails }
        })
        .collect()
}

/// Neighbors of `host`, ordered by the edge's lowest subnet.
fn neighbors(edges: &[Edge], host: usize) -> Vec<&Edge> {
    let mut out: Vec<&Edge> = edges
        .iter()
        .filter(|e| e.hosts.0 == host || e.hosts.1 == host)
        .collect();
    out.sort_by_key(|e| {
        e.rails
            .first()
            .map(|r| r.subnet())
            .unwrap_or(Ipv4Addr::UNSPECIFIED)
    });
    out
}

/// Shape and rank order from the host graph. Blockers come back as `Err`.
pub fn cluster(d: &Discovery) -> Result<Cluster, Vec<String>> {
    let found = d.hosts.len();
    let edges = edges(d);
    match found {
        0 => Err(vec!["local probe failed".into()]),
        1 => Ok(Cluster {
            shape: Shape::Single,
            ranks: vec![0],
            found,
        }),
        2 => Ok(Cluster {
            shape: Shape::Pair,
            ranks: vec![0, 1],
            found,
        }),
        3 => {
            // Three Sparks run the two-Spark release on rank 0 plus its
            // neighbor on the lowest-subnet edge.
            let ns = neighbors(&edges, 0);
            let Some(e) = ns.first() else {
                return Err(vec!["three hosts found but no cable to rank 0".into()]);
            };
            let other = e.other(0);
            Ok(Cluster {
                shape: Shape::Pair,
                ranks: vec![0, other],
                found,
            })
        }
        4 => {
            let degrees: Vec<usize> = (0..4).map(|h| neighbors(&edges, h).len()).collect();
            if degrees.iter().all(|&deg| deg == 2) {
                // Ring: rank 1 is the lowest-subnet neighbor, then walk around.
                let mut ranks = vec![0usize];
                let first = neighbors(&edges, 0)[0].other(0);
                ranks.push(first);
                while ranks.len() < 4 {
                    let prev = ranks[ranks.len() - 2];
                    let cur = *ranks.last().unwrap();
                    let next = neighbors(&edges, cur)
                        .iter()
                        .map(|e| e.other(cur))
                        .find(|n| *n != prev)
                        .expect("degree 2");
                    if next == 0 {
                        break;
                    }
                    ranks.push(next);
                }
                if ranks.len() != 4 {
                    return Err(vec!["cabling does not form one 4-Spark ring".into()]);
                }
                Ok(Cluster {
                    shape: Shape::Ring,
                    ranks,
                    found,
                })
            } else if degrees.iter().filter(|&&deg| deg == 1).count() == 2
                && degrees.iter().all(|&deg| deg == 1 || deg == 2)
            {
                // Chain: rank 0 (the local host) must be an end.
                if degrees[0] != 1 {
                    let ends: Vec<String> = (0..4)
                        .filter(|&h| degrees[h] == 1)
                        .map(|h| {
                            d.hosts[h]
                                .probe
                                .lan_ip
                                .map(|ip| ip.to_string())
                                .unwrap_or_else(|| d.hosts[h].probe.hostname.clone())
                        })
                        .collect();
                    return Err(vec![format!(
                        "rank 0 is in the middle of the chain; run shrug on one end of the chain ({})",
                        ends.join(" or ")
                    )]);
                }
                let mut ranks = vec![0usize];
                while ranks.len() < 4 {
                    let prev = (ranks.len() >= 2).then(|| ranks[ranks.len() - 2]);
                    let cur = *ranks.last().unwrap();
                    let next = neighbors(&edges, cur)
                        .iter()
                        .map(|e| e.other(cur))
                        .find(|n| Some(*n) != prev)
                        .expect("path end reached early");
                    ranks.push(next);
                }
                Ok(Cluster {
                    shape: Shape::Chain,
                    ranks,
                    found,
                })
            } else {
                Err(vec![format!(
                    "unrecognized 4-host cabling (degrees {degrees:?}); expected a ring or a chain"
                )])
            }
        }
        _ => Err(vec![format!(
            "{found} Sparks found; at most 4 are supported"
        )]),
    }
}

/// The edge between two hosts, if any.
pub fn edge_between(edges: &[Edge], a: usize, b: usize) -> Option<&Edge> {
    edges
        .iter()
        .find(|e| (e.hosts.0 == a && e.hosts.1 == b) || (e.hosts.0 == b && e.hosts.1 == a))
}

/// Per-rank values substituted into the catalog arg templates.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RankFabric {
    pub topology: String,
    pub leader_address: String,
    pub fabric_interface: String,
    pub fabric_hca: String,
    pub left_hca: String,
    pub right_hca: String,
}

fn hca_list(edge: &Edge, host: usize) -> String {
    edge.rails
        .iter()
        .map(|r| r.end(host).hca.clone())
        .collect::<Vec<_>>()
        .join(",")
}

/// Compute per-rank fabric values.
/// `ping_from(host, ip)` and `route_dev(host, ip)` perform the TP4 checks.
pub fn rank_fabrics(
    d: &Discovery,
    c: &Cluster,
    ranks: &[usize],
    ping_from: &dyn Fn(usize, Ipv4Addr) -> bool,
    route_dev: &dyn Fn(usize, Ipv4Addr) -> Option<String>,
) -> Result<Vec<RankFabric>, Vec<String>> {
    let edges = edges(d);
    let mut blockers = Vec::new();
    let n = ranks.len();
    let topology = match n {
        1 => "single",
        2 => "pair",
        _ => c.shape.name(),
    };
    let mut out = vec![
        RankFabric {
            topology: topology.into(),
            ..Default::default()
        };
        n
    ];

    match n {
        1 => {}
        2 => {
            let e = edge_between(&edges, ranks[0], ranks[1])
                .ok_or_else(|| vec!["no cable between rank 0 and rank 1".to_string()])?;
            let rail = &e.rails[0];
            for (ri, &h) in ranks.iter().enumerate() {
                let end = rail.end(h);
                out[ri].fabric_hca = end.hca.clone();
                out[ri].fabric_interface = end.netdev.clone();
            }
            out[0].leader_address = rail.end(ranks[0]).net.ip.to_string();
            out[1].leader_address = out[0].leader_address.clone();
        }
        _ => {
            // TP4: left/right HCA lists per edge, in subnet order.
            for (ri, &h) in ranks.iter().enumerate() {
                let prev = if c.shape == Shape::Ring && ri == 0 {
                    ranks.last().copied()
                } else {
                    ri.checked_sub(1).and_then(|i| ranks.get(i)).copied()
                };
                if let Some(prev) = prev
                    && let Some(e) = edge_between(&edges, prev, h)
                {
                    out[ri].left_hca = hca_list(e, h);
                }
                let next = if c.shape == Shape::Ring && ri == n - 1 {
                    Some(ranks[0])
                } else {
                    ranks.get(ri + 1).copied()
                };
                if let Some(nh) = next
                    && let Some(e) = edge_between(&edges, h, nh)
                {
                    out[ri].right_hca = hca_list(e, h);
                }
            }
            // leader_address: first of rank 0's addresses (toward rank 1 in
            // subnet order, then its other fabric addresses, then LAN IP)
            // that every other rank pings.
            let r0 = ranks[0];
            let mut candidates: Vec<Ipv4Addr> = Vec::new();
            if let Some(e) = edge_between(&edges, r0, ranks[1]) {
                candidates.extend(e.rails.iter().map(|r| r.end(r0).net.ip));
            }
            let mut rest: Vec<Ipv4Addr> = d.hosts[r0]
                .probe
                .fabric
                .iter()
                .filter(|p| p.active)
                .flat_map(|p| p.addrs.iter().map(|a| (a.network(), a.ip)))
                .collect::<BTreeMap<_, _>>()
                .into_values()
                .collect();
            rest.retain(|ip| !candidates.contains(ip));
            candidates.extend(rest);
            if let Some(lan) = d.hosts[r0].probe.lan_ip {
                candidates.push(lan);
            }
            let leader = candidates
                .iter()
                .copied()
                .find(|ip| ranks[1..].iter().all(|&h| ping_from(h, *ip)));
            let Some(leader) = leader else {
                blockers.push(format!(
                    "no rank 0 address reachable from every rank (tried {})",
                    candidates
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                return Err(blockers);
            };
            for f in out.iter_mut() {
                f.leader_address = leader.to_string();
            }
            // fabric_interface: netdev holding the address on rank 0, else
            // the route's dev on the other ranks.
            for (ri, &h) in ranks.iter().enumerate() {
                if ri == 0 {
                    out[ri].fabric_interface = d.hosts[h]
                        .probe
                        .fabric
                        .iter()
                        .flat_map(|p| p.addrs.iter().map(move |a| (p, a)))
                        .find(|(_, a)| a.ip == leader)
                        .map(|(p, _)| p.netdev.clone())
                        .unwrap_or_default();
                } else {
                    match route_dev(h, leader) {
                        Some(dev) => out[ri].fabric_interface = dev,
                        None => blockers.push(format!("rank {ri} has no route to leader {leader}")),
                    }
                }
            }
        }
    }
    if blockers.is_empty() {
        Ok(out)
    } else {
        Err(blockers)
    }
}
