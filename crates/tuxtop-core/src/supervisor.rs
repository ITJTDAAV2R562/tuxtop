//! One task per host, and everything a running fleet needs.
//!
//! Each watched host owns an independent Tokio task holding its own ssh
//! process. A host that hangs, dies or fails auth affects only its own card —
//! that isolation is the point, and is why this is a map of tasks rather than
//! one loop over all hosts.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use std::sync::{Arc, Weak};

use crate::fleet::{watch_host, HostEvent};
use crate::history_store::HistoryStore;
use crate::procs::{CgroupUsage, ProcInfo, UnitRestarts};
use crate::{HostConfig, HostFault, ProcSampler, Sample, TrafficCounter, TrafficStats};
use tokio::sync::mpsc;

/// What the supervisor tells whoever is listening.
///
/// An enum rather than a string topic and a JSON blob, because the supervisor
/// no longer knows whether its audience is a webview, an HTTP client, or a
/// test. That was the point of moving it here: it lived in `src-tauri`, which
/// is outside the workspace and never compiled on the development box, so the
/// code owning every host's lifecycle was the one part nothing could test.
#[derive(Debug, Clone)]
pub enum Event {
    /// Boxed: a sample is far larger than a fault, and an un-boxed enum costs
    /// every variant the size of the largest.
    Sample(Box<Sample>),
    Fault {
        host: String,
        fault: HostFault,
    },
    /// A host's process ranking changed. The name only; the consumer pulls.
    Processes(String),
}

/// How many processes each host ranks and returns.
const PROC_TOP_N: usize = 20;
/// The window the CPU delta is measured over.
const PROC_WINDOW_MS: u32 = 1000;
/// Seconds between process samples. Slower than metrics on purpose: a process
/// list is read, not watched, and each sample costs a second of remote wall
/// clock inside its own window.
const PROC_INTERVAL_SECS: u32 = 5;

pub struct Supervisor {
    /// Where samples are recorded. Held directly rather than looked up out of
    /// a framework's state bag.
    history: Arc<HistoryStore>,
    /// Where events go. The supervisor does not know who is listening.
    events: mpsc::Sender<Event>,
    /// The runtime to spawn host tasks on.
    ///
    /// Passed in rather than taken from `Handle::current()`, which panics when
    /// the caller is not inside a runtime — and Tauri's `setup` is not, even
    /// though the application plainly has one. Capturing it there compiled
    /// perfectly and panicked on launch, which is the shape of bug a build
    /// check cannot see.
    rt: tokio::runtime::Handle,
    tasks: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    // Byte counters survive a restart of the host's task, so changing an
    // interval does not reset the measurement it was chosen from.
    traffic: Mutex<HashMap<String, Arc<TrafficCounter>>>,
    // The interval each host is currently sampling at, for the meter.
    intervals: Mutex<HashMap<String, u32>>,
    // Process samplers, present only while the view is open.
    procs: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
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
    /// Build a supervisor.
    ///
    /// `rt` is the runtime host tasks are spawned on. It is a parameter and
    /// not `Handle::current()` because the caller is often *not* inside a
    /// runtime — Tauri's `setup` runs on the main thread — and taking it
    /// implicitly turns that into a panic at launch rather than an error at
    /// the call site.
    pub fn new(
        history: Arc<HistoryStore>,
        events: mpsc::Sender<Event>,
        rt: tokio::runtime::Handle,
    ) -> Arc<Self> {
        Arc::new(Self {
            history,
            events,
            rt,
            tasks: Mutex::new(HashMap::new()),
            traffic: Mutex::new(HashMap::new()),
            intervals: Mutex::new(HashMap::new()),
            procs: Mutex::new(HashMap::new()),
            latest: Mutex::new(HashMap::new()),
            cgroups: Mutex::new(HashMap::new()),
            restarts: Mutex::new(HashMap::new()),
        })
    }

