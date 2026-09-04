# Security

## Reporting a vulnerability

Use GitHub's private vulnerability reporting — the **Security** tab, *Report a
vulnerability*. It opens a private thread; please do not open a public issue
for anything exploitable.

This is a small tool maintained by one person. Expect an acknowledgement within
a week rather than within a day, and no bounty.

## What is worth reporting

Tuxtop is a client. It holds no credentials, opens no listening port on a
monitored host and installs nothing there
([ADR-004](docs/DECISIONS.md#adr-004--nothing-gets-installed-on-the-monitored-host),
[ADR-010](docs/DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host)).
That removes most of the usual surface, and leaves these:

- **`tuxtop-serve`.** It parses HTTP from whatever reaches its port and serves
  files from `--web`. A path that escapes the web root, or a request that gets
  a mutating command executed on a server started without `--writable`, is a
  real finding. Both are pinned by tests (`no_path_escapes_the_web_root`,
  `a_read_only_server_refuses_every_mutating_command`) precisely because prose
  arguing they are safe does not fail when they stop being.
- **The remote command strings.** Everything Tuxtop runs on a watched host is
  built in `crates/tuxtop-core/src/sampler.rs` and `windows.rs`. A host name,
  user or address that escapes its quoting and becomes part of the command is a
  finding — that input comes from `hosts.toml`, but a shared or generated one
  is a plausible thing to have.
- **Anything parsed from a monitored host.** `/proc` output, process command
  lines, `nvidia-smi`. A hostile or compromised host answers with whatever it
  likes, and that text ends up in the parsers and then in the DOM.
- **The desktop shell.** Tauri command handlers in `src-tauri`, and the CSP in
  `tauri.conf.json`.

## What is out of scope, by design

- **`tuxtop-serve` binds loopback and has no authentication.** This is
  documented, deliberate and not a vulnerability report. Put something in front
  of it that terminates TLS and establishes identity — an `ssh -L` tunnel needs
  nothing installed, and nginx or Caddy, a VPN, `tailscale serve` and Cloudflare
  Access all do it too. A monitoring tool that invents its own session handling
  acquires a login bug for no benefit. Binding it to a public interface is a
  deployment choice this project advises against.
- **The installers are unsigned.** SmartScreen warns on first run and the
  release notes say so. `SHA256SUMS` is attached in place of a certificate. The
  reasoning, including the three signing routes that were priced and declined,
  is in `CLAUDE.md` under *Releasing*.
- **Advisories against unmaintained GTK crates in `src-tauri/Cargo.lock`.**
  Tauri pulls them for a Linux desktop build this project never makes. They are
  reported by `cargo audit` in CI and deliberately do not fail it.

## What runs on every change

`.github/workflows/security.yml`, on every push and pull request, and weekly:

| check | catches |
| --- | --- |
| `gitleaks`, over the full history and the working tree | a committed credential, including one already force-pushed away from |
| `cargo audit` on both lockfiles | a known-vulnerable crate |
| `cargo deny` | a non-permissive licence, a git dependency, a crate from an unexpected registry |
| `npm audit` | a vulnerable build-tool dependency |
| CodeQL | exploitable patterns in the frontend |
| `actionlint` + `zizmor` | a misconfigured or exploitable workflow |

A failure blocks the merge exactly as a failing test does.

**`npm audit` runs at `--audit-level=high`.** Everything in `package.json` is a
devDependency — test and release tooling that never ships inside the product,
which has no runtime dependencies at all — so a moderate advisory in a build
tool does not block a merge. It is still fixed when a fix is cheap. Three DoS
advisories against `qs` reached us that way: `typed-rest-client` pins it to an
exact version and the mutation tester pins `typed-rest-client`, so no upgrade
anywhere in the chain moves it and `overrides` in `package.json` is the only
lever. That is why the override is there, and why it should go when the pin
upstream loosens. The argument for spending anything on a moderate finding in a
tool that runs weekly on one machine is not the risk — it is that a Dependabot
alert nobody can close teaches everyone to stop reading them.

## If a secret is ever committed

Rotate it first, then remove it. The gitleaks job scans full history with
`fetch-depth: 0` for exactly this reason: rewriting the commit away does not
un-publish it, and a scan of the tip alone would report the tree as clean while
the value sat one `git log -p` away.
