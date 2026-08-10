# Roadmap

## Shipped (MVP)

- Connection Manager: create / reuse / health-check / auto-reconnect /
  LRU-evict lifecycle for OpenSSH `ControlMaster` connections
- `sshflow ssh|status|stats|doctor|connections|logs|config|version|update`
- Daemon auto-start, zero-config `~/.sshflow/config.yaml`
- Local, file-persisted metrics engine (no telemetry)
- Event-driven external-hook plugin system
- Foreground fallback for interactive auth (password / FIDO2 / host-key
  confirmation)
- Cross-platform-aware code paths (Unix implemented and tested; Windows
  written but not yet validated on real hardware/CI — see
  [ARCHITECTURE.md](ARCHITECTURE.md#windows))

## Near-term

- [ ] Real Windows named-pipe IPC transport (replace the loopback+token
      fallback described in [SECURITY.md](SECURITY.md))
- [ ] `sshflow doctor` check for OpenSSH-on-Windows `ControlMaster` support
- [ ] `sshflow connections --json` / `sshflow stats --json` for scripting
- [ ] Read existing `~/.ssh/config` `Host` aliases so `sshflow ssh myalias`
      resolves the same way plain `ssh myalias` would (`openssh-config`
      crate)
- [ ] Fuzz testing for `HostTarget::parse` and the IPC JSON decoder
- [ ] Cross-platform CI (GitHub Actions matrix: Linux/macOS/Windows) —
      see `.github/workflows/ci.yml` for the Linux/macOS starting point

## Future features (from the original project vision)

These were explicitly out of scope for the MVP and remain aspirational:

- Interactive TUI / dashboard
- SSH history, host tags, connection groups
- Jump host automation
- Vault integration
- Kubernetes integration
- Cloud instance discovery
- Prometheus metrics endpoint
- REST API
- Web dashboard
- VS Code / JetBrains integration
- Desktop notifications
- AI-assisted diagnostics

None of these should require changes to the Connection Manager's core
API — they're either new consumers of its lifecycle events (dashboard,
Prometheus, notifications) or new ways of feeding it targets (cloud
discovery, Kubernetes, jump-host automation). That separation is the
main architectural bet this project is making; see
[ARCHITECTURE.md](ARCHITECTURE.md) for why.
