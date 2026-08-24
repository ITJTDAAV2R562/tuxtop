//! Everything the two shells can do, in one place.
//!
//! The desktop app and a headless server should not each own a copy of "add a
//! host": they are the same operation, differing only in how the request
//! arrives. So the operations live here and both shells are thin dispatchers
//! over them — which is also the first time this logic has been reachable by a
//! test.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::config::Config;
use crate::history::Point;
use crate::history_store::{now_secs, HistoryStore, HistoryUsage};
use crate::hostlist::{self, effective_interval, Settings};
use crate::procs::ProcInfo;
use crate::supervisor::{Event, HostCgroup, HostTraffic, Supervisor};
use crate::HostConfig;

/// Bounds taken from the settings UI, applied here so a request that did not
/// come from that UI cannot exceed them.
const MIN_INTERVAL: u32 = 1;
const MAX_INTERVAL: u32 = 3600;
const MIN_CAP_MB: u32 = 16;
const MAX_CAP_MB: u32 = 8192;
/// A chart cannot draw more points than it has pixels, and a caller asking for
/// millions would allocate them all first.
const MAX_POINTS: usize = 4096;

pub struct Service {
    config: Config,
    sup: Arc<Supervisor>,
    history: Arc<HistoryStore>,
    events: mpsc::Sender<Event>,
}

