use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    #[serde(default = "default_manifest_path")]
    pub manifest_path: PathBuf,
    /// Bearer token for the `/admin` routes
    pub admin_token: String,
}

fn default_bind() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 8000))
}

fn default_manifest_path() -> PathBuf {
    PathBuf::from("manifest.json")
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config from {}", path.display()))?;
        let config: Self = toml::from_str(&text)
            .with_context(|| format!("parsing config from {}", path.display()))?;

        if config.admin_token.is_empty() {
            anyhow::bail!("admin_token in {} must not be empty", path.display());
        }

        Ok(config)
    }
}
