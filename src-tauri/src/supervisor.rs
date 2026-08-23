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
use tuxtop_core::fleet::{watch_host, HostEvent};
use tuxtop_core::procs::{CgroupUsage, ProcInfo, UnitRestarts};
use tuxtop_core::{HostConfig, HostFault, ProcSampler, TrafficCounter, TrafficStats};

/// Event names the frontend subscribes to.
pub const EVENT_SAMPLE: &str = "tuxtop://sample";
pub const EVENT_FAULT: &str = "tuxtop://fault";
pub const EVENT_HOSTS: &str = "tuxtop://hosts-changed";
pub const EVENT_SETTINGS: &str = "tuxtop://settings-changed";
pub const EVENT_PROCS: &str = "tuxtop://processes";

/// How many processes each host ranks and returns.
const PROC_TOP_N: usize = 20;
/// The window the CPU delta is measured over.
const PROC_WINDOW_MS: u32 = 1000;
/// Seconds between process samples. Slower than metrics on purpose: a process
/// list is read, not watched, and each sample costs a second of remote wall
/// clock inside its own window.
const PROC_INTERVAL_SECS: u32 = 5;

/// Payload for [`EVENT_FAULT`]. The host name is included because the frontend
/// routes by it and a bare fault could not be attributed to a card.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FaultEvent {
    pub host: String,
    #[serde(flatten)]
    pub fault: HostFault,
}

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
    // Process samplers, present only while the view is open.
    procs: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
    // Latest ranking per host, merged into one fleet list on read.
    latest: Mutex<HashMap<String, Vec<ProcInfo>>>,
    // Latest cgroup accounting per host, keyed the same way.
    cgroups: Mutex<HashMap<String, Vec<CgroupUsage>>>,
    // Latest restart counts per host. Kept across frames rather than replaced,
    // because the sweep runs on a slower cycle and an empty list means "no new
    // information", never "nothing has restarted".
    restarts: Mutex<HashMap<String, Vec<UnitRestarts>>>,
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
        self.stop_procs_for(name);
        self.latest.lock().unwrap().remove(name);
        self.cgroups.lock().unwrap().remove(name);
        self.restarts.lock().unwrap().remove(name);
    }

    /// Begin process sampling on every host.
    pub fn start_procs(&self, app: AppHandle, hosts: Vec<HostConfig>) {
        for cfg in hosts {
            self.stop_procs_for(&cfg.name);
            let name = cfg.name.clone();
            let handle = tauri::async_runtime::spawn(watch_procs(app.clone(), cfg));
            self.procs.lock().unwrap().insert(name, handle);
        }
    }

    /// Stop sampling everywhere. A view nobody is looking at costs nothing.
    pub fn stop_procs(&self) {
        let mut map = self.procs.lock().unwrap();
        for (_, h) in map.drain() {
            h.abort();
        }
    }

    fn stop_procs_for(&self, name: &str) {
        if let Some(h) = self.procs.lock().unwrap().remove(name) {
            h.abort();
        }
    }

    pub fn record_procs(
        &self,
        host: &str,
        list: Vec<ProcInfo>,
        cgroups: Vec<CgroupUsage>,
        restarts: Vec<UnitRestarts>,
    ) {
        self.latest.lock().unwrap().insert(host.to_string(), list);
        self.cgroups.lock().unwrap().insert(host.to_string(), cgroups);
        // Only replace on a cycle that actually swept.
        if !restarts.is_empty() {
            self.restarts.lock().unwrap().insert(host.to_string(), restarts);
        }
    }

    /// Every host's cgroup accounting, tagged with its host.
    ///
    /// Not merged into one ranking like the processes: a service name is only
    /// unique within a host, and `nginx.service` on two boxes is two things.
    pub fn fleet_cgroups(&self) -> Vec<HostCgroup> {
        self.cgroups
            .lock()
            .unwrap()
            .iter()
            .flat_map(|(host, v)| {
                let r = self.restarts.lock().unwrap();
                let units = r.get(host).cloned().unwrap_or_default();
                v.iter()
                    .map(|u| {
                        let hit = units.iter().find(|x| x.unit == u.name);
                        HostCgroup {
                            host: host.clone(),
                            restarts: hit.map(|x| x.total).unwrap_or(0),
                            restarts_since_seen: hit.map(|x| x.since_seen).unwrap_or(0),
                            usage: u.clone(),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Every host's latest ranking, merged and re-sorted as one fleet list.
    ///
    /// Sorting happens here rather than per host: the whole point is that the
    /// busiest process in the fleet floats to the top regardless of which
    /// machine it is on.
    pub fn fleet_procs(&self) -> Vec<ProcInfo> {
        let mut all: Vec<ProcInfo> = self
            .latest
            .lock()
            .unwrap()
            .values()
            .flat_map(|v| v.iter().cloned())
            .collect();
        all.sort_by(|a, b| {
            b.cpu_pct
                .partial_cmp(&a.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.rss_kb.cmp(&a.rss_kb))
        });
        all
    }
}

/// One host's process sampler, restarted with backoff if the channel drops.
async fn watch_procs(app: AppHandle, cfg: HostConfig) {
    loop {
        let (tx, mut rx) = mpsc::channel(4);
        let sampler = match ProcSampler::start(
            cfg.clone(),
            PROC_TOP_N,
            PROC_WINDOW_MS,
            PROC_INTERVAL_SECS,
            tx,
        ) {
            Ok(s) => s,
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
        };

        while let Some(frame) = rx.recv().await {
            app.state::<Supervisor>()
                .record_procs(&cfg.name, frame.procs, frame.cgroups, frame.restarts);
            // The view pulls; this only says something changed.
            let _ = app.emit(EVENT_PROCS, &cfg.name);
        }

        sampler.stop().await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// One cgroup, tagged with the host it lives on.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostCgroup {
    pub host: String,
    /// `NRestarts` as systemd reports it — no recency at all.
    pub restarts: u32,
    /// Restarts since Tuxtop first saw the unit. The actionable half.
    pub restarts_since_seen: u32,
    #[serde(flatten)]
    pub usage: CgroupUsage,
}

/// One host's measured cost, for the settings meter.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostTraffic {
    pub host: String,
    pub interval_secs: u32,
    #[serde(flatten)]
    pub stats: TrafficStats,
}

/// Feed one host's events to the frontend.
///
/// The watching itself lives in `tuxtop_core::fleet` so it can be tested; this
/// is only the adapter that turns its events into Tauri emits and history
/// writes.
///
/// `select!` rather than `join!`: if the webview goes away the forwarder ends,
/// and dropping the watcher with it drops the `SshSampler`, which is
/// `kill_on_drop` — so a closed window does not leave an ssh process running
/// against every host in the fleet.
async fn watch(app: AppHandle, cfg: HostConfig, interval_secs: u32, traffic: Arc<TrafficCounter>) {
    // Annotated: inference otherwise picks `str` for the name, because the
    // only use of it is a `&str` argument.
    let (tx, mut rx) = mpsc::channel::<(String, HostEvent)>(16);

    let forward = async {
        while let Some((host, event)) = rx.recv().await {
            match event {
                HostEvent::Sample(sample) => {
                    // Record before emitting: history must not depend on a
                    // webview being attached to receive it.
                    app.state::<crate::history_store::HistoryStore>()
                        .record(&sample);
                    if app.emit(EVENT_SAMPLE, &*sample).is_err() {
                        return; // the webview is gone; nothing left to feed
                    }
                }
                HostEvent::Fault(fault) => emit_fault(&app, &host, fault),
            }
        }
    };

    tokio::select! {
        _ = watch_host(cfg, interval_secs, traffic, tx) => {}
        _ = forward => {}
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
