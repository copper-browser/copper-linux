# Copper for Linux

A small, fast, quiet web browser: Copper, the browser from the Mac, built
natively for Linux. It is written in Rust on GTK 4, libadwaita and
WebKitGTK, so it still runs on WebKit, the same engine Safari uses.

```sh
curl -fsSL https://github.com/copper-browser/copper-linux/releases/latest/download/install.sh | sh
```

It runs on x86_64 and aarch64 and is free. On Ubuntu 24.04+ and Debian 13+ the
installer uses a `.deb`; everywhere else it uses a Flatpak.

---

## What it is

Copper is a browser with nothing in the way. There is a row of tabs, across
the top or down the left as you choose, and the page. There is no toolbar, no
start page, and no account to sign into. You type an address or a few words
in one field and you are on the page.

It is the same Copper as on the Mac. It reads and writes the same kinds of
files, and it syncs with your Mac through Copper Cloud if you link one. Agents
drive it through the same MCP tools.

## What it does

- **One field.** Type an address and you go there; type words and you
  search. Suggestions come in groups — Open Tabs, then History, then the
  search — and Enter takes the first one, so Ctrl+T, a few letters, Enter
  goes back to the tab or page you meant (a whole address still goes
  there). Tab and Shift+Tab jump between the groups, ↑ ↓ move within them,
  → or End takes the offered completion. Nothing is sent anywhere until
  you press Enter. Ctrl+K lists your open tabs and every command.
