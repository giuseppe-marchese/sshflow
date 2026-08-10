# Setup / Build-from-source guide

This walks through everything needed to go from the archive you were
given to a working `sshflow` on your own machine.

## 1. Prerequisites

- **Rust toolchain.** Install via [rustup](https://rustup.rs) if you
  don't already have one:

  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  # then restart your shell, or: source "$HOME/.cargo/env"
  rustc --version   # sanity check
  ```

  > **Note on version pins:** this codebase was developed and tested
  > against an older toolchain (`rustc`/`cargo` 1.75, installed via
  > `apt` in a sandboxed environment with no access to rustup's own
  > servers). To keep things building on that toolchain, the root
  > `Cargo.toml` pins a few dependencies below their latest release
  > (`clap = "=4.4.18"`, `indexmap = "=2.2.6"`,
  > `sysinfo = { version = "0.30", default-features = false }`) because
  > their newer releases require Rust's 2024 edition / a newer MSRV.
  > **If you install Rust via rustup as above (recommended), you have a
  > current stable toolchain and don't need any of this** — feel free to
  > relax those pins to `clap = { version = "4", features = ["derive"] }`,
  > `indexmap` removed entirely (it's only there to pin a transitive
  > dependency), and `sysinfo = "0.30"` with default features back on.
  > The code itself doesn't depend on the old versions' behavior.

- **OpenSSH client** (`ssh`) on your `PATH` — you almost certainly
  already have this on Linux/macOS. On Windows, install the optional
  "OpenSSH Client" Windows feature, or Git for Windows' bundled `ssh`.

- **(Optional, for local testing)** an SSH server you can connect to.
  If you don't have a remote box handy, you can stand up a local one for
  testing (see [§6](#6-optional-local-end-to-end-test) below).

## 2. Get the code

Unpack the archive you were given, or, if you'd rather start a fresh git
history:

```bash
cd sshflow
git init
git add -A
git commit -m "Initial import"
```

## 3. Build

```bash
cd sshflow
cargo build --release --workspace
```

This produces:

```
target/release/sshflow           # the CLI
target/release/sshflow-daemon    # the Connection Manager daemon
```

(Windows: `sshflow.exe` / `sshflow-daemon.exe`.)

Run the test suite too, while you're here:

```bash
cargo test --workspace
```

You should see all suites pass (config, core, metrics, ssh crates all
have unit tests; daemon/cli are exercised end-to-end in §6).

## 4. Install onto your `PATH`

Simplest option — copy both binaries somewhere already on your `PATH`:

```bash
# Linux/macOS example:
sudo cp target/release/sshflow target/release/sshflow-daemon /usr/local/bin/
```

Keeping both binaries **in the same directory** matters: the CLI looks
for `sshflow-daemon` next to its own executable first, falling back to
`PATH` only if that lookup fails.

Alternatively, for a Cargo-native install without `sudo`:

```bash
cargo install --path crates/cli
cargo install --path crates/daemon
# make sure ~/.cargo/bin is on your PATH (rustup's installer adds this for you)
```

## 5. First run

```bash
sshflow version
sshflow doctor
```

`doctor` should report your `ssh` binary/version, confirm
`~/.sshflow` was created with owner-only permissions, and flag anything
worth knowing before you start using it for real (for example, it will
tell you if you're running as root, which you shouldn't be for normal
use).

Then just use it like `ssh`:

```bash
sshflow ssh you@somehost
# ... first connection: normal handshake time, connection is now tracked
exit
sshflow ssh you@somehost
# ... second connection: near-instant, reusing the multiplexed session
```

Useful commands along the way:

```bash
sshflow status        # daemon uptime, active connections, plugins loaded
sshflow stats          # today's connection count, reuse rate, time saved
sshflow connections    # currently managed (multiplexed) connections
sshflow logs --lines 100
sshflow config show    # print ~/.sshflow/config.yaml
sshflow config edit    # open it in $EDITOR
```

To stop the daemon (it will simply restart on your next `sshflow` command):

```bash
# there's no `sshflow daemon stop` subcommand yet -- the simplest way is:
kill "$(cat ~/.sshflow/daemon.pid)"
```

## 6. (Optional) local end-to-end test

If you want to see the whole system work without touching a real remote
server, spin up a throwaway local `sshd` on a high port:

```bash
# Debian/Ubuntu example
sudo apt-get install -y openssh-server
sudo ssh-keygen -A                       # generate host keys if needed

ssh-keygen -t ed25519 -N "" -f ~/.ssh/id_ed25519    # if you don't have a key yet
cat ~/.ssh/id_ed25519.pub >> ~/.ssh/authorized_keys
chmod 700 ~/.ssh && chmod 600 ~/.ssh/authorized_keys

cat > /tmp/sshd_test_config <<EOF
Port 2222
ListenAddress 127.0.0.1
PermitRootLogin prohibit-password
PasswordAuthentication no
PubkeyAuthentication yes
AuthorizedKeysFile $HOME/.ssh/authorized_keys
Subsystem sftp /usr/lib/openssh/sftp-server
EOF
sudo /usr/sbin/sshd -f /tmp/sshd_test_config

# trust its host key
ssh-keyscan -p 2222 127.0.0.1 >> ~/.ssh/known_hosts

# now try it
sshflow ssh "$USER@127.0.0.1:2222" -- whoami   # first time: full handshake
sshflow ssh "$USER@127.0.0.1:2222" -- hostname # second time: instant reuse
sshflow connections
sshflow stats
```

## 7. Try a plugin

```bash
mkdir -p ~/.sshflow/plugins
cp examples/plugins/log-events.yaml ~/.sshflow/plugins/
sshflow ssh you@somehost -- true
cat /tmp/sshflow-events.log   # should contain a ConnectionOpened / ConnectionReused line
```

## 8. Where to go next

- [ARCHITECTURE.md](ARCHITECTURE.md) — how the crates fit together, the
  IPC protocol, and the full `sshflow ssh` request flow
- [SECURITY.md](SECURITY.md) — the specific security decisions and why
- [ROADMAP.md](ROADMAP.md) — what's intentionally not built yet
- [CONTRIBUTING.md](CONTRIBUTING.md) — project conventions if you want to
  extend it
