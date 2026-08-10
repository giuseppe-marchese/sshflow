//! sshflow-config
//!
//! Loads `~/.sshflow/config.yaml`, creating a secure default if none
//! exists. Zero-configuration by default: every field has a sane
//! value and the daemon/CLI work fine with an empty or missing file.
//!
//! Security: `~/.sshflow` is created with `0700` permissions on Unix
//! (owner-only) since it holds the daemon's IPC socket, PID file and
//! locally-collected metrics. We never write secrets here -- SSHFlow
//! does not store passwords or private keys, ever.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not determine home directory")]
    NoHomeDir,
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("invalid duration '{0}' (expected e.g. '30s', '10m', '1h')")]
    InvalidDuration(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_true")]
    pub auto_multiplex: bool,
    #[serde(default = "default_persist_time")]
    pub persist_time: String,
    #[serde(default = "default_health_check")]
    pub health_check: String,
    #[serde(default = "default_true")]
    pub reconnect: bool,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_true")]
    pub metrics: bool,
    #[serde(default = "default_true")]
    pub plugins: bool,
    /// No telemetry by default, and this is the only telemetry-shaped
    /// knob in the project: it is opt-in, off by default, and (in
    /// this MVP) has no implementation to send data anywhere.
    #[serde(default)]
    pub telemetry: bool,
}

fn default_true() -> bool {
    true
}
fn default_persist_time() -> String {
    "10m".to_string()
}
fn default_health_check() -> String {
    "30s".to_string()
}
fn default_max_connections() -> u32 {
    20
}

impl Default for Config {
    fn default() -> Self {
        Config {
            auto_multiplex: true,
            persist_time: default_persist_time(),
            health_check: default_health_check(),
            reconnect: true,
            max_connections: default_max_connections(),
            metrics: true,
            plugins: true,
            telemetry: false,
        }
    }
}

impl Config {
    pub fn health_check_duration(&self) -> Result<std::time::Duration, ConfigError> {
        parse_duration(&self.health_check)
    }

    pub fn persist_time_duration(&self) -> Result<std::time::Duration, ConfigError> {
        parse_duration(&self.persist_time)
    }
}

/// Tiny duration parser: `<number><unit>` where unit is one of
/// `s`, `m`, `h`. Deliberately minimal -- avoids pulling in a whole
/// duration-parsing crate for two config fields.
pub fn parse_duration(raw: &str) -> Result<std::time::Duration, ConfigError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ConfigError::InvalidDuration(raw.to_string()));
    }
    let (num_part, unit) = raw.split_at(raw.len() - 1);
    let n: u64 = num_part
        .parse()
        .map_err(|_| ConfigError::InvalidDuration(raw.to_string()))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => return Err(ConfigError::InvalidDuration(raw.to_string())),
    };
    Ok(std::time::Duration::from_secs(secs))
}

/// Resolves `~/.sshflow`, creating it (with owner-only permissions on
/// Unix) if it does not already exist.
pub fn sshflow_home() -> Result<PathBuf, ConfigError> {
    let base = directories::BaseDirs::new().ok_or(ConfigError::NoHomeDir)?;
    let home = base.home_dir().join(".sshflow");
    ensure_secure_dir(&home)?;
    Ok(home)
}

pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(sshflow_home()?.join("config.yaml"))
}

pub fn logs_dir() -> Result<PathBuf, ConfigError> {
    let dir = sshflow_home()?.join("logs");
    ensure_secure_dir(&dir)?;
    Ok(dir)
}

pub fn plugins_dir() -> Result<PathBuf, ConfigError> {
    let dir = sshflow_home()?.join("plugins");
    ensure_secure_dir(&dir)?;
    Ok(dir)
}

fn ensure_secure_dir(path: &Path) -> Result<(), ConfigError> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(path, perms).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    }
    Ok(())
}

/// Loads config from `~/.sshflow/config.yaml`. If the file does not
/// exist, writes out the default config (so the user can discover and
/// edit it) and returns the defaults.
pub fn load() -> Result<Config, ConfigError> {
    let path = config_path()?;
    if !path.exists() {
        let cfg = Config::default();
        save(&cfg)?;
        tracing::info!(path = %path.display(), "wrote default config");
        return Ok(cfg);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| ConfigError::Io {
        path: path.clone(),
        source: e,
    })?;
    let cfg: Config = serde_yaml::from_str(&raw).map_err(|e| ConfigError::Parse {
        path: path.clone(),
        source: e,
    })?;
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<(), ConfigError> {
    let path = config_path()?;
    let yaml = serde_yaml::to_string(cfg).expect("Config always serializes");
    std::fs::write(&path, yaml).map_err(|e| ConfigError::Io {
        path,
        source: e,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("30s").unwrap().as_secs(), 30);
        assert_eq!(parse_duration("10m").unwrap().as_secs(), 600);
        assert_eq!(parse_duration("1h").unwrap().as_secs(), 3600);
        assert!(parse_duration("10x").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn default_config_matches_spec_example() {
        let cfg = Config::default();
        assert!(cfg.auto_multiplex);
        assert_eq!(cfg.persist_time, "10m");
        assert_eq!(cfg.health_check, "30s");
        assert!(cfg.reconnect);
        assert_eq!(cfg.max_connections, 20);
        assert!(cfg.metrics);
        assert!(cfg.plugins);
    }
}
