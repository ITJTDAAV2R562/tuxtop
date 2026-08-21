# Frontend harness

Drives the real `src/app.js` live path in an ordinary browser by stubbing
`window.__TAURI__`. There is no other way to test the live UI without building
and clicking the GUI.

This exists because it earned its place: the "adding a second host removes the
first" bug survived several rounds of reading the code and was found in one
pass here.

## Use

```sh
mkdir -p /tmp/tuxtop-harness
cp src/index.html src/app.js src/styles.css tests/harness/stub.js /tmp/tuxtop-harness/
# inject the stub before app.js
sed -i 's|<script src="app.js">|<script src="stub.js"></script>\n<script src="app.js">|' \
  /tmp/tuxtop-harness/index.html
cd /tmp/tuxtop-harness && python3 -m http.server 8774
```

Then open `http://localhost:8774/`. `window.__STUB__.hosts()` returns the
fake backend's host list, so a test can assert the UI matches it.

`stub.js` mirrors `hostlist.rs` and `supervisor.rs`: duplicate names are
rejected, `hosts-changed` fires on add and remove, and samples arrive on a
timer. Edit the `setTimeout` in `add_host` to simulate an unreachable host
that never reports — that case is what exposed the reconciliation bug.
