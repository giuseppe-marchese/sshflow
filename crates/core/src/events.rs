use serde::{Deserialize, Serialize};

/// Events emitted by the daemon, consumed by the plugin system and by
/// the metrics engine. Kept intentionally flat/serializable so plugins
/// (external hook scripts) can receive them as plain JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Event {
    ConnectionOpened {
        key: String,
        host: String,
    },
    ConnectionClosed {
        key: String,
        host: String,
    },
    ConnectionReused {
        key: String,
        host: String,
    },
    HostAdded {
        host: String,
    },
    HostRemoved {
        host: String,
    },
    AuthenticationFailed {
        host: String,
        reason: String,
    },
    Reconnect {
        key: String,
        host: String,
        attempt: u32,
    },
}

impl Event {
    pub fn name(&self) -> &'static str {
        match self {
            Event::ConnectionOpened { .. } => "ConnectionOpened",
            Event::ConnectionClosed { .. } => "ConnectionClosed",
            Event::ConnectionReused { .. } => "ConnectionReused",
            Event::HostAdded { .. } => "HostAdded",
            Event::HostRemoved { .. } => "HostRemoved",
            Event::AuthenticationFailed { .. } => "AuthenticationFailed",
            Event::Reconnect { .. } => "Reconnect",
        }
    }
}
