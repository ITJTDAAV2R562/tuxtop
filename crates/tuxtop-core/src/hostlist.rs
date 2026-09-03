//! Add and remove operations on the watched-host list.
//!
//! Pulled out of the Tauri command layer so the behaviour is testable without
//! an AppHandle. "Adding a second host must not disturb the first" is the kind
//! of invariant that is obvious until it breaks, and a GUI is a slow place to
//! discover it.

use crate::model::HostConfig;

/// Why an add was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddError {
    EmptyName,
    EmptyAddr,
    Duplicate(String),
}

impl std::fmt::Display for AddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddError::EmptyName => write!(f, "host needs a name"),
            AddError::EmptyAddr => write!(f, "host needs an address"),
            AddError::Duplicate(n) => write!(f, "a host named {n} already exists"),
        }
    }
}

/// Append `cfg` to `list`, leaving existing entries untouched.
///
/// Trims whitespace from name and address: a trailing space in a dialog field
/// would otherwise produce a second host that looks identical to the first.
pub fn add(list: &mut Vec<HostConfig>, mut cfg: HostConfig) -> Result<(), AddError> {
    cfg.name = cfg.name.trim().to_string();
    cfg.addr = cfg.addr.trim().to_string();

    if cfg.name.is_empty() {
        return Err(AddError::EmptyName);
    }
    if cfg.addr.is_empty() {
        return Err(AddError::EmptyAddr);
    }
    if list.iter().any(|h| h.name == cfg.name) {
        return Err(AddError::Duplicate(cfg.name));
    }

    list.push(cfg);
    Ok(())
}

/// Drop the entry named `name`. Returns whether anything was removed.
pub fn remove(list: &mut Vec<HostConfig>, name: &str) -> bool {
    let before = list.len();
    list.retain(|h| h.name != name);
    before != list.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(name: &str) -> HostConfig {
        HostConfig {
            name: name.into(),
            addr: name.into(),
            user: String::new(),
            port: 22,
            beszel_url: None,
            interval_ms: None,
            interval_secs: None,
            os: String::new(),
            group: None,
            paused: false,
        }
    }

    #[test]
    fn adding_a_second_host_keeps_the_first() {
        // The reported bug: adding a second host removed the first.
        let mut list = vec![];
        add(&mut list, cfg("dove")).unwrap();
        add(&mut list, cfg("heron")).unwrap();

        assert_eq!(list.len(), 2, "both hosts must survive");
        assert_eq!(list[0].name, "dove", "the first must stay first");
        assert_eq!(list[1].name, "heron");
    }

    #[test]
    fn adding_many_hosts_accumulates() {
        let mut list = vec![];
        for n in ["dove", "heron", "wader", "falcon"] {
            add(&mut list, cfg(n)).unwrap();
        }
        let names: Vec<_> = list.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["dove", "heron", "wader", "falcon"]);
    }

    #[test]
    fn duplicate_names_are_rejected_without_touching_the_list() {
        let mut list = vec![];
        add(&mut list, cfg("dove")).unwrap();
        let err = add(&mut list, cfg("dove")).unwrap_err();

        assert_eq!(err, AddError::Duplicate("dove".into()));
        assert_eq!(list.len(), 1, "a rejected add must not mutate the list");
    }

    #[test]
    fn whitespace_is_trimmed_so_near_duplicates_are_caught() {
        let mut list = vec![];
        add(&mut list, cfg("dove")).unwrap();
        let mut padded = cfg("dove");
        padded.name = "  dove  ".into();
        assert!(add(&mut list, padded).is_err(), "' dove ' is still dove");
    }

    #[test]
    fn blank_fields_are_rejected() {
        let mut list = vec![];
        let mut c = cfg("x");
        c.name = "   ".into();
        assert_eq!(add(&mut list, c).unwrap_err(), AddError::EmptyName);

        let mut c = cfg("x");
        c.addr = "".into();
        assert_eq!(add(&mut list, c).unwrap_err(), AddError::EmptyAddr);
        assert!(list.is_empty());
    }

    #[test]
    fn remove_takes_only_the_named_host() {
        let mut list = vec![];
        add(&mut list, cfg("dove")).unwrap();
        add(&mut list, cfg("heron")).unwrap();

        assert!(remove(&mut list, "dove"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "heron", "the wrong host must not be removed");
    }

    #[test]
    fn removing_an_unknown_host_changes_nothing() {
        let mut list = vec![];
        add(&mut list, cfg("dove")).unwrap();
        assert!(!remove(&mut list, "nope"));
        assert_eq!(list.len(), 1);
    }
}

