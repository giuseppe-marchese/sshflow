mod connection_manager;
mod doctor;
mod health;
mod ipc;
mod state;

use connection_manager::ConnectionManager;
use sshflow_metrics::MetricsStore;
use sshflow_plugins::PluginManager;
use state::DaemonState;
use std::sync::Arc;
use std::time::Instant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let foreground = std::env::args().any(|a| a == "--foreground" || a == "-f");
    init_logging(foreground)?;

    let config = sshflow_config::load()?;
    let home = sshflow_config::sshflow_home()?;
    sshflow_ssh::socket_dir(&home)?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "sshflow-daemon starting"
    );

    write_pid_file(&home)?;

    let plugins = if config.plugins {
        let dir = sshflow_config::plugins_dir()?;
        PluginManager::load(&dir)
    } else {
        PluginManager::empty()
    };
    if !plugins.is_empty() {
        tracing::info!(count = plugins.len(), "plugins loaded");
    }

    let metrics = MetricsStore::load(&home);
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);

    let state = Arc::new(DaemonState {
        home: home.clone(),
        config,
        connections: ConnectionManager::new(),
        metrics,
        plugins,
        started_at: Instant::now(),
        shutdown_tx,
    });

    let health_state = state.clone();
    let health_task = tokio::spawn(async move { health::run(health_state).await });

    let signal_state = state.clone();
    let signal_task = tokio::spawn(async move { wait_for_shutdown_signal(signal_state).await });

    #[cfg(unix)]
    let ipc_result = ipc::serve_unix(state.clone()).await;
    #[cfg(windows)]
    let ipc_result = ipc::serve_windows(state.clone()).await;

    let _ = state.shutdown_tx.send(true);
    let _ = health_task.await;
    signal_task.abort();

    remove_pid_file(&home);
    tracing::info!("sshflow-daemon stopped");

    ipc_result
}

async fn wait_for_shutdown_signal(state: Arc<DaemonState>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
        tokio::select! {
            _ = term.recv() => tracing::info!("received SIGTERM"),
            _ = int.recv() => tracing::info!("received SIGINT"),
        }
    }
    #[cfg(windows)]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("received Ctrl-C");
    }
    let _ = state.shutdown_tx.send(true);
}

fn init_logging(foreground: bool) -> anyhow::Result<()> {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if foreground {
        fmt().with_env_filter(filter).init();
    } else {
        let logs_dir = sshflow_config::logs_dir()?;
        let log_path = logs_dir.join("daemon.log");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        fmt()
            .with_env_filter(filter)
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .init();
    }
    Ok(())
}

fn write_pid_file(home: &std::path::Path) -> anyhow::Result<()> {
    std::fs::write(home.join("daemon.pid"), std::process::id().to_string())?;
    Ok(())
}

fn remove_pid_file(home: &std::path::Path) {
    let _ = std::fs::remove_file(home.join("daemon.pid"));
}
