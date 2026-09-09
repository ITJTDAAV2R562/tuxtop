//! Reading a `tuxtop-serve` response: the parser half of remote mode.
//!
//! [ADR-018](../../../docs/DECISIONS.md) decision 3 splits the remote data
//! plane the same way the frontend is split — pure logic in a module that is
//! tested, sockets outside it. Everything here decides a *value*; nothing here
//! opens a connection. The connect, the read loop and the blocking `POST` live
//! in `src-tauri/src/remote.rs`, which is outside the workspace and is
//! therefore never compiled by `cargo test` or by CI's `core` job. A parser
//! placed there would be a parser no test ever runs, in the one project whose
//! stated thesis is that the tests are the memory it does not otherwise have.
//!
//! **A parser is not a client.** Core gains the ability to *read* a frame, not
//! to fetch one — a `tuxtop-serve` that could view another `tuxtop-serve` is
//! federation, an ADR-017 non-goal, and building its mechanism here while
//! relying on nobody calling it is how a non-goal ships by accident.
//!
//! ## The wire, captured rather than assumed
//!
//! `tests/fixtures/sse-capture.bin` is a real response, taken off the socket
//! from a running server, and `tests/sse_capture.rs` feeds it to this module
//! one byte at a time. Two things about it are the normal case rather than
//! edge cases, and both were discovered by looking instead of reasoning:
//!
//! - **It is chunked.** A client reading `data:` lines straight off the socket
//!   parses chunk-length lines as content, and a chunk boundary lands mid-frame
//!   by construction. So [`dechunk`] runs before [`split_sse_frames`].
//! - **A quiet fleet sends `:\n\n`** — axum's keep-alive, a complete SSE frame
//!   carrying a comment and no `data:` line, on every idle connection.
//!   Decoding that as a truncated event is this project's founding bug
//!   arriving over a new transport, so [`decode_event`] answers
//!   [`Decoded::Keepalive`] rather than an event or an error.
//!
//! The chunk sizes in that capture are upper-case hex (`4B9`), which is a
//! reminder that the length line is not ours to predict either.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::history_store::HistoryStore;
use crate::supervisor::Event;

// ---------------------------------------------------------------------------
// The endpoint
// ---------------------------------------------------------------------------

/// The port `tuxtop-serve` listens on unless told otherwise. Repeated here
/// rather than imported because core does not depend on the server crate, and
/// a viewer that guessed a different default would fail to reach a server
/// started with no flags at all.
pub const DEFAULT_PORT: u16 = 8787;

/// A server this viewer can be pointed at.
///
/// Only ever built by [`parse_endpoint`], so an `Endpoint` in hand is one that
/// has already passed the refusals below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Host or IP, without the brackets an IPv6 literal carries in a URL.
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    /// `host:port`, with an IPv6 literal bracketed again — what goes in a
    /// `Host:` header and what `TcpStream::connect` accepts.
    pub fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// What the chrome shows the user, and what `[settings] server` holds.
    pub fn origin(&self) -> String {
        format!("http://{}", self.authority())
    }
}

/// Why an endpoint was refused.
///
/// A variant per reason rather than a formatted `String`, for the reason
/// `tuxtop-serve`'s `ArgError` gives: a test asserting "returns some error" is
/// also satisfied by a typo elsewhere in the parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointError {
    /// Nothing was typed.
    Empty,
    /// `https://`. Refused loudly, with the fix named — see [`ADR-018`
    /// decision 2](../../../docs/DECISIONS.md): the viewer speaks plain HTTP
    /// and delegates encryption to the transport underneath it.
    Tls(String),
    /// Some other scheme entirely.
    Scheme(String),
    /// A path beyond `/`. The server's routes are at the root, so a path here
    /// would be silently dropped from every request built from it.
    Path(String),
    BadPort(String),
    BadHost(String),
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "no server address"),
            Self::Tls(u) => write!(
                f,
                "{u} is https, and this viewer speaks plain HTTP. Reach the \
                 server over `ssh -L` or a tailnet address and use http:// - \
                 the encryption is the transport's job, not this app's"
            ),
            Self::Scheme(u) => write!(f, "{u}: only http:// is understood"),
            Self::Path(u) => write!(
                f,
                "{u} names a path. A server address is a host and a port; the \
                 API lives at the root"
            ),
            Self::BadPort(v) => write!(f, "bad port: {v}"),
            Self::BadHost(v) => write!(f, "bad server address: {v}"),
        }
    }
}