impl Service {
    pub fn new(
        config: Config,
        sup: Arc<Supervisor>,
        history: Arc<HistoryStore>,
        events: mpsc::Sender<Event>,
    ) -> Self {
        Self {
            config,
            sup,
            history,
            events,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn history(&self) -> &Arc<HistoryStore> {
        &self.history
    }

    /// Start watching everything in the config, and apply the memory cap.
    ///
    /// The cap is applied before any sampling begins, so the store is never
    /// briefly uncapped on a fleet large enough to need it.
    pub fn start_all(&self) -> Result<Settings, String> {
        let f = self.config.load_file()?;
        self.history.set_cap_mb(f.settings.history_cap_mb);
        for cfg in f.hosts {
            let iv = effective_interval(&cfg, &f.settings);
            self.sup.start(cfg, iv);
        }
        Ok(f.settings)
    }

    fn announce_hosts(&self, hosts: &[HostConfig]) {
        // try_send, not send: this is called from synchronous command paths,
        // and a slow consumer must not be able to block a host being added.
        let _ = self.events.try_send(Event::HostsChanged(hosts.to_vec()));
    }

    pub fn list_hosts(&self) -> Result<Vec<HostConfig>, String> {
        self.config.load()
    }

    pub fn add_host(&self, cfg: HostConfig) -> Result<Vec<HostConfig>, String> {
        let mut all = self.config.load()?;
        hostlist::add(&mut all, cfg).map_err(|e| e.to_string())?;
        self.config.save(&all)?;

        // Start with the trimmed copy the list actually stored, not the raw
        // input: a trailing space in a dialog field would otherwise be watched
        // under a name that does not match the one on disk.
        let stored = all.last().cloned().expect("just pushed");
        let settings = self.config.load_settings()?;
        self.sup
            .start(stored, effective_interval(all.last().unwrap(), &settings));
        self.announce_hosts(&all);
        Ok(all)
    }

    pub fn remove_host(&self, name: &str) -> Result<Vec<HostConfig>, String> {
        let mut all = self.config.load()?;
        hostlist::remove(&mut all, name);
        self.config.save(&all)?;

        self.sup.stop(name);
        self.sup.forget(name);
        self.history.forget_host(name);
        self.announce_hosts(&all);
        Ok(all)
    }

    pub fn reorder_hosts(&self, names: &[String]) -> Result<Vec<HostConfig>, String> {
        let mut all = self.config.load()?;
        hostlist::reorder(&mut all, names);
        self.config.save(&all)?;
        self.announce_hosts(&all);
        Ok(all)
    }

    pub fn get_settings(&self) -> Result<Settings, String> {
        self.config.load_settings()
    }

    /// Replace settings, restarting only the hosts whose effective interval
    /// changed. Changing the global rate when most hosts carry an override
    /// should not tear down connections already sampling correctly.
    pub fn set_settings(&self, settings: Settings) -> Result<Settings, String> {
        let mut f = self.config.load_file()?;
        let before = f.settings;
        f.settings = Settings {
            interval_secs: settings.interval_secs.clamp(MIN_INTERVAL, MAX_INTERVAL),
            history_cap_mb: settings.history_cap_mb.clamp(MIN_CAP_MB, MAX_CAP_MB),
            always_on_top: settings.always_on_top,
        };
        self.config.save_file(&f)?;
        self.history.set_cap_mb(f.settings.history_cap_mb);

        for h in &f.hosts {
            if effective_interval(h, &before) != effective_interval(h, &f.settings) {
                self.sup
                    .start(h.clone(), effective_interval(h, &f.settings));
            }
        }
        let _ = self.events.try_send(Event::SettingsChanged(f.settings));
        Ok(f.settings)
    }

    pub fn set_host_interval(
        &self,
        name: &str,
        interval_secs: Option<u32>,
    ) -> Result<Vec<HostConfig>, String> {
        let mut f = self.config.load_file()?;
        let Some(h) = f.hosts.iter_mut().find(|h| h.name == name) else {
            return Err(format!("no host named {name}"));
        };
        h.interval_secs = interval_secs.map(|v| v.clamp(MIN_INTERVAL, MAX_INTERVAL));
        let updated = h.clone();
        self.config.save_file(&f)?;

        self.sup
            .start(updated.clone(), effective_interval(&updated, &f.settings));
        self.announce_hosts(&f.hosts);
        Ok(f.hosts)
    }

    /// Set or clear a host's group. Nothing about sampling changes, so no
    /// sampler is restarted — only the arrangement on screen.
    pub fn set_host_group(
        &self,
        name: &str,
        group: Option<&str>,
    ) -> Result<Vec<HostConfig>, String> {
        let mut f = self.config.load_file()?;
        if !hostlist::set_group(&mut f.hosts, name, group) {
            return Err(format!("no host named {name}"));
        }
        self.config.save_file(&f)?;
        self.announce_hosts(&f.hosts);
        Ok(f.hosts)
    }

    /// Set a host's operating system, and restart its sampler.
    ///
    /// Unlike the group, this changes the remote command itself, so leaving
    /// the old sampler running would look like the setting had not worked.
    pub fn set_host_os(&self, name: &str, os: &str) -> Result<Vec<HostConfig>, String> {
        let mut f = self.config.load_file()?;
        let Some(h) = f.hosts.iter_mut().find(|h| h.name == name) else {
            return Err(format!("no host named {name}"));
        };
        h.os = if os.eq_ignore_ascii_case("windows") {
            "windows".into()
        } else {
            String::new()
        };
        let updated = h.clone();
        self.config.save_file(&f)?;

        self.sup
            .start(updated.clone(), effective_interval(&updated, &f.settings));
        self.announce_hosts(&f.hosts);
        Ok(f.hosts)
    }

    pub fn traffic_stats(&self) -> Vec<HostTraffic> {
        self.sup.traffic()
    }

    /// Start or stop fleet-wide process sampling.
    ///
    /// Driven by the view being open: sampling costs remote wall clock per
    /// host per cycle, so a view nobody is looking at should cost nothing.
    pub fn set_processes_enabled(&self, on: bool) -> Result<(), String> {
        if on {
            self.sup.start_procs(self.config.load()?);
        } else {
            self.sup.stop_procs();
        }
        Ok(())
    }

    pub fn process_list(&self) -> Vec<ProcInfo> {
        self.sup.fleet_procs()
    }

    pub fn cgroup_list(&self) -> Vec<HostCgroup> {
        self.sup.fleet_cgroups()
    }

    /// A window of history for one series.
    ///
    /// The bounds are seconds *before now*, so a caller never needs its clock
    /// to agree with this one. Downsampling happens here, where the data is,
    /// so only what can be drawn crosses the wire.
    pub fn query_history(
        &self,
        host: &str,
        metric: &str,
        from_secs_ago: u64,
        to_secs_ago: u64,
        max_points: usize,
    ) -> Vec<Point> {
        let now = now_secs();
        self.history.query(
            host,
            metric,
            now.saturating_sub(from_secs_ago),
            now.saturating_sub(to_secs_ago),
            max_points.clamp(1, MAX_POINTS),
        )
    }

    /// Several series for one host in one call. A 32-core host needs 32
    /// series to draw its grid; asking one at a time would be 32 round trips
    /// per redraw for data behind the same lock.
    pub fn query_history_many(
        &self,
        host: &str,
        metrics: Vec<String>,
        from_secs_ago: u64,
        to_secs_ago: u64,
        max_points: usize,
    ) -> HashMap<String, Vec<Point>> {
        let now = now_secs();
        let from = now.saturating_sub(from_secs_ago);
        let to = now.saturating_sub(to_secs_ago);
        let budget = max_points.clamp(1, MAX_POINTS);
        metrics
            .into_iter()
            .map(|m| {
                let pts = self.history.query(host, &m, from, to, budget);
                (m, pts)
            })
            .collect()
    }

    pub fn history_usage(&self) -> HistoryUsage {
        self.history.usage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(name: &str) -> (Service, mpsc::Receiver<Event>, std::path::PathBuf) {
        let mut path = std::env::temp_dir();
        path.push(format!("tuxtop-svc-{name}-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let (tx, rx) = mpsc::channel(64);
        let history = Arc::new(HistoryStore::new());
        let sup = Supervisor::new(
            history.clone(),
            tx.clone(),
            tokio::runtime::Handle::current(),
        );
        (Service::new(Config::new(&path), sup, history, tx), rx, path)
    }

    fn host(name: &str) -> HostConfig {
        HostConfig {
            name: name.into(),
            addr: "127.0.0.1".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn adding_a_host_persists_it_and_announces_it() {
        let (s, mut rx, p) = svc("add");
        let all = s.add_host(host("dove")).unwrap();
        assert_eq!(all.len(), 1);
        // It is on disk, not merely in memory: a restart must find it.
        assert_eq!(s.list_hosts().unwrap()[0].name, "dove");
        assert!(matches!(rx.try_recv(), Ok(Event::HostsChanged(h)) if h.len() == 1));
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn a_duplicate_name_is_refused_rather_than_silently_merged() {
        // Two cards with the same name would each claim the other's samples.
        let (s, _rx, p) = svc("dup");
        s.add_host(host("dove")).unwrap();
        assert!(s.add_host(host("dove")).is_err());
        assert_eq!(s.list_hosts().unwrap().len(), 1);
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn removing_a_host_forgets_its_history_too() {
        // Otherwise re-adding a name later inherits the old machine's charts.
        let (s, _rx, p) = svc("rm");
        s.add_host(host("gone")).unwrap();
        s.history().record(&crate::Sample {
            host: "gone".into(),
            cpu: 50.0,
            ..Default::default()
        });
        assert!(s.history().usage().series > 0);

        s.remove_host("gone").unwrap();
        assert_eq!(s.history().usage().series, 0);
        assert!(s.list_hosts().unwrap().is_empty());
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn settings_are_clamped_whatever_the_caller_asked_for() {
        // The UI enforces these bounds; a request that did not come from the
        // UI - an HTTP client, say - must not be able to exceed them.
        let (s, _rx, p) = svc("clamp");
        let out = s
            .set_settings(Settings {
                interval_secs: 99_999,
                history_cap_mb: 1,
                always_on_top: false,
            })
            .unwrap();
        assert_eq!(out.interval_secs, MAX_INTERVAL);
        assert_eq!(out.history_cap_mb, MIN_CAP_MB);
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn editing_a_host_that_does_not_exist_says_so() {
        // Silently succeeding would let a UI show a change that never landed.
        let (s, _rx, p) = svc("missing");
        assert!(s.set_host_os("ghost", "windows").is_err());
        assert!(s.set_host_group("ghost", Some("x")).is_err());
        assert!(s.set_host_interval("ghost", Some(5)).is_err());
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn a_history_query_cannot_ask_for_unbounded_points() {
        // The budget is a chart's pixel width. A caller asking for millions
        // would have them allocated before anyone noticed.
        let (s, _rx, p) = svc("points");
        assert!(s.query_history("nobody", "cpu", 60, 0, usize::MAX).len() <= MAX_POINTS);
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn os_is_normalised_rather_than_stored_as_typed() {
        // "Windows", "WINDOWS" and "windows" are one thing; anything else is
        // Linux. Storing the raw string would make the sampler branch on case.
        let (s, _rx, p) = svc("os");
        s.add_host(host("n1")).unwrap();
        s.set_host_os("n1", "WiNdOwS").unwrap();
        assert_eq!(s.list_hosts().unwrap()[0].os, "windows");
        s.set_host_os("n1", "something else").unwrap();
        assert_eq!(s.list_hosts().unwrap()[0].os, "");
        let _ = std::fs::remove_file(p);
    }
}
