# SSHFlow

**SSHFlow is a Connection Manager for OpenSSH.**

It does not replace `ssh`, and it is not "just a multiplexing wrapper." Its
job is to own the full lifecycle of your SSH connections — create, reuse,
health-check, recover, evict, observe — so that every `ssh` you run through
it is fast, monitored, and backed by real diagnostics. Multiplexing
(OpenSSH's `ControlMaster`/`ControlPersist`) is simply the mechanism the
Connection Manager uses today to make reuse cheap. Metrics, health
monitoring and the plugin/event system are not bolted on afterward — they
all observe the same connection lifecycle the manager defines.

```
$ sshflow ssh deploy@prod-01
sshflow: connected to deploy@prod-01 (211 ms handshake, now multiplexed)
deploy@prod-01:~$ exit

$ sshflow ssh deploy@prod-01
sshflow: reusing connection to deploy@prod-01 (4 ms)
deploy@prod-01:~$
```

## Why

If you `ssh` into the same handful of hosts dozens of times a day, every one
of those connections pays a full TCP + key-exchange + auth round trip,
even though OpenSSH has supported connection multiplexing since 2007.
Nobody uses it, because turning it on by hand (`ControlMaster`,
`ControlPath`, `ControlPersist` in `~/.ssh/config`) is fiddly, invisible,
and gives you zero feedback about what it's actually saving you or when a
stale socket has gone bad. SSHFlow makes that mechanism automatic,
observable and self-healing, and is built to grow into a full connection
management platform (host inventory, jump-host automation, cloud discovery,
Prometheus export, ... see [ROADMAP.md](ROADMAP.md)).

## Architecture

```
sshflow/
├── crates/
│   ├── core/       shared types: HostTarget, IPC protocol, events, errors
│   ├── config/     ~/.sshflow/config.yaml loading, secure directory setup
│   ├── ssh/        OpenSSH ControlMaster wrapper (the only crate that shells out to `ssh`)
│   ├── metrics/    local-only connection metrics, persisted to metrics.json
│   ├── plugins/    event-driven external hook plugin system
│   ├── daemon/     sshflow-daemon: the Connection Manager + IPC server + health monitor
│   └── cli/        sshflow: the user-facing command line tool
```

The **Connection Manager** (`crates/daemon/src/connection_manager.rs`) is
the core abstraction: everything else in the daemon is a thin caller of its
API (`ensure`, `mark_reused`, `evict_lru`, `list`, ...). See
[ARCHITECTURE.md](ARCHITECTURE.md) for the full design, the IPC protocol,
and how a `sshflow ssh host` invocation flows end to end.

## Install

See [SETUP.md](SETUP.md) for full build-from-source instructions
(prerequisites, exact commands, and how to install the two binaries onto
your `PATH`).

```bash
cargo build --release --workspace
# binaries land in target/release/sshflow and target/release/sshflow-daemon
```

## Usage

```bash
sshflow ssh user@host              # connect; auto-starts the daemon on first use
sshflow status                     # daemon uptime, active connections, plugins
sshflow stats                      # connections today, reuse rate, time saved
sshflow connections                # list currently managed (multiplexed) connections
sshflow doctor                     # environment diagnostics (ssh binary, permissions, config)
sshflow logs --lines 100           # tail the daemon's log
sshflow config show|path|edit      # inspect or edit ~/.sshflow/config.yaml
```

`sshflow` is zero-configuration: the first time you run `sshflow ssh`, it
writes a default `~/.sshflow/config.yaml`, starts the daemon in the
background, and connects. No server-side changes are required — SSHFlow is
a pure client-side layer and works with any OpenSSH server you can already
reach with plain `ssh`.

### Configuration

`~/.sshflow/config.yaml`:

```yaml
autoMultiplex: true
persistTime: 10m
healthCheck: 30s
reconnect: true
maxConnections: 20
metrics: true
plugins: true
```

### Plugins

A plugin is a small YAML file in `~/.sshflow/plugins/` describing an
external command to run when specific connection events fire (see
[examples/plugins](examples/plugins)). No native code is ever loaded into
the daemon — see [SECURITY.md](SECURITY.md) for why.

## Security

SSHFlow never stores passwords or touches private key material; it only
ever shells out to your existing, trusted `ssh` binary and relies entirely
on your existing agent, keys, or FIDO2 device for authentication. Full
details, including the specific injection and permission protections
implemented, are in [SECURITY.md](SECURITY.md).

## Status

This is an early, functional MVP: the CLI, daemon, Connection Manager,
metrics engine, health monitor and plugin system all work end to end today
on Linux/macOS (and are written to be Windows-compatible — see
[ARCHITECTURE.md](ARCHITECTURE.md#windows) for the one platform-specific
caveat). Everything under [ROADMAP.md](ROADMAP.md) ("Future Features" in
the original spec) is intentionally not built yet.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## License

MIT — see [LICENSE](LICENSE).