/// Global settings, stored alongside the host list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    /// Default sample interval in **milliseconds**, overridable per host.
    ///
    /// Milliseconds rather than seconds because 4 Hz and 2 Hz are now offered
    /// and neither is expressible in whole seconds. Files written before that
    /// carry `interval_secs`; `HostsFile::migrate` folds them in on load, so
    /// an existing setting is never silently dropped back to the default.
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u32,
    /// Superseded by `interval_ms`. Read on load and then dropped; never
    /// written. Present only so an older file keeps its setting.
    #[serde(default, skip_serializing)]
    pub interval_secs: Option<u32>,
    /// Ceiling on the in-memory history store.
    ///
    /// Expressed in MB rather than hours because the tiers already express the
    /// span: you set a memory budget and the UI shows what span it buys. At
    /// ~23 MB for a 19-host fleet this is a setting most people never touch;
    /// it earns its place around 100 hosts.
    #[serde(default = "default_history_mb")]
    pub history_cap_mb: u32,
    /// Keep the window above others.
    ///
    /// Task Manager has the same option, for the same reason: monitoring is
    /// something you do while doing something else, and a monitor you have to
    /// alt-tab to is half useless.
    #[serde(default)]
    pub always_on_top: bool,
    /// Ask GitHub once per launch whether a newer release exists.
    ///
    /// This is the only thing the app does that reaches anywhere other than
    /// the fleet, which is why it is a setting at all: everything else here
    /// talks to hosts you listed. An isolated deployment sets it false and the
    /// app makes no outbound connection of its own ever again.
    ///
    /// It governs the *check* only. Nothing installs itself: the check can do
    /// no more than raise a dismissable banner, and the download runs when
    /// somebody presses the button.
    #[serde(default = "default_update_check")]
    pub update_check: bool,
}

/// One second: the rate this shipped with, and still the sensible default.
/// Sub-second is opt-in - it multiplies both traffic and the load the sampler
/// puts on the watched host, and most of a fleet never needs it.
pub const DEFAULT_INTERVAL_MS: u32 = 1000;

fn default_interval_ms() -> u32 {
    DEFAULT_INTERVAL_MS
}

fn default_history_mb() -> u32 {
    256
}

/// On by default: a monitoring tool that silently runs a stale build is worse
/// than one outbound request per launch. Turn it off for an isolated fleet.
fn default_update_check() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_ms: default_interval_ms(),
            interval_secs: None,
            history_cap_mb: default_history_mb(),
            update_check: default_update_check(),
            always_on_top: false,
        }
    }
}

/// The on-disk shape of `hosts.toml`.
///
/// A wrapper struct is required because TOML has no bare root array.
/// `settings` must be declared before `hosts`: TOML requires plain tables to
/// precede arrays-of-tables in a document, so field order here is load-bearing
/// rather than cosmetic.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HostsFile {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, rename = "host")]
    pub hosts: Vec<HostConfig>,
}

impl HostsFile {
    /// Fold any pre-milliseconds `interval_secs` into `interval_ms`.
    ///
    /// Called on every load. Renaming the field without this would read an
    /// existing `interval_secs = 10` as absent and silently reset that host to
    /// the default - a settings change nobody asked for, visible only as a
    /// host sampling ten times faster than it was told to.
    pub fn migrate(&mut self) {
        if let Some(secs) = self.settings.interval_secs.take() {
            self.settings.interval_ms = secs.saturating_mul(1000);
        }
        for h in &mut self.hosts {
            if let Some(secs) = h.interval_secs.take() {
                h.interval_ms = Some(secs.saturating_mul(1000));
            }
        }
    }
}

/// The lower bound on any sample interval, in milliseconds.
///
/// 4 Hz is the fastest offered. Below that the SSH round trip, not the kernel,
/// becomes the limit - and the sampler starts costing the watched host real
/// CPU, which a monitoring tool has no business doing.
pub const MIN_INTERVAL_MS: u32 = 250;
/// An hour, the slowest that still counts as monitoring.
pub const MAX_INTERVAL_MS: u32 = 3_600_000;

/// The interval that applies to `host`, in milliseconds, given the global
/// default. A per-host value always wins - the whole point of the override is
/// watching one box closely without paying for the other eighteen.
pub fn effective_interval_ms(host: &HostConfig, settings: &Settings) -> u32 {
    host.interval_ms
        .unwrap_or(settings.interval_ms)
        .clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS)
}

