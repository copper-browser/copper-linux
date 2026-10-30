# Addons: the Agent sidebar

An addon is a small web app from your Copper Cloud that fills a part of the
Copper window. In this version, the only such part is the **Agent sidebar**.
Addons update without a new Copper, and the same bundle runs on every Copper.

## The Agent sidebar

**Ctrl+E** shows the sidebar or puts it away. It works from a page or from
Copper's own controls. The command palette and the window menu have it too, as
"Agent sidebar".

- It sits to the right of the page and runs the full height of the window.
- Drag its left edge to change its width, from 340 to 620 pixels (400 at
  first). Copper keeps the width in `settings.json` as `agent.paneWidth`, the
  same key the Mac uses.
- When you put it away, the addon keeps running: a chat keeps its place, and
  nothing loads again when you bring it back.
- With no addon, it says "No agent here yet." and has a button that opens
  Settings › Extensions.

## Where addons come from

Addons come from the Copper Cloud you link in Settings › Cloud. The cloud
decides which addon you get. Copper asks for that list:

- about five seconds after it starts;
- when the link or the signed-in account changes;
- every six hours;
- when Settings › About checks (on its own, or when you press **Check now**);
- when the cloud's event stream says something changed.

What Copper does with the answer:

- **A removed addon, or new settings** take effect at once.
- **A new addon or a new version** is downloaded, checked against its SHA-256
  and staged. Nothing runs yet. Settings › About shows "Update available —
  Hello addon 1.0.0 for the Agent sidebar" with an **Install** button, and a
  toast says the same once for each version. **Install** loads the new version
  into the sidebar; Copper doesn't need to restart.

**Settings › Extensions** lists each addon with its name, version, slot and the
cloud it came from, and offers:

- **Use in Agent sidebar**: turn the addon off on this computer only. The
  sidebar shows its placeholder instead.
- **Remove…**: take the addon off this computer. It stays off until your Copper
  Cloud changes what it hands out.
- **Check for addon updates**.

Installed addons belong to the cloud and account they came from. If you link a
different cloud or sign in as someone else, they are removed.

Files live under Copper's data folder (`~/.local/share/copper/addons/`):

- `state.json`
- unpacked bundles in `bundles/<id>/<sha256>/`
- downloads awaiting Install in `staging/`
- each addon's `copper.storage` in `data/<id>/storage.json`, readable by you
  alone (0600)

## What an addon can do

An addon page is served from `copper-addon://<id>/`, and only from its own
installed bundle. It runs in a web view of its own with a separate profile, so
it never sees your browsing cookies or storage. Every page gets a strict
Content Security Policy:

- no network from the page itself;
- no frames;
- no plugins;
- no forms that leave the addon.

Everything else goes through `window.copper`, bridge v1. The shim is the same
bytes on every platform:

| Call | On Linux |
|---|---|
| `copper.info()` | the bridge version, `linux`, the slot, the addon, Copper's version, the theme, and the capabilities this Copper grants |
| `copper.identity()` | the Copper Cloud account (`null` when not signed in) |
| `copper.settings()` | the addon's settings as the cloud resolved them |
| `copper.storage.get/set/remove/keys` | per addon, up to 1 MB |
| `copper.fetch(url, init)` | only for hosts the manifest lists. No cookies or ambient credentials, and every redirect is checked too. |
| `copper.openUrl(url)` | opens a new foreground tab (http and https only) |
| `copper.page.current()` | the active tab's `{url, title}`; `null` for a private tab |
| `copper.oauth.authorize({url, redirectPrefix})` | opens a "Sign in — <addon>" window with your normal browsing profile. It ends at the first address that starts with `redirectPrefix`. Closing the window gives `cancelled`; running past the time limit gives `timeout`. |
| `copper.ui.close()` | puts the sidebar away |
| `copper.agent.*` | `unavailable`: Copper for Linux has no native agent yet |

Events: `visibility` (the sidebar was shown or put away), `theme` (light or
dark), `settings`, and `page` (the active tab changed). `page` needs the
`page` capability.

A link the person clicks opens in a new tab. Any other attempt to leave
`copper-addon://<id>/` is refused, and so is every new window. Developer tools
are available only in debug builds and with a developer folder.

## Building an addon

The manifest, the bundle format and bridge v1 are shared by every Copper. The
rules are in `crates/copper-core/src/addons/`, and the hello fixture below is a
working example of each call.

To work on an addon without a cloud, point Copper at its unpacked folder:

```sh
COPPER_WORLD=addon-dev COPPER_ADDONS_DEV_DIR=$PWD/my-addon copper
```

The folder replaces whatever addon the cloud gave its slot. Copper reads the
files on every load, so reloading the page shows your changes: right-click in
the sidebar and choose **Inspect Element**, then press **Ctrl+R** in the
inspector. The developer folder works only in debug builds or in a test world
(`COPPER_WORLD`).

