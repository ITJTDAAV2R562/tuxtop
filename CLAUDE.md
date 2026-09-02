# CLAUDE.md — Tuxtop

Standing rules for any session working in this repo. Read
[docs/DECISIONS.md](docs/DECISIONS.md) before proposing an architectural
change — the reasoning is recorded there with the alternatives that were
rejected and why.

---

## What this is

A Windows-native Task Manager for remote Linux hosts. Tauri 2 + Rust backend,
HTML/CSS frontend. One data plane: SSH sampling at a configurable interval
(1 Hz default), feeding both the live grid and an in-memory history cascade.

---

## The central hazard: plausible wrong numbers

This project exists because a monitoring agent reported `0.14%` while a host
was at 25% load, and `21.95%` for 13 seconds after the load had stopped. No
error, no crash, no warning — a confident, well-formatted, wrong number.

That is the failure mode to design against. It shapes several rules below.

**A number that looks reasonable is not evidence that it is right.** When you
touch sampling maths, check it against independent ground truth (`top`,
`htop`, `/proc` read by hand) — not against whether the output looks plausible.

---

## Hard rules

**Never let the parser see a partial frame.**
A truncated `/proc/stat` parses successfully into a wrong snapshot. `split_frames`
returns only complete frames and buffers the tail. Do not "simplify" this by
parsing whatever has arrived.

**CPU percentage is always a delta of two samples.**
Never report an instantaneous value. Specifically:
- `iowait` counts as **idle** — otherwise an NFS stall reads as 100% CPU.
- `guest`/`guest_nice` are **excluded** from the total — the kernel already
  counts them inside `user`/`nice`, so adding them double-counts.
- Memory uses **`MemAvailable`**, never `MemFree` — `MemFree` reports a healthy
  Linux box as nearly out of memory.

Each of these is pinned by a named test. If you change one, the test name tells
you what invariant you are breaking.

**One SSH connection per host, held open.**
Never spawn a process or open a connection per sample. A handshake per second
costs more than the interval it is measuring.