/// Parse `hosts.toml`. Kept separate from file I/O so it is testable.
pub fn parse(text: &str) -> Result<Vec<HostConfig>, String> {
    Ok(parse_file(text)?.hosts)
}

/// Parse the whole file, settings included.
pub fn parse_file(text: &str) -> Result<HostsFile, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

/// Render the list back to TOML.
pub fn render(hosts: &[HostConfig]) -> Result<String, String> {
    render_file(&HostsFile {
        settings: Settings::default(),
        hosts: hosts.to_vec(),
    })
}

pub fn render_file(f: &HostsFile) -> Result<String, String> {
    toml::to_string_pretty(f).map_err(|e| e.to_string())
}

#[cfg(test)]
mod file_tests {
    use super::*;

    /// The exact file seeded onto the Windows box, comments and all.
    const SEEDED: &str = r#"# Tuxtop watched hosts. Edit here or use "Add host" in the app.
#
# `addr` is anything ssh accepts. An alias from ~/.ssh/config is best: it
# brings ProxyJump, IdentityFile and User along with it.
# Omit `user` to let ssh decide.

[[host]]
name = "dove"
addr = "dove"
"#;

    #[test]
    fn the_seeded_file_parses() {
        let hosts = parse(SEEDED).expect("seeded hosts.toml must parse");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "dove");
        assert_eq!(hosts[0].user, "", "omitted user defers to ssh config");
    }

    #[test]
    fn paused_survives_a_write_and_a_read() {
        // Maintenance outlives a session. A flag that did not round-trip
        // through hosts.toml would come back watching on the next launch,
        // which is the wall of red this feature exists to prevent.
        let mut list = parse(SEEDED).unwrap();
        list[0].paused = true;
        let text = render(&list).expect("renders");
        assert!(parse(&text).unwrap()[0].paused, "wrote:\n{text}");
    }

    #[test]
    fn a_watched_host_writes_no_paused_key_at_all() {
        // hosts.toml is hand-edited. `paused = false` on all nineteen hosts
        // is noise in a file whose readability is the reason it is TOML.
        let list = parse(SEEDED).unwrap();
        let text = render(&list).expect("renders");
        assert!(!text.contains("paused"), "wrote:\n{text}");
    }

    #[test]
    fn round_trip_preserves_every_host() {
        let mut list = parse(SEEDED).unwrap();
        super::add(
            &mut list,
            HostConfig {
                name: "heron".into(),
                addr: "heron".into(),
                user: String::new(),
                port: 22,
                beszel_url: None,
                interval_ms: None,
                interval_secs: None,
                os: String::new(),
                group: None,
                paused: false,
            },
        )
        .unwrap();

        let text = render(&list).expect("renders");
        let back = parse(&text).expect("re-parses what we just wrote");

        assert_eq!(
            back.len(),
            2,
            "writing then reading must not lose a host\n{text}"
        );
        assert_eq!(back[0].name, "dove");
        assert_eq!(back[1].name, "heron");
    }

    #[test]
    fn three_hosts_survive_a_round_trip() {
        let mut list = vec![];
        for n in ["dove", "heron", "wader"] {
            super::add(
                &mut list,
                HostConfig {
                    name: n.into(),
                    addr: n.into(),
                    user: String::new(),
                    port: 22,
                    beszel_url: None,
                    interval_ms: None,
                    interval_secs: None,
                    os: String::new(),
                    group: None,
                    paused: false,
                },
            )
            .unwrap();
        }
        let text = render(&list).unwrap();
        assert_eq!(parse(&text).unwrap().len(), 3, "wrote:\n{text}");
    }

    #[test]
    fn a_host_with_beszel_url_round_trips() {
        // Some(..) and None serialise differently; make sure a mixed list
        // does not lose the entries after the first Option.
        let list = vec![
            HostConfig {
                name: "dove".into(),
                addr: "dove".into(),
                user: String::new(),
                port: 22,
                beszel_url: Some("https://dove.example".into()),
                interval_ms: None,
                interval_secs: None,
                os: String::new(),
                group: None,
                paused: false,
            },
            HostConfig {
                name: "heron".into(),
                addr: "heron".into(),
                user: String::new(),
                port: 22,
                beszel_url: None,
                interval_ms: None,
                interval_secs: None,
                os: String::new(),
                group: None,
                paused: false,
            },
        ];
        let text = render(&list).unwrap();
        let back = parse(&text).unwrap();
        assert_eq!(back.len(), 2, "wrote:\n{text}");
        assert_eq!(back, list);
    }
}

