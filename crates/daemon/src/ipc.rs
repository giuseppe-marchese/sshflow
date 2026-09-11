use crate::state::DaemonState;
use sshflow_core::{DaemonRequest, DaemonResponse, DaemonStatus, Event};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Newline-delimited JSON, one request/response pair per connection.
/// Simple and sufficient for a local control-plane where the CLI opens
/// a fresh connection per command.
#[cfg(unix)]
async fn handle_stream<S>(mut stream: S, state: Arc<DaemonState>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (read_half, mut write_half) = tokio::io::split(&mut stream);
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }

    let response = match serde_json::from_str::<DaemonRequest>(line.trim()) {
        Ok(req) => dispatch(req, &state).await,
        Err(e) => DaemonResponse::Error(format!("malformed request: {e}")),
    };

    if let Ok(mut out) = serde_json::to_string(&response) {
        out.push('\n');
        let _ = write_half.write_all(out.as_bytes()).await;
        let _ = write_half.flush().await;
    }
}

async fn dispatch(req: DaemonRequest, state: &Arc<DaemonState>) -> DaemonResponse {
    match req {
        DaemonRequest::Ping => DaemonResponse::Pong,

        DaemonRequest::EnsureConnection { target } => ensure_connection(state, target).await,

        DaemonRequest::RegisterExternalMaster {
            target,
            control_socket,
            pid,
        } => {
            let key = target.key();
            state
                .connections
                .insert_new(key.clone(), target.clone(), control_socket, pid, None);
            state
                .metrics
                .record_connection(&key, &target.display(), false, 0);
            state.plugins.dispatch(&Event::ConnectionOpened {
                key,
                host: target.display(),
            });
            DaemonResponse::Ack
        }

        DaemonRequest::Status => DaemonResponse::Status(DaemonStatus {
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            uptime_secs: state.started_at.elapsed().as_secs(),
            active_connections: state.connections.len(),
            plugins_loaded: state.plugins.len(),
        }),

        DaemonRequest::Stats => DaemonResponse::Stats(state.metrics.snapshot()),

        DaemonRequest::Connections => DaemonResponse::Connections(state.connections.list()),

        DaemonRequest::Doctor => DaemonResponse::Doctor(crate::doctor::run(state)),

        DaemonRequest::CloseConnection { key } => {
            if let Some(conn) = state.connections.remove(&key) {
                let socket = conn.control_socket.clone();
                let target = conn.target.clone();
                let _ =
                    tokio::task::spawn_blocking(move || sshflow_ssh::stop_master(&socket, &target))
                        .await;
                state.plugins.dispatch(&Event::ConnectionClosed {
                    key,
                    host: conn.target.display(),
                });
                DaemonResponse::Ack
            } else {
                DaemonResponse::Error("no such managed connection".to_string())
            }
        }

        DaemonRequest::RecordEvent(event) => {
            state.metrics.record_event(&event);
            state.plugins.dispatch(&event);
            DaemonResponse::Ack
        }

        DaemonRequest::Shutdown => {
            let _ = state.shutdown_tx.send(true);
            DaemonResponse::Ack
        }
    }
}

/// The single entry point for "I want to talk to this host" -- this
/// is the Connection Manager's core lifecycle decision: reuse an
/// existing, healthy connection; recreate a stale one; or establish a
/// brand-new one, respecting `maxConnections` along the way.
async fn ensure_connection(
    state: &Arc<DaemonState>,
    target: sshflow_core::HostTarget,
) -> DaemonResponse {
    let key = target.key();

    // Fast path: reuse an already-alive master.
    if let Some(existing) = state.connections.socket_for(&key) {
        let socket = existing.clone();
        let t = target.clone();
        let start = std::time::Instant::now();
        let alive = tokio::task::spawn_blocking(move || sshflow_ssh::is_master_alive(&socket, &t))
            .await
            .unwrap_or(false);
        let latency_ms = start.elapsed().as_millis() as u64;

        if alive {
            state.connections.mark_reused(&key, latency_ms);
            state
                .metrics
                .record_connection(&key, &target.display(), true, latency_ms);
            state.plugins.dispatch(&Event::ConnectionReused {
                key: key.clone(),
                host: target.display(),
            });
            return DaemonResponse::ConnectionReady {
                control_socket: existing,
                reused: true,
                latency_ms,
            };
        }
        // Stale entry -- drop it and fall through to create a new one.
        state.connections.remove(&key);
    }

    if state.connections.len() >= state.config.max_connections as usize {
        state.evict_lru();
    }

    let home = state.home.clone();
    let socket_path = match sshflow_ssh::control_socket_path(&home, &target) {
        Ok(p) => p,
        Err(e) => {
            return DaemonResponse::Error(format!("could not prepare control socket path: {e}"))
        }
    };

    let persist = state
        .config
        .persist_time_duration()
        .unwrap_or(Duration::from_secs(600));

    let socket_for_task = socket_path.clone();
    let target_for_task = target.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        sshflow_ssh::start_master_noninteractive(
            &socket_for_task,
            &target_for_task,
            persist,
            Duration::from_secs(10),
        )
    })
    .await;

    match outcome {
        Ok(Ok(result)) => {
            state.connections.insert_new(
                key.clone(),
                target.clone(),
                socket_path.clone(),
                None,
                Some(result.elapsed_ms),
            );
            state
                .metrics
                .record_connection(&key, &target.display(), false, result.elapsed_ms);
            state.plugins.dispatch(&Event::ConnectionOpened {
                key,
                host: target.display(),
            });
            DaemonResponse::ConnectionReady {
                control_socket: socket_path,
                reused: false,
                latency_ms: result.elapsed_ms,
            }
        }
        Ok(Err(sshflow_ssh::StartMasterError::NeedsInteractiveAuth)) => {
            DaemonResponse::NeedsInteractiveAuth {
                control_socket: socket_path,
            }
        }
        Ok(Err(sshflow_ssh::StartMasterError::Failed(msg))) => {
            state.metrics.record_failure(&key, &target.display());
            state.plugins.dispatch(&Event::AuthenticationFailed {
                host: target.display(),
                reason: msg.clone(),
            });
            DaemonResponse::Error(msg)
        }
        Err(join_err) => DaemonResponse::Error(format!("internal error: {join_err}")),
    }
}

