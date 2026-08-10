//! sshflow-core
//!
//! Shared types used across every SSHFlow crate: the SSH target model,
//! the daemon <-> CLI IPC protocol, connection/event bookkeeping types
//! and the crate-wide error type.
//!
//! Security note: `HostTarget::parse` deliberately rejects destination
//! strings that start with `-`. SSHFlow never builds a shell command
//! line (all subprocess arguments are passed as discrete `OsString`
//! entries via `std::process::Command`, never interpolated into a
//! shell string), so classic shell injection is not possible. However,
//! a hostname/user string starting with `-` could still be misread by
//! the `ssh` binary itself as a command-line flag ("argument /
//! option injection"). We reject that shape up front instead of
//! relying on the downstream binary to do the right thing.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod events;
pub use events::Event;

#[derive(Debug, thiserror::Error)]
pub enum SshFlowError {
    #[error("invalid ssh destination '{0}': {1}")]
    InvalidTarget(String, &'static str),
    #[error("daemon is not reachable: {0}")]
    DaemonUnreachable(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("ssh master connection failed: {0}")]
    MasterFailed(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SshFlowError>;

/// A parsed SSH destination: `[user@]host[:port]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HostTarget {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl HostTarget {
    /// Parse a destination string exactly as a user would type it for
    /// `ssh`, e.g. `deploy@10.0.0.4:2222` or just `myserver`.
    pub fn parse(raw: &str) -> Result<Self> {
        if raw.is_empty() {
            return Err(SshFlowError::InvalidTarget(raw.to_string(), "empty destination"));
        }
        if raw.starts_with('-') {
            // Prevents option/argument injection into the ssh binary.
            return Err(SshFlowError::InvalidTarget(
                raw.to_string(),
                "destination must not start with '-'",
            ));
        }

        let (user, rest) = match raw.split_once('@') {
            Some((u, r)) => {
                if u.is_empty() {
                    return Err(SshFlowError::InvalidTarget(raw.to_string(), "empty user before '@'"));
                }
                (Some(u.to_string()), r)
            }
            None => (None, raw),
        };

        if rest.is_empty() {
            return Err(SshFlowError::InvalidTarget(raw.to_string(), "empty host"));
        }

        // host[:port] -- be careful not to split IPv6 literals like [::1]:22.
        let (host, port) = if let Some(stripped) = rest.strip_prefix('[') {
            match stripped.split_once(']') {
                Some((h, tail)) => {
                    let port = if let Some(p) = tail.strip_prefix(':') {
                        Some(p.parse::<u16>().map_err(|_| {
                            SshFlowError::InvalidTarget(raw.to_string(), "invalid port")
                        })?)
                    } else {
                        None
                    };
                    (h.to_string(), port)
                }
                None => {
                    return Err(SshFlowError::InvalidTarget(raw.to_string(), "unterminated '['"))
                }
            }
        } else {
            match rest.rsplit_once(':') {
                Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
                    (h.to_string(), Some(p.parse::<u16>().map_err(|_| {
                        SshFlowError::InvalidTarget(raw.to_string(), "invalid port")
                    })?))
                }
                _ => (rest.to_string(), None),
            }
        };

        if host.starts_with('-') {
            return Err(SshFlowError::InvalidTarget(raw.to_string(), "host must not start with '-'"));
        }

        Ok(HostTarget { user, host, port })
    }

    /// Stable key used to identify a distinct control-master connection
    /// (a given user+host+port tuple gets its own multiplexed socket).
    pub fn key(&self) -> String {
        format!(
            "{}@{}:{}",
            self.user.as_deref().unwrap_or(""),
            self.host,
            self.port.map(|p| p.to_string()).unwrap_or_default()
        )
    }

    /// A short, filesystem-safe fingerprint (used in the control socket
    /// path, which has a strict length limit on most platforms).
    pub fn socket_fingerprint(&self) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.key().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    pub fn display(&self) -> String {
        match (&self.user, self.port) {
            (Some(u), Some(p)) => format!("{u}@{}:{p}", self.host),
            (Some(u), None) => format!("{u}@{}", self.host),
            (None, Some(p)) => format!("{}:{p}", self.host),
            (None, None) => self.host.clone(),
        }
    }
}

impl fmt::Display for HostTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display())
    }
}

