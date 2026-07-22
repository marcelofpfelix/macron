use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HostConfig {
    pub host: String,
    #[serde(default = "default_username")]
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_ref: Option<String>,
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HostDb {
    #[serde(default)]
    pub hosts: BTreeMap<String, HostConfig>,
}

impl HostDb {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&data).with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let data = toml::to_string_pretty(self).context("failed to serialize host database")?;
        fs::write(&path, data).with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn get(&self, name: &str) -> Option<&HostConfig> {
        self.hosts.get(name)
    }

    pub fn set(&mut self, name: String, config: HostConfig) {
        self.hosts.insert(name, config);
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.hosts.remove(name).is_some()
    }
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("AMT_CONFIG") {
        return Ok(PathBuf::from(path));
    }

    let local = local_config_path();
    if local.exists() {
        return Ok(local);
    }

    Ok(dirs::config_dir()
        .context("could not locate user config directory")?
        .join("amt")
        .join("hosts.toml"))
}

pub fn local_config_path() -> PathBuf {
    Path::new(".config").join("amt").join("hosts.toml")
}

fn default_username() -> String {
    "admin".to_string()
}

fn default_protocol() -> String {
    "http".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_db() {
        let db: HostDb = toml::from_str(
            r#"
[hosts.nuc]
host = "10.0.0.10"
password_ref = "gopass:homelab/amt"
"#,
        )
        .unwrap();
        let host = db.get("nuc").unwrap();
        assert_eq!(host.username, "admin");
        assert_eq!(host.protocol, "http");
        assert_eq!(host.password_ref.as_deref(), Some("gopass:homelab/amt"));
        assert!(host.password.is_none());
    }

    #[test]
    fn set_updates_and_remove_deletes_hosts() {
        let mut db = HostDb::default();
        db.set(
            "nuc".to_string(),
            HostConfig {
                host: "10.0.0.10".to_string(),
                username: "admin".to_string(),
                password: None,
                password_ref: Some("gopass:homelab/amt".to_string()),
                protocol: "https".to_string(),
            },
        );
        assert_eq!(db.get("nuc").unwrap().protocol, "https");

        db.set(
            "nuc".to_string(),
            HostConfig {
                host: "10.0.0.11".to_string(),
                username: "admin".to_string(),
                password: Some("new-secret".to_string()),
                password_ref: None,
                protocol: "http".to_string(),
            },
        );
        assert_eq!(db.get("nuc").unwrap().host, "10.0.0.11");
        assert!(db.remove("nuc"));
        assert!(!db.remove("nuc"));
        assert!(db.get("nuc").is_none());
    }
}