**A Windows remote loop must be able to end itself, and you must soak-test it.**
Killing the local `ssh` client does not stop the far side on Windows: sshd
leaves the command running with both pipes intact, so a broken stdout never
surfaces and stdin never reaches EOF. The loop watches sshd's session process
instead and exits when it goes — [ADR-013](docs/DECISIONS.md#adr-013--a-windows-remote-loop-watches-its-sshd-session-not-its-pipes).
Linux is unaffected (SIGHUP), which is why nothing on the dev box reproduces
any of this.

Two things follow. **Unit tests cannot check it** — they assert on the script
text we generate, and the whole failure mode is that the far side behaves
differently than the text implies. A heartbeat design passed every unit test
and would have dropped every Windows host in the fleet 30 s in. **So verify
against a real Windows host, and soak for longer than any timeout in the
mechanism**: confirm a session survives well past it, *then* that the remote
exits after the client dies. Checking only the second half is how the broken
design got as far as it did.

**Nothing is installed on the monitored host.**
No agent, no binary copied over, no package, no open port, no firewall change.
If a feature seems to need one, it is collected over the existing SSH stream or
it does not ship. [ADR-004](docs/DECISIONS.md#adr-004--nothing-gets-installed-on-the-monitored-host).

**Nothing is *changed* on the monitored host either — Tuxtop only observes.**
No kill, no renice, no `systemctl start|stop|restart`, no writes. Every remote
command is a read. This was decided deliberately, not left undone:
[ADR-010](docs/DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host).

The argument is not privilege — Tuxtop uses the user's own SSH credentials and
grants no capability a terminal does not. It is **aiming**. This UI exists to
make nineteen machines look alike at a glance; that is good for seeing and bad
for targeting, and `kill 1` on the card you thought was another host is a
mistake it would help you make. A confirmation dialog does not help when the
target was selected correctly and misidentified.

If a future phase seems to want a "safe" exception, it reopens Phase 5's
process actions, Phase 10's start/stop and group-level actions all at once.
Treat it as load-bearing.

**Never let one data plane's absence blank a card.**
No Beszel agent means no history — the live grid still works. SSH down means no
live data — history still renders, marked stale. Always state which part is
missing; never show a generic "offline".

**One SSH connection per host carries both planes.**
Metrics and the process ranking share one `ssh` and one byte stream, told apart
by two frame delimiters — never by their line tags, however distinct those look.
Both parsers skip what they do not recognise, so a merged frame looks free: it
is not, and `a_process_line_cannot_fabricate_a_disk_reading` shows a Java
command line parsing as 4.1 MB of disk I/O that never happened. The process
ranking's window is the metric loop's own sleep, not one of its own
([ADR-014](docs/DECISIONS.md#adr-014--one-connection-per-host-carries-both-planes)).
A second connection is also how the pause rule below acquired its one loophole.

**Fields after `comm` in `/proc/[pid]/stat` cannot be read positionally.**
The line is `pid (comm) state ...` and `comm` is neither quoted nor escaped, so
`spiceproxy work` or `postgres: writer` is two whitespace fields and everything
after it shifts by one. `awk '{print $24}'` reported **408 GB** of RSS for a
process using 55.8 MB — a confident, well-formatted, 7,300× wrong number, which
is this project's founding bug in a place nobody had looked. Strip through the
last `)` first (`sub(/^[0-9]+ \(.*\) /, "", s)`, greedy `.*`), after which
stat field *N* is `f[N-2]`.

**Pause is enforced in `Supervisor::start`, and nowhere else.**
`start` stops the host's task and refuses to restart a paused one. This is true
again rather than aspirationally: `start_procs` used to be a second way a host
acquired an ssh connection, with its own `cfg.paused` check to remember, and
folding the planes onto one connection deleted it. Do not add a
`cfg.paused` check to a *caller* instead — five of them restart a host as a side
effect of an unrelated edit (`start_all`, `set_settings`, `set_host_interval`,
`set_host_os`, `add_host`), and the one that forgets silently resumes a machine
somebody took down. Changing the global sample rate un-pausing the whole fleet
is the shape of that bug; `changing_the_global_interval_does_not_resume_a_paused_host`
is the test that catches it. A paused host is also **neither up nor down** — it
blanks its readings rather than freezing them, and is counted apart in the
tally, because folding it into "up" makes pausing a dying box look like a
fleet that got healthier. [ADR-012](docs/DECISIONS.md#adr-012--pause-is-a-third-host-state-and-it-lives-in-hoststoml).

**Faults keep their reason.**
`HostFault` distinguishes `Unreachable`, `AuthFailed`, `SamplerFailed` and
`Stalled`. Do not collapse them. Telling `AuthFailed` from `Unreachable` is the
difference between a thirty-second fix and an hour of guessing.

**No silently-swallowed errors.**
Every error path either logs with context, propagates, or renders as a
`HostFault` the user can see. A `unwrap_or_default()` that hides a parse failure
reproduces exactly the bug this project was built in response to. The one
sanctioned exception is per-field parsing inside `/proc` parsers, where a
malformed field degrades that field to zero rather than discarding the whole
snapshot — that is deliberate and documented at each site.

**`src-tauri` holds the window and nothing else.**
The supervisor, the history store, the samplers and the fleet loop all live in
`tuxtop-core`. What remains in `src-tauri` is a thin adapter: Tauri commands,
window chrome, and one task turning `supervisor::Event` into webview events.
Anything that would be needed by a headless server belongs in core — that is
what makes one possible, and what makes the code testable at all, since the
next rule means nothing in `src-tauri` is ever compiled here.

**`src-tauri` is not a workspace member — keep it that way.**
Tauri pulls webkit2gtk on Linux, which is absent on the WSL dev box. Adding it
to the workspace breaks `cargo test` on the machine where development happens.
[ADR-006](docs/DECISIONS.md#adr-006--tuxtop-core-is-a-separate-crate-outside-the-tauri-workspace).

**No blocking I/O in async tasks.**
One Tokio task per host. A host that hangs must degrade only its own card.

---

## A feature is not done until you can name the click path

Say, in one sentence, what a user clicks to reach the thing you built. If you
cannot, it is not finished — however green the tests are.

```sh
python3 scripts/check-commands-reachable.py
```

Three commands had shipped with no caller before anyone noticed:

- `set_host_group` — host grouping was reachable only from the Add host
  dialog, so it worked for a host that did not exist yet and for no other.
  Every host already in the fleet had no path to the feature at all.
- `history_usage` — the settings panel multiplied a hardcoded 79.9 KB by a
  series count and reported the product as fact, while the measured figure sat
  in the backend unused.
- `active_hosts` — dead outright.

Each was individually invisible: the Rust compiled and was tested, the JS ran,
and neither side can see that the other never calls it. This is the failure
mode of building a backend and a frontend in the same sitting, and it is worth
a check rather than care.

**A new field on `HostConfig` needs a control, not just a parser.** The
reachability check above covers commands and cannot cover fields: `HostFacts`
also has an `os`, so nothing automated can tell `cfg.os` from `facts.os`.
Host `os` shipped with a backend, a `hosts.toml` entry and a documented
example — and no way to set it, so a Windows host added through the UI was
created as a Linux one and failed with "the system cannot find the path
specified", an error explaining nothing. When adding a field, wire **both**
paths: the Add host dialog for new hosts, and the per-host table in Settings
for the ones that already exist. The second is the one that gets forgotten,
and it is the one that matters for a fleet already running.

Two related habits:

- **Prefer a measurement to an estimate whenever one exists.** This project was
  built in response to a confident wrong number; computing a plausible figure
  in the frontend while the real one is a command away is the same sin in
  miniature.
- **When a stub command is missing, fix the stub.** Twice now a gap in
  `tests/harness/stub.js` has presented as an application bug. The stub is test
  infrastructure — an error there costs debugging time and teaches nothing.

---

## Frontend rules

**All colours come from CSS custom properties.** No literal hex, `rgb()` or
`hsl()` in component rules. Hardcoded colours do not survive the theme switch
and typically break in only one direction, which makes them easy to miss.

**Every colour must be defined in all three theme states — and there is a checker.**

```sh
python3 scripts/check-theme-tokens.py
```

Run it after touching tokens. This bug has landed twice (`--viz-*`, then the
metal/reveal set): tokens go into the `@media` block and miss
`:root[data-theme="dark"]`, so an explicit dark toggle silently falls back to
light colours. It fails in one direction only, which is why eyeballing misses it.

The three states: bare `:root` carries the complete light palette;
`@media (prefers-color-scheme: dark)` guarded as `:root:not([data-theme="light"])`
redefines the tokens; `:root[data-theme="dark"]` redefines them again. A token
defined in only some of them renders one theme's colour on another theme's
ground.

**Any numeral drawn over a load-coloured surface needs a contrast halo.** The
background colour is unpredictable by definition. Use `--tile-halo`.
[ADR-005](docs/DECISIONS.md#adr-005--load-is-encoded-three-ways-at-once).

**Check both themes before committing a visual change.** A change verified in
dark only is unverified.

---

## Two shells, one service

| crate | what it is |
| --- | --- |
| `tuxtop-core` | everything: samplers, supervisor, history, config, and `service::Service` — the operations both shells call |
| `src-tauri` | the desktop window. Commands are one-line delegations; it owns the window and turns events into webview topics |
| `tuxtop-serve` | a headless server. Same service, reached over HTTP |

`tuxtop-serve` binds **loopback only and has no authentication**, deliberately.
Put `tailscale serve` in front of it for TLS and identity, which is how Beszel
is already reached on this fleet — a monitoring tool inventing its own session
handling acquires a login bug for no benefit.

It is **read-only unless `--writable`**. Viewing is harmless; `add_host` makes
the serving machine open an SSH connection with its own keys, which is not a
capability a browser tab should have merely by reaching the port. Controls that
a read-only server would refuse are hidden rather than left to fail, via a
`capabilities` command — a button that can only return an error looks like a
capability.

`axum` is the one heavyweight dependency, and only `tuxtop-serve` pays for it
(62 crates against core's 29). We hand-rolled base64 because the format is
forty lines and fully specified; an HTTP parser faces the network, and writing
one is how a monitoring tool acquires its first remote code execution.

## Testing

```sh
cargo test        # 269 tests, no GUI toolchain needed, runs anywhere
cargo clippy --all-targets
cargo fmt
node --test 'tests/*.test.js'           # pure logic: aggregation, scale, filters
npx playwright test                     # the browser: load order, layout, controls
python3 scripts/check-theme-tokens.py   # CSS tokens in all three theme states
python3 scripts/check-agg-declared.py   # every metric declares how it aggregates
python3 scripts/check-commands-reachable.py   # no command shipped unreachable

cargo mutants -j16       # what the tests would not have noticed (run on dove)
npm run test:mutants     # the same for the six pure JS modules
```

**Mutation testing is a diagnostic to read, never a score to raise.** It runs
weekly on dove (`.github/workflows/ci.yml`), `continue-on-error`, with **no
threshold in either tool** — deliberately. The moment a mutation score has to
go up, tests get written to kill mutants rather than to state rules, and a test
corresponding to no rule is worse than none: it breaks on refactor without
catching anything. This repo has never had a coverage percentage; the point of
adopting this was to get the signal without acquiring one. Read the survivor
list and ask one question per line — *if this shipped, would I care?* Yes means
write the test and name it after the invariant. No means the mutant is
equivalent, or the code did not need to exist.

Exclusions live in `.cargo/mutants.toml` and `stryker.conf.json`, each with the
reason it was judged not worth a test. Two things learned the hard way and
worth not relearning: cargo-mutants' `exclude_re` matches the **whole mutant
description** ("replace foo -> T with ..."), so anchoring on `^name$` silently
matches nothing and the exclusion looks applied while doing nothing; and
`--in-place` refuses `--jobs`, so reaching for it costs you all parallelism —
only do so when copying the tree is the problem, which it is when `TMPDIR` is a
tmpfs, because the copy includes a 4 GB `target/`.

**It found what hand-mutation had missed, which is the argument for it.** The
practice below — check that breaking a rule fails a test — was already written
down here, and was being done. It caught the `Supervisor::start` pause rule.
It had not caught that `Service::start_all` could be replaced with
`Ok(Default::default())` while `start_all_skips_a_paused_host_on_launch` still
passed, because `add_host` had already started the hosts and the test asserted
a state that was true before the call. A test named for launch that did not
test launch. Nor had anyone noticed that `tuxtop-serve` contained **zero
tests** while holding both security-shaped invariants in the codebase: the
directory-traversal guard in `serve`, and the `--writable` check whose `!` can
be deleted to invert read-only enforcement entirely. Both were argued for
carefully in prose. A comment does not fail.

**When a test asserts an error status, check it cannot pass for the wrong
reason.** The first traversal test asserted `NOT_FOUND` for `../secret.toml`
and killed nothing: a path the guard *lets through* also 404s when it names a
file that does not exist, so the assertion held with the guard removed. Every
rejected name in that test now points at a file that genuinely exists, which is
what makes the guard the only reason it fails.

**Tests are the memory this project does not otherwise have.** Each test name
states the invariant it protects — `iowait_counts_as_idle`,
`partitions_are_not_double_counted`, `identical_samples_report_zero_not_nan`,
`mean_of_ratios_is_not_the_group_percentage`. Write names that way: a future
session should learn the rule from the failure message alone.

**JS tests use node's built-in runner** (`node --test`), which needs no npm,
no package.json and no bundler — matching how the frontend is already built.

**Pure logic goes in a module beside `app.js`, not inside it.** `app.js` keeps
the DOM; everything that decides a *value* lives in a UMD module and is tested:

| module | decides |
| --- | --- |
| `src/format.js` | numbers → the strings on screen |
| `src/scale.js` | value → position, band, column count |
| `src/pick.js` | which of a host's many readings matters (fullest mount, hottest sensor) |
| `src/filter.js` | what to show and in what order |
| `src/agg.js` | how a group combines (ADR-008) |
| `src/heat.js` | how a span of time becomes one cell (ADR-011) |

Load order in `index.html` matters: the modules come first and `app.js` binds
them at the top of its IIFE. Adding logic to `app.js` that could be in a module
means adding logic nothing can test — the frontend went 2,792 lines with zero
coverage that way, and shipped two bugs in one session because of it.

**The harness mirrors the real fleet, and links rather than copies.**
`tests/harness/fleet.json` holds the shape — nineteen hosts, the physical and
virtual mix, the group names, the Windows host — because twice in one session a
harness built around five convenient hosts let a real bug reach the user: a
nineteen-host tally is ~70px wider, which was exactly the margin the toolbar
had. `scripts/harness.py` symlinks `src/` rather than copying it; a copy goes
stale and a suite that passes against code that no longer exists is worse than
no suite.

**A slow E2E suite is a bug in the harness, not a reason to raise a timeout.**
`scripts/harness.py --serve` uses `ThreadingTCPServer`. It was a plain
`TCPServer` - one request at a time - and the page pulls ten files while
several Playwright workers load it at once, so requests queued behind each
other and page loads crept toward the 30 s per-test timeout. That presented as
three layout tests failing at random, was written off as flaky twice, and was
neither: the worst test went 23.9 s to 8.0 s and the suite 2.2 m to 1.0 m by
making the server threaded. Before touching `retries` or a timeout, check what
the suite is actually waiting on.

**The suite runs four workers, and that number is load-bearing.**
`playwright.config.js` pins `workers: 4` rather than taking Playwright's
default of half the cores. Every page animates nineteen hosts at 2.5 Hz against
canvas, so a worker is a sustained CPU load, not a browser waiting on a server.
At the default eight this box saturates: pages stop repainting promptly,
toolbar controls never hold still for two consecutive frames, and Playwright
refuses to click an element that is not *stable*, so it waits out the full 30 s.
It presents as three to six tests failing at random across unrelated specs —
indistinguishable from having broken the app, and it cost most of an afternoon.
Eight workers gave one to six failures per run; four gives 34/34 repeatably.

This wears the face of the harness-server bug below but is not it: that server
was measured at eight parallel page loads in under 80 ms. Nor is it a timeout to
raise. Do not raise `workers` without re-measuring.

**Interact with the toolbar only after the fleet has stopped arriving.**
Cards are created as hosts report, and the tally counts up from `0 up` to
`19 up` while `ncores` fills in — so for the first second the toolbar is
resizing and the grid is being torn down and rebuilt several times a second. A
click resolved against a card that is then replaced lands on a detached node,
and because the handler is delegated on the grid the event never arrives: the
action silently does not happen. Both `layout.spec` and `pause.spec` have a
`load(page)` helper that polls until `#nup` equals `#nhosts`. Use it in any new
spec that clicks something.

One test legitimately needs more than the 30 s default —
`leaving a view restores the layout it found` drives twelve view switches and
four full heat renders, measured at 24 s alone — and carries its own
`test.setTimeout(60_000)` with that measurement written down. That is the
exception, not a pattern to copy.

**Then check your own machine before blaming the suite.** The residual failures
after that fix were self-inflicted - a `tuxtop-serve` and a desktop app running
against nineteen hosts each, 45 ssh sessions on a 16-core box. With those
stopped: four consecutive clean runs at 1.1 m. Load average is worth a glance
before a diagnosis.

**A browser test can pass without testing anything.** The core-column test ran
at a width where eight charts fit and eleven did not — so "snap to a multiple
of eight" and "as many as fit" both answered 8, and it passed against a
deliberately broken implementation. It now runs at a width where the two rules
disagree, and asserts that they could have. When a test compares two rules,
check the inputs actually distinguish them.

**When a rule matters, check that breaking it fails the test.** A test written
against already-correct code can pass for the wrong reason. Both aggregation
rules in ADR-008 were verified by mutation: turning the ratio into a mean, and
taking severity from the aggregate, each failed exactly the tests named after
those rules.

`crates/tuxtop-core/tests/real_host.rs` checks parsing against captured
`/proc/stat` from a real 32-core host and cross-checks the computed percentage
against `top` from the same second. When you change the maths, this is the test
that catches a plausible-but-wrong result.

---

## CI

`.github/workflows/ci.yml` runs on every push to `main` and every pull request:

| job | runs on | covers |
| --- | --- | --- |
| `core` | **dove**, self-hosted | `cargo fmt --check`, `clippy -D warnings`, `cargo test`, JS unit tests, the four checkers |
| `browser` | **dove**, self-hosted | the Playwright suite — layout and load order |
| `windows` | **dove**, self-hosted | **`cargo xwin build` for `x86_64-pc-windows-msvc`** |

Every job runs on our own hardware, because GitHub-hosted jobs on this account
do not start — a payment state — so CI was written and never ran. Self-hosted
minutes are not billed, and dove is 32 cores against a hosted runner's 2.
**This is only safe because the repo is private with no forks**: on a public
repo any fork PR would execute on dove. See [docs/CI.md](docs/CI.md).

The Windows job cross-compiles rather than running on n1, which is the machine
someone is actually using: a four-minute build on every push was taking cores
from the person at the keyboard. `cargo-xwin` supplies the MSVC headers and
CRT, so it is the real target triple the installer ships, not a gnu
approximation. Only the **installer** still needs n1, because the MSI needs
WiX — and that runs on a tag, not on every push.

The Windows job exists at all because a commit that does not compile has
reached `main` more than once: `src-tauri` is deliberately outside the
workspace (ADR-006), so nothing on the development box ever compiles it, and
the error only surfaced when someone built on Windows. Before CI, "someone"
was the user.

```sh
bash scripts/verify.sh          # everything CI runs, locally
bash scripts/verify.sh --quick  # without the browser suite
```

**Compiling is not running, and `verify.sh` now launches the app.** Two startup
panics shipped past a green build in one afternoon: `Handle::current()` taken
in Tauri's `setup`, which runs outside the runtime, and a `state()` lookup for
a type registered under a different one. Neither is visible to a compiler and
both are obvious a second after launch. If you change anything about
construction, state registration or the runtime, run the smoke test.

**Run `verify.sh` before pushing, and do not rely on CI being available.** It
runs the same gates, including `cargo clippy --all-targets -- -D warnings`
(stricter than a bare `cargo clippy`) — and it builds `src-tauri` through the
Windows toolchain at `/mnt/c`, which is the one gate a Linux box cannot
otherwise close. It skips loudly rather than passing quietly when something is
unavailable.

**That Windows build is a separate clone, and the script now refuses a stale
one.** `/mnt/c/Users/sam/tuxtop` is its own checkout, so it can only build what
you have committed *and pushed* and then pulled there. The script used to say
so in a note and report `ok` anyway — it was caught reporting a green Windows
build while that checkout sat several commits behind, in the one gate that
exists because commits which do not compile have reached `main`. It now
compares the two HEADs, requires both trees clean, and skips with the single
command that fixes it. **A skip there means the Windows build did not happen**,
not that it passed.

The smoke test's teardown check waits for the ssh processes to go rather than
sampling once. It sampled at 3 s, which across five runs on two commits gave
2, 11, 2, 6 and 5 still alive — a coin flip that failed at random and read as
a regression in whatever change was in flight. It also does not verify
`kill_on_drop`, whatever its comment once claimed: `taskkill /F` leaves no
chance to run a destructor, so what is really checked is that the orphans do
not outlive the app.

## Releasing

```sh
bash scripts/verify.sh                 # the gate, including the Windows build
# bump the version in all four files, commit
git tag v0.3.0 && git push --tags
```

`.github/workflows/release.yml` then builds both shells from the tagged commit
and attaches them to a GitHub Release: a Windows installer (`.msi` and an NSIS
`-setup.exe`) and a `tuxtop-serve` tarball for Linux.

**The version lives in four files** — three `Cargo.toml`s and
`tauri.conf.json` — and nothing keeps them together, so
`scripts/check-version.py` asserts they agree. It runs in CI on every push, and
again in the release guard *with the tag*, before any build minutes are spent.
A release whose binaries report a different version than the tag is worse than
no release: the tag is what anyone quotes in a bug report.

**The release re-runs the full CI gate on the tagged commit** rather than
assuming it passed. A tag can point at a commit that never went through CI —
pushed together, or moved afterwards.

**The Linux tarball ships `src/` beside the binary.** `--web` defaults to
`./src`, so the binary alone is a server that starts and then 404s every page.
The tarball is laid out so the default resolves from the extracted directory.

**Nothing is code-signed, and that is a decision rather than a gap.** Windows
SmartScreen warns on first run of the installer; the release notes say so,
because an unexplained warning is how a user learns to click through warnings.
`SHA256SUMS` is attached instead, since there is no certificate for anyone to
check against.

Reviewed 2026-09-01 and declined. Two separate warnings are at stake: UAC's
"Unknown publisher", which any signature chaining to a trusted root removes,
and SmartScreen's "Windows protected your PC", which is reputation-based and
can still appear on a freshly-signed binary. Three routes exist and none earns
its cost here:

- **Self-signed, imported to Trusted Root and Trusted Publishers** on the
  machines that run it. Free, and the installer job already runs on n1 so the
  key would never enter a CI secret. Removes both warnings — on those machines
  only, and at the price of a key that could sign anything they would then
  trust.
- **Azure Trusted Signing**, around $10/month, publicly trusted and built for
  CI. The cheapest real option, subject to identity validation and region
  rules.
- **A commercial OV or EV certificate**, roughly $200–700/year. Note that
  since the CA/Browser Forum's 2023 change, OV keys must live on FIPS hardware
  — there is no `.pfx` to drop into CI, so this needs a token or a cloud HSM.

The repo is private, the tool watches one fleet, and the install base is the
machines in it. Paying a CA to reassure yourself about software you built is
buying a certificate for an audience of one. Revisit only if this is ever
handed to someone outside the fleet — at which point Azure Trusted Signing is
the route, via Tauri's `bundle.windows.signCommand`.

**The bundler is `@tauri-apps/cli`, pinned in `package-lock.json`** — a
prebuilt binary, so the release job downloads it rather than spending four
minutes on `cargo install tauri-cli`. It builds the installer and is not part
of the app; the frontend still has no build step and no runtime dependencies.

## Commit style

- Subject ≤70 chars, imperative, no trailing period.
- Body explains *why*, wrapped at 72.
- Prefixes: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`.
- No `Co-Authored-By:` trailer.

---

## Keeping docs true

A decision that changes behaviour, an interface, or a settled approach must
update the affected doc **in the same commit**:

- [docs/DECISIONS.md](docs/DECISIONS.md) — supersede the old ADR, don't delete
  it. The rejected path is the valuable part.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — if data flow or layout moved.
- [docs/ROADMAP.md](docs/ROADMAP.md) — tick phases only when the observable
  outcome is verified, not when the code exists.

Two sources disagreeing is worse than one source missing.