/// Read `[settings] server` into something a socket can be opened to.
///
/// Accepts `http://host`, `http://host:port`, `host:port`, `host`, and IPv6
/// in brackets. Refuses `https://` and anything unparseable — which is a
/// *verdict*, not a failure: the read loop never retries one, because a retry
/// that cannot ever succeed is a loop with no exit.
pub fn parse_endpoint(text: &str) -> Result<Endpoint, EndpointError> {
    let raw = text.trim();
    if raw.is_empty() {
        return Err(EndpointError::Empty);
    }

    let rest = match raw.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("https") => {
            return Err(EndpointError::Tls(raw.to_string()))
        }
        Some(_) => return Err(EndpointError::Scheme(raw.to_string())),
        None => raw,
    };

    // A single trailing slash is what a browser's address bar produces and is
    // not a path. Anything after it is.
    let authority = match rest.split_once('/') {
        Some((a, "")) => a,
        Some(_) => return Err(EndpointError::Path(raw.to_string())),
        None => rest,
    };
    if authority.is_empty() {
        return Err(EndpointError::BadHost(raw.to_string()));
    }

    let (host, port) = split_authority(authority)?;
    if host.is_empty() || host.contains(char::is_whitespace) || host.contains('@') {
        return Err(EndpointError::BadHost(raw.to_string()));
    }
    Ok(Endpoint { host, port })
}

/// `host`, `host:port`, `[v6]` or `[v6]:port` into its two parts.
fn split_authority(authority: &str) -> Result<(String, u16), EndpointError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, tail)) = rest.split_once(']') else {
            return Err(EndpointError::BadHost(authority.to_string()));
        };
        let port = match tail {
            "" => DEFAULT_PORT,
            _ => {
                let Some(p) = tail.strip_prefix(':') else {
                    return Err(EndpointError::BadHost(authority.to_string()));
                };
                p.parse()
                    .map_err(|_| EndpointError::BadPort(p.to_string()))?
            }
        };
        return Ok((host.to_string(), port));
    }

    // A bare IPv6 literal has several colons; only one means host:port.
    match authority.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Ok((
            h.to_string(),
            p.parse()
                .map_err(|_| EndpointError::BadPort(p.to_string()))?,
        )),
        _ => Ok((authority.to_string(), DEFAULT_PORT)),
    }
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// The event stream. Two requests exist against a server in this repository,
/// and this is the one the read loop holds open.
pub const EVENTS_PATH: &str = "/api/events";

/// `GET /api/events`, ready to write to a socket.
pub fn events_request(ep: &Endpoint) -> String {
    format!(
        "GET {EVENTS_PATH} HTTP/1.1\r\n\
         Host: {}\r\n\
         Accept: text/event-stream\r\n\
         Cache-Control: no-cache\r\n\
         \r\n",
        ep.authority()
    )
}

/// `POST /api/<command>` with a JSON body — the request half of the proxy.
///
/// The command name is checked rather than trusted. It arrives from the
/// frontend through a Tauri command, so a name carrying `\r\n` would let the
/// caller append headers of its own; refusing anything but the shape a command
/// actually has makes that impossible by construction rather than by escaping.
pub fn command_request(ep: &Endpoint, command: &str, body: &str) -> Result<String, String> {
    if command.is_empty()
        || !command
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(format!(
            "{command:?} is not a command name: letters, digits and underscores only"
        ));
    }
    Ok(format!(
        "POST /api/{command} HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        ep.authority(),
        body.len()
    ))
}

// ---------------------------------------------------------------------------
// The response head
// ---------------------------------------------------------------------------

/// What the head of a response says about the body that follows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Head {
    pub status: u16,
    /// `transfer-encoding: chunked`. True for the event stream, and the reason
    /// [`dechunk`] exists at all.
    pub chunked: bool,
    /// Present on a command response, absent on the stream.
    pub content_length: Option<usize>,
}

/// Split the head off a response, returning it and the bytes it consumed.
///
/// `Ok(None)` means the head has not fully arrived — the same rule
/// `split_frames` holds, for the same reason: half a head parses into a
/// plausible and wrong one. A malformed status line is an error rather than a
/// default, because a viewer that read "some response arrived" out of a proxy
/// error page would then decode its HTML as events.
pub fn split_head(buf: &[u8]) -> Result<Option<(Head, usize)>, String> {
    let Some(end) = find(buf, b"\r\n\r\n") else {
        return Ok(None);
    };
    let text = String::from_utf8_lossy(&buf[..end]);
    let mut lines = text.split("\r\n");

    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.split(' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/") {
        return Err(format!("not an HTTP response: {status_line:?}"));
    }
    let status: u16 = parts
        .next()
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("no status code in {status_line:?}"))?;

    let mut head = Head {
        status,
        ..Head::default()
    };
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("transfer-encoding") {
            head.chunked = value.eq_ignore_ascii_case("chunked");
        } else if name.eq_ignore_ascii_case("content-length") {
            head.content_length = value.parse().ok();
        }
    }
    Ok(Some((head, end + 4)))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// De-chunking
