//! Loading and saving the watched-host list.
//!
//! Stored as TOML in the OS config directory so it survives reinstalls and is
//! hand-editable. On Windows that is
//! `%APPDATA%\dev.tuxtop.app\hosts.toml`.

use std::path::PathBuf;

use tuxtop_core::hostlist::{self, HostsFile, Settings};
use tuxtop_core::HostConfig;

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
    Ok(load_file(app)?.hosts)
}

/// Read the whole file: settings and hosts.
pub fn load_file(app: &tauri::AppHandle) -> Result<HostsFile, String> {
    let p = path(app)?;

    if !p.exists() {
        return Ok(HostsFile::default());
    }

    let text = std::fs::read_to_string(&p).map_err(|e| format!("reading {}: {e}", p.display()))?;

    hostlist::parse_file(&text).map_err(|e| format!("{} is not valid TOML: {e}", p.display()))
}

/// Read settings alone.
pub fn load_settings(app: &tauri::AppHandle) -> Result<Settings, String> {
    Ok(load_file(app)?.settings)
}

/// Write the whole file back.
pub fn save_file(app: &tauri::AppHandle, f: &HostsFile) -> Result<(), String> {
    let p = path(app)?;
    let text = hostlist::render_file(f)?;
    std::fs::write(&p, text).map_err(|e| format!("writing {}: {e}", p.display()))
}

/// Replace the host list, preserving whatever settings are on disk.
///
/// Reads before writing so a host edit never silently reverts a setting
/// changed in another window or by hand.
pub fn save(app: &tauri::AppHandle, hosts: &[HostConfig]) -> Result<(), String> {
    let mut f = load_file(app)?;
    f.hosts = hosts.to_vec();
    save_file(app, &f)
}
