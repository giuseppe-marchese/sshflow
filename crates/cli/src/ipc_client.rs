//! Blocking IPC client. The CLI is short-lived (one command, one
//! request, exit), so there's no need to pull in an async runtime just
//! for this -- plain blocking sockets keep the client binary small and
//! simple, as called for by the "lightweight" design principle.

#[allow(unused_imports)]
use anyhow::{anyhow, bail, Context, Result};
use sshflow_core::{DaemonRequest, DaemonResponse};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

pub fn send(req: &DaemonRequest) -> Result<DaemonResponse> {
    ensure_daemon_running()?;
    send_raw(req)
}

#[cfg(unix)]
fn send_raw(req: &DaemonRequest) -> Result<DaemonResponse> {
    use std::os::unix::net::UnixStream;
    let home = sshflow_config::sshflow_home()?;
    let socket_path = home.join("daemon.sock");
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("connecting to {}", socket_path.display()))?;
    write_request(&mut stream, req)?;
    read_response(&mut stream)
}

#[cfg(windows)]
fn send_raw(req: &DaemonRequest) -> Result<DaemonResponse> {
    use std::net::TcpStream;
    let home = sshflow_config::sshflow_home()?;
    let info_path = home.join("daemon.port");
    let raw = std::fs::read_to_string(&info_path)
        .with_context(|| format!("reading {}", info_path.display()))?;
    let mut lines = raw.lines();
    let port: u16 = lines
        .next()
        .ok_or_else(|| anyhow!("malformed daemon.port"))?
        .parse()?;
    let token = lines
        .next()
        .ok_or_else(|| anyhow!("malformed daemon.port"))?
        .to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .with_context(|| format!("connecting to 127.0.0.1:{port}"))?;
    writeln!(stream, "{token}")?;
    write_request(&mut stream, req)?;
    read_response(&mut stream)
}

fn write_request<S: Write>(stream: &mut S, req: &DaemonRequest) -> Result<()> {
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn read_response<S: std::io::Read>(stream: S) -> Result<DaemonResponse> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim().is_empty() {
        bail!("daemon closed the connection without responding");
    }
    Ok(serde_json::from_str(line.trim())?)
}

fn is_daemon_reachable() -> bool {
    send_raw(&DaemonRequest::Ping).is_ok()
}

/// Auto-starts the daemon (detached, background) if it isn't already
/// reachable, then waits briefly for it to come up. This is what makes
/// SSHFlow "zero configuration": the very first `sshflow ssh ...` a
/// user runs transparently starts the daemon for them.
fn ensure_daemon_running() -> Result<()> {
    if is_daemon_reachable() {
        return Ok(());
    }

    let daemon_path = locate_daemon_binary()?;
    tracing_eprintln(&format!(
        "starting sshflow-daemon ({})...",
        daemon_path.display()
    ));

    let mut cmd = Command::new(&daemon_path);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // New process group so the daemon survives the CLI's shell
        // session ending (e.g. terminal closing) rather than being
        // killed by SIGHUP along with its parent.
        cmd.process_group(0);
    }

    cmd.spawn().context("failed to spawn sshflow-daemon")?;

    for _ in 0..40 {
        if is_daemon_reachable() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    bail!("sshflow-daemon did not become reachable after starting it")
}

fn locate_daemon_binary() -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "sshflow-daemon.exe"
    } else {
        "sshflow-daemon"
    };

    // Prefer a copy sitting next to this executable (typical install
    // layout: both binaries in the same bin/ directory).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    // Otherwise trust PATH, same as invoking `ssh` itself.
    Ok(PathBuf::from(name))
}

fn tracing_eprintln(msg: &str) {
    eprintln!("sshflow: {msg}");
}
