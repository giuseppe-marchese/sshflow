//! sshflow-metrics
//!
//! Purely local, purely in-file metrics: connection counts, reuse
//! rate, estimated handshake time saved, per-host usage. Nothing here
//! ever leaves the machine -- there is no network client in this
//! crate, matching the "no telemetry by default" requirement.

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sshflow_core::{now_unix, Event, StatsSnapshot};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SECS_PER_DAY: u64 = 86_400;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DailyTotals {
    connections: u64,
    reused: u64,
    failures: u64,
    reconnects: u64,
    time_saved_ms: u64,
    latency_sum_ms: u64,
    latency_samples: u64,
    bytes_transferred: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HostStat {
    label: String,
    connections: u64,
    reused: u64,
    failures: u64,
    last_latency_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedState {
    day_index: u64,
    baseline_handshake_ms: u64,
    totals: DailyTotals,
    per_host: HashMap<String, HostStat>,
}

pub struct MetricsStore {
    path: PathBuf,
    day_index: RwLock<u64>,
    baseline_handshake_ms: RwLock<u64>,
    totals: RwLock<DailyTotals>,
    per_host: DashMap<String, HostStat>,
}

fn today_index() -> u64 {
    now_unix() / SECS_PER_DAY
}

impl MetricsStore {
    /// Load persisted metrics from `<home>/metrics.json`, resetting the
    /// "today" counters if the stored day differs from today (UTC day
    /// boundary, computed as `unix_seconds / 86400` -- no calendar
    /// library required for that).
    pub fn load(home: &Path) -> Self {
        let path = home.join("metrics.json");
        let today = today_index();

        let loaded: PersistedState = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();

        let (day_index, totals) = if loaded.day_index == today {
            (loaded.day_index, loaded.totals)
        } else {
            (today, DailyTotals::default())
        };

        let per_host = DashMap::new();
        for (k, v) in loaded.per_host {
            per_host.insert(k, v);
        }

        let store = MetricsStore {
            path,
            day_index: RwLock::new(day_index),
            baseline_handshake_ms: RwLock::new(loaded.baseline_handshake_ms.max(1)),
            totals: RwLock::new(totals),
            per_host,
        };
        store.persist();
        store
    }

    fn roll_day_if_needed(&self) {
        let today = today_index();
        let mut day = self.day_index.write();
        if *day != today {
            *day = today;
            *self.totals.write() = DailyTotals::default();
        }
    }

    /// Record a connection: either a freshly-established control
    /// master (`reused = false`, `latency_ms` = handshake time) or a
    /// reuse of an already-alive master (`reused = true`, `latency_ms`
    /// = the near-instant `-O check` round trip).
    pub fn record_connection(&self, key: &str, host_label: &str, reused: bool, latency_ms: u64) {
        self.roll_day_if_needed();

        {
            let mut totals = self.totals.write();
            totals.connections += 1;
            totals.latency_sum_ms += latency_ms;
            totals.latency_samples += 1;
            if reused {
                totals.reused += 1;
                let baseline = *self.baseline_handshake_ms.read();
                totals.time_saved_ms += baseline.saturating_sub(latency_ms);
            } else {
                // Update the rolling handshake-time baseline used to
                // estimate time saved by future reuses.
                let mut baseline = self.baseline_handshake_ms.write();
                *baseline = (*baseline + latency_ms) / 2;
            }
        }

        self.per_host
            .entry(key.to_string())
            .and_modify(|s| {
                s.connections += 1;
                if reused {
                    s.reused += 1;
                }
                s.last_latency_ms = latency_ms;
            })
            .or_insert(HostStat {
                label: host_label.to_string(),
                connections: 1,
                reused: if reused { 1 } else { 0 },
                failures: 0,
                last_latency_ms: latency_ms,
            });

        self.persist();
    }

    pub fn record_failure(&self, key: &str, host_label: &str) {
        self.roll_day_if_needed();
        self.totals.write().failures += 1;
        self.per_host
            .entry(key.to_string())
            .and_modify(|s| s.failures += 1)
            .or_insert(HostStat {
                label: host_label.to_string(),
                failures: 1,
                ..Default::default()
            });
        self.persist();
    }

    pub fn record_reconnect(&self) {
        self.roll_day_if_needed();
        self.totals.write().reconnects += 1;
        self.persist();
    }

    pub fn add_bytes(&self, n: u64) {
        self.totals.write().bytes_transferred += n;
    }

    /// Fold a plugin-visible `Event` into the metrics store. Kept
    /// separate from `record_connection`/`record_failure` because
    /// those carry richer data (latency); this is the coarse path used
    /// when only the event itself is available.
    pub fn record_event(&self, event: &Event) {
        match event {
            Event::AuthenticationFailed { host, .. } => self.record_failure(host, host),
            Event::Reconnect { .. } => self.record_reconnect(),
            _ => {}
        }
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        self.roll_day_if_needed();
        let totals = self.totals.read().clone();
        let avg_latency = if totals.latency_samples > 0 {
            totals.latency_sum_ms as f64 / totals.latency_samples as f64
        } else {
            0.0
        };
        let mut most_used: Vec<(String, u64)> = self
            .per_host
            .iter()
            .map(|e| (e.value().label.clone(), e.value().connections))
            .collect();
        most_used.sort_by(|a, b| b.1.cmp(&a.1));
        most_used.truncate(5);

        StatsSnapshot {
            connections_today: totals.connections,
            reused_today: totals.reused,
            failures_today: totals.failures,
            reconnects_today: totals.reconnects,
            time_saved_ms: totals.time_saved_ms,
            average_latency_ms: avg_latency,
            bytes_transferred: totals.bytes_transferred,
            most_used_hosts: most_used,
        }
    }

    fn persist(&self) {
        let state = PersistedState {
            day_index: *self.day_index.read(),
            baseline_handshake_ms: *self.baseline_handshake_ms.read(),
            totals: self.totals.read().clone(),
            per_host: self
                .per_host
                .iter()
                .map(|e| (e.key().clone(), e.value().clone()))
                .collect(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&state) {
            if let Err(e) = std::fs::write(&self.path, json) {
                tracing::warn!(error = %e, "failed to persist metrics");
            }
        }
    }
}

/// Formats a millisecond duration as e.g. `12m 41s` for CLI output,
/// matching the spec's example (`Time saved: 12m 41s`).
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    if mins > 0 {
        format!("{mins}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "sshflow-metrics-test-{}-{}-{}",
            std::process::id(),
                                                    now_unix(),
                                                    unique
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn records_fresh_and_reused_connections() {
        let home = temp_home();
        let store = MetricsStore::load(&home);

        store.record_connection("k1", "host1", false, 200);
        store.record_connection("k1", "host1", true, 5);
        store.record_connection("k1", "host1", true, 4);

        let snap = store.snapshot();
        assert_eq!(snap.connections_today, 3);
        assert_eq!(snap.reused_today, 2);
        assert!(snap.average_latency_ms > 0.0);
        assert_eq!(snap.most_used_hosts[0].0, "host1");
        assert_eq!(snap.most_used_hosts[0].1, 3);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn tracks_failures_separately_from_connections() {
        let home = temp_home();
        let store = MetricsStore::load(&home);

        store.record_failure("k1", "host1");
        store.record_failure("k1", "host1");

        let snap = store.snapshot();
        assert_eq!(snap.failures_today, 2);
        assert_eq!(snap.connections_today, 0);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn persists_and_reloads_state() {
        let home = temp_home();
        {
            let store = MetricsStore::load(&home);
            store.record_connection("k1", "host1", false, 150);
        }
        let reloaded = MetricsStore::load(&home);
        let snap = reloaded.snapshot();
        assert_eq!(snap.connections_today, 1);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn format_duration_matches_spec_style() {
        assert_eq!(format_duration_ms(761_000), "12m 41s");
        assert_eq!(format_duration_ms(24_000), "24s");
        assert_eq!(format_duration_ms(78), "78ms");
    }
}
