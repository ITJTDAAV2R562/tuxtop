//! The remote wire, checked against a response a server actually sent.
//!
//! `fixtures/sse-capture.bin` is 4,709 bytes taken off the socket with `nc`
//! from a running `tuxtop-serve` on 2026-09-09 — head, faults, samples, a
//! `processes` frame and axum's keep-alive, exactly as they arrived. It is the
//! same argument `real_host.rs` makes about `/proc/stat`: a fixture written to
//! match the parser tests the parser against itself, and every assumption made
//! about this response in prose before anyone looked was wrong in at least one
//! way. The chunk lengths are upper-case hex; one event happened to be one
//! chunk and nothing guarantees it; and a quiet fleet sends `:\n\n`.
//!
//! **Fed one byte at a time**, because that is the shape of the failure this
//! whole layer exists to prevent: a read boundary that lands mid-frame and
//! yields a truncated JSON object that parses into a plausible wrong number.
//!
//! The capture was taken against invented host names pointed at real machines,
//! so the bytes are genuine and nobody's infrastructure is in a public tree —
//! the rule `tests/harness/fleet.json` already states for the harness.

use tuxtop_core::remote::{dechunk, decode_event, split_head, split_sse_frames, Decoded};
use tuxtop_core::supervisor::Event;

const CAPTURE: &[u8] = include_bytes!("fixtures/sse-capture.bin");

/// What one run of the parser produced, so runs fed at different read
/// boundaries can be compared to each other.
#[derive(Debug, Default, PartialEq)]
struct Seen {
    samples: Vec<String>,
    faults: Vec<String>,
    processes: Vec<String>,
    keepalives: usize,
    errors: Vec<String>,
    /// Whether a complete response head arrived at all.
    head: bool,
}

impl Seen {
    fn events(&self) -> usize {
        self.samples.len() + self.faults.len() + self.processes.len()
    }
}

/// Drive the whole stack — head, de-chunk, frame, decode — over `input`,
/// handing it `step` bytes at a time.
///
/// This is the read loop `src-tauri` owns with the socket replaced by a slice,
/// and everything it calls is in core precisely so this test can exist at all
/// (ADR-018 decision 3).
fn read(input: &[u8], step: usize) -> Seen {
    let mut seen = Seen::default();
    let mut socket: Vec<u8> = Vec::new();
    let mut body: Vec<u8> = Vec::new();

    for arrived in input.chunks(step) {
        socket.extend_from_slice(arrived);

        if !seen.head {
            match split_head(&socket) {
                Ok(None) => continue,
                Ok(Some((_, used))) => {
                    socket.drain(..used);
                    seen.head = true;
                }
                Err(e) => {
                    seen.errors.push(e);
                    return seen;
                }
            }
        }

        let decoded = match dechunk(&socket) {
            Ok(d) => d,
            Err(e) => {
                seen.errors.push(e);
                return seen;
            }
        };
        socket.drain(..decoded.consumed);
        body.extend_from_slice(&decoded.data);

        let (frames, rest) = split_sse_frames(&body);
        let outcomes: Vec<_> = frames.into_iter().map(decode_event).collect();
        let keep = rest.len();
        for out in outcomes {
            match out {
                Ok(Decoded::Keepalive) => seen.keepalives += 1,
                Ok(Decoded::Event(Event::Sample(s))) => seen.samples.push(s.host.clone()),
                Ok(Decoded::Event(Event::Fault { host, .. })) => seen.faults.push(host),
                Ok(Decoded::Event(Event::Processes(h))) => seen.processes.push(h),
                Ok(Decoded::Event(other)) => seen.errors.push(format!("unexpected {other:?}")),
                Err(e) => seen.errors.push(e),
            }
        }
        // Only the unconsumed tail survives to the next read. Keeping the
        // whole buffer would re-decode every frame on every arriving byte.
        body.drain(..body.len() - keep);
    }
    seen
}

#[test]
fn the_captured_response_is_chunked_and_says_so() {
    // The line ADR-018 decision 2 hid behind "SSE framing is `data: …\n\n`". A
    // client reading `data:` lines straight off this socket would parse `4B9`
    // as content.
    let (head, used) = split_head(CAPTURE).unwrap().expect("a complete head");
    assert_eq!(head.status, 200);
    assert!(head.chunked, "the stream is chunked: {head:?}");
    assert_eq!(head.content_length, None, "a stream has no length");
    assert!(used > 0 && used < CAPTURE.len());
}

