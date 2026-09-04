# Access — a dedicated user, its key, and Windows

The rule this all serves is in the [README](../README.md#ssh-access): **if
`ssh <name>` works from your terminal, Tuxtop works**, because it is literally
the same client ([ADR-007](DECISIONS.md#adr-007--shell-out-to-the-system-ssh-dont-link-an-ssh-library)).
This page is the part that does not fit there: how to give it an account of its
own rather than yours.

---

## Tuxtop needs no privilege, and that changes the advice

The usual least-privilege recipe starts by working out what to drop. Here
there is nothing to drop — every read Tuxtop makes is world-readable, and a
brand-new account with no groups and no sudo can make all of them. Measured
2026-09-04 by running the exact read set as an ordinary user:

| read | needs |
| --- | --- |
| `/proc/stat`, `meminfo`, `diskstats`, `net/dev`, `loadavg`, `uptime` | nothing |
| `/proc/<pid>/stat`, `comm`, `cmdline`, `status`, `cgroup` — **including other users' processes** | nothing |
| `/sys/class/hwmon/*/temp*_input` | nothing |
| `/sys/fs/cgroup/system.slice/*/` | nothing |
| `df -P -k`, `nproc`, `getconf`, `uname`, `/etc/os-release` | nothing |
| `systemctl show --property=Id --property=NRestarts` | nothing — reading unit properties is not a privileged operation |
| `nvidia-smi` | nothing, where the driver is installed |

So **a dedicated user is not a privilege reduction.** It buys three other
things, and it is worth being clear that these are the actual reasons:

- **A credential you can revoke without touching a person's account.** Rotating
  your own key because a laptop was replaced should not take the fleet view
  down, and taking the fleet view down should not lock you out of your hosts.
- **A key you can restrict.** Your login key has to be able to do everything
  you do. This one does not — see `restrict` below.
- **A name in the auth log that says what it was.** `Accepted publickey for
  tuxtop` answers at a glance a question that `Accepted publickey for sam`
  makes you go and reconstruct.

None of it is required. A fleet you already reach with your own key works
today; this is the tidier arrangement, not a prerequisite.

---

## The user

```sh
sudo useradd --system --create-home --shell /bin/sh tuxtop
sudo passwd -l tuxtop          # no password; the key is the only way in
```

No groups, no sudo, no `sudoers` entry. Nothing in the table above wants one.

**It needs a real shell.** `sshd` runs the sampler through the account's login
shell, so `/usr/sbin/nologin` or `/bin/false` breaks sampling — and breaks it
with `This account is currently not available`, which arrives as a
`SamplerFailed` whose text explains nothing about what you did. `/bin/sh` is
the right answer: the loop is POSIX `sh` on purpose and never wanted bash.

---

## The key

Give it its own keypair rather than reusing one. On the workstation running
Tuxtop — the same command in PowerShell or in a shell:

```sh
ssh-keygen -t ed25519 -f ~/.ssh/tuxtop -C "tuxtop@workstation"
```

On Windows that path is `%USERPROFILE%\.ssh\tuxtop`.

**Leave the passphrase empty, or load the key into an agent.** Tuxtop runs with
`BatchMode=yes` and cannot prompt for anything. An encrypted key with no agent
holding it fails as `AuthFailed` every time, on every host, with a message
about permission that is technically true and completely misleading.

Install it:

```sh
ssh-copy-id -i ~/.ssh/tuxtop.pub tuxtop@dove
```

`ssh-copy-id` does not ship with Windows' OpenSSH client; there, append the
contents of `tuxtop.pub` to the remote `~/.ssh/authorized_keys` by hand.

---

## Restricting the key

The account cannot do much, but the *key* can be narrowed further. In the
remote `~/.ssh/authorized_keys`, prefix the line:

```
restrict,from="10.0.0.0/24" ssh-ed25519 AAAAC3Nza... tuxtop@workstation
```

- **`restrict`** turns off port forwarding, agent forwarding, X11 forwarding,
  PTY allocation and `~/.ssh/rc`. All five are safe to lose: Tuxtop passes
  `-T` and asks for none of them. If a future OpenSSH adds another capability,
  `restrict` denies that too by default, which is the point of spelling it this
  way rather than listing `no-port-forwarding,no-agent-forwarding,…`.
- **`from=`** limits which addresses may use the key at all. Worth it — this
  key sits unencrypted on a workstation, which is the trade you accepted above.

### A forced `command=` does not work here

The obvious next step is to pin the key to one command, and it is worth knowing
why it is a dead end rather than discovering it later. The sampler script is
**generated per host from its interval**: the `sleep`, the divisors that pace
`df` and `nvidia-smi`, and the process-ranking schedule are all computed from
`interval_ms` and baked into the text. A forced command therefore pins the
sample rate too. Changing a host's interval in Settings would appear to work
and change nothing, because the far side would keep running the pinned script —
a silent disagreement between the UI and the machine, which is the failure mode
this project exists to avoid.

If you want it anyway, it is defensible on a host whose rate you will never
change: capture the exact string the app sends and pin that, knowing you must
edit `authorized_keys` to change the rate.

---

## Keeping it out of the way of your own SSH config

Put Tuxtop's hosts in their own file rather than editing the config you use by
hand:

```sh
mkdir -p ~/.ssh/config.d
```

`~/.ssh/config.d/tuxtop.conf`:

```
Host dove heron wader
  User tuxtop
  IdentityFile ~/.ssh/tuxtop
  IdentitiesOnly yes
```

And in `~/.ssh/config`:

```
Include ~/.ssh/config.d/tuxtop.conf
```

Two details that bite:

- **The `Include` must come before your own `Host` blocks.** `ssh_config` takes
  the *first* value obtained for each keyword, not the last, so an `Include` at
  the bottom loses every setting you have already matched above it. This is the
  opposite of how most config files behave and it fails quietly — you get your
  own key, and an `AuthFailed` you will blame on the server.
- **`IdentitiesOnly yes` is not optional in practice.** Without it the agent
  offers every key it holds before the one you named. A host with the default
  `MaxAuthTries 6` disconnects after six, and OpenSSH says *Too many
  authentication failures* — which Tuxtop reports as `AuthFailed`, accurately
  and unhelpfully, because the key it needed was in the list and never got
  tried.

Then `addr = "dove"` in `hosts.toml` picks all of this up, and the Add host
dialog does too.

---

## When the process list comes back nearly empty

If a host is mounted with `hidepid=1` or `hidepid=2` on `/proc`, an
unprivileged account sees **only its own processes** — so the metric grid is
perfectly healthy while the process view shows almost nothing. Nothing errors,
because nothing failed: the kernel handed over a short list.

Check with `mount | grep /proc`. The fix that keeps the restriction is
`hidepid=2,gid=<group>` and putting `tuxtop` in that group; the alternative is
accepting a process view that covers one account. Either way, know which one
you chose — a quiet short list is exactly the kind of plausible wrong answer
this project is built to distrust.

---

## Windows hosts

Windows ships OpenSSH Server as a first-party optional feature and PowerShell
in the box, so this stays inside
[ADR-004](DECISIONS.md#adr-004--nothing-gets-installed-on-the-monitored-host).
In an **elevated** PowerShell on the host, once:

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
Start-Service sshd
Set-Service -Name sshd -StartupType Automatic
New-NetFirewallRule -Name sshd -DisplayName 'OpenSSH Server (sshd)' `
  -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
```

Then set `os = "windows"` on the host — in the Add host dialog, or the per-host
table in Settings. It is asked for rather than probed: guessing wrong produces
`the system cannot find the path specified`, an error that explains nothing.

### The trap: an administrator's key does not live in `~/.ssh`

**This is the one that costs an afternoon.** Windows' `sshd_config` ships with
this block active:

```
Match Group administrators
       AuthorizedKeysFile __PROGRAMDATA__/ssh/administrators_authorized_keys
```

So for any account in the local **Administrators** group, `~/.ssh/authorized_keys`
is *ignored* — silently, with a plain `Permission denied (publickey)` — and the
key must go in `C:\ProgramData\ssh\administrators_authorized_keys`, a shared
file whose ACLs must grant only `SYSTEM` and `Administrators` or sshd refuses
it without saying why. Verified on a Windows 11 host in this fleet, 2026-09-04:
the block is present and uncommented in the stock configuration.

**A dedicated non-administrator user removes the problem rather than working
around it.** Its `~/.ssh/authorized_keys` is read normally, its key is not in a
file shared with every other admin on the box, and there are no ACLs to get
right:

```powershell
New-LocalUser -Name tuxtop -NoPassword -Description "Tuxtop monitoring"
# deliberately no group beyond the default Users
```

On Windows this is the *practical* argument as well as the tidy one — which is
the reverse of Linux, where the dedicated user is purely hygiene.

### It needs no privilege there either

Same measurement, run 2026-09-04 on a Windows 11 host from a **non-elevated**
token whose only relevant memberships were `Users` and `Authenticated Users`
(`BUILTIN\Administrators` present but marked *deny only*, so it granted
nothing):

| class | rows | result |
| --- | --- | --- |
| `Win32_OperatingSystem`, `Win32_Processor`, `Win32_ComputerSystem` | 1 each | ok |
| `Win32_PerfRawData_PerfOS_Processor` | 17 | ok |
| `Win32_PerfRawData_Tcpip_NetworkInterface` | 3 | ok |
| `Win32_PerfRawData_PerfDisk_PhysicalDisk` | 3 | ok |
| `Win32_PerfRawData_PerfProc_Process` | 431 | ok |
| `Win32_Service`, `Win32_Process` | 334 / 430 | ok |

No `Performance Monitor Users` membership, no elevation, no WMI namespace
change. (17 processor rows is 16 cores plus the `_Total` row Windows computes
itself, which the parser keeps apart from the real cores.)

### The shell does not matter

The default shell for Windows OpenSSH is `cmd.exe` unless `DefaultShell` is set
under `HKLM:\SOFTWARE\OpenSSH`. Tuxtop does not care and you need not change
it: the script is sent as `powershell -EncodedCommand`, UTF-16LE then base64,
so what crosses the wire is `[A-Za-z0-9+/=]` with no metacharacter for any
shell in the path to mangle. Setting `DefaultShell` to PowerShell is a
reasonable thing to want for your own sessions and changes nothing here.

---

## What is different about a Windows host, once it is up

Windows has no `/proc`, so the readings come from CIM performance classes and
the metric set is smaller: **no temperatures, no GPU, and no filesystem
capacity**. Per-core CPU, memory, uptime, network, disk I/O and the process
ranking all work. What is read, and the inverse-counter trap that makes the CPU
figure correct, is in [specs/windows-hosts.md](specs/windows-hosts.md).