// ---------------------------------------------------------------------------

/// A chunk longer than this is a corrupt stream, not a big frame.
///
/// The largest thing the server sends in one piece is a `hosts-changed` list;
/// a nineteen-host fleet is a few kilobytes. Refusing early means a garbled
/// length line cannot make us allocate on its say-so.
pub const MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024;

/// As much of a chunked body as has fully arrived.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dechunked {
    /// The decoded bytes, chunk framing removed.
    pub data: Vec<u8>,
    /// How many input bytes those consumed. The caller keeps the rest and
    /// prepends it to the next read.
    pub consumed: usize,
    /// The terminal zero-length chunk arrived: the server ended the body
    /// deliberately, rather than the socket dying under us. The read loop
    /// tells those apart when it decides whether to say anything.
    pub end: bool,
}

/// Strip chunked transfer encoding, stopping at the first incomplete chunk.
///
/// A partial chunk is never decoded — the [`split_sse_frames`] rule one layer
/// down, applied one layer up, because a chunk cut in half yields bytes that
/// look exactly like a short frame.
pub fn dechunk(buf: &[u8]) -> Result<Dechunked, String> {
    let mut out = Dechunked::default();
    let mut pos = 0usize;

    while let Some(eol) = find(&buf[pos..], b"\r\n") {
        let line = &buf[pos..pos + eol];
        // A chunk extension (`;name=value`) is legal and unused by our server.
        let digits = match line.iter().position(|b| *b == b';') {
            Some(i) => &line[..i],
            None => line,
        };
        let text = std::str::from_utf8(digits)
            .map_err(|_| "chunk length is not text".to_string())?
            .trim();
        // Upper *or* lower case: the captured response uses `4B9`.
        let size =
            usize::from_str_radix(text, 16).map_err(|_| format!("bad chunk length {text:?}"))?;
        if size > MAX_CHUNK_BYTES {
            return Err(format!("chunk of {size} bytes refused"));
        }

        let data = pos + eol + 2;
        if size == 0 {
            out.consumed = data;
            out.end = true;
            break;
        }
        // The chunk's own trailing CRLF has to be here too, or the next length
        // line would be read out of the middle of this chunk's data.
        if buf.len() < data + size + 2 {
            break;
        }
        if &buf[data + size..data + size + 2] != b"\r\n" {
            return Err("chunk is not followed by CRLF".to_string());
        }
        out.data.extend_from_slice(&buf[data..data + size]);
        pos = data + size + 2;
        out.consumed = pos;
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// SSE framing
// ---------------------------------------------------------------------------

/// The frame delimiter. Our own server writes `\n\n`; this parser reads what
/// that server sent and nothing else.
const SSE_DELIMITER: &[u8] = b"\n\n";

/// Split a decoded body into complete SSE frames, returning the unconsumed
/// tail.
///
/// The mirror of `sampler::split_frames`, and it inherits the hard rule that
/// function's name points at: **never let the parser see a partial frame.** An
/// event split across two reads that decodes as a truncated JSON object is
/// this project's founding bug arriving over a new transport.
pub fn split_sse_frames(buf: &[u8]) -> (Vec<&[u8]>, &[u8]) {
    let mut frames = Vec::new();
    let mut rest = buf;
    while let Some(idx) = find(rest, SSE_DELIMITER) {
        frames.push(&rest[..idx]);
        rest = &rest[idx + SSE_DELIMITER.len()..];
    }
    (frames, rest)
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// What one complete SSE frame turned out to be.
///
/// [`Decoded::Keepalive`] is a variant rather than `None` or an error because
/// it is the *normal* state of an idle connection, and the three outcomes have
/// to stay distinguishable: an event to deliver, nothing to deliver, and a
/// frame that did not make sense and must be logged.
#[derive(Debug)]
pub enum Decoded {
    Event(Event),
    Keepalive,
}

/// Read one complete SSE frame.
///
/// The inverse of `tuxtop-serve`'s `api::encode_event`, under the same topic
/// names — `every_event_the_encoder_writes_the_viewer_can_read` in that crate
/// is what keeps the two halves from drifting.
pub fn decode_event(frame: &[u8]) -> Result<Decoded, String> {
    let text = std::str::from_utf8(frame).map_err(|_| "frame is not UTF-8".to_string())?;

    let mut data = String::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        // A comment. axum's keep-alive is exactly this and nothing else.
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        if field == "data" {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    if data.is_empty() {
        return Ok(Decoded::Keepalive);
    }

    let v: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| format!("event is not JSON: {e}"))?;
    let topic = v
        .get("event")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "event has no topic".to_string())?;
    let payload = v.get("payload").cloned().unwrap_or(serde_json::Value::Null);
    let bad = |e: serde_json::Error| format!("{topic}: {e}");

    let ev = match topic {
        "tuxtop://sample" => Event::Sample(serde_json::from_value(payload).map_err(bad)?),
        "tuxtop://fault" => {
            // The encoder inserts `host` into the fault object, because a bare
            // fault cannot be attributed to a card and attributing one to the
            // wrong card is worse than dropping it. Taken back out here rather
            // than left for the enum to ignore, so the shape either matches or
            // says why.
            let mut o = payload
                .as_object()
                .cloned()
                .ok_or_else(|| "fault payload is not an object".to_string())?;
            let host = o
                .remove("host")
                .and_then(|h| h.as_str().map(str::to_string))
                .ok_or_else(|| "fault carries no host".to_string())?;
            let fault = serde_json::from_value(serde_json::Value::Object(o)).map_err(bad)?;
            Event::Fault { host, fault }
        }
        "tuxtop://processes" => Event::Processes(serde_json::from_value(payload).map_err(bad)?),
        "tuxtop://hosts-changed" => {
            Event::HostsChanged(serde_json::from_value(payload).map_err(bad)?)
        }
        "tuxtop://settings-changed" => {
            Event::SettingsChanged(serde_json::from_value(payload).map_err(bad)?)
        }
        // Named rather than dropped, for the reason `api::command` names an
        // unknown command: the viewer and the server drifting apart is exactly
        // the bug silence would hide.
        other => return Err(format!("unknown event {other}")),
    };
    Ok(Decoded::Event(ev))
}

/// Record what an arriving remote event carries into the local history store.
///
/// History is **not** proxied, deliberately: ADR-017 rule 2 makes it in-memory
/// per instance and discarded on a switch, and fetching the server's would
/// make that rule meaningless and blend two fleets' `db1` the moment endpoints
/// can be switched. So the read loop records what arrives, exactly as
/// `Supervisor` does when it is sampling, and a remote viewer's charts honestly
/// mean *what this window has seen since it connected*.
///
/// Here rather than in the read loop for ADR-018 decision 3's reason: the loop
/// is in `src-tauri`, which nothing in this workspace ever compiles.
pub fn record_event(history: &HistoryStore, ev: &Event) {
    if let Event::Sample(s) = ev {
        history.record(s);
    }
}

// ---------------------------------------------------------------------------
// Freshness
// ---------------------------------------------------------------------------

/// How many missed samples make a reading stale.
///
/// One is a blip — a scheduler hiccup, a re-connect. Three in a row is the
/// server or the link, which is what the chrome exists to say.
pub const STALE_INTERVALS: u32 = 3;

/// The floor under [`stale_after_ms`], so a 4 Hz fleet does not flicker
/// between fresh and stale on a 800 ms pause that means nothing.
pub const STALE_FLOOR_MS: u32 = 3_000;

/// When a reading taken at the server's `interval_ms` stops being current.
///
/// **Not a constant.** A server sampling at 5 s reads as permanently stale
/// against a 1 Hz expectation, which is a window shouting about a fleet that
/// is fine — and a warning nobody believes is worse than none. The rule lives
/// here and the number it yields travels in [`Capabilities::stale_after_ms`],
/// so the browser, which has no core, carries no second copy of it to drift.
pub fn stale_after_ms(interval_ms: u32) -> u32 {
    interval_ms
        .saturating_mul(STALE_INTERVALS)
        .max(STALE_FLOOR_MS)
}

/// Whether data last seen `age_ms` ago is stale, given the server's interval.
pub fn is_stale(age_ms: u64, interval_ms: u32) -> bool {
    age_ms > u64::from(stale_after_ms(interval_ms))
}

// ---------------------------------------------------------------------------
// Reconnecting
// ---------------------------------------------------------------------------

/// How long to wait before each reconnect, in milliseconds, saturating at the
/// last entry.
///
/// **It retries forever and it does not back off.** Exponential backoff exists
/// to protect a shared service from many clients; this is one client against
/// one machine the user named. What backoff would buy instead is a delay grown
/// to minutes, so the server comes back and the grid does not — which reads as
/// the app being broken at exactly the moment it was fixed.
pub const RECONNECT_MS: &[u64] = &[250, 500, 1_000, 2_000, 5_000];

/// The ceiling, named so the test can assert it without restating the table.
pub const RECONNECT_MAX_MS: u64 = 5_000;

/// How long to wait before the `attempt`-th reconnect.
///
/// Pure, and here rather than in the read loop, because the loop lives in
/// `src-tauri` and a number written inline there is a number no test ever
/// sees.
pub fn reconnect_delay(attempt: usize) -> Duration {
    Duration::from_millis(RECONNECT_MS[attempt.min(RECONNECT_MS.len() - 1)])
}

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

/// What the window can actually do, and whose readings it is showing.
///
/// Asked at startup and re-asked on `tuxtop://settings-changed`, because
/// switching endpoints changes every field of it. It reports *effective*
/// capability: `app.js` used to treat the command's absence as "the desktop
/// app, which can do it all", which stops being true the moment that app
/// points at a read-only server, and the result is buttons that can only
/// return an error — the exact thing this exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Whether a write will be accepted. False for a read-only server, and
    /// false for a viewer in remote mode, where the local `hosts.toml` is not
    /// the fleet on screen.
    pub writable: bool,
    /// The server whose readings these are, or `None` when sampling locally.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// [`stale_after_ms`] applied to the interval of whatever is sampling.
    pub stale_after_ms: u32,
    /// The version of whatever produced the events.
    #[serde(default)]
    pub version: String,
    /// Set when this window is a different build from the one above — see
    /// [`version_mismatch`]. `None` when they agree, which is every local
    /// session, since there the two are the same process.
    #[serde(default)]
    pub version_note: Option<String>,
}

/// What to say when the viewer and the source of its events are different
/// builds. `None` when they agree.
///
/// Stated, never guessed: this does not decide which is newer or whether the
/// difference matters. A viewer one release ahead of its server reads a renamed
/// field as absent and draws a plausible wrong number, which is this project's
/// founding hazard — and the honest response to "these disagree" is to say so,
/// not to compute a verdict from a version string.
pub fn version_mismatch(viewer: &str, source: &str) -> Option<String> {
    let viewer = viewer.trim();
    let source = source.trim();
    if source.is_empty() {
        return Some(format!(
            "this window is {viewer}; the server does not say which build it is"
        ));
    }
    if viewer == source {
        return None;
    }
    Some(format!(
        "this window is {viewer}; its readings come from {source}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sample;

    // -- the endpoint -------------------------------------------------------

    #[test]
    fn an_https_endpoint_is_refused_with_the_fix_named() {
        // ADR-018 decision 2. A silent failure, or a connection attempt that
        // dies obscurely, teaches nothing - the message has to say what to do
        // instead, because the answer ("tunnel it") is not guessable from
        // "connection failed".
        let e = parse_endpoint("https://tuxtop.example.ts.net").unwrap_err();
        assert_eq!(
            e,
            EndpointError::Tls("https://tuxtop.example.ts.net".into())
        );
        let msg = e.to_string();
        assert!(msg.contains("ssh -L"), "the fix must be named: {msg}");
        assert!(msg.contains("http://"), "and what to type: {msg}");

        // Case is not the rule either.
        assert!(matches!(
            parse_endpoint("HTTPS://dove"),
            Err(EndpointError::Tls(_))
        ));
    }

    #[test]
    fn an_endpoint_that_cannot_be_parsed_is_refused_rather_than_guessed() {
        // Each of these is a verdict, not a failure: the read loop never
        // retries one, so a wrong guess here is a window that never connects
        // and never says why.
        for (text, want) in [
            ("", EndpointError::Empty),
            ("   ", EndpointError::Empty),
            ("ftp://dove", EndpointError::Scheme("ftp://dove".into())),
            (
                "http://dove/api",
                EndpointError::Path("http://dove/api".into()),
            ),
            ("http://dove:no", EndpointError::BadPort("no".into())),
            (
                "http:// dove",
                EndpointError::BadHost("http:// dove".into()),
            ),
        ] {
            assert_eq!(parse_endpoint(text), Err(want), "{text:?} was accepted");
        }
    }

    #[test]
    fn an_endpoint_without_a_port_reaches_a_server_started_with_no_flags() {
        // The server's default is 8787. A viewer defaulting to 80 would fail
        // against the commonest deployment there is.
        assert_eq!(
            parse_endpoint("http://dove").unwrap(),
            Endpoint {
                host: "dove".into(),
                port: DEFAULT_PORT
            }
        );
        // A bare host, a trailing slash and an explicit port all land in the
        // same place.
        assert_eq!(parse_endpoint("dove").unwrap().port, DEFAULT_PORT);
        assert_eq!(parse_endpoint("http://dove/").unwrap().port, DEFAULT_PORT);
        assert_eq!(parse_endpoint("dove:9000").unwrap().port, 9000);
    }

    #[test]
    fn an_ipv6_literal_is_not_read_as_a_port() {
        // `::1` has colons in it, and splitting on the last one gives a host
        // of `:` and a port of `1` - a plausible parse of a real address that
        // connects somewhere else entirely.
        let ep = parse_endpoint("http://[::1]:8787").unwrap();
        assert_eq!(ep.host, "::1");
        assert_eq!(ep.port, 8787);
        assert_eq!(ep.authority(), "[::1]:8787", "the Host header re-brackets");
        assert_eq!(ep.origin(), "http://[::1]:8787");

        let bare = parse_endpoint("[fd00::1]").unwrap();
        assert_eq!((bare.host.as_str(), bare.port), ("fd00::1", DEFAULT_PORT));
    }

    // -- requests -----------------------------------------------------------

    #[test]
    fn the_event_request_asks_for_the_stream_at_the_right_host() {
        let req = events_request(&parse_endpoint("http://dove:8787").unwrap());
        assert!(req.starts_with("GET /api/events HTTP/1.1\r\n"), "{req}");
        assert!(req.contains("Host: dove:8787\r\n"), "{req}");
        assert!(req.ends_with("\r\n\r\n"), "the head must be terminated");
    }

    #[test]
    fn a_command_name_cannot_smuggle_a_header_into_the_request() {
        // The name arrives from the frontend. A `\r\n` in it would append
        // headers of the caller's choosing to a request we signed as ours.
        let ep = parse_endpoint("dove").unwrap();
        for bad in [
            "list_hosts\r\nX-Evil: 1",
            "list hosts",
            "../events",
            "",
            "list_hosts?x=1",
        ] {
            assert!(
                command_request(&ep, bad, "{}").is_err(),
                "{bad:?} was accepted as a command name"
            );
        }
        let req = command_request(&ep, "list_hosts", r#"{"a":1}"#).unwrap();
        assert!(
            req.starts_with("POST /api/list_hosts HTTP/1.1\r\n"),
            "{req}"
        );
        assert!(req.contains("Content-Length: 7\r\n"), "{req}");
        assert!(req.ends_with("\r\n\r\n{\"a\":1}"), "{req}");
    }

    // -- the response head --------------------------------------------------

    #[test]
    fn a_partial_head_is_never_parsed() {
        // The `split_frames` rule at the top of the stream: half a head parses
        // into a plausible and wrong one, and there is no second chance to
        // notice - everything after it is decoded on its say-so.
        let full = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n";
        for cut in 0..full.len() {
            assert_eq!(
                split_head(&full[..cut]),
                Ok(None),
                "a head cut at {cut} was parsed anyway"
            );
        }
        let (head, used) = split_head(full).unwrap().unwrap();
        assert_eq!(used, full.len());
        assert_eq!(
            head,
            Head {
                status: 200,
                chunked: true,
                content_length: None
            }
        );
    }

    #[test]
    fn a_response_that_is_not_ours_is_an_error_rather_than_a_default() {
        // A proxy error page, or a plain-text refusal. Reading "some response
        // arrived" out of it would send its HTML to the frame parser.
        assert!(split_head(b"<html>oops</html>\r\n\r\n").is_err());
        assert!(split_head(b"HTTP/1.1 no-status\r\n\r\n").is_err());

        // A command response carries a length instead of chunking.
        let (head, _) = split_head(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 12\r\n\r\n")
            .unwrap()
            .unwrap();
        assert_eq!(head.status, 403);
        assert_eq!(head.content_length, Some(12));
        assert!(!head.chunked);
    }

    // -- de-chunking --------------------------------------------------------

    #[test]
    fn a_partial_chunk_stays_buffered() {
        let buf = b"4\r\nabcd\r\n5\r\nefg";
        let d = dechunk(buf).unwrap();
        assert_eq!(d.data, b"abcd");
        assert_eq!(d.consumed, 9, "only the complete chunk is consumed");
        assert!(!d.end);
    }

    #[test]
    fn the_terminal_chunk_is_an_ending_rather_than_a_drop() {
        // A server that closed the body deliberately and a socket that died
        // are different events, and the read loop says different things about
        // them.
        let d = dechunk(b"3\r\nabc\r\n0\r\n\r\n").unwrap();
        assert_eq!(d.data, b"abc");
        assert!(d.end);
    }

    #[test]
    fn a_chunk_length_is_read_in_either_case() {
        // The captured response uses upper-case hex (`4B9`), which is not what
        // anybody writing this from memory would have assumed.
        assert_eq!(
            dechunk(b"1F\r\n0123456789abcdef0123456789abcde\r\n")
                .unwrap()
                .data
                .len(),
            31
        );
        assert_eq!(
            dechunk(b"1f\r\n0123456789abcdef0123456789abcde\r\n")
                .unwrap()
                .data
                .len(),
            31
        );
    }

    #[test]
    fn a_garbled_chunk_length_is_reported_rather_than_defaulted() {
        assert!(dechunk(b"zz\r\nabc\r\n").is_err());
        // A length claiming more than the stream could hold must not make us
        // allocate on its say-so.
        assert!(dechunk(b"FFFFFFFF\r\n").is_err());
        // A chunk not followed by its own CRLF means the next length line
        // would be read out of the middle of this chunk's data.
        assert!(dechunk(b"2\r\nabXX").is_err());
    }

    // -- SSE framing --------------------------------------------------------

    #[test]
    fn split_sse_frames_returns_only_complete_frames() {
        // The mirror of `split_frames_returns_only_complete_frames`, and the
        // rule it is named for: an event split across two reads that decodes
        // as a truncated JSON object is the founding bug on a new transport.
        let (frames, rest) = split_sse_frames(b"data: one\n\ndata: two\n\ndata: par");
        assert_eq!(frames, vec![&b"data: one"[..], &b"data: two"[..]]);
        assert_eq!(rest, b"data: par", "incomplete frame must stay buffered");
    }

    #[test]
    fn a_frame_boundary_inside_a_payload_is_not_invented() {
        // A single newline is a field separator, not a frame end. Splitting on
        // one would cut a multi-line data field in half.
        let (frames, rest) = split_sse_frames(b"data: a\ndata: b\n\n");
        assert_eq!(frames, vec![&b"data: a\ndata: b"[..]]);
        assert_eq!(rest, b"");
    }

    // -- decoding -----------------------------------------------------------

    #[test]
    fn a_keepalive_comment_is_not_an_event() {
        // axum's keep-alive on every idle connection: a complete frame with a
        // comment and no `data:` line. Decoding it as a truncated event is the
        // founding bug wearing a new transport; erroring on it would fill the
        // log of every quiet fleet.
        assert!(matches!(decode_event(b":").unwrap(), Decoded::Keepalive));
        assert!(matches!(decode_event(b": hb").unwrap(), Decoded::Keepalive));
        assert!(matches!(decode_event(b"").unwrap(), Decoded::Keepalive));
    }

    #[test]
    fn a_truncated_event_is_an_error_rather_than_a_partial_sample() {
        // The other half of the rule above. Silence here is a card that stops
        // updating with nothing said about why.
        let err = decode_event(br#"data: {"event":"tuxtop://sample","payl"#).unwrap_err();
        assert!(err.contains("not JSON"), "{err}");
        assert!(
            decode_event(br#"data: {"payload":{}}"#).is_err(),
            "no topic"
        );
        let err = decode_event(br#"data: {"event":"tuxtop://nope","payload":1}"#).unwrap_err();
        assert!(err.contains("unknown event"), "{err}");
    }

    #[test]
    fn a_fault_keeps_the_host_it_belongs_to() {
        // Attributing a fault to the wrong card is worse than dropping it, so
        // the encoder inserts `host` and this has to take it back out.
        let frame = br#"data: {"event":"tuxtop://fault","payload":{"kind":"auth_failed","detail":"no key","host":"wader"}}"#;
        let Decoded::Event(ev) = decode_event(frame).unwrap() else {
            panic!("a fault is an event");
        };
        match ev {
            Event::Fault { host, fault } => {
                assert_eq!(host, "wader");
                assert_eq!(fault, crate::HostFault::AuthFailed("no key".into()));
            }
            other => panic!("decoded as {other:?}"),
        }

        // A fault with no host is refused rather than attributed to nobody.
        assert!(decode_event(
            br#"data: {"event":"tuxtop://fault","payload":{"kind":"auth_failed","detail":"x"}}"#
        )
        .is_err());
    }

    #[test]
    fn remote_samples_are_recorded_in_the_local_history_store() {
        // History is not proxied (ADR-017 rule 2): the read loop records what
        // arrives, so a remote viewer's charts mean what this window has seen
        // since it connected. Here rather than in the read loop because the
        // loop is in `src-tauri`, which nothing in this workspace compiles.
        let h = HistoryStore::new();
        assert_eq!(h.usage().series, 0);

        record_event(
            &h,
            &Event::Sample(Box::new(Sample {
                host: "dove".into(),
                cpu: 42.0,
                ..Default::default()
            })),
        );
        assert!(h.usage().series > 0, "an arriving sample was not recorded");

        // And nothing else is. A `hosts-changed` recorded as data would invent
        // a series for a host that has reported nothing.
        let before = h.usage().series;
        record_event(&h, &Event::Processes("dove".into()));
        record_event(&h, &Event::HostsChanged(vec![]));
        assert_eq!(h.usage().series, before);
    }

    // -- freshness ----------------------------------------------------------

    #[test]
    fn freshness_is_measured_against_the_servers_interval_not_a_constant() {
        // A server sampling at 5 s reads as permanently stale against a 1 Hz
        // expectation - a window shouting about a fleet that is fine, which is
        // how people learn to ignore the one warning that matters.
        assert!(is_stale(6_000, 1_000), "6 s old at 1 Hz is stale");
        assert!(!is_stale(6_000, 5_000), "6 s old at 5 s is one interval");

        // The threshold moves with the interval, which is the whole rule.
        assert!(stale_after_ms(5_000) > stale_after_ms(1_000));
        assert_eq!(stale_after_ms(5_000), 15_000);

        // And the floor holds under it, so a 4 Hz fleet does not flicker.
        assert_eq!(stale_after_ms(250), STALE_FLOOR_MS);
        assert!(!is_stale(1_000, 250), "a 1 s pause at 4 Hz is not stale");

        // An absurd interval must not wrap into "always fresh".
        assert!(stale_after_ms(u32::MAX) >= STALE_FLOOR_MS);
    }

    // -- reconnecting -------------------------------------------------------

    #[test]
    fn reconnect_delay_is_bounded_so_a_returning_server_is_noticed_promptly() {
        // The property that fails if somebody later "improves" this into an
        // exponential backoff: a delay grown to minutes means the server comes
        // back and the grid does not, which reads as the app being broken at
        // exactly the moment it was fixed.
        for attempt in [0, 1, 2, 5, 50, 5_000, usize::MAX] {
            assert!(
                reconnect_delay(attempt) <= Duration::from_millis(RECONNECT_MAX_MS),
                "attempt {attempt} waits longer than the ceiling"
            );
        }
        // It retries forever: there is no attempt that yields "give up".
        assert!(reconnect_delay(usize::MAX) > Duration::ZERO);
        // And the first retry is prompt, so a restarted server is picked up in
        // well under a second.
        assert!(reconnect_delay(0) < Duration::from_millis(500));
    }

    // -- capabilities -------------------------------------------------------

    #[test]
    fn a_version_mismatch_is_stated_not_guessed() {
        // A viewer one release ahead reads a renamed field as absent and draws
        // a plausible wrong number. The honest response is to say the two
        // disagree - not to compute a verdict about whether it matters.
        assert_eq!(version_mismatch("0.7.0", "0.7.0"), None);

        let note = version_mismatch("0.8.0", "0.7.0").expect("a difference is stated");
        assert!(note.contains("0.8.0") && note.contains("0.7.0"), "{note}");
        // Both directions, and neither says which is right.
        let back = version_mismatch("0.7.0", "0.8.0").expect("the other way too");
        assert!(!back.contains("newer") && !back.contains("older"), "{back}");

        // A server old enough to send no version at all is the case that
        // matters most, and the one an equality check answers "they agree".
        let silent = version_mismatch("0.8.0", "").expect("an absent version is stated");
        assert!(silent.contains("does not say"), "{silent}");
    }

    #[test]
    fn capabilities_survives_the_wire_it_travels_on() {
        // It is proxied in remote mode, so it goes out as JSON and comes back
        // as JSON. A field that did not round-trip would read as absent, and
        // absent is what `app.js` used to treat as "can do it all".
        let c = Capabilities {
            writable: false,
            endpoint: Some("http://dove:8787".into()),
            stale_after_ms: 3_000,
            version: "0.7.0".into(),
            version_note: None,
        };
        let back: Capabilities = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);

        // An older server answers `{"writable":true}` and nothing else. That
        // has to parse, or pointing a new viewer at one is a blank window.
        let old: Capabilities = serde_json::from_str(r#"{"writable":true,"stale_after_ms":3000}"#)
            .expect("an older server's answer must still parse");
        assert_eq!(old.endpoint, None);
        assert_eq!(old.version, "");
    }
}
