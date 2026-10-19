# Changelog

Copper for Linux. Versions follow the `.deb` and the Flatpak; dates are
release dates.

## 0.4.0 — 2026-10-19

Addons: the Agent sidebar, filled by your Copper Cloud.

### Added

- **Agent sidebar (Ctrl+E).** A pane on the right of the window, full height,
  340–620 px wide (`agent.paneWidth`, as on the Mac). It keeps running when
  hidden. With no addon it says so and points to Settings › Addons.
- **Addons from Copper Cloud.** Copper asks the linked cloud which addon fills
  each slot (`/v1/addons`) at launch, on link or account changes, every six
  hours, on Settings › Updates › Check now and on the cloud's event stream. A
  new addon or version is downloaded, checked against its SHA-256 and staged;
  Settings › Updates and a toast say "Update available — NAME VERSION for the
  Agent sidebar", and **Install** loads it without a relaunch.
- **Settings › Addons** lists installed addons, turns one off on this computer,
  removes one, and checks for updates. It replaces the page that said
  extensions aren't available.
- **`copper-addon://`**: addons run from their verified bundle in a view of
  their own, with a strict content policy, their own storage, no off-origin
  navigation, and the bridge v1 API (`copper.info`, `copper.identity`,
  `copper.settings`, `copper.storage`, `copper.fetch` for the hosts their
  manifest names, `copper.openUrl`, `copper.page.current`,
  `copper.oauth.authorize` in a sign-in window, `copper.ui.close`).
- `COPPER_ADDONS_DEV_DIR` runs an addon folder directly in a test world, and
  `COPPER_SELFTEST=addons` checks the whole path. See
  [docs/addons.md](docs/addons.md).

## 0.3.0 — 2026-10-19

Copper Cloud single sign-on, and Copper as an agent's own browser with no
display.

### Added

- **`copper cloud pair CODE`** pairs this profile with Copper Cloud while
  Copper is not running: it links, signs in, adopts the device id the server
  answers with, and writes `cloud.json` and the sync switches, so the browser
  syncs as soon as it starts. `--sync none|all|tabs,bookmarks,…`,
  `--name NAME`, `--trust FP` and `--json`
  (`{"ok":true,"email","device_id",…}`). It refuses while a Copper window owns
  the profile, and never prints the session token or a key.
- **`copper cloud sso [CODE]`** signs in with your company account in the
  system browser: a loopback redirect on `127.0.0.1` with PKCE (S256) and a
  state check, a 5-minute wait, and plain-words errors (`sso_domain` → "Use
  your @example.com account"). Sync turns on as with pairing.
- **`copper cloud status`** shows the link, the account, this device, the
  sync switches, and where the model keys come from, without a key.
- Copper Cloud **sign-in links** (`copper-cloud://HOST/#sso=google`, `fp=`
  optional) and **deep links** (`copper://cloud/join?code=…`) are recognised
  wherever a code is read. The desktop entry registers
  `x-scheme-handler/copper`.
- **`COPPER_ROUTER_URL`, `COPPER_ROUTER_KEY`, `COPPER_ROUTER_MODEL`** name the
  OpenAI-compatible router for this run, beating both the cloud's and
  `intelligence.json`'s (reported as source `env`). Unset, nothing changes.
- [docs/headless.md](docs/headless.md): Copper with no display (headless
  weston with pixman, a session D-Bus, the WebKit sandbox in containers), the
  `agent.json` schema, pairing before the first start, and the environment.

### Changed

- A link or pairing code **without `fp=`** is now checked against the public
  certificate authorities (Mozilla's list, built in) with the host name, as
  on the Mac and as docs/cloud.md already said. Before, Linux accepted no
  certificate without a pin. A code with `fp=` is still pinned to exactly
  that certificate, and a self-signed server without one is refused with its
  fingerprint shown.
- Pairing (and single sign-on) adopts the device id in the server's answer,
  so a code minted for a particular device signs in as that device.

### Not yet

- Settings › Cloud has no *Continue with Google* button yet, and opening a
  `copper://cloud/join` link does not open Settings › Cloud yet; use
  `copper cloud sso` and `copper cloud pair`.

## 0.2.0 — 2026-10-16

Big canvases are fast, agents can read them a page at a time, and tabs are
easier to keep: on-screen-only canvas drawing, paged `canvas_read`, Ctrl+T
sections (Open Tabs, History, Search), and Arc-style Saved tabs.

## 0.1.1 — 2026-10-13

The installer refreshes apt package lists on fresh machines.

## 0.1.0 — 2026-10-13

First Linux release.
