use crate::ipc_client;
use anyhow::{bail, Result};
use sshflow_core::{DaemonRequest, DaemonResponse, HostTarget};

fn fmt_ago(unix_secs: u64) -> String {
    let now = sshflow_core::now_unix();
    let delta = now.saturating_sub(unix_secs);
    if delta < 60 {
        format!("{delta}s ago")
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

pub fn ssh(target: &str, extra_args: Vec<String>) -> Result<()> {
    let host_target = HostTarget::parse(target)?;
    let response = ipc_client::send(&DaemonRequest::EnsureConnection {
        target: host_target.clone(),
    })?;

    let control_socket = match response {
        DaemonResponse::ConnectionReady {
            control_socket,
            reused,
            latency_ms,
        } => {
            if reused {
                eprintln!("sshflow: reusing connection to {target} ({latency_ms} ms)");
            } else {
                eprintln!("sshflow: connected to {target} ({latency_ms} ms handshake, now multiplexed)");
            }
            control_socket
        }
        DaemonResponse::NeedsInteractiveAuth { control_socket } => {
            eprintln!("sshflow: {target} needs interactive authentication, connecting directly...");
            let config = sshflow_config::load()?;
            sshflow_ssh::start_master_interactive(
                &control_socket,
                &host_target,
                config.persist_time_duration().unwrap_or(std::time::Duration::from_secs(600)),
            )?;
            let pid = None;
            let _ = ipc_client::send(&DaemonRequest::RegisterExternalMaster {
                target: host_target.clone(),
                control_socket: control_socket.clone(),
                pid,
            });
            control_socket
        }
        DaemonResponse::Error(msg) => bail!("{msg}"),
        _ => bail!("unexpected daemon response"),
    };

    #[cfg(unix)]
    {
        let err = sshflow_ssh::exec_client(&control_socket, &host_target, &extra_args);
        bail!("failed to launch ssh: {err}");
    }
    #[cfg(windows)]
    {
        let code = sshflow_ssh::exec_client(&control_socket, &host_target, &extra_args)?;
        std::process::exit(code);
    }
}

pub fn status() -> Result<()> {
    match ipc_client::send(&DaemonRequest::Status)? {
        DaemonResponse::Status(s) => {
            let h = s.uptime_secs / 3600;
            let m = (s.uptime_secs % 3600) / 60;
            println!("SSHFlow daemon v{} (pid {})", s.version, s.pid);
            println!("Uptime: {h}h {m}m");
            println!("Active connections: {}", s.active_connections);
            println!("Plugins loaded: {}", s.plugins_loaded);
            Ok(())
        }
        DaemonResponse::Error(e) => bail!(e),
        _ => bail!("unexpected daemon response"),
    }
}

pub fn stats() -> Result<()> {
    match ipc_client::send(&DaemonRequest::Stats)? {
        DaemonResponse::Stats(s) => {
            println!("Connections today: {}", s.connections_today);
            println!("Reused: {}", s.reused_today);
            println!("Time saved: {}", sshflow_metrics::format_duration_ms(s.time_saved_ms));
            println!("Average latency: {:.0} ms", s.average_latency_ms);
            if s.failures_today > 0 {
                println!("Failures: {}", s.failures_today);
            }
            if s.reconnects_today > 0 {
                println!("Reconnects: {}", s.reconnects_today);
            }
            if !s.most_used_hosts.is_empty() {
                println!();
                println!("Most used hosts:");
                for (host, count) in &s.most_used_hosts {
                    println!("  {host:<40} {count}");
                }
            }
            Ok(())
        }
        DaemonResponse::Error(e) => bail!(e),
        _ => bail!("unexpected daemon response"),
    }
}

pub fn connections() -> Result<()> {
    match ipc_client::send(&DaemonRequest::Connections)? {
        DaemonResponse::Connections(list) => {
            if list.is_empty() {
                println!("No active managed connections.");
                return Ok(());
            }
            println!("{:<30} {:<8} {:<10} {:<10}", "HOST", "HEALTHY", "REUSED", "LAST USED");
            for c in list {
                println!(
                    "{:<30} {:<8} {:<10} {:<10}",
                    c.target.display(),
                    if c.healthy { "yes" } else { "no" },
                    c.reused_count,
                    fmt_ago(c.last_used_unix)
                );
            }
            Ok(())
        }
        DaemonResponse::Error(e) => bail!(e),
        _ => bail!("unexpected daemon response"),
    }
}

pub fn doctor() -> Result<()> {
    match ipc_client::send(&DaemonRequest::Doctor)? {
        DaemonResponse::Doctor(report) => {
            for check in &report.checks {
                let mark = if check.ok { "✅" } else { "❌" };
                println!("{mark} {:<28} {}", check.name, check.detail);
            }
            if !report.all_ok() {
                std::process::exit(1);
            }
            Ok(())
        }
        DaemonResponse::Error(e) => bail!(e),
        _ => bail!("unexpected daemon response"),
    }
}

pub fn logs(lines: usize) -> Result<()> {
    let path = sshflow_config::logs_dir()?.join("daemon.log");
    if !path.exists() {
        println!("No logs yet ({} does not exist).", path.display());
        return Ok(());
    }
    let content = std::fs::read_to_string(&path)?;
    let all_lines: Vec<&str> = content.lines().collect();
    let start = all_lines.len().saturating_sub(lines);
    for line in &all_lines[start..] {
        println!("{line}");
    }
    Ok(())
}

pub fn config_show() -> Result<()> {
    let path = sshflow_config::config_path()?;
    let content = std::fs::read_to_string(&path)?;
    print!("{content}");
    Ok(())
}

pub fn config_path() -> Result<()> {
    println!("{}", sshflow_config::config_path()?.display());
    Ok(())
}

pub fn config_edit() -> Result<()> {
    let path = sshflow_config::config_path()?;
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
        if cfg!(windows) { "notepad".to_string() } else { "vi".to_string() }
    });
    let status = std::process::Command::new(editor).arg(&path).status()?;
    if !status.success() {
        bail!("editor exited with a non-zero status");
    }
    Ok(())
}

pub fn version() {
    println!("sshflow {}", env!("CARGO_PKG_VERSION"));
}

pub fn update() {
    println!("sshflow {}", env!("CARGO_PKG_VERSION"));
    println!("Automatic self-update isn't implemented yet.");
    println!("Please reinstall via your package manager or `cargo install sshflow-cli`,");
    println!("or check https://github.com/sshflow/sshflow/releases for the latest build.");
}
