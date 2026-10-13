# Packaging Copper for Linux

Two packages ship from the same tree. Both are built on a native runner for
each architecture (x86_64/amd64 and aarch64/arm64), with no containers:

| Package | For | Built by | WebKit comes from |
|---|---|---|---|
| `.deb` | Ubuntu 24.04+, Debian 13+ | `packaging/build-deb.sh` (cargo-deb) | apt (`libwebkitgtk-6.0-4`) |
| `.flatpak` bundle | Fedora 41+ and everything else | `packaging/flatpak/build.sh` (flatpak-builder) | the GNOME 51 runtime (Flathub), codecs from its `codecs-extra` extension |

People install with the one-liner, which picks one of the two
(`install.sh` at the repo root, attached to every release):

```sh
curl -fsSL https://github.com/copper-browser/copper-linux/releases/latest/download/install.sh | sh
```

It picks the `.deb` wherever apt has `libwebkitgtk-6.0-4`, and the Flatpak
elsewhere. Neither engine has WebRTC or EME, so the Flatpak buys nothing there.
The `.deb` is the smaller download and reaches the keyring directly. The
measurements are in `docs/engine.md`.

## Layout

```
packaging/
  deb.toml                       [package.metadata.deb] template (see "Why a shim manifest")
  build-deb.sh                   cargo build --release + cargo deb → target/debian/copper_<v>-1_<arch>.deb
  debian/copper.apparmor         /etc/apparmor.d/copper — lets WebKit's bwrap sandbox make a user namespace
  debian/maintainer-scripts/     postinst (alternatives + load profile), prerm, postrm
  share/com.copperbrowser.Copper.desktop        desktop entry (http/https/text/html handler)
  share/com.copperbrowser.Copper.metainfo.xml   AppStream
  icons/com.copperbrowser.Copper.svg            scalable icon (generated)
  icons/hicolor/{64,128,256,512}/apps/*.png     raster icons (generated)
  icons/make-icons.sh            regenerates both from the macOS icon (Icon/plate.jpg)
  flatpak/com.copperbrowser.Copper.yml          Flatpak manifest (GNOME 51 + rust-stable)
  flatpak/vendor.sh              Cargo.lock → cargo-sources.json (flatpak-cargo-generator, pinned)
  flatpak/build.sh               vendor + flatpak-builder + build-bundle
  version-manifest.py            writes copper-version.json for a release
```

## The `.deb`

```sh
packaging/build-deb.sh                 # release build + .deb for this arch
packaging/build-deb.sh --no-build      # package an existing target/release/copper
bin/dbx packaging/build-deb.sh         # from the Mac
```

Needs `cargo install --locked cargo-deb` (3.8.0 in CI) and `dpkg-dev`.

What it installs:

- `/usr/bin/copper`
- `/usr/share/applications/com.copperbrowser.Copper.desktop` — `Categories=Network;WebBrowser;`,
  `MimeType=text/html;application/xhtml+xml;x-scheme-handler/http;x-scheme-handler/https;`
- `/usr/share/icons/hicolor/{64x64,128x128,256x256,512x512}/apps/com.copperbrowser.Copper.png`
  and `/usr/share/icons/hicolor/scalable/apps/com.copperbrowser.Copper.svg`
- `/usr/share/metainfo/com.copperbrowser.Copper.metainfo.xml`
- `/etc/apparmor.d/copper` (a conffile)

Control fields:

- `Depends: $auto, libwebkitgtk-6.0-4, libgtk-4-1, libadwaita-1-0 (>= 1.5), libsecret-1-0`
- `Recommends: gstreamer1.0-libav, gstreamer1.0-plugins-bad, gstreamer1.0-plugins-good,
  gstreamer1.0-gl, gstreamer1.0-pipewire | gstreamer1.0-pulseaudio | gstreamer1.0-alsa,
  libgl1-mesa-dri, xdg-desktop-portal-gtk | xdg-desktop-portal-gnome | xdg-desktop-portal-kde,
  fonts-noto-color-emoji`. WebKitGTK alone pulls in only plugins-base/-good/-gl, and
  without libav and plugins-bad H.264, AAC, HEVC, AV1 and MSE Opus all fail, which
  breaks YouTube and most sites' video (measured in `docs/engine.md`). `install.sh`
  installs Recommends explicitly (`-o APT::Install-Recommends=true`) and prints which
  it added, which were already there, and any apt could not find.
- `Suggests: gstreamer1.0-plugins-ugly, gstreamer1.0-nice, speech-dispatcher,
  gnome-keyring | kwalletmanager | keepassxc`: x264 and a52, ICE for WebKit builds
  that have WebRTC, a speech backend for `speechSynthesis`, and a Secret Service
  for passwords.
- `Provides: www-browser, gnome-www-browser`.

Maintainer scripts:

- **postinst** registers `/usr/bin/copper` for `x-www-browser` and
  `gnome-www-browser` (priority 40) with update-alternatives, and — only when
  AppArmor is present and enabled — runs `apparmor_parser -r -T -W /etc/apparmor.d/copper`.
  A load failure is reported, not fatal.