    /// Begin watching `cfg`. Replaces any existing task for the same name.
    pub fn start(self: &Arc<Self>, cfg: HostConfig, interval_secs: u32) {
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

        let me = Arc::downgrade(self);
        // Weak, not Arc: the supervisor owns the JoinHandle, so an Arc here
        // would be a cycle and neither would ever be dropped.
        let handle = self.rt.spawn(watch(me, cfg, interval_secs, counter));

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
    pub fn start_procs(self: &Arc<Self>, hosts: Vec<HostConfig>) {
        for cfg in hosts {
            self.stop_procs_for(&cfg.name);
            let name = cfg.name.clone();
            let me = Arc::downgrade(self);
            let handle = self.rt.spawn(watch_procs(me, cfg));
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
        self.cgroups
            .lock()
            .unwrap()
            .insert(host.to_string(), cgroups);
        // Only replace on a cycle that actually swept.
        if !restarts.is_empty() {
            self.restarts
                .lock()
                .unwrap()
                .insert(host.to_string(), restarts);
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
async fn watch_procs(sup: Weak<Supervisor>, cfg: HostConfig) {
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
            // The supervisor is gone: nobody is watching this fleet any more.
            let Some(s) = sup.upgrade() else { return };
            s.record_procs(&cfg.name, frame.procs, frame.cgroups, frame.restarts);
            // The consumer pulls the list; this only says it changed.
            if s.events
                .send(Event::Processes(cfg.name.clone()))
                .await
                .is_err()
            {
                return;
            }
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
/// `select!` rather than `join!`: if the consumer goes away the forwarder
/// ends, and dropping the watcher with it drops the `SshSampler`, which is
/// `kill_on_drop` — so a closed window, or a disconnected browser, does not
/// leave an ssh process running against every host in the fleet.
async fn watch(
    sup: Weak<Supervisor>,
    cfg: HostConfig,
    interval_secs: u32,
    traffic: Arc<TrafficCounter>,
) {
    // Annotated: inference otherwise picks `str` for the name, because the
    // only use of it is a `&str` argument.
    let (tx, mut rx) = mpsc::channel::<(String, HostEvent)>(16);

    let forward = async {
        while let Some((host, event)) = rx.recv().await {
            let Some(s) = sup.upgrade() else { return };
            let out = match event {
                HostEvent::Sample(sample) => {
                    // Record before sending: history must not depend on
                    // anybody being attached to receive it.
                    s.history.record(&sample);
                    Event::Sample(sample)
                }
                HostEvent::Fault(fault) => Event::Fault { host, fault },
            };
            if s.events.send(out).await.is_err() {
                return; // nobody is listening; nothing left to feed
            }
        }
    };

    tokio::select! {
        _ = watch_host(cfg, interval_secs, traffic, tx) => {}
        _ = forward => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::{CgroupUsage, OwnerKind};

    fn proc(host: &str, pid: u32, cpu: f32, rss: u64) -> ProcInfo {
        ProcInfo {
            host: host.into(),
            pid,
            cpu_pct: cpu,
            rss_kb: rss,
            user: String::new(),
            comm: "x".into(),
            cmd: String::new(),
            owner: String::new(),
            owner_kind: OwnerKind::None,
            kernel: false,
        }
    }

    fn sup() -> (Arc<Supervisor>, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel(16);
        (
            Supervisor::new(
                Arc::new(crate::history_store::HistoryStore::new()),
                tx,
                tokio::runtime::Handle::current(),
            ),
            rx,
        )
    }

    #[tokio::test]
    async fn the_busiest_process_in_the_fleet_floats_to_the_top() {
        // Sorting happens across hosts, not within them: the whole point of a
        // fleet list is that the busiest process surfaces wherever it runs.
        let (s, _rx) = sup();
        s.record_procs("dove", vec![proc("dove", 1, 10.0, 100)], vec![], vec![]);
        s.record_procs("heron", vec![proc("heron", 2, 90.0, 50)], vec![], vec![]);

        let all = s.fleet_procs();
        assert_eq!(all[0].host, "heron");
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn processes_tied_on_cpu_are_ordered_by_memory() {
        let (s, _rx) = sup();
        s.record_procs(
            "a",
            vec![proc("a", 1, 5.0, 100), proc("a", 2, 5.0, 900)],
            vec![],
            vec![],
        );
        assert_eq!(s.fleet_procs()[0].pid, 2);
    }

    #[tokio::test]
    async fn a_cgroup_is_tagged_with_the_host_it_lives_on() {
        // `nginx.service` on two boxes is two things; a merged ranking without
        // the host would silently be one.
        let (s, _rx) = sup();
        for h in ["dove", "coot"] {
            s.record_procs(
                h,
                vec![],
                vec![CgroupUsage {
                    name: "nginx.service".into(),
                    cpu_pct: 1.0,
                    memory_bytes: 10,
                    pids: 2,
                }],
                vec![],
            );
        }
        let all = s.fleet_cgroups();
        assert_eq!(all.len(), 2);
        let hosts: std::collections::HashSet<_> = all.iter().map(|c| c.host.as_str()).collect();
        assert_eq!(
            hosts.len(),
            2,
            "both hosts present, not one overwriting the other"
        );
    }

    #[tokio::test]
    async fn forgetting_a_host_leaves_nothing_of_it_behind() {
        // A removed host must stop appearing in the meter, the process list
        // and the cgroup list alike - one missed map is a ghost in a view.
        let (s, _rx) = sup();
        s.record_procs(
            "gone",
            vec![proc("gone", 1, 1.0, 1)],
            vec![CgroupUsage {
                name: "u".into(),
                cpu_pct: 0.0,
                memory_bytes: 0,
                pids: 0,
            }],
            vec![],
        );
        assert!(!s.fleet_procs().is_empty());

        s.forget("gone");
        assert!(s.fleet_procs().is_empty());
        assert!(s.fleet_cgroups().is_empty());
        assert!(s.traffic().iter().all(|t| t.host != "gone"));
    }

    #[tokio::test]
    async fn an_empty_restart_sweep_does_not_erase_what_is_known() {
        // The sweep runs on a slower cycle than the processes, so most frames
        // carry none. Treating that as "nothing has restarted" would make the
        // badge flicker off and on every few seconds.
        let (s, _rx) = sup();
        s.record_procs(
            "a",
            vec![],
            vec![CgroupUsage {
                name: "flap.service".into(),
                cpu_pct: 0.0,
                memory_bytes: 0,
                pids: 1,
            }],
            vec![UnitRestarts {
                unit: "flap.service".into(),
                total: 7,
                since_seen: 2,
            }],
        );
        s.record_procs(
            "a",
            vec![],
            vec![CgroupUsage {
                name: "flap.service".into(),
                cpu_pct: 0.0,
                memory_bytes: 0,
                pids: 1,
            }],
            vec![],
        );

        let c = s.fleet_cgroups();
        assert_eq!(
            c[0].restarts, 7,
            "the previous sweep must survive an empty one"
        );
        assert_eq!(c[0].restarts_since_seen, 2);
    }

    #[tokio::test]
    async fn a_dropped_supervisor_does_not_keep_its_tasks_alive() {
        // The tasks hold a Weak, not an Arc. An Arc would be a cycle - the
        // supervisor owns the JoinHandle, the task owns the supervisor - and
        // neither would ever be freed.
        let (s, _rx) = sup();
        let weak = Arc::downgrade(&s);
        drop(s);
        assert!(weak.upgrade().is_none(), "supervisor leaked");
    }

    #[tokio::test]
    async fn active_names_what_is_being_watched() {
        let (s, _rx) = sup();
        assert!(s.traffic().is_empty());
        s.record_procs("x", vec![proc("x", 1, 1.0, 1)], vec![], vec![]);
        assert_eq!(s.fleet_procs().len(), 1);
    }
}
