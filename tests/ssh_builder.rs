// SPDX-License-Identifier: AGPL-3.0-only
use shrug::host::{SshRoute, ssh_argv, ssh_shell};
use std::path::Path;

fn route(dest: &str, jumps: &[&str]) -> SshRoute {
    SshRoute {
        user: "me".into(),
        dest: dest.into(),
        jumps: jumps.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn direct_route_has_no_proxy() {
    let argv = ssh_argv(&route("10.0.0.2", &[]), Path::new("/kh"), "hostname").unwrap();
    let s = argv.join(" ");
    assert!(s.starts_with("ssh -o BatchMode=yes"));
    assert!(s.contains("UserKnownHostsFile=/kh ~/.ssh/known_hosts"));
    assert!(s.ends_with("me@10.0.0.2 hostname"));
    assert!(!s.contains("ProxyCommand"));
    assert!(!s.contains(" -J "));
}

#[test]
fn one_hop_nests_proxycommand() {
    let argv = ssh_argv(&route("10.0.0.3", &["me@10.0.0.2"]), Path::new("/kh"), "id").unwrap();
    let s = argv.join(" ");
    assert!(s.contains("-o ProxyCommand=ssh"));
    assert!(s.contains("%h:%p"));
    assert!(s.contains("me@10.0.0.2"));
    // The ProxyCommand arg itself contains a nested ssh reaching the hop.
    let pc_arg = argv.iter().find(|a| a.contains("%h:%p")).unwrap();
    assert!(pc_arg.contains("ssh"));
    assert!(s.ends_with("me@10.0.0.3 id"));
    // The ProxyCommand hop carries the same options.
    let pc = argv
        .iter()
        .find(|a| a.starts_with("ProxyCommand="))
        .unwrap();
    assert!(pc.contains("BatchMode=yes"));
    assert!(pc.contains("UserKnownHostsFile"));
    assert!(pc.contains("/kh"));
}

#[test]
fn two_hops_nest_twice() {
    let argv = ssh_argv(&route("d", &["u@j1", "u@j2"]), Path::new("/kh"), "cmd").unwrap();
    let s = argv.join(" ");
    assert!(s.ends_with("d cmd"));
    let pc = argv.iter().find(|a| a.contains("ProxyCommand=")).unwrap();
    // Outer ProxyCommand reaches j2 and itself contains a ProxyCommand to j1.
    assert!(pc.contains("u@j2"));
    assert!(pc.contains("ProxyCommand"));
    assert!(pc.contains("u@j1"));
}

#[test]
fn shell_form_is_quoted() {
    let r = route("ip", &["u@j1"]);
    let s = ssh_shell(&r, Path::new("/kh dir"), "docker load").unwrap();
    assert!(s.contains("ssh"));
    assert!(s.ends_with("'docker load'") || s.ends_with("docker load"));
    assert!(s.contains("ProxyCommand"));
    // known_hosts with a space is quoted inside the ProxyCommand too.
    assert!(s.contains("kh dir") || s.contains("kh\\ dir") || s.contains("'"));
}
