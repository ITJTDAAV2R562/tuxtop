# CI

Three workflows, all on GitHub-hosted runners.

| workflow | when | jobs |
| --- | --- | --- |
| [`ci.yml`](../.github/workflows/ci.yml) | push to `main`, every PR, called by `release.yml` | `core`, `browser`, `windows`, and `mutants` weekly |
| [`security.yml`](../.github/workflows/security.yml) | push to `main`, every PR, weekly | `secrets`, `deps`, `codeql`, `workflows` |
| [`release.yml`](../.github/workflows/release.yml) | a `v*` tag, or manual | `guard`, `gate`, `windows`, `linux`, `publish` |

`ci.yml`:

| job | runs on | covers |
| --- | --- | --- |
| `core` | `ubuntu-latest` | `cargo fmt --check`, `clippy -D warnings`, `cargo test`, JS unit tests, the four checkers |
| `browser` | `ubuntu-latest` | the Playwright suite — layout and load order |
| `windows` | `windows-latest` | `cargo build --locked` in `src-tauri` |
| `mutants` | `ubuntu-latest`, weekly | cargo-mutants and Stryker, `continue-on-error`, no threshold |

A manual run of `release.yml` builds both artefacts and publishes nothing —
`publish` is gated on the ref being a tag. That is the dry run.

## Why the runners are GitHub-hosted

They were not always. CI ran on two of our own machines, and this document used
to explain at length why that was fine. It was fine under exactly one
condition, stated here at the time:

> This repo is private, has zero forks and one owner, so no untrusted party can
> trigger a workflow. If the repo is ever made public, remove the self-hosted
> runner first — not afterwards.

That condition has now fired. A self-hosted runner executes whatever a pull
request contains; on a public repo, anyone who can open a PR from a fork gets
code execution on the runner host — which in our case ran as a user with
passwordless sudo. So the runners came out before the repo went in.

The reason for self-hosting in the first place was that GitHub-hosted jobs on
this account did not start: *"recent account payments have failed or your
spending limit needs to be increased."* CI was written and never ran once.
**Public repositories get GitHub-hosted minutes at no charge**, so going public
removed the reason and the option in the same move.

What it costs: the runners are 2 cores rather than 32, and `target/` starts
cold. `Swatinem/rust-cache` is now present in `ci.yml`, having been
deliberately absent before for the opposite reason — a self-hosted workspace
was already warm and restoring a cache over it was slower.

It is **absent from `release.yml`**, and that is not an oversight. An Actions
cache is writable from a pull-request run and would be readable by the release
build: a path from "anyone can open a PR" to "bytes of unknown origin inside a
published installer". A release happens on a tag a few times a year, so a cold
build is cheap and provenance is worth more than speed.

The `windows` job now builds natively on `windows-latest` instead of
cross-compiling with `cargo-xwin`. The cross-compile existed to keep a
four-minute build off the machine somebody was sitting at; a hosted runner has
no such owner, and the target triple is `x86_64-pc-windows-msvc` either way.

## Supply chain

Every third-party action is pinned to a **commit SHA**, with the version in a
trailing comment:

```yaml
- uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
```

A tag is mutable. Whoever controls the action's repository can repoint `v4` at
new code, and that code then runs inside our job with our token. A SHA cannot
be repointed. `.github/dependabot.yml` opens a weekly PR to move the pins and
rewrite the comment, which is the only maintainable way to keep them current.

Tools downloaded inside a job — gitleaks, actionlint — are pinned to a release
version **and verified against a SHA-256 checksum in the workflow**. Same trust
decision as a SHA-pinned action, made explicit: this exact artefact or nothing.

Both workflows declare `permissions: contents: read` at the top level; only
`release.yml`'s `publish` job asks for `contents: write`. Without an explicit
block a workflow inherits the repository default, which somebody can widen in
settings without ever touching a file in this tree. Every `actions/checkout`
sets `persist-credentials: false`, so the token does not survive in the local
git config for a later step to use.

