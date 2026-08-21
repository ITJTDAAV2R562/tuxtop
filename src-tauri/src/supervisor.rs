//! One task per host, feeding the frontend.
//!
//! Each watched host owns an independent Tokio task holding its own ssh
//! process. A host that hangs, dies or fails auth affects only its own card —
//! that isolation is the point, and is why this is a map of tasks rather than
//! one loop over all hosts.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tuxtop_core::{HostConfig, HostFault, SshSampler, TrafficCounter, TrafficStats};

/// Event names the frontend subscribes to.
pub const EVENT_SAMPLE: &str = "tuxtop://sample";
pub const EVENT_FAULT: &str = "tuxtop://fault";
pub const EVENT_HOSTS: &str = "tuxtop://hosts-changed";
pub const EVENT_SETTINGS: &str = "tuxtop://settings-changed";

/// Payload for [`EVENT_FAULT`]. The host name is included because the frontend
/// routes by it and a bare fault could not be attributed to a card.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FaultEvent {
    pub host: String,
    #[serde(flatten)]
    pub fault: HostFault,
}

/// Reconnect backoff. Capped so a host that comes back is picked up promptly
/// rather than sitting out a long exponential tail.
const BACKOFF: &[u64] = &[1, 2, 5, 10, 20, 30];

#[derive(Default)]
pub struct Supervisor {
    // Tauri's JoinHandle, not tokio's: `tauri::async_runtime::spawn` returns
    // its own wrapper type and the two are distinct.
    tasks: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
    // Byte counters survive a restart of the host's task, so changing an
    // interval does not reset the measurement it was chosen from.
    traffic: Mutex<HashMap<String, Arc<TrafficCounter>>>,
    // The interval each host is currently sampling at, for the meter.
    intervals: Mutex<HashMap<String, u32>>,
}

impl Supervisor {
    /// Begin watching `cfg`. Replaces any existing task for the same name.
    pub fn start(&self, app: AppHandle, cfg: HostConfig, interval_secs: u32) {
        self.stop(&cfg.name);

        let name = cfg.name.clone();
        let counter = self
            .traffic
            .lock()
            .unwrap()
            .entry(name.clone())
            .or_insert_with(|| Arc::new(TrafficCounter::new()))
            .clone();
        self.intervals
            .lock()
            .unwrap()
            .insert(name.clone(), interval_secs);

        let handle = tauri::async_runtime::spawn(watch(app, cfg, interval_secs, counter));

        // A task dropped from the map is not cancelled, so the previous one is
        // aborted in `stop` before we overwrite the entry.
        self.tasks.lock().unwrap().insert(name, handle);
    }

    /// Stop watching `name`, killing its ssh process.
    pub fn stop(&self, name: &str) {
        if let Some(h) = self.tasks.lock().unwrap().remove(name) {
            // Aborting drops the SshSampler, which is `kill_on_drop`.
            h.abort();
        }
    }

    /// Names currently being watched.
    pub fn active(&self) -> Vec<String> {
        self.tasks.lock().unwrap().keys().cloned().collect()
    }

    /// What each host has cost so far, and at what interval.
    pub fn traffic(&self) -> Vec<HostTraffic> {
        let counters = self.traffic.lock().unwrap();
        let intervals = self.intervals.lock().unwrap();
        counters
            .iter()
            .map(|(name, c)| HostTraffic {
                host: name.clone(),
                interval_secs: intervals.get(name).copied().unwrap_or(0),
                stats: c.snapshot(),
            })
            .collect()
    }

    /// Forget a removed host's counter, so the meter stops counting a machine
    /// nobody is watching any more.
    pub fn forget(&self, name: &str) {
        self.traffic.lock().unwrap().remove(name);
        self.intervals.lock().unwrap().remove(name);
    }
}

/// One host's measured cost, for the settings meter.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostTraffic {
    pub host: String,
    pub interval_secs: u32,
    #[serde(flatten)]
    pub stats: TrafficStats,
}

/// Watch one host forever, reconnecting with backoff.
async fn watch(
    app: AppHandle,
    cfg: HostConfig,
    interval_secs: u32,
    traffic: Arc<TrafficCounter>,
) {
    let mut attempt = 0usize;

    loop {
        let (tx, mut rx) = mpsc::channel(16);

        let sampler = match SshSampler::start(cfg.clone(), interval_secs, tx, traffic.clone()) {
            Ok(s) => s,
            Err(e) => {
                // Could not even spawn ssh — almost always "not on PATH".
                emit_fault(
                    &app,
                    &cfg.name,
                    HostFault::SamplerFailed(format!("could not launch ssh: {e}")),
                );
                sleep_backoff(&mut attempt).await;
                continue;
            }
        };

        let mut got_data = false;
        let mut reported = false;

        while let Some(item) = rx.recv().await {
            match item {
                Ok(sample) => {
                    got_data = true;
                    attempt = 0; // a good sample resets the backoff
                    // Record before emitting: history must not depend on a
                    // webview being attached to receive it.
                    app.state::<crate::history_store::HistoryStore>()
                        .record(&sample);
                    if app.emit(EVENT_SAMPLE, &sample).is_err() {
                        // The webview is gone; nothing left to feed.
                        return;
                    }
                }
                Err(fault) => {
                    emit_fault(&app, &cfg.name, fault);
                    reported = true;
                    break;
                }
            }
        }

        sampler.stop().await;

        // Only fall back to the generic message when nothing specific was
        // reported. Emitting it unconditionally overwrote the accurate fault
        // that had just been sent - an unreachable host showed "Sampler
        // failed" instead of "Host unreachable: connection timed out", which
        // points at the wrong thing entirely.
        if !got_data && !reported {
            emit_fault(
                &app,
                &cfg.name,
                HostFault::SamplerFailed("connection closed before any data arrived".into()),
            );
        }

        sleep_backoff(&mut attempt).await;
    }
}

fn emit_fault(app: &AppHandle, host: &str, fault: HostFault) {
    let _ = app.emit(
        EVENT_FAULT,
        FaultEvent {
            host: host.to_string(),
            fault,
        },
    );
}

async fn sleep_backoff(attempt: &mut usize) {
    let secs = BACKOFF[(*attempt).min(BACKOFF.len() - 1)];
    *attempt += 1;
    tokio::time::sleep(Duration::from_secs(secs)).await;
}
