//! sshflow-plugins
//!
//! Extensibility without recompiling *and* without the security risk
//! of dynamically loading arbitrary native code (`dlopen`/`libloading`
//! of untrusted `.so`/`.dll` files running in-process as the daemon).
//! Instead, a plugin is a small YAML file describing an external
//! command to run when specific events fire. The event is delivered as
//! JSON on the child process's stdin. This keeps every plugin in its
//! own process, sandboxed by the OS the same way any other subprocess
//! is, and keeps the daemon itself simple to audit.
//!
//! Example `~/.sshflow/plugins/notify.yaml`:
//! ```yaml
//! name: notify-on-failure
//! events: ["AuthenticationFailed"]
//! command: /usr/local/bin/notify-send-wrapper.sh
//! ```

use serde::Deserialize;
use sshflow_core::Event;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct PluginDef {
    pub name: String,
    /// Event names to subscribe to (see `Event::name`). Empty means
    /// "all events".
    #[serde(default)]
    pub events: Vec<String>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

pub struct PluginManager {
    plugins: Vec<PluginDef>,
}

impl PluginManager {
    /// Loads every `*.yaml` file in `dir` as a plugin definition.
    /// Malformed files are logged and skipped rather than failing
    /// daemon startup.
    pub fn load(dir: &Path) -> Self {
        let mut plugins = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    continue;
                }
                match std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|raw| serde_yaml::from_str::<PluginDef>(&raw).ok())
                {
                    Some(def) => {
                        tracing::info!(plugin = %def.name, "loaded plugin");
                        plugins.push(def);
                    }
                    None => {
                        tracing::warn!(path = %path.display(), "failed to parse plugin definition, skipping")
                    }
                }
            }
        }
        PluginManager { plugins }
    }

    pub fn empty() -> Self {
        PluginManager { plugins: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Fire-and-forget dispatch to every plugin subscribed to this
    /// event. Never blocks on plugin completion and never panics the
    /// caller -- a broken plugin hook must not take the daemon down.
    pub fn dispatch(&self, event: &Event) {
        let payload = match serde_json::to_string(event) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize event for plugins");
                return;
            }
        };
        for plugin in &self.plugins {
            if !plugin.events.is_empty() && !plugin.events.iter().any(|e| e == event.name()) {
                continue;
            }
            if let Err(e) = run_hook(plugin, &payload) {
                tracing::warn!(plugin = %plugin.name, error = %e, "plugin hook failed to start");
            }
        }
    }
}

fn run_hook(plugin: &PluginDef, payload: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // No shell involved: `command` and `args` are passed straight to
    // execvp/CreateProcess, so plugin YAML cannot smuggle in extra
    // shell syntax even if it wanted to.
    let mut child = Command::new(&plugin.command)
        .args(&plugin.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
    }
    // Fire-and-forget: reap the child on a background thread so it
    // never becomes a zombie, without blocking the caller.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
