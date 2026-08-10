# Contributing to SSHFlow

Thanks for considering a contribution. This is a young project, so process
is intentionally light.

## Getting set up

See [SETUP.md](SETUP.md) for how to build the workspace from scratch.

## Before opening a PR

```bash
cargo fmt --all
cargo build --workspace
cargo test --workspace
```

There's no `cargo clippy` gate configured yet — running it locally and
fixing anything it flags is appreciated but not required.

## Project layout

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the crates fit together.
A few rules that keep that architecture intact:

- **Only `crates/ssh` shells out to the `ssh` binary.** If you need a new
  interaction with OpenSSH, it belongs there, behind a typed function —
  not as an inline `Command::new("ssh")` somewhere else in the tree.
- **Only `crates/daemon/src/connection_manager.rs` mutates connection
  state.** IPC handlers, the health-check loop, etc. call its API; they
  don't reach into a `DashMap` directly.
- **No shell strings.** Every subprocess call builds its argument list as
  discrete `Command::arg(...)` calls. If you find yourself building a
  `String` that looks like a command line, stop — see
  [SECURITY.md](SECURITY.md) for why.
- **No dynamic native-code loading for plugins.** The plugin system is
  external-process hooks by design; keep it that way (again, see
  [SECURITY.md](SECURITY.md)).

## Commit style

Small, focused commits with a clear subject line are preferred over large
mixed changes. No specific format is enforced.

## Reporting bugs / requesting features

Open a GitHub issue. For anything that could be a security issue, see the
reporting note at the bottom of [SECURITY.md](SECURITY.md) instead of a
public issue.

## Code of Conduct

By participating in this project you agree to abide by the
[Code of Conduct](CODE_OF_CONDUCT.md).
