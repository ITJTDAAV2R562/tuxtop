#!/usr/bin/env bash
# Everything CI runs, locally - plus the one thing it cannot.
#
# CI covers the tests, the lints, the checkers and the scanners
# (.github/workflows/ci.yml and security.yml). What it cannot do is launch the
# built app: compiling is not running, and two startup panics shipped past a
# green build in one afternoon. This box reaches the Windows toolchain through
# /mnt/c, so it closes that gap. Where it cannot, it says so rather than
# passing quietly - a skip is not a pass.
#
#     bash scripts/verify.sh            # everything available
#     bash scripts/verify.sh --quick    # skip the browser suite
#
# The scanners are skipped rather than installed on demand: cargo-audit and
# cargo-deny are several minutes of build each and CI runs them regardless, so
# a missing one should not stop you committing.
set -uo pipefail
cd "$(dirname "$0")/.."

FAIL=0
SKIPPED=()
step() {
  printf '\n\033[1m== %s\033[0m\n' "$1"; shift
  if "$@"; then printf '   ok\n'; else printf '\033[31m   FAILED\033[0m\n'; FAIL=1; fi
}
# The reason travels into the summary, not only the inline line. A run whose
# last words are "skipped: windows build" tells you nothing about whether that
# was a missing toolchain or a stale checkout you can fix in one command.
skip() { SKIPPED+=("$1 — $2"); printf '\n\033[33m== %s — skipped: %s\033[0m\n' "$1" "$2"; }

step "cargo fmt"            cargo fmt --check
step "clippy (-D warnings)" cargo clippy --all-targets -- -D warnings
step "cargo test"           cargo test --quiet
step "JS unit tests"        node --test 'tests/*.test.js'
step "theme tokens"         python3 scripts/check-theme-tokens.py
step "aggregation rules"    python3 scripts/check-agg-declared.py
step "command reachability" python3 scripts/check-commands-reachable.py
step "version agreement"    python3 scripts/check-version.py

# The scanners. Each is a gate in .github/workflows/security.yml; here they run
# only if the tool happens to be installed, and say so plainly when it is not.
if command -v gitleaks >/dev/null; then
  # History and working tree both: hosts.toml is gitignored precisely because
  # it holds real addresses, and gitignored is not absent.
  step "secrets (history)"     gitleaks git --no-banner --redact --exit-code 1 .
  step "secrets (working tree)" gitleaks dir --no-banner --redact --exit-code 1 .
else
  skip "secret scan" "gitleaks not installed - https://github.com/gitleaks/gitleaks"
fi

# Both lockfiles. src-tauri is outside the workspace (ADR-006) and carries its
# own, which no workspace-wide command ever opens.
if command -v cargo-audit >/dev/null; then
  step "advisories (workspace)" cargo audit
  step "advisories (src-tauri)" cargo audit --file src-tauri/Cargo.lock
else
  skip "advisories" "cargo-audit not installed - cargo install cargo-audit --locked"
fi

if command -v cargo-deny >/dev/null; then
  step "licences and sources (workspace)" cargo deny check
  step "licences and sources (src-tauri)" cargo deny --manifest-path src-tauri/Cargo.toml check
else
  skip "licences and sources" "cargo-deny not installed - cargo install cargo-deny --locked"
fi

if [ "${1:-}" != "--quick" ]; then
  if [ -d node_modules/@playwright ]; then
    step "browser suite" npx playwright test
  else
    skip "browser suite" "npm ci not run"
  fi
fi

# The one CI can do and a Linux box cannot - unless the Windows checkout and
# toolchain are reachable, which on this machine they are.
#
# The Windows side is one person's machine, so nothing about it is hardcoded:
# TUXTOP_WIN_USER names the Windows account (defaulting to this shell's user,
# which is what a WSL install usually matches), and TUXTOP_WIN_REPO points at
# the separate clone if it does not sit in that account's home.
WIN_USER=${TUXTOP_WIN_USER:-$USER}
WIN_CARGO=${TUXTOP_WIN_CARGO:-/mnt/c/Users/$WIN_USER/.cargo/bin/cargo.exe}
WIN_REPO=${TUXTOP_WIN_REPO:-/mnt/c/Users/$WIN_USER/tuxtop}
WIN_SRC=$WIN_REPO/src-tauri

# Is the Windows checkout actually holding the code we just tested?
#
# It is a *separate clone*, not this working tree, so a green build there says
# nothing about the change in front of you. The script used to print a "git
# pull there first" note and then report ok regardless - which is passing
# quietly on the wrong tree, in the one gate that exists because commits that
# do not compile have reached main. It was found reporting ok while the
# checkout sat several commits behind.
#
# Sets WIN_STALE to the reason, or leaves it empty when the two agree.
WIN_STALE=""
windows_checkout_state() {
  local local_head win_head dirty win_dirty
  local_head=$(git rev-parse HEAD 2>/dev/null)
  win_head=$(git -C "$WIN_REPO" rev-parse HEAD 2>/dev/null)

  if [ -z "$win_head" ]; then
    WIN_STALE="$WIN_REPO is not a git checkout, so there is no telling what it builds"
    return
  fi
  if [ "$win_head" != "$local_head" ]; then
    WIN_STALE="the checkout is at ${win_head:0:7}, this tree is at ${local_head:0:7} — push, then: git -C $WIN_REPO pull"
    return
  fi
  # Matching HEADs are not enough. The checkout can only ever build what is
  # committed, so uncommitted work here is invisible to it - which is the
  # normal state while writing the change the gate is supposed to cover.
  dirty=$(git status --porcelain -- src-tauri crates Cargo.toml Cargo.lock 2>/dev/null | wc -l)
  if [ "$dirty" -gt 0 ]; then
    WIN_STALE="this tree has $dirty uncommitted change(s) the checkout cannot see — commit and push, then: git -C $WIN_REPO pull"
    return
  fi
  # And the checkout itself must be clean, or it is building something that
  # exists on no branch.
  win_dirty=$(git -C "$WIN_REPO" status --porcelain 2>/dev/null | wc -l)
  if [ "$win_dirty" -gt 0 ]; then
    WIN_STALE="the checkout has $win_dirty local modification(s), so it is not building ${win_head:0:7} either"
  fi
}

