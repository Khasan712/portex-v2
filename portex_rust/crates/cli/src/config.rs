use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StoredConfig {
    pub token: Option<String>,
}

pub fn config_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable not set")?;
    Ok(home.join(".portex").join("config.toml"))
}

pub fn load() -> anyhow::Result<StoredConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(StoredConfig::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {path:?}"))?;
    toml::from_str(&raw).with_context(|| format!("parse {path:?}"))
}

pub fn store_token(token: &str) -> anyhow::Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {parent:?}"))?;
    }
    let cfg = StoredConfig { token: Some(token.to_string()) };
    let raw = toml::to_string(&cfg).context("serialize config")?;
    fs::write(&path, raw).with_context(|| format!("write {path:?}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).ok();
    }
    println!("Auth token saved to {}", path.display());
    Ok(())
}

pub fn resolve_token(override_value: Option<String>) -> anyhow::Result<String> {
    if let Some(t) = override_value {
        return Ok(t);
    }
    let cfg = load()?;
    cfg.token.context("no auth token: run `portex auth <token>` first or pass --token")
}
