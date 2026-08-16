//! Tuxtop core: everything that is not a window.
//!
//! This crate deliberately has no GUI dependency. The Windows shell lives in
//! `src-tauri/`; all sampling, parsing and rate maths lives here so it can be
//! built and tested on any platform — including a WSL box that can never
//! compile the Windows binary.
//!
//! ```text
//! fast plane   ssh -> sampler loop -> Frame -> RateTracker -> Sample -> UI
//! slow plane   Beszel hub REST/SSE -> history series -> UI
//! ```
//!
//! See `docs/ARCHITECTURE.md` for why there are two planes, and
//! `docs/DECISIONS.md` for the measurement that forced the split.

pub mod model;
pub mod proc;
pub mod sampler;
pub mod transport;

pub use model::{GpuSample, HostConfig, HostFault, HostStatus, Sample};
pub use proc::{busy_pct, core_pcts, parse_meminfo, parse_stat, CpuTimes, MemInfo, StatSnapshot};
pub use sampler::{parse_frame, sampler_command, split_frames, Frame, RateTracker, Rates};
pub use transport::{classify_ssh_error, ssh_args, SshSampler};