#[cfg(test)]
mod host_target_tests {
    use super::*;

    #[test]
    fn parses_plain_host() {
        let t = HostTarget::parse("myserver").unwrap();
        assert_eq!(t.host, "myserver");
        assert_eq!(t.user, None);
        assert_eq!(t.port, None);
    }

    #[test]
    fn parses_user_host() {
        let t = HostTarget::parse("deploy@10.0.0.4").unwrap();
        assert_eq!(t.user.as_deref(), Some("deploy"));
        assert_eq!(t.host, "10.0.0.4");
    }

    #[test]
    fn parses_user_host_port() {
        let t = HostTarget::parse("deploy@10.0.0.4:2222").unwrap();
        assert_eq!(t.user.as_deref(), Some("deploy"));
        assert_eq!(t.host, "10.0.0.4");
        assert_eq!(t.port, Some(2222));
    }

    #[test]
    fn parses_ipv6_literal_with_port() {
        let t = HostTarget::parse("[::1]:2222").unwrap();
        assert_eq!(t.host, "::1");
        assert_eq!(t.port, Some(2222));
    }

    #[test]
    fn rejects_leading_dash_option_injection() {
        assert!(HostTarget::parse("-oProxyCommand=evil").is_err());
        // A dash-leading host part (even after `user@`) is also
        // rejected -- ssh would otherwise read it as a flag.
        assert!(HostTarget::parse("user@-oProxyCommand=evil").is_err());
    }

    #[test]
    fn rejects_empty_destination() {
        assert!(HostTarget::parse("").is_err());
        assert!(HostTarget::parse("user@").is_err());
    }

    #[test]
    fn key_is_stable_for_same_target() {
        let a = HostTarget::parse("user@host:22").unwrap();
        let b = HostTarget::parse("user@host:22").unwrap();
        assert_eq!(a.key(), b.key());
        assert_eq!(a.socket_fingerprint(), b.socket_fingerprint());
    }
}

/// State of a single managed (control-master) connection, as tracked
/// by the daemon and reported to the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub key: String,
    pub target: HostTarget,
    pub control_socket: PathBuf,
    pub pid: Option<u32>,
    pub created_at_unix: u64,
    pub last_used_unix: u64,
    pub reused_count: u64,
    pub healthy: bool,
    pub last_latency_ms: Option<u64>,
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Aggregate metrics snapshot, as printed by `sshflow stats`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatsSnapshot {
    pub connections_today: u64,
    pub reused_today: u64,
    pub failures_today: u64,
    pub reconnects_today: u64,
    pub time_saved_ms: u64,
    pub average_latency_ms: f64,
    pub bytes_transferred: u64,
    pub most_used_hosts: Vec<(String, u64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn all_ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub pid: u32,
    pub uptime_secs: u64,
    pub active_connections: usize,
    pub plugins_loaded: usize,
}

/// Request sent from the CLI to the daemon over the local IPC channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonRequest {
    Ping,
    /// Ask the daemon to make sure a control-master connection exists
    /// (creating it if necessary) and return the socket path to use
    /// for the actual interactive `ssh` client invocation.
    EnsureConnection { target: HostTarget },
    Status,
    Stats,
    Connections,
    Doctor,
    /// Ask the daemon to close a specific managed connection.
    CloseConnection { key: String },
    /// The CLI created a control-master connection itself (foreground,
    /// real TTY -- needed when auth requires an interactive prompt the
    /// headless daemon cannot satisfy) and is handing it over to the
    /// daemon for health-checking and metrics tracking.
    RegisterExternalMaster {
        target: HostTarget,
        control_socket: PathBuf,
        pid: Option<u32>,
    },
    RecordEvent(Event),
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonResponse {
    Pong,
    ConnectionReady {
        control_socket: PathBuf,
        reused: bool,
        latency_ms: u64,
    },
    /// The daemon could not authenticate non-interactively (no agent
    /// key worked, batch mode forbids password/keyboard-interactive
    /// prompts). The CLI should fall back to creating the master
    /// itself in the foreground, where a real TTY is available.
    NeedsInteractiveAuth { control_socket: PathBuf },
    Status(DaemonStatus),
    Stats(StatsSnapshot),
    Connections(Vec<ConnectionInfo>),
    Doctor(DoctorReport),
    Ack,
    Error(String),
}
