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

/// The on-disk shape of `hosts.toml`.
///
/// A wrapper struct is required because TOML has no bare root array.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HostsFile {
    #[serde(default, rename = "host")]
    pub hosts: Vec<HostConfig>,
}

/// Parse `hosts.toml`. Kept separate from file I/O so it is testable.
pub fn parse(text: &str) -> Result<Vec<HostConfig>, String> {
    let f: HostsFile = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(f.hosts)
}

/// Render the list back to TOML.
pub fn render(hosts: &[HostConfig]) -> Result<String, String> {
    toml::to_string_pretty(&HostsFile {
        hosts: hosts.to_vec(),
    })
    .map_err(|e| e.to_string())
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
    fn round_trip_preserves_every_host() {
        let mut list = parse(SEEDED).unwrap();
        super::add(&mut list, HostConfig {
            name: "heron".into(), addr: "heron".into(),
            user: String::new(), port: 22, beszel_url: None,
        }).unwrap();

        let text = render(&list).expect("renders");
        let back = parse(&text).expect("re-parses what we just wrote");

        assert_eq!(back.len(), 2, "writing then reading must not lose a host\n{text}");
        assert_eq!(back[0].name, "dove");
        assert_eq!(back[1].name, "heron");
    }

    #[test]
    fn three_hosts_survive_a_round_trip() {
        let mut list = vec![];
        for n in ["dove", "heron", "wader"] {
            super::add(&mut list, HostConfig {
                name: n.into(), addr: n.into(),
                user: String::new(), port: 22, beszel_url: None,
            }).unwrap();
        }
        let text = render(&list).unwrap();
        assert_eq!(parse(&text).unwrap().len(), 3, "wrote:\n{text}");
    }

    #[test]
    fn a_host_with_beszel_url_round_trips() {
        // Some(..) and None serialise differently; make sure a mixed list
        // does not lose the entries after the first Option.
        let list = vec![
            HostConfig { name: "dove".into(), addr: "dove".into(), user: String::new(),
                         port: 22, beszel_url: Some("https://dove.example".into()) },
            HostConfig { name: "heron".into(), addr: "heron".into(), user: String::new(),
                         port: 22, beszel_url: None },
        ];
        let text = render(&list).unwrap();
        let back = parse(&text).unwrap();
        assert_eq!(back.len(), 2, "wrote:\n{text}");
        assert_eq!(back, list);
    }
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
