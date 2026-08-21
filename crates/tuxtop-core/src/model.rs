//! Types shared between the sampler, the Tauri bridge, and the frontend.
//!
//! These are the wire format: every one serialises straight to the JSON the UI
//! consumes, so field names are chosen for the frontend's benefit, not Rust's.

use serde::{Deserialize, Serialize};

/// A host Tuxtop watches. Mirrors one entry in `hosts.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostConfig {
    /// Display name and stable identity. Used as the key everywhere.
    pub name: String,
    /// Anything OpenSSH would accept: an alias from `~/.ssh/config`, a
    /// hostname, or an IP.
    pub addr: String,
    /// Login name. Empty means "let ssh decide" — the alias in
    /// `~/.ssh/config`, then ssh's own default. That is almost always right,
    /// and hardcoding a default here would override a `User` the config
    /// already specifies.
    #[serde(default)]
    pub user: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Optional Beszel hub base URL for this host's history, e.g.
    /// `https://dove.example.ts.net`. `None` means live-only: the fast plane
    /// still works, there is just no history behind it.
    #[serde(default)]
    pub beszel_url: Option<String>,
}

fn default_port() -> u16 {
    22
}

/// One second of readings from one host. This is what the UI renders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub host: String,
    /// Aggregate CPU busy percentage, `0.0..=100.0`.
    pub cpu: f32,
    /// Per-logical-core busy percentages, in kernel order.
    pub cores: Vec<f32>,
    pub mem_used_kb: u64,
    pub mem_total_kb: u64,
    /// Bytes per second, summed across non-loopback interfaces.
    pub net_rx_bps: u64,
    pub net_tx_bps: u64,
    /// Bytes per second across all physical block devices.
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuSample>,
    /// Kernel load averages over 1, 5 and 15 minutes.
    pub load: [f32; 3],
    /// CPU package temperature in degrees C. `None` when no CPU sensor is
    /// exposed, which is normal on a VM - never a zero, which would render as
    /// a plausible cold CPU.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_temp_c: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuSample {
    pub name: String,
    pub util_pct: f32,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub power_w: f32,
}

/// Why a host is not currently reporting.
///
/// Carried to the UI so a card can say what actually went wrong instead of
/// going blank. Never collapse these into a generic "offline" — telling
/// `AuthFailed` apart from `Unreachable` is the difference between a
/// thirty-second fix and an hour of guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum HostFault {
    /// TCP connect or DNS failed.
    Unreachable(String),
    /// Reached the host, but SSH refused the key.
    AuthFailed(String),
    /// Connected and authenticated, but the sampler command misbehaved.
    SamplerFailed(String),
    /// Connected, but nothing arrived within the expected interval.
    Stalled { since_secs: u64 },
}

/// What the UI shows for a host: either live numbers or a stated reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HostStatus {
    Connecting,
    Up(Box<Sample>),
    Down(HostFault),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_config_defaults_fill_in() {
        let toml = r#"name = "dove"
addr = "dove.example.ts.net"
"#;
        let h: HostConfig = toml::from_str(toml).expect("parses");
        // Empty, not "root": an omitted user defers to ~/.ssh/config rather
        // than overriding whatever `User` that file already sets.
        assert_eq!(h.user, "");
        assert_eq!(h.port, 22);
        assert_eq!(h.beszel_url, None);
    }

    #[test]
    fn fault_variants_survive_a_json_round_trip() {
        // The frontend switches on `kind`, so the tag must stay stable.
        let f = HostFault::AuthFailed("no matching key".into());
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("auth_failed"), "got {json}");
        let back: HostFault = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn stalled_carries_its_duration() {
        let f = HostFault::Stalled { since_secs: 12 };
        let json = serde_json::to_string(&f).unwrap();
        let back: HostFault = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn status_up_serialises_with_a_discriminant() {
        let s = HostStatus::Down(HostFault::Unreachable("timed out".into()));
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""state":"down""#), "got {json}");
    }
}
