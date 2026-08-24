#!/usr/bin/env bash
# Everything CI runs, locally.
#
# CI exists (.github/workflows/ci.yml) but depends on a GitHub account being in
# good standing, and the gate that matters most is the Windows build - src-tauri
# is outside the workspace (ADR-006) so nothing on a Linux box compiles it, and
# a commit that did not build has reached main more than once.
#
# This box can reach the Windows toolchain through /mnt/c, so it can close that
# gap without CI. Where it cannot, it says so rather than passing quietly.
#
#     bash scripts/verify.sh            # everything available
#     bash scripts/verify.sh --quick    # skip the browser suite
set -uo pipefail
cd "$(dirname "$0")/.."

FAIL=0
SKIPPED=()
step() {
  printf '\n\033[1m== %s\033[0m\n' "$1"; shift
  if "$@"; then printf '   ok\n'; else printf '\033[31m   FAILED\033[0m\n'; FAIL=1; fi
}
skip() { SKIPPED+=("$1"); printf '\n\033[33m== %s — skipped: %s\033[0m\n' "$1" "$2"; }

step "cargo fmt"            cargo fmt --check
step "clippy (-D warnings)" cargo clippy --all-targets -- -D warnings
step "cargo test"           cargo test --quiet
step "JS unit tests"        node --test 'tests/*.test.js'
step "theme tokens"         python3 scripts/check-theme-tokens.py
step "aggregation rules"    python3 scripts/check-agg-declared.py
step "command reachability" python3 scripts/check-commands-reachable.py
step "version agreement"    python3 scripts/check-version.py

if [ "${1:-}" != "--quick" ]; then
  if [ -d node_modules/@playwright ]; then
    step "browser suite" npx playwright test
  else
    skip "browser suite" "npm ci not run"
  fi
fi

# The one CI can do and a Linux box cannot - unless the Windows checkout and
# toolchain are reachable, which on this machine they are.
WIN_CARGO=/mnt/c/Users/sam/.cargo/bin/cargo.exe
WIN_SRC=/mnt/c/Users/sam/tuxtop/src-tauri
if [ -x "$WIN_CARGO" ] && [ -d "$WIN_SRC" ]; then
  printf '\n\033[1m== windows build (src-tauri)\033[0m\n'
  printf '   note: builds the Windows checkout, so `git pull` there first\n'
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
    sleep 3
    # And they must go when it does: the samplers are kill_on_drop, and a leak
    # here would leave an ssh process per host behind on every run.
    AFTER=$(sshcount)
    if [ "$AFTER" -gt "$((BEFORE + 2))" ]; then
      printf '\033[31m   FAILED — %s ssh sessions left behind after exit\033[0m\n' "$AFTER"
      FAIL=1
    fi
  else
    printf '   no binary to run\n'
  fi
  rm -f "$LOG"
else
  skip "windows build" "no Windows toolchain reachable — CI covers this"
  skip "windows smoke test" "same"
fi

printf '\n'
for s in "${SKIPPED[@]:-}"; do [ -n "$s" ] && printf '\033[33mskipped: %s\033[0m\n' "$s"; done
if [ "$FAIL" -eq 0 ]; then printf '\033[32mall green\033[0m\n'; else printf '\033[31msomething failed\033[0m\n'; fi
exit "$FAIL"
