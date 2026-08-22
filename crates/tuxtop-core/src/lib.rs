//! Tuxtop core: everything that is not a window.
//!
//! This crate deliberately has no GUI dependency. The Windows shell lives in
//! `src-tauri/`; all sampling, parsing and rate maths lives here so it can be
//! built and tested on any platform — including a WSL box that can never
//! compile the Windows binary.
//!
//! ```text
//! ssh -> sampler loop -> Frame -> RateTracker -> Sample -> UI
//!                                             \-> History cascade -> UI
//! ```
//!
//! One stream, read at two zoom levels. See `docs/ARCHITECTURE.md`, and
//! `docs/DECISIONS.md` for the measurement that forced us to sample ourselves
//! rather than reuse an existing agent.

pub mod facts;
pub mod fleet;
pub mod history;
pub mod hostlist;
pub mod model;
pub mod proc;
pub mod procs;
pub mod sampler;
pub mod traffic;
pub mod transport;

pub use fleet::{backoff_secs, watch_host, HostEvent};
pub use hostlist::{
    add as add_host_to, remove as remove_host_from, reorder as reorder_hosts, AddError,
};
pub use model::{GpuSample, HostConfig, HostFault, HostStatus, Sample};
pub use proc::{busy_pct, core_pcts, parse_meminfo, parse_stat, CpuTimes, MemInfo, StatSnapshot};
pub use sampler::{parse_frame, sampler_command, split_frames, Frame, RateTracker, Rates};
pub use traffic::{fleet_bytes_per_sec_at, fleet_total, TrafficCounter, TrafficStats};
pub use transport::{classify_ssh_error, ssh_args, ProcSampler, SshSampler};
