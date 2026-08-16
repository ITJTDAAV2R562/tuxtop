//! Loading and saving the watched-host list.
//!
//! Stored as TOML in the OS config directory so it survives reinstalls and is
//! hand-editable. On Windows that is
//! `%APPDATA%\dev.tuxtop.app\hosts.toml`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tuxtop_core::HostConfig;

/// The on-disk shape. A wrapper struct is needed because TOML cannot have a
/// bare array at the document root.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HostsFile {
    #[serde(default, rename = "host")]
    pub hosts: Vec<HostConfig>,
}

/// Path to `hosts.toml`, creating the parent directory if needed.
pub fn path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;

    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("no config directory available: {e}"))?;

    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    Ok(dir.join("hosts.toml"))
}

/// Read the host list.
///
/// A missing file is not an error — it is a first run, and yields an empty
/// list. A *malformed* file **is** an error and is reported: silently
/// discarding a config the user hand-edited would lose their work and leave
/// them staring at an empty window with no explanation.
pub fn load(app: &tauri::AppHandle) -> Result<Vec<HostConfig>, String> {
    let p = path(app)?;

    if !p.exists() {
        return Ok(Vec::new());
    }

    let text = std::fs::read_to_string(&p).map_err(|e| format!("reading {}: {e}", p.display()))?;

    let parsed: HostsFile =
        toml::from_str(&text).map_err(|e| format!("{} is not valid TOML: {e}", p.display()))?;

    Ok(parsed.hosts)
}

/// Write the host list back, replacing the file.
pub fn save(app: &tauri::AppHandle, hosts: &[HostConfig]) -> Result<(), String> {
    let p = path(app)?;
    let doc = HostsFile {
        hosts: hosts.to_vec(),
    };
    let text = toml::to_string_pretty(&doc).map_err(|e| format!("serialising hosts: {e}"))?;

    std::fs::write(&p, text).map_err(|e| format!("writing {}: {e}", p.display()))
}
