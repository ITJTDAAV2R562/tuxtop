//! The socket half of remote mode: connect, read, emit, record, request.
//!
//! Everything that decides a *value* is in `tuxtop_core::remote`, which this
//! file calls and does not duplicate. ADR-018 decision 3 draws the line there
//! because `src-tauri` is outside the workspace (ADR-006), so `cargo test`
//! never compiles it and CI's `core` job never sees it — only the `windows`
//! job does, and that is a `cargo build`. A parser here would be a parser no
//! test ever runs.
//!
//! What is left is genuinely I/O: opening a TCP connection, feeding bytes to
//! that parser, handing the decoded events to the channel the webview already
//! listens on, and one request the fleet-read proxy makes.
//!
//! **No blocking I/O.** ADR-018's revisit note says a blocking client belongs
//! on its own thread, which was written with `ureq` in mind. The hand-rolled
//! client uses `tokio::net` instead, so nothing here blocks a runtime worker
//! and no thread-per-request is needed. The note applies again the day `ureq`
//! is adopted; until then there is nothing to move off the runtime.

use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::Sender;
use tuxtop_core::history_store::HistoryStore;
use tuxtop_core::remote::{
    command_request, dechunk, decode_event, events_request, next_after_failure, parse_endpoint,
    reconnect_delay, split_head, split_sse_frames, Decoded, Endpoint, Next,
};
use tuxtop_core::supervisor::Event;

/// How long one `POST /api/:command` may take end to end.
///
/// A command the user is waiting on cannot hang forever on a server that
/// accepted the connection and then stopped talking — a spinner with no end is
/// worse than an error naming the endpoint. Generous enough for a tailnet round
/// trip and a fleet-sized answer.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Read buffer. One SSE frame for a 32-core host is ~1.2 KB, so this takes
/// several per syscall without being a page hog per connection.
const READ_CHUNK: usize = 16 * 1024;

/// A body larger than this from our own server is a corrupt stream.
///
/// The largest legitimate answer is a fleet-wide history query, capped at
/// `MAX_POINTS` per series in `Service`; a nineteen-host fleet is well under a
/// megabyte. Refusing early means a bad `content-length` cannot make us
/// allocate on its say-so.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

// ---------------------------------------------------------------------------
// The read loop
// ---------------------------------------------------------------------------

