// Talking to a Tuxtop server over HTTP.
//
// The frontend reaches its backend through exactly one thing - `__TAURI__`,
// with `invoke(command, args)` and `event.listen(topic, cb)`. So serving the
// same UI from a server needs no change to app.js at all: this installs an
// implementation of that interface backed by fetch and an EventSource.
//
// It installs itself only when nothing else has. Under the desktop app Tauri
// has already provided the real one, and in the test harness the stub has;
// both must win, which is why this checks rather than assigns.
(() => {
  if (globalThis.__TAURI__) return;
  if (!location.protocol.startsWith('http')) return;

  /// One stream, many listeners.
  ///
  /// A separate EventSource per topic would open five connections to push the
  /// same bytes five times. The server sends one stream tagged with the topic
  /// and this fans it out, which is what the Tauri event bus does anyway.
  const listeners = new Map();
  let stream = null;

  function connect() {
    stream = new EventSource('/api/events');
    stream.onmessage = e => {
      let msg;
      try { msg = JSON.parse(e.data); } catch { return; }
      for (const cb of listeners.get(msg.event) || []) {
        cb({ payload: msg.payload });
      }
    };
    // EventSource reconnects on its own after a network drop, but not after
    // the server closes the stream deliberately. Reopening keeps a browser
    // left open overnight alive across a server restart.
    stream.onerror = () => {
      stream.close();
      stream = null;
      setTimeout(() => { if (listeners.size) connect(); }, 2000);
    };
  }

  window.__TAURI__ = {
    core: {
      async invoke(cmd, args) {
        const res = await fetch(`/api/${encodeURIComponent(cmd)}`, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(args || {}),
        });
        const text = await res.text();
        const body = text ? JSON.parse(text) : null;
        if (!res.ok) {
          // Tauri rejects with a string, so the existing catch blocks - which
          // do `String(err)` - keep working unchanged.
          throw (body && body.error) || `${cmd} failed: ${res.status}`;
        }
        return body;
      },
    },
    event: {
      async listen(topic, cb) {
        if (!listeners.has(topic)) listeners.set(topic, []);
        listeners.get(topic).push(cb);
        if (!stream) connect();
        return () => {
          const l = listeners.get(topic) || [];
          const i = l.indexOf(cb);
          if (i >= 0) l.splice(i, 1);
        };
      },
    },
  };
})();