#[cfg(unix)]
pub async fn serve_unix(state: Arc<DaemonState>) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::{UnixListener, UnixStream};

    let socket_path = state.home.join("daemon.sock");

    if socket_path.exists() {
        // Is another daemon already listening? If so, refuse to start.
        if UnixStream::connect(&socket_path).await.is_ok() {
            anyhow::bail!(
                "a SSHFlow daemon is already listening on {}",
                socket_path.display()
            );
        }
        std::fs::remove_file(&socket_path)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!(socket = %socket_path.display(), "IPC listening (unix socket)");

    let mut shutdown_rx = state.shutdown_tx.subscribe();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let state = state.clone();
                tokio::spawn(async move { handle_stream(stream, state).await; });
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

/// Windows fallback transport: OpenSSH-on-Windows environments vary in
/// named-pipe tooling availability, so we use a loopback-only TCP
/// socket plus a random per-instance token (written to
/// `daemon.port` alongside the bound port) as a lightweight same-host
/// authentication check. This is documented as a known simplification
/// in SECURITY.md; binding to 127.0.0.1 already prevents any remote
/// access.
#[cfg(windows)]
pub async fn serve_windows(state: Arc<DaemonState>) -> anyhow::Result<()> {
    use rand::RngCore;
    use tokio::net::{TcpListener, TcpStream};

    let info_path = state.home.join("daemon.port");

    if let Some(existing) = read_windows_endpoint(&info_path) {
        if TcpStream::connect(("127.0.0.1", existing.0)).await.is_ok() {
            anyhow::bail!(
                "a SSHFlow daemon is already listening on 127.0.0.1:{}",
                existing.0
            );
        }
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let mut token_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut token_bytes);
    let token = token_bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    std::fs::write(&info_path, format!("{port}\n{token}\n"))?;
    tracing::info!(port, "IPC listening (127.0.0.1, token-authenticated)");

    let mut shutdown_rx = state.shutdown_tx.subscribe();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let state = state.clone();
                let expected_token = token.clone();
                tokio::spawn(async move {
                    handle_stream_with_token(stream, state, expected_token).await;
                });
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }

    let _ = std::fs::remove_file(&info_path);
    Ok(())
}

#[cfg(windows)]
fn read_windows_endpoint(path: &std::path::Path) -> Option<(u16, String)> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut lines = raw.lines();
    let port: u16 = lines.next()?.parse().ok()?;
    let token = lines.next()?.to_string();
    Some((port, token))
}

#[cfg(windows)]
async fn handle_stream_with_token<S>(mut stream: S, state: Arc<DaemonState>, expected_token: String)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (read_half, mut write_half) = tokio::io::split(&mut stream);
    let mut reader = BufReader::new(read_half);
    let mut token_line = String::new();
    if reader.read_line(&mut token_line).await.unwrap_or(0) == 0 {
        return;
    }
    if token_line.trim() != expected_token {
        let _ = write_half
            .write_all(b"{\"Error\":\"unauthorized\"}\n")
            .await;
        return;
    }
    let mut line = String::new();
    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    let response = match serde_json::from_str::<DaemonRequest>(line.trim()) {
        Ok(req) => dispatch(req, &state).await,
        Err(e) => DaemonResponse::Error(format!("malformed request: {e}")),
    };
    if let Ok(mut out) = serde_json::to_string(&response) {
        out.push('\n');
        let _ = write_half.write_all(out.as_bytes()).await;
        let _ = write_half.flush().await;
    }
}