/// Set or clear one host's group. Returns whether the host was found.
///
/// An empty or whitespace-only label clears the group rather than creating one
/// named `""` — a group with a blank name would be unselectable, unnameable
/// and indistinguishable on screen from ungrouped, while still splitting the
/// fleet in two.
pub fn set_group(list: &mut [HostConfig], name: &str, group: Option<&str>) -> bool {
    let Some(h) = list.iter_mut().find(|h| h.name == name) else {
        return false;
    };
    h.group = group
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(str::to_string);
    true
}

/// Every group name currently in use, in the order hosts first mention them.
///
/// Order follows the host list rather than the alphabet because host order is
/// drag-to-reorder state the user set deliberately, and groups inheriting it
/// keeps the fleet looking the way it was arranged.
pub fn group_names(list: &[HostConfig]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in list {
        if let Some(g) = h.group.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
            if !out.iter().any(|e| e == g) {
                out.push(g.to_string());
            }
        }
    }
    out
}

/// Reorder `list` to match `names`, which is the order the user dragged cards
/// into.
///
/// Names not present in `list` are ignored, and hosts missing from `names`
/// keep their relative order at the end. Both cases are ordinary races, not
/// errors: a host can be removed in another window between the drag starting
/// and the drop landing, and dropping the whole list on the floor because of
/// that would lose the user's arrangement.
pub fn reorder(list: &mut [HostConfig], names: &[String]) {
    let rank = |h: &HostConfig| {
        names
            .iter()
            .position(|n| n == &h.name)
            .unwrap_or(usize::MAX)
    };
    // Stable sort, so unmentioned hosts keep their existing relative order.
    list.sort_by_key(rank);
}