#[test]
fn the_capture_decodes_one_byte_at_a_time() {
    // The whole point: no read boundary in this response may produce a
    // truncated event, and none may lose one either.
    let seen = read(CAPTURE, 1);
    assert!(seen.head, "the capture contains a full response head");
    assert!(
        seen.errors.is_empty(),
        "a byte-at-a-time read produced errors: {:?}",
        seen.errors
    );
    assert!(!seen.samples.is_empty(), "the capture carries samples");
    assert!(!seen.faults.is_empty(), "and a fault");
    assert!(!seen.processes.is_empty(), "and a process ranking");
    assert!(
        seen.samples.iter().all(|h| h == "dove"),
        "a sample was attributed to a host that did not send it: {:?}",
        seen.samples
    );
    assert!(
        seen.faults.iter().all(|h| h == "wader"),
        "a fault landed on the wrong card: {:?}",
        seen.faults
    );
}

#[test]
fn a_keepalive_comment_is_not_an_event() {
    // axum sends `3\r\n:\n\n\r\n` on an idle connection, as the normal case
    // rather than an edge one — this capture has them, from a fleet sampling
    // every 20 s. Counting one as an event would draw a card from a frame with
    // no payload; erroring on one would fill the log of every quiet fleet.
    let bare = CAPTURE
        .windows(7)
        .filter(|w| *w == b"\r\n:\n\n\r\n")
        .count();
    assert!(
        bare >= 1,
        "the captured response contains no keep-alive, so this test proves \
         nothing — recapture from a fleet that goes idle"
    );

    let seen = read(CAPTURE, 1);
    assert!(seen.errors.is_empty(), "{:?}", seen.errors);
    // Exactly the number in the capture: one silently swallowed as an event,
    // or one counted twice, shows up here.
    assert_eq!(
        seen.keepalives, bare,
        "keep-alives were not all seen as such"
    );
}

#[test]
fn the_read_boundary_cannot_change_what_the_parser_sees() {
    // One event happened to be one chunk in this capture and nothing
    // guarantees it. A parser that works only when the reads line up with the
    // frames works until the day the network splits one — a bug that appears
    // under load and nowhere else.
    let one = read(CAPTURE, 1);
    for step in [2, 3, 7, 64, 997, CAPTURE.len()] {
        assert_eq!(
            read(CAPTURE, step),
            one,
            "reading {step} bytes at a time saw something different"
        );
    }
}

#[test]
fn a_truncated_capture_yields_no_event_for_the_frame_it_cut() {
    // The buffered-tail rule against real bytes rather than a constructed
    // string: every prefix of this response decodes to a *prefix* of its
    // events, never to one more and never to a different one.
    let full = read(CAPTURE, 1);
    let mut previous = 0;
    for cut in (1..CAPTURE.len()).step_by(29) {
        let seen = read(&CAPTURE[..cut], 1);
        assert!(
            seen.errors.is_empty(),
            "a response cut at {cut} produced {:?}",
            seen.errors
        );
        assert!(
            seen.events() >= previous,
            "a longer prefix lost an event at {cut}"
        );
        assert!(
            seen.events() <= full.events(),
            "a prefix cut at {cut} invented an event"
        );
        assert_eq!(
            seen.samples,
            full.samples[..seen.samples.len()],
            "a prefix cut at {cut} decoded a different sample"
        );
        previous = seen.events();
    }
    assert_eq!(
        read(&CAPTURE[..CAPTURE.len()], 1),
        full,
        "the last prefix is the whole thing"
    );
}

#[test]
fn a_real_sample_arrives_with_its_cores_intact() {
    // The founding hazard at the far end of a new transport: a sample that
    // decodes into something plausible and wrong. The capture came from a
    // 16-core host, and the aggregate has to be consistent with the cores
    // beside it rather than merely present.
    let (_, used) = split_head(CAPTURE).unwrap().unwrap();
    let body = dechunk(&CAPTURE[used..]).unwrap().data;
    let (frames, _) = split_sse_frames(&body);
    let sample = frames
        .into_iter()
        .filter_map(|f| match decode_event(f) {
            Ok(Decoded::Event(Event::Sample(s))) => Some(s),
            _ => None,
        })
        .next()
        .expect("the capture carries at least one sample");

    assert_eq!(
        sample.cores.len(),
        16,
        "the captured host has 16 logical cores"
    );
    assert!(
        (0.0..=100.0).contains(&sample.cpu),
        "aggregate CPU out of range: {}",
        sample.cpu
    );
    assert!(sample.mem_total_kb > 0, "MemTotal did not survive the wire");
    assert!(
        sample.mem_used_kb < sample.mem_total_kb,
        "used {} of {} kB",
        sample.mem_used_kb,
        sample.mem_total_kb
    );
    // Consistent with its own cores, not merely present — which is what a
    // decoder that mapped the array badly would still manage.
    let mean = sample.cores.iter().sum::<f32>() / sample.cores.len() as f32;
    assert!(
        (sample.cpu - mean).abs() < 1.0,
        "aggregate {} does not match its own cores (mean {mean})",
        sample.cpu
    );
}
