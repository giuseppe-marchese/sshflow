//! sshflow-ssh
//!
//! Thin, careful wrapper around the real `ssh` binary's ControlMaster /
//! ControlPersist multiplexing feature. SSHFlow never re-implements the
//! SSH protocol and never touches keys, agents or passwords directly --
//! it only decides *when* to start/reuse/stop a master connection and
//! shells out to the user's own trusted `ssh`.
//!
//! Security notes:
//! - Every argument is passed as a discrete `OsString` via
//!   `std::process::Command`; nothing is ever interpolated into a
//!   shell string, so shell injection is not possible by construction.
//! - We never override `StrictHostKeyChecking` or other host-key
//!   verification settings. The user's own `ssh_config` and known_hosts
//!   policy is always respected.
//! - Non-interactive (daemon-initiated) master creation uses
//!   `BatchMode=yes`, so a host that requires a password/keyboard-
//!   interactive prompt fails fast instead of hanging the daemon; the
//!   CLI then retries in the foreground where a real TTY exists.
//! - Control socket files are chmod'd to `0600` after creation.

use sshflow_core::{HostTarget, Result, SshFlowError};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Directory holding one control-socket file per active managed
/// connection. Kept separate from the rest of `~/.sshflow` so it can
/// be locked down independently.
pub fn socket_dir(home: &Path) -> std::io::Result<PathBuf> {
    let dir = home.join("sockets");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Control socket path for a given target. Uses a short hash rather
/// than the raw host/user string both for readability and because
/// Unix domain socket paths have a hard length limit (~104-108 bytes)
/// that a long hostname could blow past.
pub fn control_socket_path(home: &Path, target: &HostTarget) -> std::io::Result<PathBuf> {
    Ok(socket_dir(home)?.join(format!("{}.sock", target.socket_fingerprint())))
}

fn destination_string(target: &HostTarget) -> String {
    match &target.user {
        Some(u) => format!("{u}@{}", target.host),
        None => target.host.clone(),
    }
}

/// Best-effort discovery of the `ssh` binary and its reported version,
/// used by `sshflow doctor`. Actual invocations elsewhere just use the
/// bare program name `"ssh"` (or `"ssh.exe"`) and let the OS search
/// `PATH`, exactly like a shell would.
pub fn detect_ssh() -> Option<(PathBuf, String)> {
    for candidate in ssh_candidates() {
        if let Ok(output) = Command::new(&candidate)
            .arg("-V")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
        {
            let mut version = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if version.is_empty() {
                version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
            if !version.is_empty() {
                return Some((candidate, version));
            }
        }
    }
    None
}

fn ssh_candidates() -> Vec<PathBuf> {
    let name = if cfg!(windows) { "ssh.exe" } else { "ssh" };
    let mut out = vec![PathBuf::from(name)];
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                out.push(candidate);
            }
        }
    }
    out
}

pub fn ssh_program() -> String {
    if cfg!(windows) { "ssh.exe".to_string() } else { "ssh".to_string() }
}