#[cfg(test)]
mod reorder_tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<HostConfig> {
        names
            .iter()
            .map(|n| HostConfig {
                name: (*n).into(),
                addr: (*n).into(),
                user: String::new(),
                port: 22,
                beszel_url: None,
                interval_ms: None,
                interval_secs: None,
                os: String::new(),
                group: None,
                paused: false,
            })
            .collect()
    }

    fn names(l: &[HostConfig]) -> Vec<String> {
        l.iter().map(|h| h.name.clone()).collect()
    }

    #[test]
    fn reorders_to_the_given_sequence() {
        let mut l = list(&["dove", "heron", "wader"]);
        reorder(&mut l, &["wader".into(), "dove".into(), "heron".into()]);
        assert_eq!(names(&l), ["wader", "dove", "heron"]);
    }

    #[test]
    fn unmentioned_hosts_go_last_in_their_existing_order() {
        // A host added in another window mid-drag must not be dropped.
        let mut l = list(&["dove", "heron", "wader", "falcon"]);
        reorder(&mut l, &["wader".into(), "dove".into()]);
        assert_eq!(names(&l), ["wader", "dove", "heron", "falcon"]);
    }

    #[test]
    fn unknown_names_are_ignored() {
        let mut l = list(&["dove", "heron"]);
        reorder(&mut l, &["ghost".into(), "heron".into(), "dove".into()]);
        assert_eq!(names(&l), ["heron", "dove"]);
        assert_eq!(l.len(), 2, "no host invented from an unknown name");
    }

    #[test]
    fn an_empty_order_leaves_the_list_alone() {
        let mut l = list(&["dove", "heron", "wader"]);
        reorder(&mut l, &[]);
        assert_eq!(names(&l), ["dove", "heron", "wader"]);
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    fn host(name: &str, iv: Option<u32>) -> HostConfig {
        HostConfig {
            name: name.into(),
            addr: name.into(),
            user: String::new(),
            port: 22,
            beszel_url: None,
            interval_ms: iv,
            interval_secs: None,
            os: String::new(),
            group: None,
            paused: false,
        }
    }

    #[test]
    fn a_file_without_settings_still_parses() {
        // Every existing hosts.toml predates settings; none of them may break.
        let f = parse_file("[[host]]\nname = \"dove\"\naddr = \"dove\"\n").unwrap();
        assert_eq!(f.hosts.len(), 1);
        assert_eq!(f.settings.interval_ms, DEFAULT_INTERVAL_MS);
        assert_eq!(f.settings.history_cap_mb, 256);
        assert!(!f.settings.always_on_top, "off unless asked for");
    }

    #[test]
    fn settings_round_trip_with_hosts() {
        let f = HostsFile {
            settings: Settings {
                interval_ms: 5_000,
                interval_secs: None,
                history_cap_mb: 512,
                always_on_top: true,
                // Non-default on purpose: a setting that does not survive the
                // round trip is one the user turns off and finds back on.
                update_check: false,
            },
            hosts: vec![host("dove", None), host("heron", Some(30))],
        };
        let text = render_file(&f).unwrap();
        let back = parse_file(&text).unwrap();

        assert_eq!(back.settings.interval_ms, 5_000, "wrote:\n{text}");
        assert_eq!(back.settings.history_cap_mb, 512);
        assert!(
            back.settings.always_on_top,
            "the toggle must survive a restart"
        );
        assert_eq!(
            back.hosts.len(),
            2,
            "settings must not swallow the hosts\n{text}"
        );
        assert_eq!(back.hosts[1].interval_ms, Some(30));
    }

    #[test]
    fn settings_are_written_before_the_host_array() {
        // TOML requires plain tables to precede arrays-of-tables. Getting this
        // backwards produces a file that serialises fine and fails to parse.
        let text = render_file(&HostsFile {
            settings: Settings::default(),
            hosts: vec![host("dove", None)],
        })
        .unwrap();
        assert!(
            text.find("[settings]").unwrap() < text.find("[[host]]").unwrap(),
            "settings must come first:\n{text}"
        );
    }

    #[test]
    fn a_host_without_an_override_follows_the_global_interval() {
        let s = Settings {
            interval_ms: 10000,
            ..Settings::default()
        };
        assert_eq!(effective_interval_ms(&host("dove", None), &s), 10_000);
    }

    #[test]
    fn a_per_host_override_wins() {
        let s = Settings {
            interval_ms: 10000,
            ..Settings::default()
        };
        assert_eq!(effective_interval_ms(&host("dove", Some(1_000)), &s), 1_000);
    }

    #[test]
    fn absurd_intervals_are_clamped_not_obeyed() {
        // Zero would spin the remote loop as fast as sh can fork.
        let s = Settings {
            interval_ms: 1000,
            ..Settings::default()
        };
        assert_eq!(
            effective_interval_ms(&host("dove", Some(0)), &s),
            MIN_INTERVAL_MS
        );
        assert_eq!(
            effective_interval_ms(&host("dove", Some(999_999_999)), &s),
            MAX_INTERVAL_MS
        );
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;

    fn host(name: &str, group: Option<&str>) -> HostConfig {
        HostConfig {
            name: name.into(),
            addr: name.into(),
            user: String::new(),
            port: 22,
            beszel_url: None,
            interval_ms: None,
            interval_secs: None,
            os: String::new(),
            group: group.map(str::to_string),
            paused: false,
        }
    }

    #[test]
    fn a_blank_group_label_clears_rather_than_creating_one() {
        // A group named "" is unselectable and looks ungrouped on screen while
        // still splitting the fleet in two.
        let mut list = vec![host("dove", Some("workstations"))];
        assert!(set_group(&mut list, "dove", Some("   ")));
        assert_eq!(list[0].group, None);
    }

    #[test]
    fn group_names_follow_host_order_not_the_alphabet() {
        // Host order is drag-to-reorder state the user set deliberately.
        let list = vec![
            host("a", Some("zulu")),
            host("b", Some("alpha")),
            host("c", Some("zulu")),
            host("d", None),
        ];
        assert_eq!(group_names(&list), vec!["zulu", "alpha"]);
    }

    #[test]
    fn setting_a_group_on_a_missing_host_reports_it() {
        // Silently succeeding would let the UI show a group that does not
        // exist on disk until the next reload contradicts it.
        let mut list = vec![host("dove", None)];
        assert!(!set_group(&mut list, "ghost", Some("x")));
    }

    #[test]
    fn a_group_survives_a_toml_round_trip() {
        let list = vec![host("dove", Some("workstations")), host("heron", None)];
        let text = render(&list).unwrap();
        let back = parse(&text).unwrap();
        assert_eq!(back[0].group.as_deref(), Some("workstations"));
        assert_eq!(back[1].group, None, "ungrouped must stay absent, not empty");
    }

    #[test]
    fn a_file_written_before_groups_existed_still_parses() {
        // hosts.toml is hand-editable and long-lived; a missing field is a
        // normal older file, not a corrupt one.
        let old = "[[host]]\nname = \"dove\"\naddr = \"dove\"\n";
        let back = parse(old).expect("pre-group files must still load");
        assert_eq!(back[0].group, None);
    }
}
