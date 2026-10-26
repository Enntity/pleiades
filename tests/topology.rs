// SPDX-License-Identifier: AGPL-3.0-only
mod common;
use common::*;
use shrug::plan::Checks as _;
use shrug::topo::{self, Shape};

fn fabrics(d: &shrug::discover::Discovery) -> Vec<topo::RankFabric> {
    let c = topo::cluster(d).unwrap();
    topo::rank_fabrics(d, &c, &c.ranks, &|_, _| true, &|h, ip| {
        FakeChecks::default().route_dev(h, ip)
    })
    .unwrap()
}

#[test]
fn ring_order_and_golden_hcas() {
    let d = ring();
    let c = topo::cluster(&d).unwrap();
    assert_eq!(c.shape, Shape::Ring);
    assert_eq!(c.ranks, vec![0, 1, 2, 3]);
    let f = fabrics(&d);
    // Golden: matches LLooM's chain config on the same cabling.
    assert_eq!(f[0].right_hca, "rocep1s0f0,roceP2p1s0f0");
    assert_eq!(f[0].left_hca, "rocep1s0f1,roceP2p1s0f1");
    assert_eq!(f[1].left_hca, "rocep1s0f0,roceP2p1s0f0");
    assert_eq!(f[1].right_hca, "rocep1s0f1,roceP2p1s0f1");
    assert_eq!(f[2].left_hca, "rocep1s0f0,roceP2p1s0f0");
    assert_eq!(f[2].right_hca, "rocep1s0f1,roceP2p1s0f1");
    assert_eq!(f[3].left_hca, "rocep1s0f1,roceP2p1s0f1");
    assert_eq!(f[3].right_hca, "rocep1s0f0,roceP2p1s0f0");
    // leader: rank 0's lowest-subnet fabric address toward rank 1.
    assert_eq!(f[0].leader_address, "10.0.12.2");
    assert_eq!(f[0].fabric_interface, "enp1s0f0np0");
    assert!(f.iter().all(|x| x.topology == "ring"));
}

#[test]
fn chain_order_and_ends() {
    let d = chain();
    let c = topo::cluster(&d).unwrap();
    assert_eq!(c.shape, Shape::Chain);
    assert_eq!(c.ranks, vec![0, 1, 2, 3]);
    let f = fabrics(&d);
    assert!(f[0].left_hca.is_empty(), "chain rank 0 has no left");
    assert_eq!(f[0].right_hca, "rocep1s0f0,roceP2p1s0f0");
    assert!(f[3].right_hca.is_empty(), "chain rank 3 has no right");
    assert_eq!(f[3].left_hca, "rocep1s0f1,roceP2p1s0f1");
    assert!(f.iter().all(|x| x.topology == "chain"));
}

#[test]
fn chain_middle_is_blocker() {
    let d = middle_chain();
    let errs = topo::cluster(&d).unwrap_err();
    let msg = errs.join(" ");
    assert!(msg.contains("middle of the chain"), "{msg}");
    assert!(
        msg.contains("192.0.2.11") && msg.contains("192.0.2.14"),
        "{msg}"
    );
}

#[test]
fn pair_uses_lowest_subnet_rail() {
    let d = pair();
    let c = topo::cluster(&d).unwrap();
    assert_eq!(c.shape, Shape::Pair);
    let f = fabrics(&d);
    assert_eq!(f[0].leader_address, "10.0.12.2");
    assert_eq!(f[0].fabric_hca, "rocep1s0f0");
    assert_eq!(f[0].fabric_interface, "enp1s0f0np0");
    assert_eq!(f[1].fabric_hca, "rocep1s0f0");
    assert_eq!(f[1].fabric_interface, "enp1s0f0np0");
    assert!(f[0].left_hca.is_empty() && f[0].right_hca.is_empty());
}

#[test]
fn single_and_three_hosts() {
    let d = single();
    let c = topo::cluster(&d).unwrap();
    assert_eq!(c.shape, Shape::Single);
    let d = three();
    let c = topo::cluster(&d).unwrap();
    assert_eq!(c.shape, Shape::Pair);
    assert_eq!(c.found, 3);
    assert_eq!(c.ranks, vec![0, 1]);
}

#[test]
fn peer_candidates_24() {
    let d = ring();
    let cands = d.hosts[0].probe.peer_candidates();
    // a's fabric addrs are all ".2" on /24 → peers are the ".1"s.
    assert_eq!(cands.len(), 4);
    assert!(cands.contains(&"10.0.12.1".parse().unwrap()));
    assert!(cands.contains(&"10.0.18.2".parse().unwrap()));
    assert!(cands.contains(&"10.0.19.2".parse().unwrap()));
}
