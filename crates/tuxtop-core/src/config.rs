//! Reading and writing `hosts.toml`.
//!
//! Path-based rather than framework-based: the desktop app takes its path from
//! the OS config directory, a headless server takes one from its command line,
//! and a test takes a temporary file. Nothing here knows which.

use std::path::{Path, PathBuf};

use crate::hostlist::{self, HostsFile, Settings};
use crate::HostConfig;

/// The watched-host list on disk.
#[derive(Debug, Clone)]
pub struct Config {
    path: PathBuf,
}

impl Config {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the whole file: settings and hosts.
    ///
    /// A missing file is not an error — it is a first run, and yields an empty
    /// list. A *malformed* file **is** an error and is reported: silently
    /// discarding a config someone hand-edited would lose their work and leave
    /// them staring at an empty window with no explanation.
    pub fn load_file(&self) -> Result<HostsFile, String> {
        if !self.path.exists() {
            return Ok(HostsFile::default());
        }
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("reading {}: {e}", self.path.display()))?;
        let mut f = hostlist::parse_file(&text)
            .map_err(|e| format!("{} is not valid TOML: {e}", self.path.display()))?;
        // Every load, not just the first: a file written by an older build can
        // reappear at any time - a rollback, a restored backup, a hand-edit
        // from an old example. Doing this at the single point every read goes
        // through means no caller has to remember it.
        f.migrate();
        Ok(f)
    }

    pub fn load(&self) -> Result<Vec<HostConfig>, String> {
        Ok(self.load_file()?.hosts)
    }

    pub fn load_settings(&self) -> Result<Settings, String> {
        Ok(self.load_file()?.settings)
    }

    pub fn save_file(&self, f: &HostsFile) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        }
        let text = hostlist::render_file(f)?;
        std::fs::write(&self.path, text)
            .map_err(|e| format!("writing {}: {e}", self.path.display()))
    }

    /// Replace the host list, preserving whatever settings are on disk.
    ///
    /// Reads before writing so a host edit never silently reverts a setting
    /// changed in another window or by hand.
    pub fn save(&self, hosts: &[HostConfig]) -> Result<(), String> {
        let mut f = self.load_file()?;
        f.hosts = hosts.to_vec();
        self.save_file(&f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tuxtop-cfg-{name}-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn a_missing_file_is_a_first_run_not_an_error() {
        let c = Config::new(temp("missing"));
        assert!(c.load().unwrap().is_empty());
    }

    #[test]
    fn a_malformed_file_is_reported_rather_than_discarded() {
        // Silently starting empty would lose a hand-edited config and explain
        // nothing about why the window came up blank.
        let p = temp("bad");
        std::fs::write(&p, "this is not toml {{{").unwrap();
        let err = Config::new(&p).load().unwrap_err();
        assert!(err.contains("not valid TOML"), "{err}");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn saving_hosts_preserves_settings_written_elsewhere() {
        // Two windows, or a hand edit between reads: a host change must not
        // revert an interval somebody else set.
        let p = temp("merge");
        let c = Config::new(&p);
        let mut f = HostsFile::default();
        f.settings.interval_ms = 30_000;
        c.save_file(&f).unwrap();

        c.save(&[HostConfig {
            name: "dove".into(),
            addr: "dove".into(),
            port: 22,
            ..Default::default()
        }])
        .unwrap();

        let back = c.load_file().unwrap();
        assert_eq!(back.settings.interval_ms, 30_000, "setting was clobbered");
        assert_eq!(back.hosts.len(), 1);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_default_host_connects_to_port_22_not_port_0() {
        // The reason Default is written by hand. A derived one would make
        // every field-by-field construction fail against port 0, naming
        // nothing about why.
        assert_eq!(HostConfig::default().port, 22);
    }

    #[test]
    fn a_round_trip_keeps_every_field() {
        let p = temp("round");
        let c = Config::new(&p);
        c.save(&[HostConfig {
            name: "n1".into(),
            addr: "10.0.0.1".into(),
            port: 2222,
            os: "windows".into(),
            group: Some("physical".into()),
            interval_ms: Some(10),
            interval_secs: None,
            ..Default::default()
        }])
        .unwrap();
        let h = &c.load().unwrap()[0];
        assert_eq!(
            (h.port, h.os.as_str(), h.interval_ms),
            (2222, "windows", Some(10))
        );
        assert_eq!(h.group.as_deref(), Some("physical"));
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn an_old_file_keeps_its_interval_in_milliseconds() {
        // The rename from interval_secs is the kind of change that loses data
        // quietly: serde reads the old key as absent and the host silently
        // reverts to the default, which shows up only as a box sampling ten
        // times faster than it was told to.
        let mut p = std::env::temp_dir();
        p.push(format!("tuxtop-migrate-{}.toml", std::process::id()));
        std::fs::write(
            &p,
            "[settings]\ninterval_secs = 30\n\n[[host]]\nname = \"dove\"\naddr = \"dove\"\ninterval_secs = 5\n",
        )
        .unwrap();

        let f = Config::new(&p).load_file().unwrap();
        assert_eq!(f.settings.interval_ms, 30_000, "global setting survives");
        assert_eq!(f.hosts[0].interval_ms, Some(5_000), "override survives");
        assert_eq!(f.settings.interval_secs, None, "and is not read twice");

        // Written back, the old key is gone rather than left to contradict.
        let text = hostlist::render_file(&f).unwrap();
        assert!(!text.contains("interval_secs"), "wrote:\n{text}");
        assert!(text.contains("interval_ms"), "wrote:\n{text}");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_new_file_is_left_alone() {
        let mut p = std::env::temp_dir();
        p.push(format!("tuxtop-migrate-new-{}.toml", std::process::id()));
        std::fs::write(&p, "[settings]\ninterval_ms = 250\n").unwrap();
        let f = Config::new(&p).load_file().unwrap();
        assert_eq!(f.settings.interval_ms, 250);
        let _ = std::fs::remove_file(p);
    }
}