/// The one reader feeding this window, and the handle that can stop it.
///
/// **Exactly one reader is a property of this type, not a rule its callers have
/// to remember** — the ADR-012 lesson applied one layer up. Two loops decoding
/// two fleets into the same cards is what a switch produces when the old one is
/// spawned and forgotten, and it is invisible: both fleets simply appear.
/// `start` aborts whatever was running before it installs a replacement, and
/// `stop` is the other half, for the switch that ends in local mode.
///
/// The channel and the store are held here rather than passed to `start`,
/// because they are properties of *this window* and not of an endpoint: the
/// switching command has neither in hand, and giving it either would be two
/// more things to thread through a Tauri state bag correctly.
pub struct ReadLoop {
    tx: Sender<Event>,
    history: Arc<HistoryStore>,
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl ReadLoop {
    /// The reader for one window: where its events go, and where its samples
    /// are recorded.
    pub fn new(tx: Sender<Event>, history: Arc<HistoryStore>) -> Self {
        Self {
            tx,
            history,
            task: Mutex::new(None),
        }
    }

    /// Point this window at `endpoint`, replacing any reader already running.
    pub fn start(&self, endpoint: String) {
        let mut slot = self.task.lock().expect("read loop lock");
        if let Some(old) = slot.take() {
            old.abort();
        }
        *slot = Some(tauri::async_runtime::spawn(run(
            endpoint,
            self.tx.clone(),
            self.history.clone(),
        )));
    }

    /// Stop reading.
    ///
    /// The half a caller forgets, and the reason it shipped unused: switching
    /// *back to local* is the only path that ends with no reader at all, so it
    /// is the only path `start` cannot cover. A loop left running there would
    /// keep painting the server's fleet over the local one that has just been
    /// restarted — with no error anywhere, because both are answering
    /// correctly.
    pub fn stop(&self) {
        if let Some(old) = self.task.lock().expect("read loop lock").take() {
            old.abort();
        }
    }
}

/// Connect, read, reconnect, forever.
///
/// **It retries forever and does not back off.** A viewer left open overnight
/// against a server that restarts must come back on its own; backoff exists to
/// protect a shared service from many clients, and this is one client against
/// one machine the user named. What backoff would buy instead is a delay grown
/// to minutes, so the server returns and the grid does not — which reads as the
/// app being broken at exactly the moment it was fixed. The delay is
/// `tuxtop_core::remote::reconnect_delay`, which is bounded and tested.
async fn run(endpoint: String, tx: Sender<Event>, history: Arc<HistoryStore>) {
    let ep = match parse_endpoint(&endpoint) {
        Ok(ep) => ep,
        Err(e) => {
            // A verdict rather than a failure, so there is no loop around it.
            debug_assert_eq!(next_after_failure(true, false), Next::Refuse);
            eprintln!("remote: {endpoint} refused — {e}");
            return;
        }
    };

    let mut attempt = 0usize;
    let mut ever = false;
    loop {
        let mut connected = false;
        let outcome = session(&ep, &tx, &history, &mut connected).await;
        if connected {
            ever = true;
            // A session that got as far as a response head starts the next
            // reconnect from the short end of the table, so a server that
            // restarts is picked up promptly rather than at the ceiling.
            attempt = 0;
        }
        match outcome {
            Ok(()) => eprintln!("remote: {} closed the stream", ep.authority()),
            Err(e) => match next_after_failure(false, ever) {
                // Nothing was ever reached: this may be a typo rather than an
                // outage, and only the person who typed it can tell.
                //
                // Logged here, and *reported* by `use_endpoint`, which is the
                // path that can hand the failure back to the window
                // synchronously. This line cannot be the report: a release
                // build has no stderr (`windows_subsystem = "windows"`), so it
                // exists for a debug build and the smoke test. It still runs on
                // every retry of a first connect that keeps failing, which is
                // where a launch-time endpoint that was never reachable shows
                // up — nothing typed it, so nothing was waiting on an answer.
                Next::Report => eprintln!("remote: nothing reached at {} — {e}", ep.authority()),
                // An established connection dropped. The chrome states it.
                Next::Quiet => {}
                Next::Refuse => unreachable!("the endpoint parsed before the loop"),
            },
        }
        tokio::time::sleep(reconnect_delay(attempt)).await;
        attempt = attempt.saturating_add(1);
    }
}

/// One connection, from `GET /api/events` until it ends.
///
/// Sets `connected` once a 200 response head has arrived, which is what tells
/// a typo apart from an outage upstairs. Returns `Ok` when the server ended
/// the stream and `Err` when it broke.
async fn session(
    ep: &Endpoint,
    tx: &Sender<Event>,
    history: &Arc<HistoryStore>,
    connected: &mut bool,
) -> Result<(), String> {
    let mut sock = TcpStream::connect(ep.authority())
        .await
        .map_err(|e| format!("connect: {e}"))?;
    sock.write_all(events_request(ep).as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;

    // Raw bytes off the socket, and the de-chunked body they decode to. Two
    // buffers because they are two layers: a chunk boundary lands mid-frame by
    // construction, so neither can be parsed from the other's tail.
    let mut socket: Vec<u8> = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    let mut have_head = false;
    let mut buf = vec![0u8; READ_CHUNK];

    loop {
        let n = sock
            .read(&mut buf)
            .await
            .map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Ok(());
        }
        socket.extend_from_slice(&buf[..n]);

        if !have_head {
            match split_head(&socket)? {
                // Never parse a partial head: half a head parses into a
                // plausible and wrong one, and everything after it is decoded
                // on its say-so.
                None => continue,
                Some((head, used)) => {
                    if head.status != 200 {
                        return Err(format!("{} answered {}", ep.authority(), head.status));
                    }
                    socket.drain(..used);
                    have_head = true;
                    *connected = true;
                }
            }
        }

        let decoded = dechunk(&socket)?;
        socket.drain(..decoded.consumed);
        body.extend_from_slice(&decoded.data);

        // Frames borrow `body`, so they are decoded into owned values before
        // the tail is trimmed. The tail is what survives to the next read.
        let (frames, rest) = split_sse_frames(&body);
        let keep = rest.len();
        let outcomes: Vec<_> = frames.into_iter().map(decode_event).collect();
        body.drain(..body.len() - keep);

        for out in outcomes {
            match out {
                // The normal state of an idle connection, and not an event.
                Ok(Decoded::Keepalive) => {}
                Ok(Decoded::Event(ev)) => {
                    // History is recorded here rather than proxied: ADR-017
                    // rule 2 makes it in-memory per instance and discarded on
                    // a switch, so a remote viewer's charts mean what this
                    // window has seen since it connected.
                    tuxtop_core::remote::record_event(history, &ev);
                    // Into the channel the webview already listens on, so the
                    // window cannot tell where its events came from - which is
                    // the property that makes this a data-plane change rather
                    // than a second frontend (ADR-018 decision 1).
                    if tx.send(ev).await.is_err() {
                        return Ok(());
                    }
                }
                // Logged, never swallowed: a frame that did not make sense is
                // how a renamed field or a version skew announces itself, and
                // silence here is a card that stops updating for no stated
                // reason.
                Err(e) => eprintln!("remote: {} sent a frame we could not read — {e}", ep.authority()),
            }
        }

        if decoded.end {
            return Ok(());
        }
    }
}

// ---------------------------------------------------------------------------
// The request the fleet-read proxy makes
// ---------------------------------------------------------------------------

/// `POST /api/<command>` against a `tuxtop-serve`, decoded into `T`.
///
/// The other half of ADR-018 decision 2's two requests. A server error comes
/// back as `{"error": "..."}` with a 4xx, and is returned as that string so the
/// frontend's existing catch blocks show the server's own words rather than a
/// status code.
pub async fn post<T: DeserializeOwned>(
    endpoint: &str,
    command: &str,
    args: serde_json::Value,
) -> Result<T, String> {
    let ep = parse_endpoint(endpoint).map_err(|e| e.to_string())?;
    let payload = serde_json::to_string(&args).map_err(|e| format!("encoding {command}: {e}"))?;
    let req = command_request(&ep, command, &payload)?;

    let bytes = tokio::time::timeout(REQUEST_TIMEOUT, fetch(&ep, req))
        .await
        .map_err(|_| {
            format!(
                "{command}: {} did not answer within {}s",
                ep.authority(),
                REQUEST_TIMEOUT.as_secs()
            )
        })??;

    serde_json::from_slice(&bytes)
        .map_err(|e| format!("{command}: {} sent something unreadable — {e}", ep.authority()))
}

/// Send one request and read one response body.
async fn fetch(ep: &Endpoint, req: String) -> Result<Vec<u8>, String> {
    let mut sock = TcpStream::connect(ep.authority())
        .await
        .map_err(|e| format!("connect {}: {e}", ep.authority()))?;
    sock.write_all(req.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;

    let mut raw: Vec<u8> = Vec::new();
    let mut head = None;
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        let n = sock
            .read(&mut buf)
            .await
            .map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if head.is_none() {
            if let Some((h, used)) = split_head(&raw)? {
                if let Some(len) = h.content_length {
                    if len > MAX_BODY_BYTES {
                        return Err(format!("a {len}-byte answer was refused"));
                    }
                }
                raw.drain(..used);
                head = Some(h);
            }
        }
        // Stop as soon as the whole declared body is in hand, rather than
        // waiting for a close the server has no reason to send on a keep-alive
        // connection.
        if let Some(h) = &head {
            if h.content_length.is_some_and(|len| raw.len() >= len) {
                break;
            }
        }
        if raw.len() > MAX_BODY_BYTES {
            return Err("the answer outgrew what we will hold".to_string());
        }
    }

    let head = head.ok_or_else(|| format!("{} closed before answering", ep.authority()))?;
    if let Some(len) = head.content_length {
        raw.truncate(len);
    }
    if head.status != 200 {
        // The server's own sentence, not a status code: it explains why a
        // read-only server refused, and the frontend shows it verbatim.
        let said: Option<serde_json::Value> = serde_json::from_slice(&raw).ok();
        let msg = said
            .as_ref()
            .and_then(|v| v.get("error"))
            .and_then(|e| e.as_str())
            .map(str::to_string);
        return Err(msg.unwrap_or_else(|| format!("{} answered {}", ep.authority(), head.status)));
    }
    Ok(raw)
}
