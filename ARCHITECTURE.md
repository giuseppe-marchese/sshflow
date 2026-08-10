# Architecture

## The core idea: a Connection Manager, not a wrapper

Early drafts of this project framed it as "an intelligent layer on top of
`ssh`" — implicitly, a wrapper. The actual center of gravity is different:
SSHFlow's daemon owns a **Connection Manager**
(`crates/daemon/src/connection_manager.rs`) whose only job is the lifecycle
of a connection:

```
        ensure()
           │
           ▼
   ┌───────────────┐   alive?   ┌──────────────┐
   │ known & alive │──yes──────▶│ reuse (fast) │
   └───────────────┘            └──────────────┘
           │ no / unknown
           ▼
   ┌────────────────────┐  ok   ┌───────────────┐
   │ start_master()      │─────▶│ track & serve │
   │ (non-interactive)   │      └───────────────┘
   └────────────────────┘
           │ needs interactive auth / unknown host key
           ▼
   ┌────────────────────────┐
   │ CLI retries in foreground│──▶ RegisterExternalMaster ──▶ track & serve
   │ (real TTY)               │
   └────────────────────────┘

   health-check loop (every `healthCheck`):
     alive?  → mark healthy
     dead + reconnect:true → try to recreate, else drop
```

Every other subsystem is a *consumer* of this lifecycle, not a driver of
it:

- **Metrics** (`crates/metrics`) records a data point every time the
  Connection Manager opens, reuses, or fails a connection. It never
  decides *when* those things happen.
- **Plugins** (`crates/plugins`) subscribe to the same lifecycle events
  (`ConnectionOpened`, `ConnectionReused`, `AuthenticationFailed`, ...)
  and fire external hooks. They cannot influence the manager's decisions
  in this version — deliberately, to keep the core simple and auditable.
- **Doctor / diagnostics** (`crates/daemon/src/doctor.rs`) inspects the
  manager's state (active connection count vs. `maxConnections`, etc.)
  alongside environment checks (is `ssh` on `PATH`, are directory
  permissions correct, is the process running as root).

This is what makes "multiplexing is one feature, not the point" true in
the code, not just in the README: you could swap out
`sshflow_ssh::start_master_noninteractive` for a completely different
connection-establishment mechanism (a jump-host chain, a cloud-discovered
bastion, ...) without touching metrics, plugins, or the IPC protocol at
all, because none of them know how a connection is actually established —
only that one now exists, or no longer does.

## Process model

Two binaries:

- **`sshflow-daemon`** — the Connection Manager, IPC server and health
  monitor. Long-running, one instance per user. Started automatically by
  the CLI the first time it's needed ("zero configuration").
- **`sshflow`** — the CLI. Short-lived: parses a command, talks to the
  daemon over a local IPC channel, prints the result, exits. For
  `sshflow ssh <target>`, it doesn't print-and-exit — it hands off to (on
  Unix, literally `exec()`s into) a real `ssh` process pointed at the
  managed control socket, so you get a completely normal, full-fidelity
  interactive SSH session with real TTY/signal passthrough.

## IPC protocol

Newline-delimited JSON, one request/response pair per connection
(`crates/core/src/lib.rs` defines `DaemonRequest`/`DaemonResponse`; the
transport lives in `crates/daemon/src/ipc.rs`).

- **Unix (Linux/macOS)**: a Unix domain socket at `~/.sshflow/daemon.sock`,
  created with `0600` permissions. Access control is the filesystem
  permission on the socket itself — only the owning user can connect.
- **Windows**: no named-pipe crate is bundled in this MVP, so the fallback
  is a TCP listener bound to `127.0.0.1` only, plus a random per-instance
  token written to `~/.sshflow/daemon.port` that every request must echo
  first. Binding to loopback already blocks any remote access; the token
  is a lightweight same-host check against other local users. This is a
  known simplification — see [SECURITY.md](SECURITY.md) and the
  [Roadmap](ROADMAP.md) for a real named-pipe transport.

## The `sshflow ssh <target>` flow, end to end

1. CLI parses `<target>` into a `HostTarget` (`crates/core`), rejecting
   anything that could be read as an `ssh` flag (leading `-`).
2. CLI ensures the daemon is reachable, auto-starting it (detached,
   `Stdio::null()`, new process group on Unix) if not.
3. CLI sends `EnsureConnection { target }`.
4. Daemon's Connection Manager:
   - if a tracked connection exists for this target, verifies it's still
     alive (`ssh -S <socket> -O check`) and, if so, returns it as reused;
   - otherwise attempts to start a new `ControlMaster` non-interactively
     (`BatchMode=yes`, `ConnectTimeout=10`);
   - if that fails specifically because it needed a password, a
     keyboard-interactive prompt, or host-key confirmation, it responds
     `NeedsInteractiveAuth` instead of failing outright.
5. If `NeedsInteractiveAuth`: the CLI creates the master itself in the
   foreground (inheriting its own TTY, so the user can type a password,
   touch a FIDO2 key, or confirm a new host key exactly as with plain
   `ssh`), then tells the daemon about it via `RegisterExternalMaster` so
   it's tracked for health-checking and metrics going forward.
6. CLI receives the control socket path and (Unix) `exec()`s
   `ssh -S <socket> <target> [extra args...]` — a real, ordinary `ssh`
   process, just multiplexed.

## Windows

The design is written to be cross-platform (every platform-specific path
is behind `cfg(unix)`/`cfg(windows)`), but this MVP has only been built
and exercised on Linux. The Windows-specific pieces that would need real
hardware/CI to validate before calling it production-ready:

- IPC transport (loopback TCP + token, see above — a proper named pipe
  would be a strict improvement).
- OpenSSH-on-Windows's `ControlMaster`/`ControlPath` support depends on
  the installed OpenSSH build; `sshflow doctor` should grow an explicit
  check for this (tracked in [ROADMAP.md](ROADMAP.md)).
- Process detachment (`CommandExt::process_group` is Unix-only; the
  Windows daemon spawn path currently just relies on `Stdio::null()`).

## Toolchain note

This codebase currently targets a conservative Rust edition/dependency
set (see [SETUP.md](SETUP.md) for exact version pins) because it was
built and tested against `rustc`/`cargo` 1.75. If you build with a newer
toolchain you can likely drop the `indexmap`/`clap`/`sysinfo` version
pins in the root `Cargo.toml` and pick up their latest releases.