- **Tabs that stay out of the way.** Pin the pages you keep open all day.
  Saved tabs work like Arc's: Ctrl+W on one only puts it to sleep — it stays
  in Saved, where it was — and the × on the row (or Unsave in its menu) is
  what takes it out; Ctrl+Shift+T brings it back to its place. Drag any tab
  into Saved to keep it; a line shows where it will land. Folders (yours,
  and the Mac's through Copper Cloud) show with their tabs stepped in under
  them; click one to shut or open it, drag a tab in or out. Tabs from your
  last session come back instantly and cost nothing until you click them.
  Ctrl+Shift+S moves the tabs between a column and a row.
- **Spaces.** Rows of tabs with a name, an icon and a colour the whole
  column wears. Ctrl+Alt+← and → move between them.
- **Private tabs.** Ctrl+Shift+N. Cookies and site data stay in memory and
  are gone when the tab closes; nothing goes into history or the session.
- **An ad blocker that runs before the page.** Trackers and ad networks are
  stopped inside WebKit's networking. It is on by default, and you can turn
  it off for a site that breaks.
- **Hide anything, for good.** Press Ctrl+Shift+H, then click a cookie banner
  or an overlay. It stays gone on that site.
- **Reading mode.** Ctrl+Alt+R strips a page down to the article.
- **History, bookmarks, downloads.** Each is a searchable panel, one key
  away.
- **Passwords in your keyring.** Copper offers to save a sign-in once it has
  worked, and offers your saved accounts when you click into the field. It
  never fills anything on its own. Passwords live in the Secret Service
  (GNOME Keyring, KWallet or KeePassXC). Copper only opens the keyring when
  it needs it, never at launch. It can also fill from Bitwarden and
  1Password through their command-line tools, and from a paired
  [BotPass](https://github.com/pprasha/botpass) server, whose grants decide
  which accounts Copper and its agents may use on which sites
  (`bp connect copper`; [docs/agents.md](docs/agents.md#botpass)).
- **Move in from Chrome, Chromium or Brave.** One sheet brings over
  bookmarks, history, open tabs and passwords. Nothing leaves the computer.
- **Copper Cloud, if you want it.** See below.
- **Canvas.** Whiteboards beside your tabs (Ctrl+Shift+E). You can share
  them through Copper Cloud.
- **An MCP server for your agents.** See below.

## What it doesn't do

- No telemetry, no analytics, no crash reports sent anywhere. The only things
  that leave the computer are the pages you ask for, their icons, and a check
  every six hours for a newer version.
- No sync, no account, no cloud, unless you link a Copper Cloud instance
  yourself.

## Syncing with your Mac

Copper Cloud is a server you (or your team) run:
[copper-cloud](https://github.com/Exowatt-Labs/copper-cloud). Link Copper to
it in Settings › Cloud. The fastest way is a pairing code: on a Mac that is
already signed in, go to Settings › Cloud › **Pair another Mac** and press
**Make a code**, then paste the code into Settings › Cloud here.

You choose what follows you between computers:

- spaces and their kept tabs
- the look-and-behaviour settings
- bookmarks
- history
- a view of the tabs open on your other devices
- your Personal canvas

Passwords never travel through Copper Cloud. The instance is pinned by its
certificate, and your session token stays in a file only you can read.

Details: [docs/cloud.md](docs/cloud.md).

## Agents

Turn on Settings › Agents › **Let agents drive this window**. Copper then
serves MCP at `http://127.0.0.1:4123/mcp`, on loopback, behind a bearer
token. Claude Code, phi, Cursor and Claude Desktop can use your open tabs and
sign-ins with Playwright MCP's tool names. **Copy config** gives you the
client config. With Jev mode on, an agent can hand Copper a whole goal
(`jev_run`).

```sh
claude mcp add --transport http copper http://127.0.0.1:4123/mcp --header "Authorization: Bearer <token>"
```

The `copper` command also talks to the running window: `copper tabs`,
`copper run "…"`, `copper snapshot` and so on. Run `copper help` for the
list.

Copper also runs with no display, as an agent's own browser: a headless
compositor, a pre-written `agent.json`, and `copper cloud pair` to sign the
profile in to Copper Cloud before it starts.

Details: [docs/agents.md](docs/agents.md), [docs/headless.md](docs/headless.md).

## Keyboard

Ctrl stands in for the Mac's ⌘. Where Linux browsers agree on a key (Ctrl+D
to bookmark, Alt+← to go back, F5 to reload), Copper uses that key.

| | |
|---|---|
| `Ctrl+L` (or `Alt+D`, `F6`) address · `Ctrl+K` tabs and commands · `Ctrl+T` new tab · `Ctrl+W` close (a Saved tab sleeps) · `Ctrl+Shift+T` reopen | `Alt+←` `Alt+→` (or `Ctrl+[` `Ctrl+]`) back, forward · `Ctrl+Tab` `Ctrl+Shift+Tab` next, previous tab · `Ctrl+1`–`Ctrl+9` jump |
| `Ctrl+Shift+N` private tab · `Ctrl+Shift+D` pin · `Ctrl+Shift+S` tabs in a column or a row · `Ctrl+S` hide or show the column · `F11` full screen | `Ctrl+R` reload · `Ctrl+Shift+R` hard reload · `Ctrl+Shift+I` (or `F12`) inspector · `Ctrl+P` print · `Ctrl+Shift+M` stop sound |
| `Ctrl+F` find · `Ctrl+G` `Ctrl+Shift+G` next, previous match · `Ctrl+=` `Ctrl+-` `Ctrl+0` zoom | `Ctrl+Alt+R` reading mode · `Ctrl+Shift+H` hide something · `Ctrl+Shift+U` hidden on this site · `Ctrl+Shift+K` clear tabs |
| `Ctrl+D` bookmark · `Ctrl+Y` history · `Ctrl+Shift+Y` downloads | `Alt+1`–`Alt+9` jump to a space · `Alt+N` new space · `Ctrl+Alt+←` `Ctrl+Alt+→` previous, next space |
| `Ctrl+Shift+E` new canvas · `Ctrl+Shift+O` your Personal canvas | `Ctrl+E` Agent sidebar · `Ctrl+,` settings · `Ctrl+Q` quit |

## Known gaps

These work on the Mac but not here:

- **Extensions.** WebKitGTK has no web-extension support.
- **DRM.** There is no Widevine for WebKitGTK, so Netflix, Spotify and other
  protected video can't play.
- **Calls.** Neither Ubuntu's WebKitGTK nor the Flatpak's GNOME runtime is
  built with WebRTC, so Meet, Teams, Slack huddles and Discord voice can't
  connect. [docs/engine.md](docs/engine.md) has the measurements.
- **Passkeys and security keys.** WebKitGTK has no WebAuthn, so sites fall
  back to passwords.
- **Voice** and the Listen transcript.
- **Picture-in-picture above other apps.** Wayland doesn't let an app keep
  a window on top.
- **The Mac's own agent chat and agent links.** Ctrl+E opens the Agent
  sidebar, which shows the addon your Copper Cloud hands out
  ([docs/addons.md](docs/addons.md)); the Mac's built-in chat, its driver
  timeline, and links to an agents app aren't here. The MCP server and Jev are.
- **Importing from Arc, Safari or Dia.**
- **Self-updating.** Copper tells you when a newer version is out. To update,
  run the install line again, which upgrades in place; apt or Flatpak own the
  files.

Video needs the codecs the `.deb` recommends (`gstreamer1.0-libav`,
`gstreamer1.0-plugins-bad`). The installer adds them. If you installed the
`.deb` some other way with Recommends turned off, install those two yourself.

## Where things are

| What | Where | Who can read it |
|---|---|---|
| Passwords | Your Secret Service keyring, as items tagged `Copper` | You, through your keyring |
| Tabs, history, bookmarks, hidden elements | JSON files in `~/.local/share/copper/` | You |
| Settings | `~/.config/copper/settings.json` | You |
| Agent token, Copper Cloud link and token | `agent.json`, `cloud.json` in `~/.local/share/copper/`, mode 0600 | You alone |
| BotPass pairing (server, pinned fingerprint, device token) | `botpass.json` in `~/.local/share/copper/`, mode 0600 | You alone |
| Calendar pin's calendars (secret iCal addresses) | `calendar.json` in `~/.local/share/copper/`, mode 0600 | You alone |
| Cookies and site data | WebKit's store under `~/.local/share/copper/` | The sites that set them |

The Flatpak keeps all of this under `~/.var/app/com.copperbrowser.Copper/`.
It reaches the keyring through the Secret portal.

## Installing, in detail

```sh
curl -fsSL https://github.com/copper-browser/copper-linux/releases/latest/download/install.sh | sh
curl -fsSL …/install.sh | sh -s -- --flatpak     # the Flatpak even on Debian/Ubuntu
curl -fsSL …/install.sh | sh -s -- --dry-run     # say what it would do
```

- **Debian and Ubuntu.** The installer downloads `copper_<arch>.deb` and
  checks its SHA-256. It installs the package with apt (through sudo), which
  brings in WebKitGTK, the media codecs and an AppArmor profile that lets
  WebKit's sandbox start. It then lists what it added.
- **Everything else.** The installer downloads the `.flatpak` bundle, adds
  Flathub for your user if it is missing, and installs Copper for your user.
  The GNOME 51 runtime and its codecs come from Flathub.

You can also download the packages from the
[latest release](https://github.com/copper-browser/copper-linux/releases/latest).

---

## Building from source

WebKitGTK doesn't build on macOS, so builds and tests run on Linux. You need
Ubuntu 24.04+ (or similar) with these packages, plus stable Rust:

- `libwebkitgtk-6.0-dev`
- `libadwaita-1-dev`
- `libgtk-4-dev`
- `libsecret-1-dev`

```sh
cargo build --release -p copper-app      # target/release/copper
COPPER_WORLD=dev target/release/copper   # a separate profile: copper-dev folders
```

`COPPER_WORLD=<name>` keeps everything in `copper-<name>` folders, so a build
you are working on never touches your real profile or your Copper Cloud
link. Use it for every run that isn't your everyday browser.

On Ubuntu 23.10 and later, WebKit's web process needs an AppArmor profile
that allows user namespaces for the binary. Without one, pages never load
(`bwrap: setting up uid map: Permission denied`). The `.deb` installs one for
`/usr/bin/copper`. For a checkout, load the same profile with your
`target/` path in it.

From a Mac, `bin/dbx` rsyncs the tree to a Linux build host and runs the
command there, with a headless Wayland compositor and a session D-Bus already
up:

```sh
bin/dbx cargo build --workspace
bin/dbx cargo clippy --workspace --all-targets -- -D warnings
bin/dbx packaging/build-deb.sh            # the .deb; lintian it after
```

Packaging, releases and CI are described in
[packaging/README.md](packaging/README.md).

| Path | What |
|---|---|
| `crates/copper-core` | models and atomic persistence (no GTK, no WebKit) |
| `crates/copper-engine` | `Engine` / `PageView` traits, events, value types |
| `crates/copper-engine-webkitgtk` | the WebKitGTK implementation; owns every `webkit6` call |
| `crates/copper-pagejs` | page JavaScript ported from macOS |
| `crates/copper-shield` | content-blocker rule builder |
| `crates/copper-creds` | Secret Service store, Bitwarden and 1Password CLIs |
| `crates/copper-botpass` | BotPass: pairing, `botpass.json`, per-origin fill, `copper botpass` |
| `crates/copper-calendar` | the Calendar pin's card: `calendar.json`, iCal reading, the next meetings |
| `crates/copper-import` | Chrome, Chromium and Brave import |
| `crates/copper-cloud` | Copper Cloud client and sync |
| `crates/copper-mcp` | MCP server, tools and the `copper` CLI |
| `crates/copper-app` | the GTK shell; binary `copper` |
| `packaging/` | `.deb`, Flatpak, desktop entry, icons, AppArmor profile |
| `docs/` | agents, headless use, Copper Cloud, the engine |
| `bin/dbx` | run a command on the Linux build host |

## License

MIT; see [LICENSE](LICENSE). Copper and its icon are Collin and Felipe's.
