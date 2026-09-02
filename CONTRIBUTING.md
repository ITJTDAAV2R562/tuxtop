# Contributing

Tuxtop is public so that it can update itself and so the code is readable, not
because it is looking for maintainers. It watches one small fleet, and design
decisions are made for that fleet. Issues and pull requests are welcome; a pull
request that changes behaviour may still be declined on grounds of scope, and
that is not a reflection on the work.

Read [CLAUDE.md](CLAUDE.md) first. It is the standing rules — what the tests are
for, which invariants are load-bearing, and the failure mode the whole project
is designed against — and it is more useful than this file.

## The gate

```sh
bash scripts/verify.sh          # everything CI runs, locally
bash scripts/verify.sh --quick  # without the browser suite
```

It skips loudly rather than passing quietly when something is unavailable, so
read the summary. A skip is not a pass.

CI runs the same checks plus the scanners in
[`.github/workflows/security.yml`](.github/workflows/security.yml).

## Things that will come up in review

- **A test name states an invariant.** `iowait_counts_as_idle`,
  `mean_of_ratios_is_not_the_group_percentage`. A future reader should learn the
  rule from the failure message alone. Tests are the memory this project does
  not otherwise have.
- **Logic that decides a *value* goes in a module beside `app.js`,** not inside
  it — `format.js`, `scale.js`, `pick.js`, `filter.js`, `agg.js`, `heat.js`.
  `app.js` keeps the DOM. Logic added to `app.js` is logic nothing can test.
- **No literal colours.** Every colour is a CSS custom property defined in all
  three theme states; `python3 scripts/check-theme-tokens.py` checks it, because
  this bug fails in one direction only and eyeballing misses it.
- **Nothing is installed on, or changed on, a monitored host.** Every remote
  command is a read. This is
  [ADR-010](docs/DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host)
  and it is load-bearing, not an unfinished feature.
- **A feature is not done until you can name the click path.** Three commands
  once shipped with no caller. `python3 scripts/check-commands-reachable.py`.
- **Real host names, addresses and tailnet names do not belong in the tree.**
  The fixtures and `tests/harness/fleet.json` use invented names on purpose;
  `hosts.toml` is gitignored. The gitleaks job scans the working tree as well as
  history, but it cannot recognise your own infrastructure — that part is on us.

## Commit style

- Subject ≤70 chars, imperative, no trailing period.
- Body explains *why*, wrapped at 72.
- Prefixes: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`.
- No `Co-Authored-By:` trailer.

## Docs

A change to behaviour, an interface or a settled approach updates the affected
doc in the same commit — [`docs/DECISIONS.md`](docs/DECISIONS.md) (supersede an
ADR, never delete it: the rejected path is the valuable part),
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md),
[`docs/ROADMAP.md`](docs/ROADMAP.md). Two sources disagreeing is worse than one
source missing.
