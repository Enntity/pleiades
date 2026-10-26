// SPDX-License-Identifier: AGPL-3.0-only
//! The release catalog: types, the embedded copy, and signed remote updates.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const SCHEMA: &str = "shrug.catalog/v1";
pub const DEFAULT_URL: &str = "https://github.com/Enntity/shrug/releases/download/catalog";
pub const EMBEDDED: &[u8] = include_bytes!("../catalog/catalog.json");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Catalog {
    pub schema: String,
    pub serial: u64,
    #[serde(default)]
    pub defaults: std::collections::BTreeMap<String, String>,
    pub releases: Vec<Release>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Release {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    pub model_id: String,
    pub sparks: u32,
    #[serde(default)]
    pub topologies: Vec<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub context_tokens: u64,
    #[serde(default)]
    pub concurrency: u32,
    #[serde(default)]
    pub qualified_by: String,
    #[serde(default)]
    pub source: Value,
    pub engine: Engine,
    #[serde(default)]
    pub weights: Vec<Weight>,
    #[serde(default)]
    pub extra_disk_gib: f64,
    pub launcher: Launcher,
    #[serde(default)]
    pub container_name: String,
    #[serde(default)]
    pub api_port: u16,
    #[serde(default)]
    pub args: Vec<ArgSpec>,
    #[serde(default)]
    pub receipts: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Engine {
    pub image: String,
    pub atlas_repo: String,
    pub atlas_commit: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Weight {
    pub repo: String,
    pub revision: String,
    #[serde(default)]
    pub size_gib: f64,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub license_url: String,
    #[serde(default)]
    pub noncommercial: bool,
}

impl Weight {
    /// Directory under the model root: `owner--name`.
    pub fn dir_name(&self) -> String {
        self.repo.replace('/', "--")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Launcher {
    pub file: String,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub script: String,
    #[serde(default)]
    pub sha256: String,
}

/// One arg template element: a string, or an optional flag/value pair.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ArgSpec {
    Lit(String),
    Opt {
        flag: String,
        value: String,
        #[serde(default)]
        optional: bool,
    },
}

impl Catalog {
    pub fn release(&self, id: &str) -> Option<&Release> {
        self.releases.iter().find(|r| r.id == id)
    }
}

pub fn parse(bytes: &[u8]) -> Result<Catalog> {
    let c: Catalog = serde_json::from_slice(bytes).context("catalog parse")?;
    if c.schema != SCHEMA {
        bail!("catalog schema {:?}, expected {SCHEMA}", c.schema);
    }
    Ok(c)
}

pub fn embedded() -> Result<Catalog> {
    parse(EMBEDDED)
}

/// Where the plan's catalog came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Source {
    Remote,
    Cache,
    Embedded,
}

impl Source {
    pub fn name(&self) -> &'static str {
        match self {
            Source::Remote => "remote",
            Source::Cache => "cache",
            Source::Embedded => "embedded",
        }
    }
}

pub struct Loaded {
    pub catalog: Catalog,
    pub source: Source,
}

pub fn load(home: &Path, offline: bool) -> Loaded {
    let embedded = embedded().expect("embedded catalog parses");
    let cache_path = home.join("catalog-cache.json");
    let sig_path = home.join("catalog-cache.json.sig");
    let cached = std::fs::read(&cache_path).ok().and_then(|bytes| {
        let sig = std::fs::read(&sig_path).ok()?;
        verify_signature(&bytes, &sig).ok()?;
        parse(&bytes).ok()
    });

    if !offline && let Some(c) = fetch_remote(&embedded, cached.as_ref(), &cache_path, &sig_path) {
        return Loaded {
            catalog: c,
            source: Source::Remote,
        };
    }
    if let Some(c) = cached
        && c.serial >= embedded.serial
    {
        return Loaded {
            catalog: c,
            source: Source::Cache,
        };
    }
    Loaded {
        catalog: embedded,
        source: Source::Embedded,
    }
}

fn base_url() -> String {
    std::env::var("SHRUG_CATALOG_URL").unwrap_or_else(|_| DEFAULT_URL.into())
}

fn fetch_remote(
    embedded: &Catalog,
    cached: Option<&Catalog>,
    cache_path: &Path,
    sig_path: &Path,
) -> Option<Catalog> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let base = base_url();
    let body = client
        .get(format!("{base}/catalog.json"))
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .ok()?;
    let sig = client
        .get(format!("{base}/catalog.json.sig"))
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .ok()?;
    verify_signature(&body, &sig).ok()?;
    let c = parse(&body).ok()?;
    let floor = cached.map(|c| c.serial).unwrap_or(0).max(embedded.serial);
    if c.serial < floor {
        return None;
    }
    let _ = std::fs::create_dir_all(cache_path.parent()?);
    let _ = std::fs::write(cache_path, &body);
    let _ = std::fs::write(sig_path, &sig);
    Some(c)
}

/// Ed25519 verification against the pinned public keys.
pub fn verify_signature(doc: &[u8], sig_b64: &[u8]) -> Result<()> {
    verify_with(doc, sig_b64, &crate::keys::CATALOG_PUBLIC_KEYS)
}

/// Verify against an explicit key set (used by tests).
pub fn verify_with(doc: &[u8], sig_b64: &[u8], keys: &[&str]) -> Result<()> {
    use base64::Engine as _;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(String::from_utf8_lossy(sig_b64).trim())
        .context("signature is not base64")?;
    let sig = Signature::from_slice(&sig_bytes).context("bad signature length")?;
    for key_b64 in keys {
        let key_bytes = base64::engine::general_purpose::STANDARD.decode(key_b64)?;
        if let Ok(key) = VerifyingKey::from_bytes(
            &key_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("bad key"))?,
        ) && key.verify(doc, &sig).is_ok()
        {
            return Ok(());
        }
    }
    bail!("signature does not verify against any of the keys")
}

pub fn sign(doc: &[u8], secret_b64: &str) -> Result<String> {
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    let secret: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(secret_b64.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("signing key must be 32 bytes, base64"))?;
    let key = SigningKey::from_bytes(&secret);
    Ok(base64::engine::general_purpose::STANDARD.encode(key.sign(doc).to_bytes()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `shrug catalog build`: releases.json + launcher files -> catalog.json.
pub fn build(dir: &Path) -> Result<Vec<u8>> {
    let path = dir.join("catalog/releases.json");
    let mut doc: Value = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )?;
    let releases = doc
        .get_mut("releases")
        .and_then(Value::as_array_mut)
        .context("releases.json has no releases array")?;
    for rel in releases {
        let launcher = rel
            .get_mut("launcher")
            .and_then(Value::as_object_mut)
            .context("release missing launcher object")?;
        let file = launcher
            .get("file")
            .and_then(Value::as_str)
            .context("launcher missing file")?
            .to_string();
        if file.contains("..") || file.starts_with('/') {
            bail!("launcher path {file} escapes catalog/");
        }
        let script = std::fs::read(dir.join("catalog").join(&file))
            .with_context(|| format!("read launcher {file}"))?;
        launcher.insert(
            "script".into(),
            Value::String(String::from_utf8(script.clone())?),
        );
        launcher.insert("sha256".into(), Value::String(sha256_hex(&script)));
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&doc)?).into_bytes())
}
