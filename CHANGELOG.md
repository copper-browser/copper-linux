# Changelog

Copper for Linux. Versions follow the `.deb` and the Flatpak; dates are
release dates.

## Unreleased

### Fixed

- **A computer can only overwrite what was changed on it (Copper Cloud
  spaces; incident 2026-10-28, cu-2th).** One device whose state merely
  lacked a filing — a copied session, folders made elsewhere — could win
  the merge against an unchanged server and unfile every tab in a folder on
  every device. Now each change made here to filing and kept tabs is
  recorded as an intent (`copper_cloud::sync::intent`, kept in
  `cloud-sync.json`), and where the base has an entry only an intent lets
  this device's copy through. The first spaces sync after launch or a
  reconnect pulls and applies the server's copy first. A publish or apply
  that would unfile or remove more than 10 kept tabs (or drop a folder that
  holds tabs, going out) is held and asked about — *Sync It* / *Undo Here*,
  or *Accept* / *Keep Mine* — through the shell's own question, so Copper
  for Windows asks too. Every spaces publish and apply is logged
  (`copper_cloud::sync`, now on by default). Same semantics as the Mac.

## 0.5.0 — 2026-10-28

Calendar peek on the Calendar pin, folder-aware dragging, and pins that
sync once per site.

### Added

- **Calendar peek.** Rest the pointer on a Google Calendar pin for half a
  second and a card beside it lists your meetings in the next 12 hours (up
  to four, soonest first), with **Join** for a Meet, Zoom, Teams or Webex
  call; all-day, cancelled and declined events are left out. It reads the
  calendars whose secret iCal addresses you add in Settings › Tabs ›
  Calendar, kept in `calendar.json` (0600) on this computer and never sent
  through Copper Cloud. The reading is the toolkit-free `copper-calendar`
  crate, for the Windows shell to draw too.

### Fixed

- **Dragging a tab in the column, as the Mac does (Copper#113).** Over
  its own place no marker shows, since letting go there moves nothing. With
  no favourites yet, a dragged row opens a *Drop here to pin* square where
  they go, so a row can still be pinned by dragging. The folder-aware drop
  placement Copper for Windows draws (`copper_shell_core::sidedrop`) and
  **Remove from Folder** (`agents::groups::take_out`) live in the shared
  crate; this window does not draw folders yet.
- **Pins sync once per site (Mac: Copper#92, #109–#111).** Pins are keyed
  once, under the first space; a pin for a site already pinned here takes
  that pin's key instead of pinning it again; every spaces merge keeps one
  pin per site (the lowest key); a second pin for a pinned site closes; and
  a pin another device re-keys keeps its page rather than closing and
  reopening.
- **Start-up reads history.json beside the session**, and visits synced
  from other devices write history once a minute instead of on every pull.

## 0.4.1 — 2026-10-19

Company sign-in in Settings › Cloud, and `copper://cloud/join` links that
open it.

### Added

- **Continue with Google in Settings › Cloud.** Paste a sign-in link
  (`copper-cloud://HOST/#sso=google`) where a link code goes: the page shows
  the host and the fingerprint, and a *Continue with …* button named the way
  the instance names its provider. The browser opens; the row becomes a
  spinner, *Finish signing in in your browser…* and **Cancel**. Signed in,
  it is kept exactly like a pairing and sync turns on with the default
  switches through the window's own sync. A self-signed instance with no
  `fp=` gets the same compare-and-trust step as a pairing code. Refusals
  are in words: *Use your @exowatt.com account*, *Nothing came back from the
  browser in 5 minutes — try again*.
- An instance connected with a link code that offers single sign-on shows
  **Continue with Google** above the email and password form; when its admin
  turned password sign-in off (`password_signin: false`), the form waits
  behind **Sign in with a password instead**.
- **`copper://cloud/join?code=…` opens Settings › Cloud** with the code
  filled in — from the command line, from a second `copper` handing it to
  the running one, from `GApplication.Open`, or typed into the address field
  (which offers *Join Copper Cloud — HOST*, never a search). A sign-in link
  starts the sign-in at once. A link for another instance than the one
  Copper is connected to never switches it: the page says *This link is for
  HOST* with Dismiss (and Disconnect and use it…, which asks first); one for
  the instance it is signed in to says *Already signed in to HOST as EMAIL*.
- The palette has **Join Copper Cloud — Link Code, Join Link or SSO
  Sign-In…** while not signed in (Settings has no search on Linux).
- The sign-in and the join-link decisions live in copper-shell-core
  (`cloud::sso`, `cloud::join`), toolkit-free, for the Windows shell to draw
  too.

### Fixed

- Settings › Cloud said *Plain HTTP on this device* for an instance with a
  publicly trusted certificate and no pin; it says *Publicly trusted
  certificate* now.

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
