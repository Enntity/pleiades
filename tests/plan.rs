// SPDX-License-Identifier: AGPL-3.0-only
mod common;
use common::*;
use shrug::catalog;
use shrug::plan::{self, Options};
use shrug::state::Config;

fn opts<'a>(cfg: &'a Config, model: Option<&'a str>) -> Options<'a> {
    Options {
        model,
        accept_noncommercial: true,
        no_clock_cap: false,
        config: cfg,
    }
}

fn build(d: &shrug::discover::Discovery, o: &Options, checks: &dyn plan::Checks) -> plan::Plan {
    let cat = catalog::embedded().unwrap();
    plan::build(d, &cat, "embedded", o, checks)
}

#[test]
fn plan_ring_default_is_glm_4x() {
    let d = ring();
    let cfg = Config::default();
    let p = build(&d, &opts(&cfg, None), &FakeChecks::default());
    assert_eq!(p.release_id.as_deref(), Some("glm-5.3-flash-4x"));
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    assert_eq!(p.ranks.len(), 4);
    assert_eq!(p.ranks[0].args.join(" "), {
        let a = &p.ranks[0].args;
        assert_eq!(a[0..4], ["--world-size", "4", "--topology", "ring"]);
        assert_eq!(a[4..6], ["--rank", "0"]);
        assert_eq!(a[6..8], ["--leader-address", "10.0.12.2"]);
        assert_eq!(a[8..10], ["--fabric-interface", "enp1s0f0np0"]);
        let i = a.iter().position(|x| x == "--left-hca").unwrap();
        assert_eq!(a[i + 1], "rocep1s0f1,roceP2p1s0f1");
        let j = a.iter().position(|x| x == "--right-hca").unwrap();
        assert_eq!(a[j + 1], "rocep1s0f0,roceP2p1s0f0");
        a.join(" ")
    });
}

#[test]
fn golden_args_every_release() {
    let cfg = Config::default();
    // 1x
    let d = single();
    let p = build(
        &d,
        &opts(&cfg, Some("qwen3.8-27b-1x")),
        &FakeChecks::default(),
    );
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    let a = &p.ranks[0].args;
    assert_eq!(
        a[0..10],
        [
            "--image",
            "ghcr.io/atlas-inf/atlas-gb10@sha256:0a75306346baf0dcbe56a2aae677247b62ce468db031e5b06cdfa71cb0b156f4",
            "--model-root",
            "/home/spark/.shrug/models",
            "--checkpoint",
            "nvidia--Qwen3.8-27B-NVFP4",
            "--cuda-cache",
            "/home/spark/.cache/atlas-cuda",
            "--name",
            "shrug-rank0"
        ]
    );
    let sep = a.iter().position(|x| x == "--").unwrap();
    assert_eq!(
        &a[sep + 1..sep + 5],
        ["--host", "127.0.0.1", "--port", "8893"]
    );

    // 2x pair, both releases
    let d = pair();
    for (id, profile) in [
        ("glm-5.3-flash-2x", "4x1m"),
        ("qwen3.8-flash-next-2x", "8x262k"),
    ] {
        let p = build(&d, &opts(&cfg, Some(id)), &FakeChecks::default());
        assert!(p.blockers.is_empty(), "{id}: {:?}", p.blockers);
        assert_eq!(p.ranks.len(), 2);
        let a = &p.ranks[0].args;
        assert_eq!(a[0..2], ["--rank", "0"]);
        assert!(a.windows(2).any(|w| w == ["--leader-address", "10.0.12.2"]));
        assert!(a.windows(2).any(|w| w == ["--fabric-hca", "rocep1s0f0"]));
        assert!(
            a.windows(2)
                .any(|w| w == ["--fabric-interface", "enp1s0f0np0"])
        );
        assert!(a.windows(2).any(|w| w == ["--profile", profile]));
        assert!(a.windows(2).any(|w| w == ["--name", "shrug-rank0"]));
        let b = &p.ranks[1].args;
        assert!(b.windows(2).any(|w| w == ["--rank", "1"]));
        assert!(b.windows(2).any(|w| w == ["--fabric-hca", "rocep1s0f0"]));
    }

    // 4x chain: rank 0 has no left, rank 3 no right.
    let d = chain();
    let p = build(
        &d,
        &opts(&cfg, Some("glm-5.3-flash-4x")),
        &FakeChecks::default(),
    );
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    let a0 = &p.ranks[0].args;
    assert!(!a0.contains(&"--left-hca".to_string()));
    assert!(a0.contains(&"--right-hca".to_string()));
    assert!(a0.windows(2).any(|w| w == ["--topology", "chain"]));
    let a3 = &p.ranks[3].args;
    assert!(a3.contains(&"--left-hca".to_string()));
    assert!(!a3.contains(&"--right-hca".to_string()));
}

