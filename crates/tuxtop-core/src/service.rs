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
use crate::hostlist::{
    self, effective_interval_ms, FleetSettings, HostsFile, Settings, ViewerSettings,
    MAX_INTERVAL_MS, MIN_INTERVAL_MS,
};
use crate::procs::ProcInfo;
use crate::remote::{stale_after_ms, Capabilities};
use crate::supervisor::{Event, HostCgroup, HostTraffic, Supervisor};
use crate::HostConfig;

/// Bounds taken from the settings UI, applied here so a request that did not
/// come from that UI cannot exceed them.
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
    ///
    /// Starts nothing when an endpoint is set: remote mode replaces the local
    /// data plane entirely, and a viewer that sampled as well would be exactly
    /// the duplication ADR-017 exists to remove — nineteen more sshd sessions
    /// on machines we promised only to observe. The read loop in the shell
    /// produces the events instead.
    pub fn start_all(&self) -> Result<Settings, String> {
        let f = self.config.load_file()?;
        // Applied in both modes: the local store holds arriving remote samples
        // too, so its ceiling is not something remote mode gets to skip.
        self.history.set_cap_mb(f.settings.fleet.history_cap_mb);
        if f.settings.viewer.server.is_none() {
            for cfg in f.hosts {
                let iv = effective_interval_ms(&cfg, &f.settings.fleet);
                self.sup.start(cfg, iv);
            }
        }
        Ok(f.settings)
    }

    /// The server this window is watching, or `None` when it samples locally.
    ///
    /// **Derived, never stored.** A `mode = "remote"` field can contradict the
    /// URL beside it, and then something has to decide which wins; the presence
    /// of an endpoint cannot contradict itself (ADR-017).
    pub fn endpoint(&self) -> Result<Option<String>, String> {
        Ok(self.config.load_settings()?.viewer.server)
    }

    /// What this window can actually do, and whose readings it is showing.
    ///
    /// Re-read rather than read once: switching endpoints changes every field
    /// of it. `writable` is *effective* capability — a viewer in remote mode
    /// reports false, because its local `hosts.toml` is not the fleet on
    /// screen, and `tuxtop-serve` narrows it further with its own `--writable`.
    ///
    /// `endpoint` absent means *this process* is sampling. A browser tab served
    /// by `tuxtop-serve` is still a remote viewer (ADR-017 rule 1) and takes
    /// its endpoint from its own origin, which the server cannot know.
    pub fn capabilities(&self) -> Result<Capabilities, String> {
        let s = self.config.load_settings()?;
        Ok(Capabilities {
            writable: s.viewer.server.is_none(),
            endpoint: s.viewer.server,
            stale_after_ms: stale_after_ms(s.fleet.interval_ms),
            // All six version sites are held equal by `scripts/check-version.py`,
            // so core's is the one that cannot drift from the build around it.
            version: env!("CARGO_PKG_VERSION").to_string(),
            // Locally the viewer and the source of its events are the same
            // process. The shell fills this in when it proxies.
            version_note: None,
        })
    }

    /// Refuse a write when this window is watching somebody else's fleet.
    ///
    /// **One choke point, not a check in each of seven methods.** ADR-012's
    /// lesson is that a rule living in the callers acquires a caller that
    /// forgets — the pause rule survives only because it lives in
    /// `Supervisor::start` and nowhere else.
    ///
    /// It lives here rather than in the frontend because `capabilities.writable`
    /// hides the controls, and a hidden control is a fact about a stylesheet:
    /// the command behind it stays reachable. In remote mode the fleet on screen
    /// is the server's, so `set_host_paused` drawn beside those cards would
    /// appear to succeed and edit a different `dove` — ADR-010's aiming
    /// argument, arriving through a door nobody had opened.
    fn refuse_if_remote(&self, what: &str) -> Result<(), String> {
        let Some(server) = self.config.load_settings()?.viewer.server else {
            return Ok(());
        };
        Err(format!(
            "{what} would edit this machine's hosts.toml, and this window is \
             watching {server}. The fleet on screen is that server's, so the \
             change would land on a different machine of the same name."
        ))
    }

    /// Persist a new host list. Goes through the refusal above.
    fn save_hosts(&self, what: &str, hosts: &[HostConfig]) -> Result<(), String> {
        self.refuse_if_remote(what)?;
        self.config.save(hosts)
    }

    /// Persist a change to the whole file. Goes through the refusal above.
    fn save_fleet(&self, what: &str, f: &HostsFile) -> Result<(), String> {
        self.refuse_if_remote(what)?;
        self.config.save_file(f)
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
        self.save_hosts("adding a host", &all)?;

        // Start with the trimmed copy the list actually stored, not the raw
        // input: a trailing space in a dialog field would otherwise be watched
        // under a name that does not match the one on disk.
        let stored = all.last().cloned().expect("just pushed");
        let settings = self.config.load_settings()?;
        self.sup.start(
            stored,
            effective_interval_ms(all.last().unwrap(), &settings.fleet),
        );
        self.announce_hosts(&all);
        Ok(all)
    }

    pub fn remove_host(&self, name: &str) -> Result<Vec<HostConfig>, String> {
        let mut all = self.config.load()?;
        hostlist::remove(&mut all, name);
        self.save_hosts("removing a host", &all)?;

        self.sup.stop(name);
        self.sup.forget(name);
        self.history.forget_host(name);
        self.announce_hosts(&all);
        Ok(all)
    }

    pub fn reorder_hosts(&self, names: &[String]) -> Result<Vec<HostConfig>, String> {
        let mut all = self.config.load()?;
        hostlist::reorder(&mut all, names);
        self.save_hosts("reordering the fleet", &all)?;
        self.announce_hosts(&all);
        Ok(all)
    }

    pub fn get_settings(&self) -> Result<Settings, String> {
        self.config.load_settings()
    }

    /// Replace settings, restarting only the hosts whose effective interval
    /// changed. Changing the global rate when most hosts carry an override
    /// should not tear down connections already sampling correctly.
    ///
    /// **The two halves are treated differently, and that is the whole reason
    /// `Settings` splits.** The fleet half describes the machine doing the
    /// sampling, so in remote mode a change to it is refused like any other
    /// write. The viewer half is this window's and saves in both modes —
    /// `always_on_top` is a property of *this* window, and a remote viewer that
    /// could not be pinned because pinning is a "setting" would be absurd
    /// (ADR-018 decision 4).
    ///
    /// This is the one write that does not go through `save_fleet`, because it
    /// is the one write that is *partly* allowed. `refuse_if_remote` is still
    /// the only place the refusal lives.
    pub fn set_settings(&self, settings: Settings) -> Result<Settings, String> {
        let mut f = self.config.load_file()?;
        let before = f.settings.clone();

        // Bounds from the settings UI, applied here so a request that did not
        // come from that UI cannot exceed them.
        let fleet = FleetSettings {
            interval_ms: settings
                .fleet
                .interval_ms
                .clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS),
            interval_secs: None,
            history_cap_mb: settings.fleet.history_cap_mb.clamp(MIN_CAP_MB, MAX_CAP_MB),
        };
        // Compared after clamping, so a request the UI already bounded reads as
        // unchanged rather than as an attempted edit. A viewer saving only its
        // own half sends the fleet half back untouched, and must not be refused
        // for it.
        if fleet != before.fleet {
            self.refuse_if_remote("changing the sample interval or the history limit")?;
            f.settings.fleet = fleet;
        }
        f.settings.viewer = ViewerSettings {
            // From disk, never from the request. `app.js` has two save paths
            // and one of them rebuilds the payload from four named fields
            // without carrying `server`; honouring the request here would let
            // an unrelated settings save switch this window back to local.
            // Switching endpoints is `use_endpoint`, and it has no second door.
            server: before.viewer.server.clone(),
            always_on_top: settings.viewer.always_on_top,
            update_check: settings.viewer.update_check,
        };
        self.config.save_file(&f)?;
        self.history.set_cap_mb(f.settings.fleet.history_cap_mb);

        for h in &f.hosts {
            if effective_interval_ms(h, &before.fleet)
                != effective_interval_ms(h, &f.settings.fleet)
            {
                self.sup
                    .start(h.clone(), effective_interval_ms(h, &f.settings.fleet));
            }
        }
        let _ = self
            .events
            .try_send(Event::SettingsChanged(f.settings.clone()));
        Ok(f.settings)
    }

    pub fn set_host_interval(
        &self,
        name: &str,
        interval_ms: Option<u32>,
    ) -> Result<Vec<HostConfig>, String> {
        let mut f = self.config.load_file()?;
        let Some(h) = f.hosts.iter_mut().find(|h| h.name == name) else {
            return Err(format!("no host named {name}"));
        };
        h.interval_ms = interval_ms.map(|v| v.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS));
        let updated = h.clone();
        self.save_fleet("changing a host's interval", &f)?;

        self.sup.start(
            updated.clone(),
            effective_interval_ms(&updated, &f.settings.fleet),
        );
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
        self.save_fleet("changing a host's group", &f)?;
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
        self.save_fleet("changing a host's OS", &f)?;

        self.sup.start(
            updated.clone(),
            effective_interval_ms(&updated, &f.settings.fleet),
        );
        self.announce_hosts(&f.hosts);
        Ok(f.hosts)
    }

    /// Suspend or resume watching one host.
    ///
    /// For planned maintenance. The alternative users reach for - remove the
    /// host, add it back afterwards - throws away its history, its group, its
    /// interval override and its position in the grid, and `remove_host`
    /// deliberately calls `history.forget_host`. Pause keeps every one of
    /// those and stops only the sampling.
    ///
    /// Both directions are the same call: `Supervisor::start` stops the
    /// existing task first and refuses to start a paused one, so this asks for
    /// the host to be watched and the supervisor decides whether that means
    /// running or stopped. History is untouched either way - the whole point.
    pub fn set_host_paused(&self, name: &str, paused: bool) -> Result<Vec<HostConfig>, String> {
        let mut f = self.config.load_file()?;
        let Some(h) = f.hosts.iter_mut().find(|h| h.name == name) else {
            return Err(format!("no host named {name}"));
        };
        h.paused = paused;
        let updated = h.clone();
        self.save_fleet("pausing or resuming a host", &f)?;

        // One call, one connection: the process plane rides the same ssh
        // process, so pause is enforced in exactly one place - `start` - and
        // there is no second sampler for a caller to forget.
        self.sup.start(
            updated.clone(),
            effective_interval_ms(&updated, &f.settings.fleet),
        );
        self.announce_hosts(&f.hosts);
        Ok(f.hosts)
    }

    pub fn traffic_stats(&self) -> Vec<HostTraffic> {
        self.sup.traffic()
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

    /// One metric across the whole fleet, in one call.
    ///
    /// The mirror of `query_history_many`, and for the same reason: the
    /// heatmap draws every host at once, so asking per host would be nineteen
    /// round trips per redraw - and the slider redraws on every drag - for
    /// data behind the same lock.
    ///
    /// Hosts with no history for the metric are returned as empty vectors
    /// rather than omitted, so the caller can tell "not reporting" from "not
    /// configured" without cross-checking the host list.
    pub fn query_history_fleet(
        &self,
        metric: &str,
        from_secs_ago: u64,
        to_secs_ago: u64,
        max_points: usize,
    ) -> Result<HashMap<String, Vec<Point>>, String> {
        let now = now_secs();
        let from = now.saturating_sub(from_secs_ago);
        let to = now.saturating_sub(to_secs_ago);
        let budget = max_points.clamp(1, MAX_POINTS);
        // Propagated, not defaulted: an unreadable host list rendered as an
        // empty heatmap is a view that says "the fleet is quiet" when what it
        // means is "I could not find out".
        Ok(self
            .config
            .load()?
            .into_iter()
            .map(|h| {
                let pts = self.history.query(&h.name, metric, from, to, budget);
                (h.name, pts)
            })
            .collect())
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

    /// A second `Service` over the same config file, with its own empty
    /// supervisor. That is what a relaunch actually is: the file on disk is
    /// the only state that carries over.
    fn relaunch(path: &std::path::Path) -> (Service, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel(64);
        let history = Arc::new(HistoryStore::new());
        let sup = Supervisor::new(
            history.clone(),
            tx.clone(),
            tokio::runtime::Handle::current(),
        );
        (Service::new(Config::new(path), sup, history, tx), rx)
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
    async fn a_host_with_no_history_is_an_empty_series_not_a_missing_key() {
        // The heatmap draws one row per configured host and needs to say "no
        // data" for a host that is not reporting. If the map simply omitted
        // it, the row would have to be inferred from a separate host-list
        // call, and a host that never reported would silently vanish from a
        // view whose whole job is showing the whole fleet. This is how N1,
        // misconfigured and delivering nothing, still gets a labelled row.
        let (s, _rx, p) = svc("fleet-empty");
        s.add_host(host("dove")).unwrap();
        s.add_host(host("silent")).unwrap();

        let out = s.query_history_fleet("cpu", 60, 0, 30).unwrap();
        assert_eq!(out.len(), 2, "every configured host is a key");
        assert!(out.contains_key("silent"));
        assert!(out["silent"].is_empty());
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
                fleet: FleetSettings {
                    interval_ms: 99_999_999,
                    history_cap_mb: 1,
                    ..FleetSettings::default()
                },
                ..Settings::default()
            })
            .unwrap();
        assert_eq!(out.fleet.interval_ms, MAX_INTERVAL_MS);
        assert_eq!(out.fleet.history_cap_mb, MIN_CAP_MB);
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn turning_the_update_check_off_survives_a_save() {
        // `set_settings` rebuilds the struct field by field so it can clamp,
        // which means anything it forgets to name is dropped on every save -
        // silently, and only noticed by whoever set it and found it back on.
        // This is the guard for that shape of bug, not for this field alone.
        let (s, _rx, p) = svc("update-check");
        let out = s
            .set_settings(Settings {
                viewer: ViewerSettings {
                    update_check: false,
                    ..ViewerSettings::default()
                },
                ..Settings::default()
            })
            .unwrap();
        assert!(!out.viewer.update_check, "the value returned to the caller");
        assert!(
            !s.get_settings().unwrap().viewer.update_check,
            "and the value read back from disk"
        );
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
    async fn a_paused_host_is_not_watched() {
        let (s, _rx, p) = svc("pause");
        s.add_host(host("dove")).unwrap();
        assert!(s.sup.is_watching("dove"));

        s.set_host_paused("dove", true).unwrap();
        assert!(!s.sup.is_watching("dove"), "pause must drop the ssh task");
        assert!(s.list_hosts().unwrap()[0].paused);

        s.set_host_paused("dove", false).unwrap();
        assert!(s.sup.is_watching("dove"), "resume must bring it back");
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn changing_the_global_interval_does_not_resume_a_paused_host() {
        // The bug the choke point in `Supervisor::start` exists to prevent.
        // `set_settings` restarts every host whose effective interval changed,
        // and a paused host's does - so with the check in the callers instead,
        // touching the global rate would silently resume the whole fleet.
        let (s, _rx, p) = svc("pause-settings");
        s.add_host(host("dove")).unwrap();
        s.set_host_paused("dove", true).unwrap();

        s.set_settings(Settings {
            fleet: FleetSettings {
                interval_ms: 5_000,
                ..FleetSettings::default()
            },
            ..Settings::default()
        })
        .unwrap();

        assert!(
            !s.sup.is_watching("dove"),
            "a settings change resumed a paused host"
        );
        assert!(
            s.list_hosts().unwrap()[0].paused,
            "and the flag must survive it"
        );
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn editing_a_paused_host_does_not_resume_it() {
        // The same hazard by the other three doors: every one of these calls
        // `Supervisor::start` to make its change take effect.
        let (s, _rx, p) = svc("pause-edit");
        s.add_host(host("dove")).unwrap();
        s.set_host_paused("dove", true).unwrap();

        s.set_host_interval("dove", Some(2_000)).unwrap();
        assert!(
            !s.sup.is_watching("dove"),
            "an interval override resumed it"
        );
        s.set_host_os("dove", "windows").unwrap();
        assert!(!s.sup.is_watching("dove"), "an OS change resumed it");
        s.set_host_group("dove", Some("maintenance")).unwrap();
        assert!(!s.sup.is_watching("dove"), "a group change resumed it");

        // And the edits themselves still landed.
        let h = &s.list_hosts().unwrap()[0];
        assert_eq!((h.interval_ms, h.os.as_str()), (Some(2_000), "windows"));
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn start_all_skips_a_paused_host_on_launch() {
        // Pause has to survive a restart of the app, or it is useless for the
        // maintenance window it exists for - which routinely outlives a
        // session.
        //
        // Written against a *fresh* supervisor rather than the one that has
        // been running, and the empty assertion below is the point. The first
        // version of this test called `start_all` on a service whose hosts
        // `add_host` had already started, so the state it checked was true
        // before `start_all` ran: cargo-mutants replaced the whole function
        // body with `Ok(Default::default())` and the test still passed. It
        // was named for launch and did not test launch.
        let (s, _rx, p) = svc("pause-launch");
        s.add_host(host("dove")).unwrap();
        s.add_host(host("heron")).unwrap();
        s.set_host_paused("dove", true).unwrap();

        let (fresh, _rx2) = relaunch(&p);
        assert!(
            !fresh.sup.is_watching("heron"),
            "a new supervisor must watch nothing until start_all runs, or \
             this test cannot tell whether start_all did anything"
        );

        fresh.start_all().unwrap();
        assert!(
            !fresh.sup.is_watching("dove"),
            "a paused host was watched on launch"
        );
        assert!(
            fresh.sup.is_watching("heron"),
            "and its neighbour must still be"
        );
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn pausing_a_host_keeps_its_history_and_removing_one_does_not() {
        // The entire difference between the two operations, and the reason
        // pause exists rather than "delete it and add it back afterwards".
        let (s, _rx, p) = svc("pause-history");
        s.add_host(host("dove")).unwrap();
        s.history().record(&crate::Sample {
            host: "dove".into(),
            cpu: 42.0,
            ..Default::default()
        });
        let before = s.history().usage().series;
        assert!(before > 0);

        s.set_host_paused("dove", true).unwrap();
        assert_eq!(
            s.history().usage().series,
            before,
            "pausing threw away the history it exists to preserve"
        );

        s.remove_host("dove").unwrap();
        assert_eq!(s.history().usage().series, 0);
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn pausing_a_host_that_does_not_exist_says_so() {
        let (s, _rx, p) = svc("pause-ghost");
        assert!(s.set_host_paused("ghost", true).is_err());
        let _ = std::fs::remove_file(p);
    }

    /// Point an existing config at a server, the way a hand-edited
    /// `hosts.toml` or (from step 3) `use_endpoint` would.
    fn point_at(path: &std::path::Path, server: &str) {
        let c = Config::new(path);
        let mut f = c.load_file().unwrap();
        f.settings.viewer.server = Some(server.into());
        c.save_file(&f).unwrap();
    }

    #[tokio::test]
    async fn remote_mode_is_derived_not_stored() {
        // A stored `mode = "remote"` can contradict the URL beside it, and then
        // something has to decide which wins. Setting the endpoint is the whole
        // of switching modes, and clearing it is the whole of switching back.
        let (s, _rx, p) = svc("mode");
        s.add_host(host("dove")).unwrap();
        assert_eq!(s.endpoint().unwrap(), None, "no endpoint is local mode");
        assert!(s.capabilities().unwrap().writable);

        point_at(&p, "http://dove:8787");
        assert_eq!(s.endpoint().unwrap().as_deref(), Some("http://dove:8787"));
        assert!(
            !s.capabilities().unwrap().writable,
            "a viewer of someone else's fleet cannot write its own host list"
        );

        // Nothing on disk names a mode, so nothing on disk can disagree with
        // the endpoint. This is the half that fails if somebody later adds a
        // `mode` field "for clarity".
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            !text.contains("mode"),
            "the mode was stored as well as derived:\n{text}"
        );

        let c = Config::new(&p);
        let mut f = c.load_file().unwrap();
        f.settings.viewer.server = None;
        c.save_file(&f).unwrap();
        assert_eq!(
            s.endpoint().unwrap(),
            None,
            "clearing it goes back to local"
        );
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn start_all_starts_nothing_when_an_endpoint_is_set() {
        // Remote mode replaces the local data plane. A viewer that also sampled
        // would be the duplication ADR-017 exists to remove - nineteen more
        // sshd sessions on machines we promised only to observe - and it would
        // do it invisibly, because the grid would look right either way.
        //
        // Against a *fresh* supervisor, for the reason
        // `start_all_skips_a_paused_host_on_launch` records: `add_host` has
        // already started these, so a service that has been running asserts a
        // state that was true before the call.
        let (s, _rx, p) = svc("remote-launch");
        s.add_host(host("dove")).unwrap();
        s.add_host(host("heron")).unwrap();
        point_at(&p, "http://elsewhere:8787");

        let (fresh, _rx2) = relaunch(&p);
        assert!(
            !fresh.sup.is_watching("dove"),
            "a fresh supervisor watches nothing"
        );
        fresh.start_all().unwrap();
        assert!(
            !fresh.sup.is_watching("dove") && !fresh.sup.is_watching("heron"),
            "a remote viewer opened ssh connections of its own"
        );

        // And the local list is still there, because it is what you switch
        // back *to* - not started, not forgotten.
        assert_eq!(fresh.list_hosts().unwrap().len(), 2);

        // The control: the same file without the endpoint starts both.
        let c = Config::new(&p);
        let mut f = c.load_file().unwrap();
        f.settings.viewer.server = None;
        c.save_file(&f).unwrap();
        let (local, _rx3) = relaunch(&p);
        local.start_all().unwrap();
        assert!(
            local.sup.is_watching("dove") && local.sup.is_watching("heron"),
            "the endpoint was not the only reason nothing started"
        );
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn a_remote_viewer_refuses_to_write_to_its_local_hosts_toml() {
        // `capabilities.writable` is false so the controls are not drawn - but
        // a hidden control is a fact about a stylesheet, and the command behind
        // it stays reachable. In remote mode the fleet on screen is the
        // server's, so `set_host_paused` here appears to succeed and edits a
        // different `dove`: ADR-010's aiming argument through a door nobody had
        // opened.
        //
        // Every write is walked rather than one example, so a write added later
        // is covered by a test that already exists.
        let (s, _rx, p) = svc("remote-ro");
        s.add_host(host("dove")).unwrap();
        point_at(&p, "http://elsewhere:8787");
        let before = std::fs::read_to_string(&p).unwrap();

        type Attempt<'a> = (&'a str, Box<dyn Fn() -> Result<(), String> + 'a>);
        let attempts: Vec<Attempt> = vec![
            (
                "add_host",
                Box::new(|| s.add_host(host("heron")).map(|_| ())),
            ),
            (
                "remove_host",
                Box::new(|| s.remove_host("dove").map(|_| ())),
            ),
            (
                "reorder_hosts",
                Box::new(|| s.reorder_hosts(&["dove".to_string()]).map(|_| ())),
            ),
            (
                "set_host_interval",
                Box::new(|| s.set_host_interval("dove", Some(2_000)).map(|_| ())),
            ),
            (
                "set_host_group",
                Box::new(|| s.set_host_group("dove", Some("x")).map(|_| ())),
            ),
            (
                "set_host_os",
                Box::new(|| s.set_host_os("dove", "windows").map(|_| ())),
            ),
            (
                "set_host_paused",
                Box::new(|| s.set_host_paused("dove", true).map(|_| ())),
            ),
            (
                "set_settings (fleet half)",
                Box::new(|| {
                    s.set_settings(Settings {
                        fleet: FleetSettings {
                            interval_ms: 5_000,
                            ..FleetSettings::default()
                        },
                        ..Settings::default()
                    })
                    .map(|_| ())
                }),
            ),
        ];

        for (name, attempt) in attempts {
            let err = attempt().expect_err(&format!("{name} was allowed in remote mode"));
            assert!(
                err.contains("elsewhere"),
                "{name} refused without naming the server being watched: {err}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            before,
            "a refused write reached the file anyway"
        );
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn pinning_the_window_still_works_when_the_fleet_is_someone_elses() {
        // The exception that forces `Settings` to split. `always_on_top` is a
        // property of *this* window; a remote viewer that could not be pinned,
        // because pinning is a "setting" and settings belong to the server,
        // would be absurd - and is what one undivided struct forces.
        let (s, _rx, p) = svc("remote-pin");
        s.add_host(host("dove")).unwrap();
        point_at(&p, "http://elsewhere:8787");

        let out = s
            .set_settings(Settings {
                // The fleet half sent back unchanged, which is what the
                // frontend's `{...s, always_on_top}` spread actually sends.
                fleet: s.get_settings().unwrap().fleet,
                viewer: ViewerSettings {
                    always_on_top: true,
                    update_check: false,
                    server: None,
                },
            })
            .expect("the viewer half saves in remote mode");

        assert!(out.viewer.always_on_top, "the window would not pin");
        assert!(!out.viewer.update_check, "and the other viewer field too");
        assert!(
            s.get_settings().unwrap().viewer.always_on_top,
            "and it must be on disk, or it lasts one session"
        );
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn viewer_settings_survive_a_fleet_settings_save() {
        // `app.js` has two save paths and one of them rebuilds the payload from
        // four named fields without carrying `server`. Taking `server` from the
        // request would let a save of the interval switch this window back to
        // local - silently, and noticed only as a fleet that changed.
        let (s, _rx, p) = svc("viewer-survives");
        point_at(&p, "http://elsewhere:8787");

        // The exact shape of that payload: no `server` in it at all.
        let sent: Settings = serde_json::from_str(
            r#"{"interval_ms":1000,"history_cap_mb":256,"always_on_top":true,"update_check":true}"#,
        )
        .unwrap();
        assert_eq!(sent.viewer.server, None, "the payload really omits it");

        let out = s
            .set_settings(sent)
            .expect("saving the viewer half is allowed");
        assert_eq!(
            out.viewer.server.as_deref(),
            Some("http://elsewhere:8787"),
            "a settings save switched the window back to local"
        );
        assert_eq!(
            s.get_settings().unwrap().viewer.server.as_deref(),
            Some("http://elsewhere:8787"),
            "and on disk too"
        );
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn capabilities_carries_the_freshness_rule_rather_than_the_browser() {
        // The threshold cannot be a constant - a server sampling at 5 s reads
        // as permanently stale against a 1 Hz expectation - and the browser has
        // no core, so it must not carry a second copy of the rule to drift.
        let (s, _rx, p) = svc("caps");
        let c = s.capabilities().unwrap();
        assert_eq!(
            c.stale_after_ms,
            crate::remote::stale_after_ms(crate::hostlist::DEFAULT_INTERVAL_MS)
        );
        assert!(
            !c.version.is_empty(),
            "the source of the events must say which build it is"
        );
        assert_eq!(
            c.version_note, None,
            "locally there is nothing to disagree with"
        );

        s.set_settings(Settings {
            fleet: FleetSettings {
                interval_ms: 5_000,
                ..FleetSettings::default()
            },
            ..Settings::default()
        })
        .unwrap();
        assert!(
            s.capabilities().unwrap().stale_after_ms > c.stale_after_ms,
            "the threshold did not follow the interval it is measured against"
        );
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
