//! The Connection Manager: the heart of SSHFlow.
//!
//! SSHFlow's mission is not "wrap the `ssh` binary" -- it is to
//! **manage the full lifecycle of SSH connections**: creation, reuse,
//! health, recovery, eviction and observability. Multiplexing (via
//! OpenSSH ControlMaster) is simply the mechanism this manager uses
//! today to make a connection cheap to reuse; metrics, diagnostics and
//! the plugin event bus all hang off of the same lifecycle this module
//! defines. Everything else in the daemon (IPC handlers, health-check
//! loop) is a thin caller of this API -- they never touch the
//! underlying map directly.

use dashmap::DashMap;
use sshflow_core::{now_unix, ConnectionInfo, HostTarget};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ManagedConnection {
    pub target: HostTarget,
    pub control_socket: PathBuf,
    pub pid: Option<u32>,
    pub created_at: u64,
    pub last_used: u64,
    pub reused_count: u64,
    pub healthy: bool,
    pub last_latency_ms: Option<u64>,
}

impl ManagedConnection {
    fn to_info(&self, key: &str) -> ConnectionInfo {
        ConnectionInfo {
            key: key.to_string(),
            target: self.target.clone(),
            control_socket: self.control_socket.clone(),
            pid: self.pid,
            created_at_unix: self.created_at,
            last_used_unix: self.last_used,
            reused_count: self.reused_count,
            healthy: self.healthy,
            last_latency_ms: self.last_latency_ms,
        }
    }
}

/// Owns the lifecycle of every managed SSH connection. This is the
/// single source of truth: nothing outside this module inserts,
/// removes or mutates a `ManagedConnection` directly.
#[derive(Default)]
pub struct ConnectionManager {
    connections: DashMap<String, ManagedConnection>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.connections.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    /// Socket path of a connection, if we currently believe one exists
    /// for this key (does not itself verify liveness -- callers use
    /// `sshflow_ssh::is_master_alive` for that, then report the result
    /// back via `mark_healthy`/`remove`).
    pub fn socket_for(&self, key: &str) -> Option<PathBuf> {
        self.connections.get(key).map(|c| c.control_socket.clone())
    }

    #[allow(dead_code)]
    pub fn contains(&self, key: &str) -> bool {
        self.connections.contains_key(key)
    }

    /// Registers a brand-new connection (fresh master or an
    /// externally-created one handed over by the CLI).
    pub fn insert_new(&self, key: String, target: HostTarget, control_socket: PathBuf, pid: Option<u32>, latency_ms: Option<u64>) {
        let now = now_unix();
        self.connections.insert(
            key,
            ManagedConnection {
                target,
                control_socket,
                pid,
                created_at: now,
                last_used: now,
                reused_count: 0,
                healthy: true,
                last_latency_ms: latency_ms,
            },
        );
    }

    /// Records a successful reuse: bumps the counters and freshens
    /// `last_used` so LRU eviction treats it as recently active.
    pub fn mark_reused(&self, key: &str, latency_ms: u64) {
        if let Some(mut c) = self.connections.get_mut(key) {
            c.reused_count += 1;
            c.last_used = now_unix();
            c.last_latency_ms = Some(latency_ms);
            c.healthy = true;
        }
    }

    pub fn mark_healthy(&self, key: &str, healthy: bool) {
        if let Some(mut c) = self.connections.get_mut(key) {
            c.healthy = healthy;
        }
    }

    pub fn touch(&self, key: &str) {
        if let Some(mut c) = self.connections.get_mut(key) {
            c.last_used = now_unix();
        }
    }

    /// Removes a connection from management (its `ssh` master process
    /// is NOT stopped by this call -- callers that want to actually
    /// terminate it should do so via `sshflow_ssh::stop_master` first,
    /// since that requires a blocking subprocess call this
    /// synchronous, lock-holding method must not perform).
    pub fn remove(&self, key: &str) -> Option<ManagedConnection> {
        self.connections.remove(key).map(|(_, v)| v)
    }

    pub fn get_clone(&self, key: &str) -> Option<ManagedConnection> {
        self.connections.get(key).map(|c| c.clone())
    }

    pub fn all_keys(&self) -> Vec<String> {
        self.connections.iter().map(|e| e.key().clone()).collect()
    }

    pub fn list(&self) -> Vec<ConnectionInfo> {
        self.connections
            .iter()
            .map(|e| e.value().to_info(e.key()))
            .collect()
    }

    /// Chooses the least-recently-used connection, for eviction when
    /// `maxConnections` is reached. Returns the key without removing
    /// it -- the caller stops the underlying master (a blocking call)
    /// and then calls `remove`.
    pub fn least_recently_used(&self) -> Option<String> {
        self.connections
            .iter()
            .min_by_key(|e| e.value().last_used)
            .map(|e| e.key().clone())
    }
}