## Testing

`crates/copper-app/qa/addons/hello/` is a fixture that calls every bridge
method and prints what came back. The self-test runs it on a private headless
weston:

```sh
crates/copper-app/qa/addons-selftest.sh            # PNGs in /tmp/copper-addons-selftest
DEV=1 crates/copper-app/qa/addons-selftest.sh      # also as a developer folder
```

The self-test (`COPPER_SELFTEST=addons`, a debug build) does the following:

1. Shows the placeholder.
2. Packs the fixture in the served format, then verifies and stages it as a
   cloud update.
3. Presses Install.
4. From the page, checks `info`, `settings`, `storage`, a refused `fetch`,
   `openUrl`, `page.current`, `identity`, `agent.state`, the scheme's 404s, a
   refused navigation, both OAuth endings, the `theme` and `visibility`
   events, and `ui.close`.
5. Turns the addon off and on, then removes it.

It takes screenshots in light and dark, prints `PASS`/`FAIL` lines, and exits 1
on any failure.

### Testing against a real Copper Cloud

`addons-cloud-e2e.sh` runs the same path a person takes, against a running
Copper Cloud instead of a staged bundle. It makes a throwaway test world, signs
it in (a new device id, `POST /v1/auth/login`, then `cloud.json` at 0600), and
runs `COPPER_SELFTEST=addons-cloud` on a private headless weston. The app's own
sync must bring the addon the cloud hands that account. Settings › About ›
Install must then put it in the Agent sidebar. The run checks `info` and
`identity`, then shows Settings › Extensions.

```sh
CLOUD_URL=http://127.0.0.1:8080 CLOUD_KEY=… \
CLOUD_EMAIL=tester@example.com CLOUD_PASSWORD_FILE=~/tester-pass \
EXPECT_ID=example.hello EXPECT_VERSION=1.0.0 \
  crates/copper-app/qa/addons-cloud-e2e.sh   # PNGs and logs in /tmp/copper-addons-cloud-e2e
```

`LINK_CODE=copper-cloud://…` (from `copper-cloud link-code`) can stand in for
`CLOUD_URL` and `CLOUD_KEY`. Plain http is for loopback only. Two options go
further:

- **`PANE_SCRIPT=file.js` drives the addon's own UI.** The body runs in the
  addon's page inside an async function, with `shot(name)` and `sleep(ms)` in
  scope. It returns `{ok, steps: ["…"]}`, and the steps are printed under
  its `PASS`/`FAIL` line. A throw or `ok: false` fails the run.
- **`UPDATE_TO=version` with `UPDATE_CMD='…'` tests an update.** Once the
  first version is up, the lane asks the harness to run `UPDATE_CMD` (which
  publishes the newer version). It then presses Check now and Install, and
  checks that the same view reloads in place at the new version.
  `UPDATE_PANE_SCRIPT` runs after that.

The account must be the one the cloud gives `EXPECT_ID` for the Agent sidebar
(the addon is on for them). The test world is wiped and its session signed out
when the run ends, unless `KEEP_WORLD=1`. The password and session token are
never printed, and the run fails if the token turns up in a log.

## How it is built

The rules live in the shared core, `copper_core::addons` and
`copper_cloud::addons`: the manifest, the bundle format, verification, the
store, the plan, the bridge's own methods, the `copper-addon://` answers and
the sync. The GTK side is `crates/copper-app/src/addons/`, and every call into
the core goes through one file, `core_glue.rs`.

| File | What | Mac |
|---|---|---|
| `mod.rs` | registration, Ctrl+E, the palette and menu entries | `Addons.swift` |
| `pane.rs` | the slot on screen: revealer, drag grip, placeholder | `AddonSlots.swift` (`AgentSlot`) |
| `view.rs` | the addon's web view, the bridge relay, events | `AddonHost.swift` |
| `scheme.rs` | `copper-addon://` on WebKitGTK | `AddonHost.swift` |
| `policy.rs` | navigation rules, the width | `AddonHost.swift` |
| `host.rs` | the bridge calls only Copper can answer | `AddonBridge.swift` |
| `oauth.rs` | the sign-in window | `AddonBridge.swift` |
| `settings.rs` | Settings › Extensions | `AddonsSettings.swift` |
| `updates.rs` | Settings › About and the toast | `AddonUpdates.swift` |
| `core_glue.rs` | the one door to the shared core | `CloudAddons.swift`, `AddonStore.swift` |
| `selftest.rs` | `COPPER_SELFTEST=addons` | `AddonsBench.swift` |
| `selftest_cloud.rs` | `COPPER_SELFTEST=addons-cloud` (a real cloud) | — |