Values from `github.*` and `needs.*` reach a `run:` block through `env:`, never
by interpolation. GitHub expands `${{ }}` before the shell parses the line, so
an interpolated tag name is code rather than an argument — and a tag is the one
input to `release.yml` that a push chooses.

## The scanners

`security.yml` is separate from `ci.yml` because it answers a different
question: `ci.yml` asks whether the code works, this asks whether publishing it
is safe. A failure blocks a merge exactly as a failing test does. See
[SECURITY.md](../SECURITY.md) for what each one covers and what is deliberately
out of scope.

Two details worth knowing:

- **gitleaks runs with `fetch-depth: 0`** — the whole history, not the tip. A
  secret that was force-pushed away from is still published; scanning only the
  tip would report the tree clean while the value sat one `git log -p` away.
  It runs `--redact`, because printing a finding into a public build log
  publishes the secret a second time.
- **CodeQL needs code scanning enabled on the repository**, which is free on a
  public repo and unavailable on a private one without GitHub Advanced
  Security. Until it is on, `init` and the scan itself succeed and only the
  upload in `analyze` fails, with *"Code scanning is not enabled for this
  repository"* — so a red `codeql` job on a fresh fork means a settings switch,
  not a finding. If GitHub's **default setup** for code scanning is enabled it
  conflicts with this workflow; pick one, not both.
- **`cargo audit` runs against both lockfiles.** `src-tauri` is outside the
  workspace ([ADR-006](DECISIONS.md#adr-006--tuxtop-core-is-a-separate-crate-outside-the-tauri-workspace))
  and carries its own lock, which no workspace-wide command ever opens — and it
  is the one with 435 dependencies against the workspace's 68. Vulnerabilities
  fail the job; `unmaintained` and `unsound` are reported and do not. Seventeen
  of those are GTK crates Tauri pulls for a Linux desktop build this project
  never makes; a gate nobody can clear is a gate somebody switches off.

## The mutation job

`mutants` runs **weekly** (Sunday 03:00 UTC) and on manual dispatch. Every
other job in `ci.yml` skips on a schedule run, and this one skips on a push. It
is `continue-on-error` and neither tool is given a threshold: it uploads a
report to read and cannot fail anything. The reasoning is at the top of
`.cargo/mutants.toml` — a mutation score that must go up buys tests written to
kill mutants instead of tests written to state rules.

It cost about twelve minutes of thirty-two cores (887 mutants, 250 minutes of
CPU) and costs hours on two, since every mutant is a rebuild. That is
affordable weekly and would not be on a push, which is why the schedule was
already the trigger before the runners changed.

## The local gate

```sh
bash scripts/verify.sh          # everything CI runs, locally
bash scripts/verify.sh --quick  # without the browser suite
```

It also does the one thing CI cannot: launch the built app and check it
survives startup. Compiling is not running — two startup panics shipped past a
green build in one afternoon, and both were obvious a second after launch.

That part needs a Windows toolchain reachable through `/mnt/c`, and it builds a
**separate clone**, not the WSL working tree — so it only says something about
your change once that change is committed, pushed and pulled there. It used to
print a "git pull there first" note and report **ok** regardless, and was
caught doing exactly that while the checkout sat several commits behind. It now
compares the two HEADs, checks both trees are clean, and skips loudly with the
one command that fixes it. **A skip there means the Windows build did not
happen**, not that it passed.

Paths are not hardcoded: `TUXTOP_WIN_USER` names the Windows account (defaults
to `$USER`), `TUXTOP_WIN_REPO` points at the clone, `TUXTOP_WIN_CARGO` at
`cargo.exe`.

## One lesson kept from the self-hosted era

`shell: bash` is not always Git Bash. On a GitHub-hosted Windows runner it is,
and it understands the Windows temp path the runner passes. On a Windows
machine with WSL installed it can resolve to `C:\WINDOWS\system32\bash.EXE`
instead, which eats the backslashes and reports *"No such file or directory"*
for a script that is plainly there. That cost a release once. Windows steps
here use PowerShell.
