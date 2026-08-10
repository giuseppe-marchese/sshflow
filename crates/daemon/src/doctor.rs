use crate::state::DaemonState;
use sshflow_core::{DoctorCheck, DoctorReport};

pub fn run(state: &DaemonState) -> DoctorReport {
    let mut checks = Vec::new();

    match sshflow_ssh::detect_ssh() {
        Some((path, version)) => checks.push(DoctorCheck {
            name: "OpenSSH client".into(),
            ok: true,
            detail: format!("{} ({})", version, path.display()),
        }),
        None => checks.push(DoctorCheck {
            name: "OpenSSH client".into(),
            ok: false,
            detail: "could not find or execute 'ssh' on PATH".into(),
        }),
    }

    checks.push(DoctorCheck {
        name: "Config file".into(),
        ok: true,
        detail: sshflow_config::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unresolvable".into()),
    });

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let home_ok = std::fs::metadata(&state.home)
            .map(|m| m.permissions().mode() & 0o777 == 0o700)
            .unwrap_or(false);
        checks.push(DoctorCheck {
            name: "~/.sshflow permissions".into(),
            ok: home_ok,
            detail: if home_ok {
                "0700 (owner-only)".into()
            } else {
                "not restricted to owner-only -- run `chmod 700 ~/.sshflow`".into()
            },
        });

        let is_root = unsafe { libc::geteuid() } == 0;
        checks.push(DoctorCheck {
            name: "Running as non-root".into(),
            ok: !is_root,
            detail: if is_root {
                "daemon is running as root -- violates least-privilege; run as a normal user".into()
            } else {
                "OK".into()
            },
        });
    }

    checks.push(DoctorCheck {
        name: "maxConnections".into(),
        ok: state.config.max_connections > 0,
        detail: state.config.max_connections.to_string(),
    });

    match state.config.health_check_duration() {
        Ok(d) => checks.push(DoctorCheck {
            name: "healthCheck interval".into(),
            ok: true,
            detail: format!("{}s", d.as_secs()),
        }),
        Err(e) => checks.push(DoctorCheck {
            name: "healthCheck interval".into(),
            ok: false,
            detail: e.to_string(),
        }),
    }

    let active = state.connections.len();
    checks.push(DoctorCheck {
        name: "Active managed connections".into(),
        ok: true,
        detail: format!("{active} / {}", state.config.max_connections),
    });

    checks.push(DoctorCheck {
        name: "Plugins".into(),
        ok: true,
        detail: format!("{} loaded", state.plugins.len()),
    });

    {
        use sysinfo::System;
        let mut sys = System::new();
        sys.refresh_memory();
        let available_mb = sys.available_memory() / 1024 / 1024;
        checks.push(DoctorCheck {
            name: "System memory".into(),
            ok: available_mb > 32,
            detail: format!("{available_mb} MB available"),
        });
    }

    DoctorReport { checks }
}
