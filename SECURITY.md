# Security

SSHFlow deliberately has a small trust boundary: it never implements the
SSH protocol itself and never touches secrets. This document lists the
specific decisions behind that, so they can be reviewed rather than taken
on faith.

## Credentials

- **SSHFlow never stores passwords.** There is no password field anywhere
  in its config, IPC protocol, or metrics store — grep the codebase, it
  isn't there.
- **SSHFlow never touches private key material.** All authentication is
  delegated entirely to the real `ssh` binary, which uses your existing
  `~/.ssh/config`, keys, `ssh-agent`, and hardware tokens exactly as it
  would if you ran it directly.
- **FIDO2 / hardware keys and SSH Agent forwarding work unmodified**,
  since SSHFlow never intercepts the authentication exchange — it only
  decides when to start/reuse a `ControlMaster` process.

## Command construction / injection

- Every subprocess invocation (`crates/ssh/src/lib.rs`) builds its
  argument list as discrete `OsString` entries via `std::process::Command`.
  Nothing is ever interpolated into a shell string, so classic shell
  injection is not possible by construction — this is true for `ssh`
  invocations and for plugin hook invocations alike.
- `HostTarget::parse` (`crates/core/src/lib.rs`) additionally rejects any
  destination string starting with `-`, on the host or user part. This
  guards against **option injection**: even though `Command` args can't be
  reinterpreted by a shell, a hostname/user string like
  `-oProxyCommand=...` could still be misread as a flag *by the `ssh`
  binary itself* if passed straight through. We reject that shape up
  front instead.

## Host key verification

SSHFlow **never overrides `StrictHostKeyChecking`** or any other host-key
policy. Your existing `~/.ssh/config` and `known_hosts` behavior applies
unchanged. When a non-interactive (daemon-initiated) connection attempt
would need a host-key confirmation prompt, the daemon does not silently
accept or silently fail — it reports `NeedsInteractiveAuth` back to the
CLI, which retries in the foreground so you get `ssh`'s normal
"are you sure you want to continue connecting?" prompt, same as running
`ssh` directly.

## Local IPC

- **Unix**: the daemon's control socket (`~/.sshflow/daemon.sock`) is
  created with `0600` permissions — only the owning user can connect. The
  `~/.sshflow` directory itself is `0700`.
- **Windows** (MVP limitation): no named-pipe implementation is bundled,
  so the fallback transport is a TCP listener bound to `127.0.0.1` only
  (never a routable interface) plus a random per-instance token that
  every IPC request must present. Loopback binding blocks all remote
  access; the token is a lightweight same-host authentication check
  against other local users on a shared Windows machine. This is called
  out as a known simplification, not a claimed equivalent of a real
  named pipe's ACL — see [ROADMAP.md](ROADMAP.md).
- The control-master's own multiplexing socket is chmod'd to `0600`
  immediately after creation (`harden_socket_permissions` in
  `crates/ssh/src/lib.rs`), since anyone who can connect to it can ride
  the already-authenticated session.

## Plugins

Plugins are **not** dynamically loaded native code. Loading arbitrary
`.so`/`.dll` files into the daemon process would mean any plugin bug or
malicious plugin instantly compromises the entire Connection Manager, the
IPC socket, and everything it touches. Instead, a plugin
(`crates/plugins`) is a YAML file naming an external command to run on
specific events; the event is delivered as JSON on that process's stdin.
Every plugin runs in its own OS process, sandboxed the same way any other
subprocess is, and a crashing or hanging plugin cannot bring down the
daemon (hooks are fire-and-forget with the child reaped on a background
thread).

## Least privilege

`sshflow doctor` explicitly checks whether the daemon is running as root
(Unix) and flags it as a failing check — SSHFlow has no reason to run
with elevated privileges, and doing so would be a needless increase in
blast radius for any bug.

## No telemetry

There is no network client anywhere in the metrics crate. Connection
counts, reuse rates, and latency samples are written to a local
`~/.sshflow/metrics.json` and nowhere else. The `telemetry` config field
exists as an explicit, off-by-default, currently-unimplemented placeholder
so that if remote telemetry is ever added, it is opt-in by construction
rather than something a user has to remember to disable.

## Reporting a vulnerability

Please do not open a public issue for a security vulnerability. Use
GitHub's private vulnerability reporting instead: go to this
repository's **Security** tab → **Report a vulnerability**. This opens
a private draft security advisory visible only to the maintainer, so
the issue can be discussed and fixed before anything is disclosed
publicly.

If for some reason you can't use that (e.g. you're reading this outside
GitHub), you can still open a regular issue, but please leave exploit
details out of it and avoid public disclosure until a maintainer has
responded.
