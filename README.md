# Copper for Linux

A native Linux build of the Copper browser, written in Rust on GTK4,
libadwaita and WebKitGTK 6.0 (the `webkit6` crate). It reads and writes the
same profile files as Copper for macOS and speaks the same MCP / Copper Cloud
contracts.

Status: bootstrap. The workspace compiles; the crates are empty shells that
later tasks fill in. `spikes/hello` proves the WebKitGTK API surface (see
`spikes/REPORT-T001.md`).

## Layout

| Path | What |
|---|---|
| `crates/copper-core` | models + atomic persistence (no GTK, no WebKit) |
| `crates/copper-engine` | `Engine` / `PageView` traits, events, value types |
| `crates/copper-engine-webkitgtk` | the WebKitGTK implementation; owns every `webkit6` call |
| `crates/copper-pagejs` | page JavaScript ported from macOS (`js/`) |
| `crates/copper-shield` | content-blocker rule builder |
| `crates/copper-creds` | Secret Service store, Bitwarden / 1Password CLIs |
| `crates/copper-import` | Chrome / Chromium / Brave import |
| `crates/copper-cloud` | Copper Cloud client |
| `crates/copper-mcp` | MCP server and tools |
| `crates/copper-app` | the GTK shell; binary `copper` |
| `spikes/hello` | T-001 WebKitGTK spike |
| `fixtures/` | golden files from a real Mac |
| `bin/dbx` | run a command on the Linux build host |

## Building

WebKitGTK does not build on macOS, so builds and tests run on a Linux host
(Ubuntu 24.04+ with `libwebkitgtk-6.0-dev libadwaita-1-dev libgtk-4-dev
libsecret-1-dev weston`, Rust stable). From a Mac checkout use `bin/dbx`,
which rsyncs the tree to the devbox and runs the command there with a headless
Wayland compositor and a session D-Bus already up:

```sh
bin/dbx cargo build --workspace
bin/dbx cargo run -p hello -- --selftest     # 9 PASS lines, exit 0
bin/dbx cargo clippy --workspace --all-targets -- -D warnings
DBX_RESTART=1 bin/dbx true                   # restart weston + dbus
```

On a Linux machine just use `cargo` directly. On Ubuntu 24.04 the WebKit
sandbox needs an AppArmor profile allowing user namespaces for the binary
(see `spikes/REPORT-T001.md`).
