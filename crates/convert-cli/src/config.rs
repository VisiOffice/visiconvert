//! Where the CLI finds its server and workspace key: environment first, then the config file.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub url: String,
    pub api_key: String,
}

pub fn path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("VISICONVERT_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config"),
    };
    Ok(base.join("visiconvert").join("config.json"))
}

pub fn load() -> Result<Config> {
    let env = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
    let saved = fs::read_to_string(path()?)
        .ok()
        .and_then(|text| serde_json::from_str::<Config>(&text).ok());
    let url = env("VISICONVERT_URL").or_else(|| saved.as_ref().map(|c| c.url.clone()));
    let api_key = env("VISICONVERT_API_KEY").or_else(|| saved.as_ref().map(|c| c.api_key.clone()));
    match (url, api_key) {
        (Some(url), Some(api_key)) => Ok(Config { url, api_key }),
        _ => bail!(
            "Not logged in. Run `visiconvert login --url https://your-server`, or set VISICONVERT_URL and VISICONVERT_API_KEY."
        ),
    }
}

/// Saves the key readable only by the current user.
pub fn save(config: &Config) -> Result<PathBuf> {
    let path = path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        if path.exists() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("Could not write {}", path.display()))?;
    file.write_all(serde_json::to_string_pretty(config)?.as_bytes())?;
    Ok(path)
}

pub fn remove() -> Result<bool> {
    let path = path()?;
    if path.exists() {
        fs::remove_file(&path)?;
        return Ok(true);
    }
    Ok(false)
}
