//! Watching many hosts at once, with each one isolated from the others.
//!
//! One task per host, each owning its own `ssh` process and its own channel.
//! A host that hangs, fails auth or has its connection killed must affect only
//! itself — that isolation is the whole reason this is a set of independent
//! tasks rather than one loop polling a list.
//!
//! This lives in `tuxtop-core` rather than beside the Tauri supervisor because
//! it is the most consequential control flow in the app and it needs tests.
//! `src-tauri` cannot be built on the development box at all (ADR-006), so
//! anything living there is, in practice, code nobody can run in isolation.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::model::{HostConfig, HostFault, Sample};
use crate::traffic::TrafficCounter;
use crate::transport::SshSampler;

/// Reconnect backoff, in seconds. Capped rather than unbounded exponential so
/// a host that comes back is picked up promptly instead of sitting out a long
/// tail — this is a tool you watch, and a five-minute wait to notice a machine
/// has returned would be useless.
pub const BACKOFF: &[u64] = &[1, 2, 5, 10, 20, 30];

/// How long to wait before the `attempt`-th reconnect.
///
/// Saturates at the last entry rather than indexing past the end.
pub fn backoff_secs(attempt: usize) -> u64 {
    BACKOFF[attempt.min(BACKOFF.len() - 1)]
}

/// Everything a watched host can produce.
///
/// `Sample` is boxed because it is far larger than a fault, and an un-boxed
/// enum costs every fault the size of a full sample.
#[derive(Debug, Clone)]
pub enum HostEvent {
    Sample(Box<Sample>),
    Fault(HostFault),
    /// The process ranking, cgroups and unit restarts, off the same
    /// connection as the samples. Boxed for the reason `Sample` is.
    Processes(Box<crate::procs::ProcFrame>),
}

/// What to report when a sampler connection ends.
///
/// The subtlety this encodes: a fault that already arrived through the channel
/// is *specific* — "host unreachable: connection timed out" — and must not be
/// overwritten by the generic message emitted on the way out of the loop.
/// Doing that unconditionally is a real bug this project shipped: an
/// unreachable host displayed "sampler failed", which points at the remote
/// `/proc` read rather than at the network, and sends you debugging the wrong
/// machine.
///
/// - Data arrived, then the connection dropped: the caller's reconnect is the
///   whole story, so say nothing.
/// - A specific fault was already reported: keep it.
/// - Nothing arrived and nothing was reported: this is the only case that
///   needs a generic message, because otherwise the card would say nothing at
///   all about why it is empty.
pub fn closing_fault(got_data: bool, already_reported: bool) -> Option<HostFault> {
    if got_data || already_reported {
        None
    } else {
        Some(HostFault::SamplerFailed(
            "connection closed before any data arrived".into(),
        ))
    }
}

/// Watch one host forever, reconnecting with backoff.
///
/// Returns only when `out` is closed — that is the consumer saying it has gone
/// away, and there is no reason to keep an ssh process alive to feed nobody.
///
/// Every event carries the host name. A bare fault cannot be attributed to a
/// card, and attributing one to the wrong card is worse than dropping it.
pub async fn watch_host(
    cfg: HostConfig,
    interval_ms: u32,
    traffic: Arc<TrafficCounter>,
    out: mpsc::Sender<(String, HostEvent)>,
) {
    let mut attempt = 0usize;

    loop {
        let (tx, mut rx) = mpsc::channel(16);
        // The process plane rides the same connection but not the same
        // channel: a process frame is not a `Sample` and must never be able to
        // reset the backoff or stand in for one. Forwarded by its own task so
        // the loop below keeps the shape its fault handling was written for.
        let (ptx, mut prx) = mpsc::channel(4);
        let pout = out.clone();
        let pname = cfg.name.clone();
        let procs = tokio::spawn(async move {
            while let Some(frame) = prx.recv().await {
                if pout
                    .send((pname.clone(), HostEvent::Processes(Box::new(frame))))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });

        let sampler = match SshSampler::start(cfg.clone(), interval_ms, tx, ptx, traffic.clone()) {
            Ok(s) => s,
            Err(e) => {
                // Could not even spawn ssh — almost always "not on PATH".
                let fault = HostFault::SamplerFailed(format!("could not launch ssh: {e}"));
                if out
                    .send((cfg.name.clone(), HostEvent::Fault(fault)))
                    .await
                    .is_err()
                {
                    return;
                }
                procs.abort();
                sleep_backoff(&mut attempt).await;
                continue;
            }
        };

        let mut got_data = false;
        let mut reported = false;

        while let Some(item) = rx.recv().await {
            let event = match item {
                Ok(sample) => {
                    got_data = true;
                    attempt = 0; // a good sample resets the backoff
                    HostEvent::Sample(Box::new(sample))
                }
                Err(fault) => {
                    reported = true;
                    HostEvent::Fault(fault)
                }
            };
            let fatal = matches!(event, HostEvent::Fault(_));
            if out.send((cfg.name.clone(), event)).await.is_err() {
                procs.abort();
                sampler.stop().await;
                return;
            }
            if fatal {
                break;
            }
        }

        procs.abort();
        sampler.stop().await;

        if let Some(fault) = closing_fault(got_data, reported) {
            if out
                .send((cfg.name.clone(), HostEvent::Fault(fault)))
                .await
                .is_err()
            {
                return;
            }
        }

        sleep_backoff(&mut attempt).await;
    }
}

async fn sleep_backoff(attempt: &mut usize) {
    let secs = backoff_secs(*attempt);
    *attempt += 1;
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_specific_fault_is_not_clobbered_by_the_generic_one() {
        // The shipped bug: an unreachable host reported "sampler failed",
        // which points at /proc rather than at the network.
        assert!(closing_fault(false, true).is_none());
    }

    #[test]
    fn a_silent_connection_still_says_something() {
        // Nothing arrived and nothing explained why. Without this the card
        // sits empty with no stated reason, which is the one thing this app
        // must never do.
        assert!(matches!(
            closing_fault(false, false),
            Some(HostFault::SamplerFailed(_))
        ));
    }

    #[test]
    fn a_dropped_connection_after_good_data_is_not_a_fault() {
        // The reconnect is the whole story; announcing a fault every time a
        // healthy host's ssh is recycled would cry wolf.
        assert!(closing_fault(true, false).is_none());
    }

    #[test]
    fn backoff_saturates_instead_of_indexing_past_the_end() {
        assert_eq!(backoff_secs(0), 1);
        assert_eq!(backoff_secs(BACKOFF.len() - 1), 30);
        assert_eq!(backoff_secs(999), 30);
    }

    #[test]
    fn backoff_is_capped_low_enough_to_notice_a_host_returning() {
        // A host that comes back must be picked up within a plausible glance
        // at the window, not minutes later.
        assert!(*BACKOFF.last().unwrap() <= 30);
    }
}
