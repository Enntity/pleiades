// SPDX-License-Identifier: AGPL-3.0-only
//! SHRUG_HOME: config, state, catalog cache, known_hosts.
use crate::host::SshRoute;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("SHRUG_HOME") {
        return PathBuf::from(h);
    }
    std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".shrug")
}

pub fn known_hosts() -> PathBuf {
    home().join("known_hosts")
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// `owner--name@revision` of non-commercial weights the user accepted.
    #[serde(default)]
    pub accepted_noncommercial: Vec<String>,
}

impl Config {
    pub fn load() -> Self {
        std::fs::read(home().join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> Result<()> {
        let dir = home();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("config.json"), serde_json::to_string_pretty(self)?)
            .context("write config.json")
    }
    pub fn accepted(&self, repo_dir: &str, revision: &str) -> bool {
        self.accepted_noncommercial
            .iter()
            .any(|a| a == &format!("{repo_dir}@{revision}"))
    }
    pub fn accept(&mut self, repo_dir: &str, revision: &str) {
        let key = format!("{repo_dir}@{revision}");
        if !self.accepted_noncommercial.contains(&key) {
            self.accepted_noncommercial.push(key);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankState {
    pub rank: usize,
    pub host_label: String,
    pub route: SshRoute,
    pub container: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub schema: String,
    pub release_id: String,
    pub catalog_serial: u64,
    pub model_id: String,
    pub api_port: u16,
    pub previous_release_id: Option<String>,
    pub ranks: Vec<RankState>,
    pub started_at_unix: u64,
}

impl State {
    pub fn load() -> Result<Option<Self>> {
        match std::fs::read(home().join("state.json")) {
            Ok(b) => Ok(Some(
                serde_json::from_slice(&b).context("parse state.json")?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self) -> Result<()> {
        let dir = home();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("state.json"), serde_json::to_string_pretty(self)?)
            .context("write state.json")
    }
}
