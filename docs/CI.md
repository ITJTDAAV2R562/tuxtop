# CI

`.github/workflows/ci.yml` runs on every push to `main`, every pull request,
and as a reusable workflow called by `release.yml`.

| job | runs on | covers |
| --- | --- | --- |
| `core` | **dove** (self-hosted) | `cargo fmt --check`, `clippy -D warnings`, `cargo test`, JS unit tests, the four checkers |
| `browser` | **dove** (self-hosted) | the Playwright suite - layout and load order |
| `windows` | **n1**, self-hosted | `cargo build` in `src-tauri` |

## Why self-hosted

GitHub-hosted jobs on this account do not start: *"recent account payments have
failed or your spending limit needs to be increased."* CI was written and then
never ran once.

**Self-hosted runner minutes are not billed**, so moving the Linux jobs to our
own hardware sidesteps the payment state entirely. It is also faster: dove is
32 cores against a hosted runner's 2, and the two Linux jobs finish in about
fifty seconds each.

The `windows` job runs on **n1**, which is the Windows desktop this project is
developed on - `hostname` returns `n1` from both Windows and its WSL, and it is
the only Windows machine in the fleet. It is also the machine that already
builds the installer by hand, so CI and the manual path use the same toolchain.
`scripts/verify.sh` still closes the same gate locally through `/mnt/c`, which
is what to use when the runner is stopped.

### The safety question

The standard warning about self-hosted runners is that **anyone who can open a
pull request can run arbitrary code on your machine**, because workflows run
untrusted contributor code. That is a public-repo problem. This repo is
private, has zero forks and one owner, so no untrusted party can trigger a
workflow. If the repo is ever made public, remove the self-hosted runner first
- not afterwards.

The runner executes as `sam` on dove, which has passwordless sudo. A workflow
can therefore do anything on that host. That is acceptable only under the
condition above.

## The runner on dove

Installed at `~/actions-runner`, registered to this repo with labels
`self-hosted, linux, x64, dove`, running as a systemd service:

```sh
ssh dove 'systemctl status actions.runner.UZ1sFED3yS-tuxtop.dove.service'
ssh dove 'cd ~/actions-runner && sudo ./svc.sh stop'     # pause CI
ssh dove 'cd ~/actions-runner && sudo ./svc.sh start'
```

Toolchains are **not** preinstalled. `dtolnay/rust-toolchain` and
`actions/setup-node` install them on first run into `~/.rustup`, `~/.cargo` and
the runner tool cache, all of which persist between runs.

`Swatinem/rust-cache` is deliberately **absent** from the self-hosted jobs: the
workspace and `~/.cargo` already persist, so `target/` is warm and restoring a
cache over it would be slower rather than faster.

Playwright installs Chromium with `--with-deps`, which needs the passwordless
sudo noted above.

### Re-registering

Registration tokens expire in an hour; mint a fresh one when needed:

```sh
TOKEN=$(gh api -X POST repos/UZ1sFED3yS/tuxtop/actions/runners/registration-token --jq .token)
ssh dove "cd ~/actions-runner && ./config.sh --unattended \
  --url https://github.com/UZ1sFED3yS/tuxtop --token '$TOKEN' \
  --name dove --labels self-hosted,linux,x64,dove --work _work --replace"
```

### Removing it

```sh
ssh dove 'cd ~/actions-runner && sudo ./svc.sh stop && sudo ./svc.sh uninstall'
TOKEN=$(gh api -X POST repos/UZ1sFED3yS/tuxtop/actions/runners/remove-token --jq .token)
ssh dove "cd ~/actions-runner && ./config.sh remove --token '$TOKEN'"
ssh dove 'rm -rf ~/actions-runner'
```

## The runner on n1

Installed at `C:\actions-runner`, labels `self-hosted, windows, x64, n1`.
`rustup` is already on the user PATH there, so the job installs no toolchain
and `src-tauri/target` stays warm between runs.

**It is not yet a Windows service.** Installing one needs Administrator, which
this session does not have, so it currently runs from `run.cmd` and will not
survive a reboot or logout. To make it permanent, from an **elevated**
PowerShell:

```powershell
cd C:\actions-runner
.\svc.cmd install
.\svc.cmd start
```

Until then, restart it after a reboot with:

```powershell
Start-Process -FilePath C:\actions-runner\run.cmd -WorkingDirectory C:\actions-runner -WindowStyle Hidden
```

## A note on the fleet

dove is one of the nineteen hosts Tuxtop watches, so a CI run shows up as a
real spike on its card and in the Heat view. That is not a conflict with
[ADR-004](DECISIONS.md#adr-004--nothing-gets-installed-on-the-monitored-host):
that decision constrains what *Tuxtop* installs in order to monitor a host, not
what its owner chooses to run there. Tuxtop still reads dove over plain SSH and
has no idea a runner exists.

## Custom runner labels and actionlint

`.github/actionlint.yaml` declares `dove` as a known self-hosted label.
Without it actionlint flags every `runs-on: [self-hosted, ..., dove]` as an
unknown label, and a linter that is always noisy is one nobody reads.
