// SPDX-License-Identifier: AGPL-3.0-only
//! Hidden dev commands: build, sign, verify, keygen.
use crate::catalog;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub fn build(dir: &Path, check: bool) -> Result<i32> {
    let bytes = catalog::build(dir)?;
    let out = dir.join("catalog/catalog.json");
    if check {
        match std::fs::read(&out) {
            Ok(cur) if cur == bytes => {
                println!("catalog/catalog.json is up to date");
                Ok(0)
            }
            _ => {
                eprintln!("catalog/catalog.json is stale; run `shrug catalog build`");
                Ok(1)
            }
        }
    } else {
        std::fs::write(&out, &bytes)?;
        println!("wrote {}", out.display());
        Ok(0)
    }
}

pub fn sign(dir: &Path, key_env: &str) -> Result<i32> {
    let secret = std::env::var(key_env).with_context(|| format!("env {key_env} unset"))?;
    let doc = std::fs::read(dir.join("catalog/catalog.json")).context("read catalog.json")?;
    let sig = catalog::sign(&doc, &secret)?;
    let out = dir.join("catalog/catalog.json.sig");
    std::fs::write(&out, format!("{sig}\n"))?;
    println!("{sig}");
    eprintln!("wrote {}", out.display());
    Ok(0)
}

pub fn verify(file: &PathBuf, sig: &str) -> Result<i32> {
    let doc = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
    let sig_bytes = match std::fs::read(sig) {
        Ok(b) => b,
        Err(_) => sig.as_bytes().to_vec(),
    };
    catalog::verify_signature(&doc, &sig_bytes)?;
    println!("signature valid");
    Ok(0)
}

pub fn keygen(out: &PathBuf) -> Result<i32> {
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;
    std::fs::create_dir_all(out)?;
    let mut secret = [0u8; 32];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut secret)?;
    let key = SigningKey::from_bytes(&secret);
    let b64 = base64::engine::general_purpose::STANDARD;
    std::fs::write(
        out.join("catalog-secret.key"),
        format!("{}\n", b64.encode(secret)),
    )?;
    std::fs::write(
        out.join("catalog-public.key"),
        format!("{}\n", b64.encode(key.verifying_key().to_bytes())),
    )?;
    eprintln!(
        "wrote {}/catalog-secret.key and catalog-public.key",
        out.display()
    );
    Ok(0)
}

pub fn no_such() -> Result<i32> {
    bail!(
        "usage: shrug catalog build [--check] | sign --key-env VAR | verify FILE SIG | keygen --out DIR"
    )
}
