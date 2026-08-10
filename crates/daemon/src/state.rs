use crate::connection_manager::ConnectionManager;
use sshflow_config::Config;
use sshflow_metrics::MetricsStore;
use sshflow_plugins::PluginManager;
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::watch;

/// Daemon-wide shared state. `connections` (the Connection Manager) is
/// the core of the project; `metrics` and `plugins` observe and react
/// to what happens inside it, they don't drive it.
pub struct DaemonState {
    pub home: PathBuf,
    pub config: Config,
    pub connections: ConnectionManager,
    pub metrics: MetricsStore,
    pub plugins: PluginManager,
    pub started_at: Instant,
    pub shutdown_tx: watch::Sender<bool>,
}

impl DaemonState {
    /// Evict the least-recently-used managed connection, stopping its
    /// master. Used to respect `maxConnections` without just refusing
    /// new work.
    pub fn evict_lru(&self) {
        let Some(key) = self.connections.least_recently_used() else {
            return;
        };
        if let Some(conn) = self.connections.remove(&key) {
            let _ = sshflow_ssh::stop_master(&conn.control_socket, &conn.target);
            tracing::info!(host = %conn.target, "evicted least-recently-used connection (maxConnections reached)");
        }
    }
}