if [ -x "$WIN_CARGO" ] && [ -d "$WIN_SRC" ]; then
  windows_checkout_state
fi

if [ ! -x "$WIN_CARGO" ] || [ ! -d "$WIN_SRC" ]; then
  skip "windows build" "no Windows toolchain reachable — CI covers this"
  skip "windows smoke test" "same"
elif [ -n "$WIN_STALE" ]; then
  # Loudly, and both halves: the smoke test runs the binary this build
  # produces, so a stale build makes a stale smoke test.
  skip "windows build" "$WIN_STALE"
  skip "windows smoke test" "would run the stale binary"
else
  printf '\n\033[1m== windows build (src-tauri)\033[0m\n'
  # Windows will not let cargo replace a running binary, and this script has
  # to launch one to smoke-test it. Closing it first is the only way the gate
  # can run twice in a row.
  if /mnt/c/Windows/System32/tasklist.exe /FI "IMAGENAME eq tuxtop.exe" 2>/dev/null | grep -q tuxtop.exe; then
    printf '   closing the running tuxtop.exe so the binary can be replaced\n'
    /mnt/c/Windows/System32/taskkill.exe /IM tuxtop.exe /F >/dev/null 2>&1
    sleep 2
  fi
  if (cd "$WIN_SRC" && "$WIN_CARGO" build --quiet); then printf '   ok\n'
  else printf '\033[31m   FAILED\033[0m\n'; FAIL=1; fi
  # Compiling is not running. Two startup panics shipped past a green build
  # in one afternoon - a runtime handle taken where there was no runtime, and
  # a state lookup for a type registered under a different one. Neither is
  # visible to a compiler; both are obvious a second after launch.
  #
  # This opens the window briefly and closes it.
  printf '\n\033[1m== windows smoke test (does it survive startup?)\033[0m\n'
  EXE="$WIN_SRC/target/debug/tuxtop.exe"
  LOG=$(mktemp)
  sshcount() { /mnt/c/Windows/System32/tasklist.exe /FI "IMAGENAME eq ssh.exe" 2>/dev/null | grep -c ssh.exe; }
  if [ -x "$EXE" ]; then
    BEFORE=$(sshcount)
    ( "$EXE" >"$LOG" 2>&1 & )
    sleep 10
    if ! /mnt/c/Windows/System32/tasklist.exe /FI "IMAGENAME eq tuxtop.exe" 2>/dev/null | grep -q tuxtop.exe; then
      printf '\033[31m   FAILED — exited during startup:\033[0m\n'
      sed -n '1,6p' "$LOG" | sed 's/^/     /'
      FAIL=1
    else
      # Alive is not the same as working. Each watched host holds one ssh
      # process, so their appearance is proof the samplers really started -
      # a refactor can leave the window up and the fleet dark.
      DURING=$(sshcount)
      if [ "$DURING" -gt "$BEFORE" ]; then
        printf '   ok — %s ssh sessions opened (was %s)\n' "$((DURING - BEFORE))" "$BEFORE"
      else
        printf '\033[31m   FAILED — running, but no host was sampled\033[0m\n'
        FAIL=1
      fi
    fi
    /mnt/c/Windows/System32/taskkill.exe /IM tuxtop.exe /F >/dev/null 2>&1
    # And they must go when it does - but give them time to, and say how long
    # it took rather than sampling once at an arbitrary moment.
    #
    # This does *not* verify kill_on_drop, whatever the comment here used to
    # claim. `taskkill /F` gives the process no chance to run a destructor, so
    # kill_on_drop cannot fire; the orphaned ssh processes exit on their own
    # when their pipes close. What is actually being checked is that they do
    # not survive the app indefinitely - a weaker claim, and the true one.
    #
    # It was a single sample after 3 seconds, which is a coin flip: measured
    # five times across two commits, "still alive after 3s" came out 2, 11, 2,
    # 6 and 5 with nineteen hosts. At 10s it was 1 every time. So the gate
    # failed at random, which is exactly as useless as passing on stale code
    # - and reads as a regression in whatever change happens to be in flight.
    SETTLE=0
    while [ "$SETTLE" -lt 20 ]; do
      AFTER=$(sshcount)
      [ "$AFTER" -le "$((BEFORE + 2))" ] && break
      sleep 2; SETTLE=$((SETTLE + 2))
    done
    if [ "$AFTER" -gt "$((BEFORE + 2))" ]; then
      printf '\033[31m   FAILED — %s ssh sessions still alive %ss after exit\033[0m\n' "$AFTER" "$SETTLE"
      FAIL=1
    else
      printf '   ok — sessions cleaned up within %ss\n' "$SETTLE"
    fi
  else
    printf '   no binary to run\n'
  fi
  rm -f "$LOG"
fi

printf '\n'
for s in "${SKIPPED[@]:-}"; do [ -n "$s" ] && printf '\033[33mskipped: %s\033[0m\n' "$s"; done
if [ "$FAIL" -eq 0 ]; then printf '\033[32mall green\033[0m\n'; else printf '\033[31msomething failed\033[0m\n'; fi
exit "$FAIL"
