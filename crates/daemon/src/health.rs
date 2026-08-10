use crate::state::DaemonState;
use sshflow_core::Event;
use std::sync::Arc;
use std::time::Duration;

/// Periodically verifies every managed control-master is still alive
/// (`ssh -S <socket> -O check`). Dead connections are either
/// transparently recreated (if `reconnect: true`) or dropped so the
/// next `sshflow ssh` invocation starts a fresh one. This loop is what
/// makes the Connection Manager proactive rather than a passive cache:
/// it's the "health" and "recovery" half of the connection lifecycle.
pub async fn run(state: Arc<DaemonState>) {
    let interval = state
        .config
        .health_check_duration()
        .unwrap_or(Duration::from_secs(30));
    let mut ticker = tokio::time::interval(interval);
    let mut shutdown_rx = state.shutdown_tx.subscribe();

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                check_all(&state).await;
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }
}

async fn check_all(state: &Arc<DaemonState>) {
    for key in state.connections.all_keys() {
        let Some(conn) = state.connections.get_clone(&key) else {
            continue;
        };
        let socket = conn.control_socket.clone();
        let target = conn.target.clone();
        let alive = tokio::task::spawn_blocking(move || sshflow_ssh::is_master_alive(&socket, &target))
            .await
            .unwrap_or(false);

        if alive {
            state.connections.mark_healthy(&key, true);
            continue;
        }

        tracing::warn!(host = %conn.target, "control master unresponsive");

        if state.config.reconnect {
            let socket = conn.control_socket.clone();
            let target = conn.target.clone();
            let persist = state
                .config
                .persist_time_duration()
                .unwrap_or(Duration::from_secs(600));
            let outcome = tokio::task::spawn_blocking(move || {
                sshflow_ssh::start_master_noninteractive(
                    &socket,
                    &target,
                    persist,
                    Duration::from_secs(10),
                )
            })
            .await;

            if let Ok(Ok(_)) = outcome {
                state.connections.mark_healthy(&key, true);
                state.connections.touch(&key);
                state.metrics.record_reconnect();
                state.plugins.dispatch(&Event::Reconnect {
                    key: key.clone(),
                    host: conn.target.display(),
                    attempt: 1,
                });
                tracing::info!(host = %conn.target, "reconnected automatically");
                continue;
            }
            tracing::warn!(host = %conn.target, "automatic reconnect failed, dropping stale entry");
        }

        state.connections.remove(&key);
        state.plugins.dispatch(&Event::ConnectionClosed {
            key: key.clone(),
            host: conn.target.display(),
        });
    }
}
