# Agents: driving Copper from Claude Code, phi, Cursor and Claude Desktop

Copper is an MCP server. When you turn it on, an agent can work in the window
you have open, with your tabs and your sign-ins. The tools use the same names
as Playwright MCP, so a skill written for Playwright MCP works here unchanged.
They are also the same tools Copper on the Mac offers, so a client config that
works with the Mac works here with only the token changed.

## Turn it on

Go to Settings › Agents and switch on **Let agents drive this window**.

- Copper starts a Streamable-HTTP MCP endpoint at
  `http://127.0.0.1:4123/mcp`. It listens on loopback only.
- Every request must carry a bearer token. The token lives in `agent.json`,
  readable by you alone (0600). For the `.deb` that is
  `~/.local/share/copper/agent.json`; for the Flatpak,
  `~/.var/app/com.copperbrowser.Copper/data/copper/agent.json`.
- A request with a foreign `Origin` header is refused, so a page open in
  another browser can't reach in.
- If another program already uses port 4123, change the port on the same
  page, under the MCP server's **Advanced** row (with the token).

## Connect a client

### Claude Code, phi, Cursor (HTTP)

Settings › Agents › *Copy for another agent* › **Copy config**. Paste it
into `~/.claude.json`, `~/.pi/agent/mcp.json`, or your editor's MCP settings:

```json
{
  "mcpServers": {
    "copper": {
      "type": "http",
      "url": "http://127.0.0.1:4123/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

For Claude Code there is also a one-liner:

```sh
claude mcp add --transport http copper http://127.0.0.1:4123/mcp --header "Authorization: Bearer <token>"
```

### Claude Desktop and other stdio-only clients

Use **Copy stdio config**, beside it. It runs `copper --mcp-stdio`, a pipe to the
running window that starts Copper if it isn't running:

```json
{
  "mcpServers": {
    "copper": { "type": "stdio", "command": "/usr/bin/copper", "args": ["--mcp-stdio"] }
  }
}
```

With the Flatpak, run it through `flatpak`:

```json
{
  "mcpServers": {
    "copper": {
      "type": "stdio",
      "command": "flatpak",
      "args": ["run", "com.copperbrowser.Copper", "--mcp-stdio"]
    }
  }
}
```

### Rotating the token

**Rotate token…** (Settings › Agents › MCP server › Advanced) makes a new token. Every client config goes stale at once,
on purpose: set each client up again with the new one.

## Tools

The tool names and argument shapes are Playwright MCP's. `ref`s come from
`browser_snapshot` and stay valid until the page changes.

**Which tab.** Each page tool works on the caller's own tab: the one it
opened with `browser_tabs new`, or chose with `browser_tabs select`. Without
one, it works on the tab you are looking at. Any call can name another tab
with `tab`, either an id from `browser_tabs list` or an index. A JSON number
is an index; so is a string of digits, unless it can only be an id: eight
digits or more, a leading zero (about one short id in fifty is all digits,
`00000012` among them), or a number past the end of the row that begins
exactly one tab's id.

An agent's tab that you aren't looking at is still worked on properly: it is
kept off screen, where pages still paint, `requestAnimationFrame` runs,
screenshots are real and clicks arrive as trusted events. Your selection and
keyboard focus don't move. If you pick the tab yourself it moves on screen.
Five minutes after the agent's last call it becomes an ordinary background
tab again, and may sleep.

| tool | does |
|---|---|
| `browser_tabs` | `list`, `new {url}`, `close {index\|tab}`, `select {index\|tab}` (+ `focus`). `new` opens a background tab of the caller's own. Neither `new` nor `select` switches your tab unless `focus: true`. `close` with nothing named closes only the caller's own tab; on a Saved tab it only puts it to sleep; it stays saved. |
| `browser_navigate`, `browser_navigate_back`, `browser_navigate_forward` | Moves the caller's tab, then waits for the load to settle. |
| `browser_snapshot` | Accessibility tree with `[ref=e12]` on interactive nodes. `interactive: true` gives a smaller tree; `selector` gives a subtree. |
| `browser_click` | Clicks by `ref` or `selector`, at the element's centre, as a real input event. Takes `doubleClick`, `button` and `modifiers`. |
| `browser_mouse_click_xy` | Clicks a point, `x` and `y` in CSS pixels from the viewport's top-left (as in a screenshot), with `button` and `doubleClick`: for canvases and maps. The answer names the element there (`canvas#c "Drawing board"`); a point off the viewport is an error. |
| `browser_type` | Types `text`; `submit` presses Enter after, `slowly` types key by key. The value is set through the native setter, so React and Vue notice. |
| `browser_fill_form` | `[{ref, type, value}]` for textbox, checkbox, radio, combobox and slider. |
| `browser_press_key` | `Enter`, `Escape`, `ArrowDown`, `Control+a`, `a` and so on, as real key events. |
| `browser_hover`, `browser_drag`, `browser_select_option`, `browser_scroll` | Hover, drag, choose an option, scroll. |
| `browser_handle_dialog` | `accept`, `promptText` (Playwright's). An alert, confirm or prompt on an agent's tab stays open after the call that raised it and is listed under `### Dialogs` in that answer; this answers it. With none open it arms the answer for the next one. The agent's next other call on that tab, or 30 s, closes it as a closed dialog would (OK, false, null); one raised while Copper's own script waits on the page (inside `browser_evaluate`) gets that answer at once. "Leave this page?" is answered at once: leave, unless a dismiss was armed. A tab on your screen keeps its dialog for you unless an answer was armed. |
| `browser_file_upload` | `paths: [absolute]` answers the file chooser your click opened; before the click it arms the next one; no `paths` cancels; with `ref` it drops the files on that element, as from a file manager, for a drop zone with no file input ([Files and browser state](#files-and-browser-state)). |
| `browser_pdf_save` | The page as a paged PDF (letter); `filename` is absolute, or a name in the downloads folder. |
| `browser_network_requests` | `[GET] url => [200] (fetch)` per request since the page loaded; `includeStatic`, `limit` (at most 300). |
| `browser_cookies` | Your tab's site's cookies: `list {name}`, `set {name, value, domain, path, expires, httpOnly, secure, sameSite}`, `delete {name}`. HttpOnly needs Agent access. |
| `browser_permissions` | `grant` / `revoke` lists of `geolocation`, `camera`, `microphone` for a `site` (default your tab's); none lists them. Camera and microphone are the same per-site Allow the page's question keeps; geolocation is kept in `permissions.geolocation`. The position pages get is the one given with `latitude` / `longitude` (as Playwright's `setGeolocation`); a geolocation grant without them is refused with what to pass. `notifications` is always refused. A site not granted is asked as before (refused on an agent's tab nobody sees). |
| `browser_take_screenshot` | PNG or JPEG of the viewport, the whole page (`fullPage`), one element (`ref` or `selector`, whole even when taller than the window) or a `region` in CSS px, with `padding` round it, at `scale` 1–4 (2 is HD), saved to `filename` if you give one. See [Screenshots](#screenshots). |
| `browser_evaluate` | Runs `function: "() => …"`, or `(element) => …` with a `ref`. |
| `browser_wait_for` | Waits for `text`, for `textGone`, or for `time`. |
| `browser_get_text`, `browser_find`, `browser_console_messages` | Read without a snapshot: page text, refs for text you name, console messages. |
| `browser_resize`, `browser_close` | Resize the window; close the caller's own tab (refused when it has none: name one with `tab`). |
| `browser_groups` | Copper's tab groups: `list`, `assign {group}`, `remove`. |
| `browser_perf_probe` | Samples the tab for frame loops, DOM churn, animations, filters, canvases, timers, long tasks and slow resources. |
| `browser_crashes` | Copper's own crash history on this computer, newest first. |
| `browser_history` | Searches your history on this computer, newest first: `query`, `limit` (20, at most 200). Not for linked apps. |
| `browser_bookmarks` | `list` (with the folders each is in), `search {query}`, `add {url, title}` (default the caller's tab's page). |
| `browser_spaces` | `list` (which is on screen, which holds the caller's tab), or `move {space}` the caller's own tab into another space: not a pinned tab, not one on your screen; its calls still land on it. |
| `browser_downloads` | What the caller's tab (or a popup it opened) downloaded this session: name, state, bytes and where the file is; `wait` (seconds, at most 60) for running ones. `copper downloads [--wait SECONDS]`. |
| `browser_handoff` | `message`, `timeout` (seconds, default 15, at most 600): a bar at the bottom of the window that holds the agent's tab, "<agent> needs you on <tab>: <message>", with Show, Done and Not now. Nothing moves until you click Show. Done answers the call, Not now is an error, and the timeout answers "no answer yet" with the bar still up, so calling again keeps waiting. |
| `browser_sign_in`, `browser_autofill` | Fill a saved sign-in, card, identity or custom field without the secret ever leaving Copper (below). |
| `browser_vault_save`, `browser_vault_note` | Keep a login or a secure note (backup codes) in the open Bitwarden vault; values never come back ([Saving to the vault](#saving-to-the-vault)). |
| `cloud_history_delete` | Deletes your history on Copper Cloud ([cloud.md](cloud.md)). History on this computer stays. Loopback clients only. |
| `canvas_*` | Until you have a canvas, the tool list offers only `canvas_create` and `canvas_join` (every canvas tool still answers by name); `canvas_apply`'s op shapes come with your first `canvas_read`. Read and draw on your canvases: `canvas_list` (with the `invitations` waiting for your answer, `{canvas, from}`, when signed in to Copper Cloud), `canvas_open`, `canvas_read`, `canvas_apply`, `canvas_select`, `canvas_focus`, `canvas_create`, `canvas_invite`, `canvas_share_link`, `canvas_join`, `canvas_screenshot`, `canvas_chat`. A big canvas reads in pages: `canvas_read` with `overview` first, then `frame` or `region`, or follow `next` as `cursor`; `ids`, `types` and `query` narrow any read. A canvas that is closed, asleep or still loading is opened behind your tab and waited for (up to a minute for a huge one). |

### Files and browser state

Each of these works on the caller's tab only.

- **Upload.** A chooser your call opened on your tab is held for you (not shown
  to anyone); the call's answer ends with "The page opened a file chooser", and
  `browser_file_upload {paths}` answers it. Unanswered, it is cancelled after a
  minute. Calling first arms the tab: its next chooser (within two minutes)
  gets the files. Every path must be absolute and exist; more than one file
  only for `multiple`. A chooser you open on a tab no agent is at is your own
  file dialog, as before.
- **PDF.** WebKit prints the page to letter-sized pages, with no dialog.
- **Network.** WebKit reports every request with its method and status, from
  the first time an agent uses the tab; nothing runs in the page. The list
  lives with the page, so a closed tab's never stands in for a new one's.
  Successful images, scripts and styles are left out unless `includeStatic`.
- **Cookies.** The tab's own cookie jar (its space's profile), cookies whose
  domain is the tab's host or a parent of it. `set` takes the host or a parent
  domain, never a bare top-level domain. HttpOnly cookies are the site's
  sign-in: their values are listed as hidden, and setting or deleting one is
  refused, unless the site is shared in Settings › Passwords › Agent access
  (the same gate as `browser_sign_in`).

### Screenshots

`browser_take_screenshot` pictures any part of a tab, in CSS pixels: at
scale 1 a picture pixel is a CSS pixel, so a point read off it can be
clicked. A `region` is measured from the viewport's top-left, or from the
page's with `fullPage`. `scale` 2 or more draws the page again with that many
pixels per CSS pixel, so text stays sharp; a picture is kept under 16384 px a
side and 64 megapixels by lowering the scale, and the answer says so.
`filename` may be a folder: the picture goes in as
`copper-YYYYMMDD-HHMMSS.png` (`-2`, `-3`… for more in the same second). The
answer starts with what was pictured:
`Screenshot 840×440 PNG at scale 2 of the region 100,100 420×220.` The
image in the answer is for the model to look at: when an HD picture went to
a file it is the same area at scale 1, and it is never more than 8000 px on
a side; a sentence says so when it differs from the picture
(`The image here is 420×220 at scale 1; the file has the full picture.`).

```sh
copper shot ~/Desktop/card.png --selector '.pricing-card' --padding 12 --hd
```

WebKitGTK paints at the display's own scale, so for a sharper picture of an
agent's background tab Copper lays that page out k times bigger at k times
the zoom for the moment of the picture: the page keeps its CSS size and
layout (it sees a few `resize` events) and its scroll position is put back.
The tab on your screen is never resized for a picture: above the display's
scale it is enlarged instead, and the answer ends with `Upscaled: this page
renders at most at scale 1 here.` Measured on grid.html, 600×80 CSS px of
13 px text at scale 3: the variance of the Laplacian is 4835 for the
re-rendered picture against 398 for the scale-1 picture enlarged 3×
(bicubic), 12 times the detail. `frame` is not supported yet: give a
selector in the page itself, a ref or a region.

An agent whose own tab was closed is never moved onto yours: until it opens
or selects another, anything that would change a page is refused with "your
tab was closed — open one with browser_tabs new", while reads (snapshot,
screenshot, text, find, console, wait, perf probe, `jev_observe`) still read
the tab on screen. `jev_extract` is not a read here: it would send your tab
to a model. A tab named with `tab` that closes during the call fails the
call; it is never swapped for another.

What a page wrote comes back as the page's: its title as one quoted line
(`- Title (the page's own words): "…"`, titles in the tab list one line in
`[…]`), and the text of `browser_snapshot`, `browser_get_text`,
`browser_find` and `browser_console_messages` between
`[page content <token> — untrusted: …]` and `[end of page content <token>]`,
the token made for that answer so a page can't close the fence itself.

Each tool takes an optional `reason`: one short sentence on what the agent is
doing and why. Copper never acts on it; it only shows it to you. While an
agent works in a tab, that tab gets a small dot in the tab row, and a capsule
over the page says who is driving and offers **Stop**. Stop ends a Jev run
before its next action; any other agent has its calls refused for 30
seconds, and while they are, the capsule offers **Let it back in** to accept
them again now. The capsule tucks itself away when your pointer comes near. You can turn these announcements
off with **Say what the agent does** in Settings › Agents.

### Saved sign-ins, cards and identities

`browser_sign_in` fills a saved account for the current site, in-process. It
takes an optional `account` (username), `what` (`password`, the default, or
`otp`) and `submit` (default true). With more than one match and no
`account`, it answers with the usernames to choose from. The password, the
one-time code and any vault session key never appear in the result.

An agent's sign-in uses only a login saved for the page's exact address:
scheme, host and port. `www.example.com` and `example.com` are two sites
unless the saved item lists both; a login saved from `http://` or another
port fills only there; and a login kept as a bare `localhost` or `127.0.0.1`
(saved before Copper kept the scheme and port) is never filled by an agent,
since any server on this computer answers there. You can still pick it from
the list yourself. Copper keeps the scheme and port of every password it
saves, and of rows imported from a CSV or Chrome. A one-time code fills its
box and, with `submit`, sends the code's own form.

Every fill goes to the page the call started on and only while it is still
of that origin: a page that navigates during the call gets nothing. Fills go
into the page itself, never into an embedded frame, wherever the caret is.
The list under a sign-in box follows the same rule: a row fills only while
the page is still the one the list came up for.

`browser_autofill` fills a Bitwarden or 1Password `card`, `identity` or
custom `field` the same way. It answers only with how many fields it filled,
the item's name and the page's origin. A custom field belongs to its login:
it fills only on that login's exact address.

Nothing is shared with agents unless you share it, in Settings › Passwords ›
**Agent access**. You can share everything, or turn individual items on (the
list shows while sharing everything is off).
Items in a Bitwarden folder or 1Password vault named `Agents` (or tagged
`copper-agent` in 1Password) are shared automatically: logins, cards and
identities alike. A custom
field `copper-agent: deny` means never, even when sharing everything. Private
tabs are always refused. Agents can't change any of this.

### Saving to the vault

An agent that was given a login (a password from a text message, a two-step
seed) or backup codes keeps them in the Bitwarden vault Copper has open:

```sh
echo '{"url":"https://example.com","username":"me@x.com","password":"…","totp":"otpauth://…"}' | copper bitwarden save -
echo '{"name":"Google backup codes (me@x.com)","text":"1234 5678\n…"}' | copper bitwarden note -
```

`save` takes `{url | host, username, password, totp?, notes?, name?, folder?}`;
the same site and username already saved changes that login's password (and
`totp`/`notes` when given) instead of adding a twin. `note` takes `{name, text,
folder?}` and makes a Secure Note, or replaces the text of the note with exactly
that name. MCP: `browser_vault_save` and `browser_vault_note`, same arguments.
Both answer `{ok, id, created, updated, name, folder}` and never a value; an
error line has every value sent cut out. Values come only in the JSON (stdin
for the CLI), never as arguments. The vault must be unlocked. While sharing
everything is off, new items go to the `Agents` folder (made if missing), so
`copper signin` uses a new login at once; an existing item changes only when
it is shared with agents.

### BotPass

[BotPass](https://github.com/pprasha/botpass) is a password source of its
own. Copper pairs with a BotPass server once, as a *device* of one BotPass
bot, and from then on `browser_sign_in` (and the list under a sign-in box)
offers that bot's accounts on the sites BotPass allows:

- **Grants decide.** BotPass is asked for the accounts the bot may `use` on
  the page's origin (`GET /v1/fill/accounts?origin=…`, usernames only). The
  password or one-time code is fetched only when it is filled, for the
  origin the page is on at that moment (`POST /v1/fill`), and BotPass checks
  the grant again and records the use in its audit log. A look-alike or
  phishing origin is offered nothing and given nothing. Grant with
  `bp grant <bot> <item> --can use` (add `--host`/`--domain` for sites other
  than the item's own URLs).
- **Shared with agents by BotPass, not Copper.** BotPass accounts arrive
  already allowed (`AgentHint::Allow`); they don't appear in Agent access,
  and the result's `source` is `botpass`. The agent never sees the secret,
  only that the sign-in happened.
- **Words, not codes.** A locked server answers *BotPass is locked — run bp
  unlock*; a removed device *Copper isn't paired with BotPass (or BotPass
  removed this device) — run bp connect copper*; a refused grant says which
  host BotPass refused.
- **Passkeys are out of scope** (WebKitGTK has no WebAuthn), and Copper never
  saves new passwords into BotPass.

Pairing is one command on a machine with `bp` signed in:

```sh
bp connect copper            # makes the bot and a code, then runs:
copper botpass pair --server https://botpass.lan:7443 \
  --fingerprint <sha256 of BotPass's certificate> --code ABCD-EFGH [--name "Copper on laptop"]
```

`copper botpass pair` also takes a `botpass://host:port/#c=CODE&fp=FP`
pairing link, `copper botpass status [--json]` says whether BotPass answers
and is unlocked (exit 0) or not (exit 1), and `copper botpass unpair` forgets
the pairing here (remove the device in BotPass to revoke its token). The same
pairing is in Settings › Passwords › **BotPass** (server, fingerprint, code →
**Pair**; then the server, the bot, locked or unlocked, **Unpair**).

The pairing lives in `botpass.json` in Copper's data folder
(`~/.local/share/copper/botpass.json`, `copper-<world>` with `COPPER_WORLD`;
`%LOCALAPPDATA%\Copper\Data\botpass.json` on Windows), written atomically and
0600: the server URL, the certificate fingerprint it is pinned to, the device
token, and the device and bot names. A running Copper picks a new pairing up
on its next fill; no restart. Plain `http://` is accepted only for a BotPass
on this machine.

## Jev mode: hand over a goal

Settings › Agents › **Let the agent hand Copper a goal** adds four tools, and
the server's instructions tell the agent to reach for them first:

| tool | does |
|---|---|
| `jev_run` | Takes a `goal` (+ `url`, `newTab`, `focus`, `private`, `tab`, `maxSteps`, `elements`, `strategy`, `model`, `json`). Runs the [jev-ultrafast](https://github.com/browser-use/jev-ultrafast) loop in the caller's tab until DONE, BLOCKED, or the budget runs out. Answers with the trace, the page and the indexed element table. `newTab` + `private` opens a private tab behind the user's (nothing kept). |
| `jev_step` | One decision and its action; calling again with the same `goal` continues the session (`strategy` and `model` are read when it starts). |
| `jev_observe` | What Jev sees: every visible control as `[3] combobox  Where to? · London`, plus the visible text. |
| `jev_extract` | Takes an `instruction` (+ `schema`, `full`) and returns one JSON object drawn from the page. |

Jev needs a Jev key (`ts-…`, made at https://console.typesafe.ai). Paste it
in **Settings › Intelligence › Jev** (Get a key opens that page), or let your
Copper Cloud provide one; while Jev mode is on and the key is missing,
Settings › Agents has an Add key button that goes there. Typing and
`jev_extract` also need a model behind an OpenAI-compatible gateway: its API
key, address, size (Haiku, Sonnet, Opus) and, under Advanced, the names your
gateway uses are on the same page, with **Check**, which asks Jev and the
gateway one small question each and says in plain words what came back. A
key or address Copper Cloud provides says so, and its box fades. Agents ›
Jev mode › Advanced names a smaller **Text model** for what Jev types (empty
uses the chosen size). All of it is kept in `intelligence.json` (0600) in
Copper's data folder, the same file the Mac uses, and is used from the next
call on. A headless Copper can name its gateway with `COPPER_ROUTER_URL` and
`COPPER_ROUTER_KEY` instead ([headless.md](headless.md#model-keys)). A
Claude account is the other lane: `copper claude signin` opens claude.ai in
a tab, and once signed in the agent and Jev's typing ask the account
(`copper claude --help`; Settings has no button for it on Linux yet).
DONE is the model's own claim, so check the page.

### Faster runs (`strategy`)

*Fast mode* is on by default (`jevFast` in `intelligence.json`; set it to
`false` for the original loop): a run takes `turbo` and `submit`, with the
pointer glide kept for a tab that is on the user's screen. `jev_run` /
`jev_step` take `strategy`, comma-separated speed-ups, to choose for one run
(`copper run --strategy …`):

| word | what changes |
|---|---|
| `fast` | no cosmetic pointer glide before a click; the trail still draws |
| `literal` | each value the goal names — what it quotes (`"maya@northwind.test"`, “…”, „…“, «…», '…'), and the places, names, airport codes and dates it says outright, in any case and in seven languages (*from zurich to london on january 20, 2027*, *ZRH → LHR on 2027-01-20*, *von Zürich nach London am 20. Januar 2027*) — becomes one Jev question in the same request — *which field holds this?* — and a TYPE_TEXT types the value Jev matched. No text-model call; a field no named value fits still asks the text model |
| `burst` | `literal`, and every other value Jev matched surely (≥ 60%) is typed in the same breath, before the next read: a sign-in form is one decision, not three. Never into or after a combobox or search box: its list of suggestions is the next read |
| `lean` | the state Jev reads is trimmed (page text to 1,200 characters, five recent actions; a date picker shows only the goal's months) |
| `paste` | a field's value goes in as one trusted edit (select all, insert text — the page hears it as typing, so an autocomplete opens) and is read back, instead of a key event per character |
| `watch` | WAIT ends when the page has answered: the data calls the last action started are back and quiet for 400 ms, or what Jev reads changed (at least 300 ms, 1.2 s at most); a click the page has not answered yet gets up to a second to show something before Jev is asked again; the same control again after it changed nothing waits 1 s, then 2 s, first |
| `near` | the settle after an input ends on the first frame with nothing changing and no animation running on screen, 400 ms at most. Fewer decisions are thrown away as stale: the run's first read and the read after a stale decision wait for the page to stand still, typing checks its field the way a click does, a scroll only that the page hasn't moved, and a WAIT the page already answered counts as done |
| `reach` | the links the goal names that sit above or below the viewport are offered like any other link (ranked by the goal's rarer words, at most 12; `jev_observe`'s table marks them *below the viewport*) and a click scrolls them in first; after four scrolls in a row that showed nothing new the goal names, scrolling is not offered until something else is done |
| `turbo` | all of the above |
| `calm` | an attribute written with the value it already had is no change, and a `style` or `class` write counts only when its element had none in the 200 ms before, so a page that restyles every frame goes quiet in the settle |
| `deep` | the read covers open shadow roots and same-origin frames too (40 roots at most): their controls and text are offered, a frame's controls at their place on the top page, and a click or typing lands there. Cross-origin frames stay unread |
| `verify` | not part of Fast mode: beside each decision Jev also says how likely something is visibly left undone; a DONE the page contradicts (≥ 50%) is refused, twice at most (`done_check`, `done_refused` in the JSON trace) |
| `submit` | Return after typing into a search box that is its form's only field and shows no suggestions: the search is made, not just written |
| `watched` | like `fast`, but the glide stays when the tab is on the user's screen (Fast mode is `turbo` + `submit` + `calm` + `deep` + `watched`) |
| `classic` | none of them: the original loop |
| `llm`, `llm=<model>` | a small LLM (the text model, else Haiku) decides instead of Jev: one chat call returns the operation, the target, any text and how sure it is; an answer that names nothing on offer is asked once more |

Some things hold whatever the strategy. A DONE or BLOCKED under 80% gets one
second look after the page settles in full; a DONE still under it ends
**unsure**, as does one whose value typed from the goal the page doesn't show.
A goal that asks something (*what is…*, *tell me…*) ends DONE when the answer
read off the page is found on it, and `jev_run json: true` returns that
`answer` with the page's words around it (`snippet`). A payment, post or
order goes once: a send the page took without a word is not sent again, and
Jev is asked DONE or BLOCKED instead; a search, a retry or a Reload may go
again. The same control clicked twice on an unchanged page becomes a WAIT,
then the run ends unsure. A control with something drawn over it is left out
and Jev uses what is on top. A control that asks for a human check — *I am
human*, a CAPTCHA, *press and hold* — is never clicked or typed into: the run
ends BLOCKED and says to hand the page to the user. What a page asks its
server is what `browser_network_requests` lists: heard from the engine, not
from a script in the page. On WebKitGTK a request's kind comes from its
address and answer, so a data call that answers with HTML counts as a page
load.

`model` asks another Jev for the run (`jev-preview`, `jev-1.14.0`).
`json: true` (`copper run --json-run`) returns the run as one object —
`status`, `elapsed_ms`, `totals` and one entry per step with where its time
went (`observe_ms`, `decide_ms`, `jev_ms`, `text_ms`, `act_ms`, `settle_ms`,
`settle_capped` and `settle_why`; `net`, `in_flight`, `sent`, `form_sent`;
`burst: true` for a field typed without a decision of its own) — for a
harness. `totals` count every decision asked, including a DONE, a BLOCKED, a
refused DONE and one the page went stale under (`stale`, `stale_ops`), with
`text_calls`, `text_reused`, `text_model`, `settles`, `settles_capped`,
`doubts` and `resends_stopped`. Jev's typing asks a fast gateway model
(`llama`) first when Settings names no text model, with Haiku behind it.

Jev can also choose **SIGN_IN** on a page with a password field when an
account for the site is shared with agents, and **AUTOFILL_CARD** /
**AUTOFILL_IDENTITY** when the page has card or address fields and a card or
identity is shared (Settings › Passwords › Agent access). Copper fills them
as `browser_sign_in` and `browser_autofill` do; the secret never reaches the
model.

## The command line

The `copper` binary is also a client for the running window. It never starts
Copper unless you pass `--launch` (or set `COPPER_LAUNCH=1`).

```sh
copper health
copper tabs
copper run "find the March invoice and stop when it is visible"
copper observe
copper snapshot --interactive
copper shot page.png
copper shot ~/Desktop/card.png --selector '.pricing-card' --padding 12 --hd
copper signin --otp
copper call browser_navigate '{"url":"https://example.com"}'
```

`copper hover`, `select`, `scroll`, `wait`, `back`, `forward`,
`upload PATH… | --cancel`, `dialog accept [TEXT] | dismiss` and `pdf [PATH]`
are each one tool, with the arguments above (`copper help`). `copper call
browser_cookies '{"action":"list"}'` for the rest.

`copper intelligence` shows and sets the model keys in the running Copper,
through its loopback server. It prints JSON and never a key:

```sh
copper intelligence                  # status: jevReady, routerReady, lane, tier, model, sources…
copper intelligence sources          # where each key and address comes from: local | cloud | none | default
copper intelligence refresh          # ask the linked Copper Cloud for its keys again now
echo "$TS_KEY" | copper intelligence set --jev -     # - reads the value from stdin
copper intelligence set --model opus --router-url https://llm.example.com
```

`set` takes `--lane key|claude`, `--model haiku|sonnet|opus`,
`--haiku-model`, `--sonnet-model`, `--opus-model`, `--jev`, `--jev-model`,
`--router-key`, `--router-url`, `--router-model` and `--text-model`, and
writes `intelligence.json` (0600). It exits 1 when Copper refuses a value.
`--jev-vision` and `--jev-fast` are Mac-only for now.

`copper help` lists everything. `--json` prints the raw result, and
`--tab ID` works in another tab for one call. With the Flatpak, the same
commands are `flatpak run com.copperbrowser.Copper tabs` and so on.

To run Copper with no display, as an agent's own browser (a container or a
microVM, a pre-written `agent.json`, `copper cloud pair` before the first
start), see [headless.md](headless.md).

## Not on Linux yet

These Mac features aren't in the Linux build:

- the agent pane (⌘E on the Mac) and its chat
- the driver timeline
- **Your agents** links to an agents app (`copper link`)
- the Listen transcript tools (`voice_status`, `voice_transcript`): there is
  no Voice on Linux
- what Jev sees (`vision`: auto, screenshot, marks, box, pixels): Jev reads
  the element table only, and `jev_run` does not offer `vision`. No Fast
  mode switch in Settings yet either; it is `jevFast` in `intelligence.json`
  (`copper intelligence set --jev-fast` / `--jev-vision` are refused)
- the CLI's `setup`, `session`, `claude`, `crashes`, `listen` and
  `transcript` commands, and `bitwarden` beyond `save -` and `note -` (sign
  in and unlock in Settings › Passwords)

## What it is not

It is not a sandbox. The agent acts as you, in your sessions. Turn the server
off when you don't need it. There is no server-to-client event stream (a GET
on `/mcp` answers 405); every tool answers in the reply to its POST.