- **prerm** removes the alternatives. **postrm** unloads the profile on remove
  and purge, and deletes `/etc/apparmor.d/local/copper` on purge.

Icon cache and desktop database are refreshed by dpkg triggers
(`gtk-update-icon-cache`, `desktop-file-utils`); no script calls them.

### Why the AppArmor profile

Ubuntu 23.10+ sets `kernel.apparmor_restrict_unprivileged_userns=1`. WebKitGTK's
web process starts inside bubblewrap, which needs an unprivileged user
namespace; without a profile granting `userns` the web process dies at start
(`bwrap: setting up uid map: Permission denied`). The profile is unconfined
and only adds `userns`, the same shape as Ubuntu's `/etc/apparmor.d/epiphany`.
Measured on the devbox (Ubuntu 24.04, aarch64) with the WebKit spike
(`spikes/hello --selftest`) installed as `/usr/bin/copper`: 9/9 pass with the
profile loaded; with it unloaded, `bwrap: setting up uid map: Permission denied`
and exit 133. The Flatpak does not need it (its sandbox goes through the
Flatpak portal). Never use `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS` instead.
The profile uses `abi <abi/4.0>`, so it needs AppArmor 4 (Ubuntu 24.04, Debian 13).

### Why a shim manifest

cargo-deb reads `[package.metadata.deb]` only from a package's `Cargo.toml`
and has no `--config` flag; the crate manifest
(`crates/copper-app/Cargo.toml`) is kept free of packaging. `build-deb.sh`
therefore renders `packaging/deb.toml` (with `@ROOT@` → the repo root and
`@VERSION@` → copper-app's version) into `target/deb-shim/Cargo.toml` — a
stand-alone, metadata-only package with its own `[workspace]` and an empty
`lib.rs` — and runs

```sh
CARGO_TARGET_DIR=<repo>/target cargo deb --manifest-path target/deb-shim/Cargo.toml --no-build
```

`target/release/copper` in the asset list then resolves to the binary the
workspace just built. It also writes a one-entry `changelog` (lintian wants
`changelog.Debian.gz`). Lintian is clean apart from `no-manual-page` and
`initial-upload-closes-no-bugs` (warnings).

If the metadata should live in the crate after all, paste the
`[package.metadata.deb]` table from `deb.toml` into `crates/copper-app/Cargo.toml`,
replace every `@ROOT@/` with `../../`, set `changelog` to a committed file (or
drop it), and run `cargo deb -p copper-app`.

## The Flatpak

```sh
packaging/flatpak/build.sh                     # → target/flatpak/copper-<arch>.flatpak
packaging/flatpak/build.sh --install           # and install it for this user
flatpak run com.copperbrowser.Copper
```

Needs `flatpak flatpak-builder elfutils librsvg2-common python3-aiohttp python3-tomlkit`
(`eu-strip` and the SVG loader `appstreamcli compose` needs for the icon; flatpak-builder
only recommends them). The first run
installs `org.gnome.Sdk//51` and `org.freedesktop.Sdk.Extension.rust-stable//26.08`
for the user from Flathub (~3 GB on disk). Everything else lives in
`target/flatpak/`.

- **Runtime:** `org.gnome.Platform//51` — the newest on Flathub (freedesktop
  26.08 base): WebKitGTK 6.0 2.54.1, GTK 4.24, libadwaita 1.10. WebKit is not
  bundled. The rust-stable 26.08 extension ships Rust 1.98.
- **Offline crates:** `vendor.sh` runs flatpak-builder-tools'
  `flatpak-cargo-generator.py` (pinned commit, sha256-checked) over `Cargo.lock`
  to make `cargo-sources.json`; the build runs `cargo --offline`. `build.sh`
  writes the generated file and a copy of the manifest (source path made
  absolute) under `target/flatpak/`, so nothing generated sits in the tree.
  `cargo-sources.json` is never committed; rerun after every `Cargo.lock` change.
- **Permissions** (PLAN §7 Security): `--share=network`, `--share=ipc`,
  `--socket=wayland`, `--socket=fallback-x11`, `--device=dri`,
  `--socket=pulseaudio`, `--talk-name=org.freedesktop.secrets`,
  `--filesystem=xdg-download`. Everything else (saving elsewhere, Chrome
  import) goes through the file-chooser portal. MCP on 127.0.0.1:4123 is covered
  by `network`.
- **Bundle:** `flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo`.
  A bundle cannot be installed over an installed copy, so `install.sh`
  uninstalls first (user data in `~/.var/app/com.copperbrowser.Copper` stays).

`flatpak-builder-lint` (org.flatpak.Builder) passes except for checks that only
matter for Flathub: `appid-url-not-reachable` (copperbrowser.com must serve
HTTPS and verify the app id), `metainfo-missing-screenshots`,
`appstream-screenshots-not-mirrored-in-ostree`. For Flathub (Q-001, later):
add screenshots to the metainfo, verify the domain, and change the `dir`
source to a `git` source with `tag` + `commit`.

## Releases

`.github/workflows/release.yml` runs on a pushed tag `v<version>`; the tag must
equal `[workspace.package] version` in `Cargo.toml` (it fails otherwise). To
release:

1. Bump `version` in the root `Cargo.toml`; let cargo update `Cargo.lock`.
2. Add a `<release version="…" date="…">` at the top of
   `packaging/share/com.copperbrowser.Copper.metainfo.xml` (the run warns if it
   is missing).
3. Optionally write `NOTES.md`; its first paragraph becomes the `notes` field.
4. Tag and push `v<version>`.

Jobs: `meta` → `deb` (amd64, arm64) and `flatpak` (x86_64, aarch64), each of
which installs its artifact on the runner, checks `copper --version` prints
`copper <version>`, and removes it → `assemble` (install.sh, checksums,
`copper-version.json`) → `publish` (only for tags; the only job with
`contents: write`; `gh release create`, then the published bytes are
re-downloaded and checked). `workflow_dispatch` is a dry run that keeps
everything as a run artifact.

Assets: `copper_<v>_{amd64,arm64}.deb`, `copper-<v>-{x86_64,aarch64}.flatpak`,
the same four under stable names (`copper_amd64.deb`, `copper-x86_64.flatpak`, …)
for `releases/latest/download/…`, a `.sha256` for each (`<hex>  <name>`),
`copper-version.json`, `install.sh` (+ `.sha256`). A tag with a pre-release
suffix makes a GitHub pre-release, which `latest` (and so the installer and the
update check) ignores.

### `copper-version.json`

Same top-level shape as the macOS manifest (copper-browser/Copper
`release.yml`, read by `Fork/Updates.swift`), with the per-zip fields moved
into `assets`:

```json
{
  "schemaVersion": 1,
  "product": "copper",
  "platform": "linux",
  "version": "0.1.0",
  "releaseId": "v0.1.0",
  "commit": "<sha>",
  "source": "https://github.com/copper-browser/copper-linux/commit/<sha>",
  "status": "lkg",
  "publishedAt": "2026-10-12T00:00:00Z",
  "installer": "install.sh",
  "latestUrl": "https://github.com/copper-browser/copper-linux/releases/latest",
  "notes": "first paragraph of NOTES.md, or empty",
  "assets": [
    { "kind": "deb", "arch": "amd64", "name": "copper_0.1.0_amd64.deb", "size": 0,
      "sha256": "<hex>", "url": ".../releases/download/v0.1.0/copper_0.1.0_amd64.deb",
      "latestName": "copper_amd64.deb", "latestUrl": ".../releases/latest/download/copper_amd64.deb" }
  ]
}
```

`assets` has four entries: deb amd64/arm64, flatpak x86_64/aarch64. The
update check (`crates/copper-app/src/updates.rs`, T-023) reads
`https://github.com/copper-browser/copper-linux/releases/latest/download/copper-version.json`,
compares `version`, and links the matching asset (or, in the Flatpak — where
`/.flatpak-info` exists — says to update with Flatpak). It never replaces
files; apt / flatpak own them.

## Contract with the app

- Application id `com.copperbrowser.Copper` — the `GtkApplication` id, so the
  Wayland app id and X11 `WM_CLASS` match the desktop file
  (`StartupWMClass=com.copperbrowser.Copper`) and the icon name.
- `copper --version` prints exactly `copper <version>` and exits 0 without a
  display; release.yml checks it for every artifact.
- `copper <url>…` opens the URLs (the desktop entry runs `copper %U`).

## CI

`.github/workflows/ci.yml` (push to main, pull requests): `cargo fmt --check`;
`cargo clippy --workspace --all-targets --locked -D warnings` and
`cargo test --workspace --locked` on `ubuntu-24.04` and `ubuntu-24.04-arm`
under headless weston (pixman, 1440×900) and a private session bus, with an
AppArmor `userns` profile for `$GITHUB_WORKSPACE/target/**` (the same one the
devbox uses); and a packaging lint job (shellcheck, desktop-file-validate,
`appstreamcli validate`, `apparmor_parser -Q`, `flatpak-builder --show-manifest`).

apt packages: `build-essential pkg-config libwebkitgtk-6.0-dev libgtk-4-dev
libadwaita-1-dev libsecret-1-dev` to build; `weston dbus-daemon
fonts-dejavu-core` to test.

## Validation

```sh
desktop-file-validate packaging/share/com.copperbrowser.Copper.desktop
appstreamcli validate --no-net packaging/share/com.copperbrowser.Copper.metainfo.xml
lintian target/debian/copper_*.deb
apparmor_parser -Q -K -T packaging/debian/copper.apparmor
```

Regenerate icons only when the Mac icon changes:
`packaging/icons/make-icons.sh /path/to/copper-ref/Icon/plate.jpg`
(needs `imagemagick librsvg2-bin`).
