# Changelog

Copper for Linux. Versions follow the `.deb` and the Flatpak; dates are
release dates.

## Unreleased

### Added

- **Tidy files your loose tabs into folders.** With three or more loose tabs
  in a space, "Tidy" stands on the line above them (also Ctrl+K › Tidy
  Tabs): one click files them into a few named folders in Saved, reusing a
  folder the space already has when it fits, and Undo (or Ctrl+Z) on the
  pill that follows puts every tab back where it was. With a model set up in
  Settings › Intelligence, the model names the folders from the tabs' titles
  and sites only, never their pages; without one, or if it takes over 8
  seconds, tabs are filed by site, two or more to a folder. A tab that fits
  nowhere stays loose, and private tabs are never filed. As on the Mac
  (copper-browser/Copper#200).
- **A morning page, once a day.** Settings › General › Morning page takes an
  address; the first launch, return to Copper or wake after 3 AM opens it as
  the active tab (or brings back the tab already showing it), and every other
  launch that day restores as it always did. Empty, the default, is off. The
  hour is `daily.hour` in `settings.json` (`"daily.hour": 4`); `daily.shown`
  keeps it to once a day (copper-shell-core `daily`). Waking is logind's
  `PrepareForSleep`, so the Flatpak may now talk to `org.freedesktop.login1`
  on the system bus. As on the Mac (copper-browser/Copper#198).
- **A light on pins with something new.** A pinned tab whose page title
  starts with an unread count, like `(1) Paper`, `(3) Inbox` or Gmail's
  `Inbox (3)`, wears a small copper dot on its square in the column and the
  tab row until you go to it. A sleeping pin keeps its dot (copper-shell-core
  `pinlight`). As on the Mac (copper-browser/Copper#199).
- **Jev finishes what it starts, faster.** A field's text goes in as one
  real edit, so a site's suggestions open; places, names, airport codes and
  dates the goal says outright (*from Zurich to London on January 20, 2027*,
  in seven languages) are typed without asking a model, and when one is
  needed a fast small one is asked first and its answer is kept for the run;
  after typing into a lone search box Jev presses Return; the page is read as
  soon as it stands still, a new page as soon as it is parsed; and a checkbox
  drawn by its label can be clicked. `jev_run` takes `private: true` for a
  tab that keeps nothing. As on the Mac (copper-browser/Copper#181).
- **Jev waits for a page's answer, and never sends a payment twice.** What a
  page asks its server (`fetch`, `XMLHttpRequest`) is now heard in the page:
  a WAIT or a click waits for those calls to come back before Jev decides
  again, and a click the page has not answered gets a second first. A form
  or a POST the page took without a word is never sent again — Jev is asked
  whether the goal is met and the run ends "submitted, no confirmation
  shown" — while a search, a retry or a Reload may go again; the same button
  clicked twice on an unchanged page becomes a WAIT, then the run stops.
  Each step's trace says what it sent and which calls it made. The Mac hears
  the calls from WebKit itself. As on the Mac (copper-browser/Copper#181).
- **Jev says Done only when sure.** A DONE or BLOCKED under 80% gets a
  second look once the page has settled in full; still doubtful, the run
  ends "unsure" ("Not sure it worked" in the chat), as does a DONE whose
  value typed from the goal the page doesn't show. A page asking "I am
  human" ends the run and is handed back, sponsored links are left out, and
  the LLM decider's retry says what was wrong. As on the Mac
  (copper-browser/Copper#181).
- **Jev reaches further and sees more of the page.** A link the goal names
  above or below the screen is offered and scrolled to on the click, four
  scrolls that find nothing new end the scrolling, a date picker shows only
  the goal's months, and Jev's read covers open shadow roots and same-origin
  frames, so it can click and type in them. The settle no longer counts a
  page writing the same value again or restyling an element every frame. As
  on the Mac (copper-browser/Copper#181).
- **Jev answers questions and gets past search panels.** A goal that asks
  something ("what is…", "tell me…") ends DONE when the answer Jev read is
  found on the page, and `jev_run json: true` returns that `answer` with the
  page's words around it (`snippet`). A control with something drawn over it
  (arXiv's search panel) is left out and Jev uses what is on top instead of
  calling the page one that "kept changing"; once the panel is closed the
  control is offered again (the Mac keeps it out after a click). As on the
  Mac (copper-browser/Copper#182).
- **An agent's click waits for a page that leaves a moment later.** A
  click whose page navigates from a short timer or once a data request
  comes back now answers on the new page (a second at most), not the old
  one. As on the Mac (copper-browser/Copper#181).

### Changed

- **Tabs and pins always show the site's own icon.** The "Tabs show: Site
  icons / Letters" choice is gone from Settings › Tabs; a site whose icon
  hasn't arrived yet shows a plain globe, never a letter, in tabs, pins,
  Ctrl+K and History. An old `letters` setting is simply not read, and the
  choice no longer syncs. As on the Mac (copper-browser/Copper#181).
- **A page that can't load says so plainly.** "Can't connect to <site>",
  why in one line (no site at that address, the site refused the
  connection, took too long, the connection isn't secure, no connection),
  a Try Again button with its key (Ctrl+R) and the engine's own words under
  Details. As on the Mac (copper-browser/Copper#181).
- **http pages say "Not secure" under the pointer.** Rest the pointer on
  the address of a page over plain http and it says so. As on the Mac
  (copper-browser/Copper#181).

### Fixed

- **An agent fills a login only on the exact address it was saved for.**
  Scheme, host and port must match: `www.example.com` and `example.com` are
  two sites unless the saved item lists both, a login saved from an `http://`
  page or another port is filled only there, and a login kept as a bare
  `localhost` or `127.0.0.1` is never filled by an agent (any server on the
  computer answers there; you can still pick it from the list yourself). The
  same holds for a custom field an agent fills. Copper now keeps the scheme
  and port of the passwords it saves, and of the rows a CSV or Chrome import
  brings. As on the Mac (copper-browser/Copper#181).
- **Agents' fills stay on the page and address they were meant for.** A
  sign-in, one-time code, card, identity or custom field an agent fills goes
  to the page the call started on, and only while that page is still of the
  same origin: a page that navigates during the call gets nothing. A row
  picked from the list under a sign-in box is held to the same rule. As on
  the Mac (copper-browser/Copper#181).
- **An agent whose tab was closed is never moved onto yours.** Until it opens
  another, it can only read the tab on screen; anything that would change a
  page is refused with "your tab was closed". `browser_close` closes only the
  agent's own tab, a tab named with `tab` that closes during a call fails the
  call, and an agent without a tab can't have Jev read yours into a model.
  As on the Mac (copper-browser/Copper#181).
- **A one-time code an agent fills is sent.** `browser_sign_in` with
  `what: otp` sends the code's own form; a code page has no password box to
  send from, so `submitted` was always false. As on the Mac
  (copper-browser/Copper#181).
- **A card or identity in the Agents folder counts as shared with agents**,
  as a login there does (a `copper-agent: deny` field still keeps it back).
  Naming an account that is not shared says where to share it. As on the Mac
  (copper-browser/Copper#181).
- **Pages can't pass their words off as Copper's.** A page's title comes back
  to an agent as one quoted line, and the text of a snapshot, a page read,
  a find or the console comes back fenced as untrusted page content, with a
  fence the page can't forge. As on the Mac (copper-browser/Copper#181).

## 0.6.4 — 2026-11-17

Agents never see a secret Copper filled.

### Fixed

- **A secret Copper filled never reads back out to an agent.** A password,
  one-time code, card number or code, or vault field Copper put into a page
  reads as `•••• (secret, hidden)` in everything an agent or a model is
  handed: `browser_snapshot`, `browser_get_text`, `browser_find`,
  `browser_evaluate`, console and network listings, Jev's and
  `jev_extract`'s reads, screenshots (the boxes are masked for the picture
  and put back after), errors, the agent pane and the driver timeline.
  Password, code and card boxes read as the placeholder even when Copper
  didn't fill them, and every secret Copper filled is also taken out of any
  text the page copies it into, for as long as the tab lives and only in
  memory (`copper-mcp` `secrets.rs`). `browser_sign_in` puts a password only
  into a real, shown password box beside the account it belongs to: it
  refuses a text box, a box holding another account, and a page that copies
  the password elsewhere (every copy is emptied). As on the Mac
  (copper-browser/Copper#188).

## 0.6.3 — 2026-11-17

Agents picture any part of a tab, in HD.

### Added

- **Screenshots of any part of a tab, in HD.** `browser_take_screenshot`
  takes `selector`, `region`, `padding` and `scale` (1–4) as on the Mac: an
  element comes out whole even when it is taller than the window, a region
  is in CSS pixels (the page's with `fullPage`), and scale 2 or more is the
  page drawn again, not enlarged, for an agent's background tab. `filename`
  may be a folder (`copper-YYYYMMDD-HHMMSS.png`) and its folders are made;
  the answer starts with what was pictured (`Screenshot 520×280 PNG at scale
  2 of #box-b.`). `copper shot` takes `--ref`, `--selector`, `--region
  X,Y,W,H`, `--padding`, `--scale` and `--hd`, and doesn't write again a
  picture Copper saved. The answer's own image stays small for the model:
  scale 1 when an HD picture went to a file, never over 8000 px a side, and
  it says so (`The image here is 240×120 at scale 1; the file has the full
  picture.`). A full page is from the page's top whatever its scroll, at
  1 px per CSS px × scale, up to 20000 px tall. (`copper-mcp`
  `screenshot.rs`, `PageView::capture`, WebKitGTK `capture.rs`.)

## 0.6.2 — 2026-11-10

Sign in with Claude in the shared crates, and picking a saved account on Google's sign-in page fills it in.

### Added

- **Sign in with Claude, in the shared crates.** The Mac's Claude account
  (`ClaudeAccount.swift`, `Claude.swift`) with no toolkit: claude.ai's
  OAuth with PKCE through a loopback listener, the paste-the-code fallback,
  a refresh five minutes before the token runs out (one at a time; a
  refresh turned away signs out) and the tokens in `claude.json` (0600).
  Signed in, the model lane is the account's: the agent pane, Jev's typing
  and Check ask Anthropic's Messages API the way Claude Code does, and
  `copper claude status|signin|paste|signout|cancel` works. Windows draws
  it in Settings › Intelligence; on Linux `copper claude signin` signs in
  for now. (`copper-mcp` `claude`, `copper-shell-core` `claude`,
  `chat/wire.rs`.)

### Fixed

- **Picking an account on Google's sign-in page fills it in.** A sign-in
  that asks for the name first (Google, Microsoft) has no password box on
  its first page, so picking a saved account said "Couldn't find the
  sign-in fields anymore". The name now goes in alone and the list comes
  back on the password page. The Mac has the same gap. (`forms/relay.js`
  `fillName`, `forms/watch.rs`.)

## 0.6.1 — 2026-10-30

The Mac's keys and small touches: fold the column, stop sound, print, full screen, space keys, Recently Closed, the Clear seam, camera choices remembered.

### Added

- **The Mac's and Windows' missing keys.** `Ctrl+S` hides or shows the
  column (for this session; the field stays over the page), `Ctrl+Shift+M`
  stops the sound in the tab, `Ctrl+Shift+U` opens Hidden on This Site,
  `Ctrl+P` prints, `F11` is full screen, `Alt+1`–`Alt+9` jump to a space and
  `Alt+N` makes one, and `Ctrl+Shift+O` opens your Personal canvas, as on
  Windows. Each shows its key in Ctrl+K, and "Switch to …" rows show their
  space's `Alt` digit. (`actions.rs`, `canvas/mod.rs`, `features/curtain.rs`.)
- **Recently Closed in the empty Ctrl+T.** The two tabs closed last, newest
  first; Return brings the first back where it was
  (`copper_shell_core::palette::complete`, `Browser::reopen_closed_at`).
- **"↓ Clear" on the seam (Copper#96).** Hovering the hairline between
  Saved and Today shows a small round button at its end that clears the
  space's loose tabs; at rest it is the plain hairline, same height.
- **Camera and microphone answers are remembered per site**, as on the Mac:
  a site you allowed or refused is not asked again (never in a private
  tab). Settings › Privacy › Permissions › Camera and microphone has
  **Forget choices** (`copper_shell_core::permit`, the same `capture.` keys
  as Copper for Windows).

### Changed

- **`Alt+1`–`Alt+9` pick a space, not a tab** (`Ctrl+1`–`Ctrl+9` still pick
  tabs), and **`Ctrl+Shift+O` is the Personal canvas**: Bookmarks is in
  Ctrl+K, as on the Mac and Windows.
- **Toasts stay as long as they take to read**: 2–6 s by their length (the
  Mac's 1.7–6 s rule, `copper_shell_core::view::toast_time`, in whole
  seconds), instead of always 2 s.

## 0.6.0 — 2026-10-30

Catching up with the Mac: folders in the column, Ctrl+Tab in most-recent order, Archive Today, Settings › Intelligence, two-step codes that fill themselves, Jev's Fast mode, and Clear History that reaches Copper Cloud.

### Added

- **Ctrl+Tab in most-recent order works.** With Settings › Tabs ›
  Ctrl+Tab switches to › "The tab you were on before", Ctrl+Tab goes to
  the tab you used last; keep Ctrl held and each Tab goes one further back,
  and the order only changes when you let go, as on the Mac. Ctrl+Page
  Up/Down still walk the row. (shell-core `recent.rs`, `Shell::switch_tab`,
  `tab_cycle_ended`; `window/mod.rs`, `actions.rs`.)
- **Archive Today after 12h / 24h / 48h / Never** (Settings › Tabs › Tabs
  you leave). Today rows nobody has looked at for that long close
  themselves and wait in Reopen Closed Tab — never the tab on screen, a
  pin, a Saved row or a canvas. Swept at launch, every half hour, and when
  you switch to a space. Same setting as the Mac's, synced by Copper Cloud.
  (shell-core `archive.rs`, `Shell::archive_today`.)
- **Zoom is remembered per site.** Ctrl+= on a site, and its pages open at
  that size from then on; Ctrl+0 forgets it. Kept as the Mac keeps it
  (`zoom.<host>`), never from a private tab. (`Shell::zoom`, `pages.rs`.)
- **Settings › Cloud › Delete history on cloud.** Pick a time range and,
  if you like, one site; Copper asks before deleting it from Copper Cloud
  for every device, as the Mac does. History here stays. The card's words
  and choices are shared (`copper_shell_core::cloud::eraser`), so Copper
  for Windows can draw the same card from them. (`cloud/page.rs`;
  docs/cloud.md.)
- **Settings reopens at the page you were last on** (`settings.page`, the
  Mac's page names; a page Linux doesn't have opens General).
- **Search in Settings.** The search button finds rows by their titles.
- **The command palette offers "Install <addon> <version> for the Agent
  sidebar"** while an addon update waits, as the Mac's does.
- **Folders in the column, as the Mac draws them (GroupsUI).** Saved and
  Today show their folders — yours and the ones synced from the Mac through
  Copper Cloud — as a line with the folder in its colour and its name, its
  tabs stepped in under it, folders inside folders stepped in again. Click a
  folder's line (or press Return) to shut or open it; that stays as you left
  it on this computer. Drag a row into a folder, out of one, across the seam
  or onto the favourites: the line shows where it will land, stepped in when
  it lands in a folder. **Remove from Folder** is on a filed row's menu.
  Renaming or ungrouping a folder is not here yet. (`window/column.rs`,
  `window/tabs.rs`; the flat `window/dropzone.rs` gives way to
  `copper_shell_core::sidedrop`.)
- **Settings › Intelligence (Copper#115, #118).** Between Extensions and
  Agents, as on the Mac: the gateway's API key and address, the model size
  (Haiku, Sonnet, Opus) with the name it asks for, Model names under
  Advanced, **Check** (one small question to Jev and one to the gateway,
  answered in plain words), and the Jev key with **Get a key**
  (console.typesafe.ai). Keys Copper Cloud provides say so and their boxes
  fade. No Claude account sign-in yet, so there is no Use chooser: the API
  key is the lane. Agents › Jev mode's key row now points here with **Add
  key**, and its Advanced holds the **Text model**. Everything is still
  kept in `intelligence.json`. The Check's words live in copper-mcp
  (`intelligence::check`), for Windows to share. (`features/settings/
  intelligence.rs`, `agents/settings.rs`; docs/agents.md.)
- **Jev runs in Fast mode, as on the Mac.** Unless `jevFast` is `false` in
  `intelligence.json`, Jev types the values your goal quotes without asking a
  model, fills a whole form in one go, and the pointer glides only when you
  can see the tab — about twice the actions a minute. `jev_run` and
  `jev_step` take `strategy` (fast, literal, burst, lean, paste, watch, turbo,
  classic, llm) and `model`; `jev_run` takes `json` for a timing trace; and
  `copper run` has `--strategy`, `--model` and `--json-run`. What Jev sees
  (pictures) is not here yet.
- **Two-step codes fill themselves.** Right after you sign in by picking an
  account that has a code (Bitwarden or 1Password) from the list, the code
  page fills itself: once per pick, within ten minutes, only on that
  account's site and only into a box that is clearly a code box. Copper
  never presses the button. Settings › Passwords › **Fill two-step codes**
  turns it off, as on the Mac. (`forms/watch.rs`, `forms/relay.js`.)
- **Settings › Passwords has the Mac's other switches.** **Fill addresses
  and cards** turns the identity and card lists off, and Bitwarden has
  **Auto-lock** (5, 15 or 60 minutes, or Never). Bitwarden and 1Password
  now stay unlocked between launches until you turn it off, as on the Mac.
- **Passwords in and out as CSV.** Settings › Passwords › Import and export
  takes a passwords CSV from Chrome, Firefox, Safari or a password manager
  (**Choose File…**) into the keyring and says what happened, and
  **Export…** writes every keyring password to a CSV readable by you only,
  after a confirmation. (`forms/csv.rs`.)
- **Add and copy saved passwords.** Saved passwords has **Add a password**
  (site, username, password) and a copy button on each row.
- **Jev can sign in and fill cards and addresses.** On a page with a
  password field and an account shared with agents, Jev's controls include
  SIGN_IN; with card or address fields and a shared card or identity,
  AUTOFILL_CARD and AUTOFILL_IDENTITY — as on the Mac. The secret never
  reaches the model. (`forms::mcp::jev_extras`.)
- **`copper intelligence status|sources|refresh|set|reload`.** The model
  keys in the running Copper from the command line, as JSON and never a key;
  `set` writes `intelligence.json`, and `-` reads a value from stdin. On
  Windows too. (docs/agents.md.)
- **Let it back in.** After Stop, the capsule over the page offers **Let it
  back in** for the 30 seconds an agent's calls are refused; pressing it
  accepts them again now, as the Mac's driver card does.
- **`canvas_list` names your invitations.** Canvases you've been invited to
  and haven't answered come back as `invitations` (`{canvas, from}`).
- **Open the Chrome guide.** When tabs or bookmarks come over from Chrome,
  the Bring In summary offers the "Switching from Chrome" canvas, as the
  Mac's move does.

### Changed

- **Addons: an unknown bridge method, or `voice.*` here, answers
  `bad_request`** (`No bridge method "…"`), the Mac's code and words, not
  `unavailable`. Addons still leave their mic out. (docs/addons.md.)

### Fixed

- **Clearing history, or removing a page from History, now reaches Copper
  Cloud.** Docs said it did; only the Mac and Windows did. While history
  syncs, Clear History deletes it on the cloud too and its question says
  so, and a page removed in the History panel goes from the cloud as well.

## 0.5.2 — 2026-10-30

Settings has the Mac's pages, in the Mac's order, and is shorter.

### Changed

- **Settings has the Mac's pages, in the Mac's order (Copper#115–#118).**
  The rail reads General, Tabs, Extensions, Agents, Passwords, Privacy,
  Cloud, About (Spaces and Intelligence take their places if Linux ever has
  them). Appearance is no longer a page: its Light/Dark/System row is on
  General › Copper on this computer, and Left/Top is the first row of a new
  **Layout** group on Tabs, with Tabs show. Addons' page is called
  **Extensions**, and Updates is **About**, whose group is now called
  Updates. Agents is shorter: "The key" is gone — the port and bearer token
  fold under **Advanced** at the foot of the MCP server group, whose
  description is the "Agents act as you" note (and this session's tool
  calls); the two Copy config rows are one **Copy for another agent** row
  with **Copy config** and **Copy stdio config** side by side; and the Jev
  key row shows only while Jev mode is on and the key is missing. Passwords
  › Agent access lists accounts only while sharing everything is off.
  Ctrl+Tab and the agent rows take the Mac's shorter subtitles.
  (`window/settings.rs`, `features/settings/`, `features/updates.rs`,
  `agents/settings.rs`, `forms/settings.rs`; docs/agents.md, docs/addons.md.)
- **Addons' bridge has `copper.voice` (Copper#119).** There is no native
  voice on Linux, so `voice.*` answers "unknown method" and addons hide
  their mic.

## 0.5.1 — 2026-10-29

Copper Cloud: a computer can only overwrite what was changed on it, and a
sync that would unfile or remove many kept tabs asks first.

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