/// Is the master at `socket_path` alive and answering `-O check`?
pub fn is_master_alive(socket_path: &Path, target: &HostTarget) -> bool {
    if !socket_path.exists() {
        return false;
    }
    Command::new(ssh_program())
        .arg("-S")
        .arg(socket_path)
        .arg("-O")
        .arg("check")
        .arg(destination_string(target))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Ask a live master to exit (its child ssh processes/tunnels are
/// unaffected until they close naturally).
pub fn stop_master(socket_path: &Path, target: &HostTarget) -> Result<()> {
    let status = Command::new(ssh_program())
        .arg("-S")
        .arg(socket_path)
        .arg("-O")
        .arg("exit")
        .arg(destination_string(target))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(SshFlowError::Other(format!(
            "failed to stop master for {}",
            target.display()
        )))
    }
}

pub struct MasterStartOutcome {
    pub elapsed_ms: u64,
}

pub enum StartMasterError {
    /// Non-interactive auth was not possible; retry in the foreground.
    NeedsInteractiveAuth,
    Failed(String),
}

/// Start a control-master connection non-interactively (no TTY, no
/// prompts). Suitable for the daemon. Returns
/// `StartMasterError::NeedsInteractiveAuth` if the host requires a
/// password or keyboard-interactive prompt, so the caller can retry
/// in the foreground instead.
pub fn start_master_noninteractive(
    socket_path: &Path,
    target: &HostTarget,
    persist_time: Duration,
    connect_timeout: Duration,
) -> std::result::Result<MasterStartOutcome, StartMasterError> {
    let start = Instant::now();
    let mut cmd = Command::new(ssh_program());
    cmd.arg("-M")
        .arg("-S")
        .arg(socket_path)
        .arg("-o")
        .arg("ControlMaster=auto")
        .arg("-o")
        .arg(format!("ControlPersist={}s", persist_time.as_secs().max(1)))
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg(format!("ConnectTimeout={}", connect_timeout.as_secs().max(1)))
        .arg("-N")
        .arg("-f");
    if let Some(port) = target.port {
        cmd.arg("-p").arg(port.to_string());
    }
    cmd.arg(destination_string(target));
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let output = cmd
        .output()
        .map_err(|e| StartMasterError::Failed(e.to_string()))?;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    if output.status.success() {
        harden_socket_permissions(socket_path);
        Ok(MasterStartOutcome { elapsed_ms })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if needs_interactive_auth(&stderr) {
            Err(StartMasterError::NeedsInteractiveAuth)
        } else {
            Err(StartMasterError::Failed(if stderr.is_empty() {
                "ssh exited with a non-zero status".to_string()
            } else {
                stderr
            }))
        }
    }
}

fn needs_interactive_auth(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("permission denied")
        || s.contains("keyboard-interactive")
        || s.contains("password:")
        // BatchMode=yes makes ssh answer "no" to an unknown/changed
        // host key prompt automatically, which surfaces as this
        // error. We never override StrictHostKeyChecking ourselves,
        // so the correct move is to let the CLI retry in the
        // foreground, where the user gets ssh's normal interactive
        // "are you sure you want to continue connecting?" prompt.
        || s.contains("host key verification failed")
}

/// Start a control-master connection in the foreground, inheriting the
/// current process's stdio so the user can answer a password /
/// FIDO2 / keyboard-interactive prompt themselves.
pub fn start_master_interactive(
    socket_path: &Path,
    target: &HostTarget,
    persist_time: Duration,
) -> Result<()> {
    let mut cmd = Command::new(ssh_program());
    cmd.arg("-M")
        .arg("-S")
        .arg(socket_path)
        .arg("-o")
        .arg("ControlMaster=auto")
        .arg("-o")
        .arg(format!("ControlPersist={}s", persist_time.as_secs().max(1)))
        .arg("-N")
        .arg("-f");
    if let Some(port) = target.port {
        cmd.arg("-p").arg(port.to_string());
    }
    cmd.arg(destination_string(target));
    // Inherit stdio: the user may need to type a password, confirm a
    // host key, or touch a FIDO2 key.
    let status = cmd.status()?;
    if status.success() {
        harden_socket_permissions(socket_path);
        Ok(())
    } else {
        Err(SshFlowError::MasterFailed(format!(
            "could not establish connection to {}",
            target.display()
        )))
    }
}

#[cfg(unix)]
fn harden_socket_permissions(socket_path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(socket_path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(socket_path, perms);
    }
}

#[cfg(not(unix))]
fn harden_socket_permissions(_socket_path: &Path) {}

/// Replace the current process with an interactive `ssh` client that
/// reuses the given control socket for instant, already-authenticated
/// connection reuse. On Unix this calls `exec()` and never returns on
/// success (true PTY/signal passthrough, identical to running `ssh`
/// directly). On Windows it spawns and waits, returning the exit code.
#[cfg(unix)]
pub fn exec_client(socket_path: &Path, target: &HostTarget, extra_args: &[String]) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(ssh_program());
    cmd.arg("-S").arg(socket_path);
    if let Some(port) = target.port {
        cmd.arg("-p").arg(port.to_string());
    }
    cmd.arg(destination_string(target));
    cmd.args(extra_args);
    cmd.exec()
}

#[cfg(windows)]
pub fn exec_client(socket_path: &Path, target: &HostTarget, extra_args: &[String]) -> Result<i32> {
    let mut cmd = Command::new(ssh_program());
    cmd.arg("-S").arg(socket_path);
    if let Some(port) = target.port {
        cmd.arg("-p").arg(port.to_string());
    }
    cmd.arg(destination_string(target));
    cmd.args(extra_args);
    let status = cmd.status()?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_interactive_auth_needed() {
        assert!(needs_interactive_auth("Permission denied (publickey,password)."));
        assert!(needs_interactive_auth("keyboard-interactive authentication required"));
        assert!(needs_interactive_auth("Host key verification failed."));
        assert!(!needs_interactive_auth("ssh: connect to host 10.0.0.1 port 22: Connection timed out"));
        assert!(!needs_interactive_auth("Could not resolve hostname foo: Name or service not known"));
    }
}