#[test]
fn skip_logic_marks_done_steps() {
    let d = pair();
    let cfg = Config::default();
    let checks = FakeChecks {
        image: true,
        weight: true,
        route_ok: true,
    };
    let p = build(&d, &opts(&cfg, None), &checks);
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    assert!(p.steps.iter().all(|s| {
        s.id.starts_with("launch")
            || s.id == "wait-health"
            || s.id == "verify"
            || s.id == "gateway"
            || s.status == "done"
    }));
    assert!(
        p.steps
            .iter()
            .any(|s| s.id == "image-rank0" && s.status == "done")
    );
}

#[test]
fn noncommercial_weight_blocks_until_accepted() {
    let d = pair();
    let cfg = Config::default();
    let o = Options {
        model: Some("glm-5.3-flash-2x"),
        accept_noncommercial: false,
        no_clock_cap: false,
        config: &cfg,
    };
    let p = build(&d, &o, &FakeChecks::default());
    assert!(p.blockers.iter().any(|b| b.contains("non-commercial")));

    let mut cfg = Config::default();
    cfg.accept(
        "incoai--GLM-5.3-Flash-DFlash2",
        "7d74cdd881ed7e32c31175984a67823127b66cfe",
    );
    let o = Options { config: &cfg, ..o };
    let p = build(&d, &o, &FakeChecks::default());
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
}

#[test]
fn blockers_from_probe_and_chain_middle() {
    let mut d = pair();
    d.hosts[1].probe.gpu_name = Some("NVIDIA L4".into());
    d.hosts[1].probe.containers.push(shrug::probe::Container {
        name: "other-workload".into(),
        image: "img".into(),
        status: "running".into(),
        holds_gpu: true,
    });
    d.hosts[1].probe.gpu_procs.push(shrug::probe::GpuProc {
        pid: 42,
        name: "python".into(),
        used_memory: "1000 MiB".into(),
    });
    let cfg = Config::default();
    let p = build(&d, &opts(&cfg, None), &FakeChecks::default());
    let all = p.blockers.join("\n");
    assert!(all.contains("no GB10"), "{all}");
    assert!(all.contains("other-workload"), "{all}");
    assert!(all.contains("python"), "{all}");

    let d = middle_chain();
    let p = build(&d, &opts(&cfg, None), &FakeChecks::default());
    assert!(p.blockers.iter().any(|b| b.contains("middle of the chain")));
}

#[test]
fn fewer_sparks_than_release_needs_blocks() {
    let d = pair();
    let cfg = Config::default();
    let p = build(
        &d,
        &opts(&cfg, Some("glm-5.3-flash-4x")),
        &FakeChecks::default(),
    );
    assert!(p.blockers.iter().any(|b| b.contains("needs 4 Sparks")));
}

#[test]
fn unknown_placeholder_is_error() {
    let vars = Default::default();
    assert!(
        shrug::render::render_args(&[shrug::catalog::ArgSpec::Lit("{bogus}".into())], &vars)
            .is_err()
    );
}

#[test]
fn catalog_sign_verify_roundtrip() {
    use base64::Engine as _;
    let mut secret = [7u8; 32];
    std::io::Read::read_exact(
        &mut std::fs::File::open("/dev/urandom").unwrap(),
        &mut secret,
    )
    .unwrap();
    let b64 = base64::engine::general_purpose::STANDARD;
    let sk = ed25519_dalek::SigningKey::from_bytes(&secret);
    let pk = b64.encode(sk.verifying_key().to_bytes());
    let sig = catalog::sign(b"doc", &b64.encode(secret)).unwrap();
    catalog::verify_with(b"doc", sig.as_bytes(), &[&pk]).unwrap();
    assert!(catalog::verify_with(b"other", sig.as_bytes(), &[&pk]).is_err());
}
