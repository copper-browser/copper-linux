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
with `tab`, either an id from `browser_tabs list` or an index.

An agent's tab that you aren't looking at is still worked on properly: it is
kept off screen, where pages still paint, `requestAnimationFrame` runs,
screenshots are real and clicks arrive as trusted events. Your selection and
keyboard focus don't move. If you pick the tab yourself it moves on screen.
Five minutes after the agent's last call it becomes an ordinary background
tab again, and may sleep.

| tool | does |
|---|---|
| `browser_tabs` | `list`, `new {url}`, `close {index\|tab}`, `select {index\|tab}` (+ `focus`). `new` opens a background tab of the caller's own. Neither `new` nor `select` switches your tab unless `focus: true`. `close` on a Saved tab only puts it to sleep; it stays saved. |
| `browser_navigate`, `browser_navigate_back`, `browser_navigate_forward` | Moves the caller's tab, then waits for the load to settle. |
| `browser_snapshot` | Accessibility tree with `[ref=e12]` on interactive nodes. `interactive: true` gives a smaller tree; `selector` gives a subtree. |
| `browser_click` | Clicks by `ref` or `selector`, at the element's centre, as a real input event. Takes `doubleClick`, `button` and `modifiers`. |
| `browser_type` | Types `text`; `submit` presses Enter after, `slowly` types key by key. The value is set through the native setter, so React and Vue notice. |
| `browser_fill_form` | `[{ref, type, value}]` for textbox, checkbox, radio, combobox and slider. |
| `browser_press_key` | `Enter`, `Escape`, `ArrowDown`, `Control+a`, `a` and so on, as real key events. |
| `browser_hover`, `browser_drag`, `browser_select_option`, `browser_scroll` | Hover, drag, choose an option, scroll. |
| `browser_take_screenshot` | PNG or JPEG at 1×, so image pixels are CSS pixels. `ref` shoots one element; `fullPage` the whole page. |
| `browser_evaluate` | Runs `function: "() => …"`, or `(element) => …` with a `ref`. |
| `browser_wait_for` | Waits for `text`, for `textGone`, or for `time`. |
| `browser_get_text`, `browser_find`, `browser_console_messages` | Read without a snapshot: page text, refs for text you name, console messages. |
| `browser_resize`, `browser_close` | Resize the window; close the tab. |
| `browser_groups` | Copper's tab groups: `list`, `assign {group}`, `remove`. |
| `browser_perf_probe` | Samples the tab for frame loops, DOM churn, animations, filters, canvases, timers, long tasks and slow resources. |
| `browser_crashes` | Copper's own crash history on this computer, newest first. |
| `browser_sign_in`, `browser_autofill` | Fill a saved sign-in, card, identity or custom field without the secret ever leaving Copper (below). |
| `cloud_history_delete` | Deletes your history on Copper Cloud ([cloud.md](cloud.md)). History on this computer stays. Loopback clients only. |
| `canvas_*` | Read and draw on your canvases: `canvas_list` (with the `invitations` waiting for your answer, `{canvas, from}`, when signed in to Copper Cloud), `canvas_open`, `canvas_read`, `canvas_apply`, `canvas_select`, `canvas_focus`, `canvas_create`, `canvas_invite`, `canvas_share_link`, `canvas_join`, `canvas_screenshot`, `canvas_chat`. A big canvas reads in pages: `canvas_read` with `overview` first, then `frame` or `region`, or follow `next` as `cursor`; `ids`, `types` and `query` narrow any read. A canvas that is closed, asleep or still loading is opened behind your tab and waited for (up to a minute for a huge one). |

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

`browser_autofill` fills a Bitwarden or 1Password `card`, `identity` or
custom `field` the same way. It answers only with how many fields it filled
and the item's name.

Nothing is shared with agents unless you share it, in Settings › Passwords ›
**Agent access**. You can share everything, or turn individual items on (the
list shows while sharing everything is off).
Items in a Bitwarden folder or 1Password vault named `Agents` are shared automatically. A custom
field `copper-agent: deny` means never, even when sharing everything. Private
tabs are always refused. Agents can't change any of this.

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
| `jev_run` | Takes a `goal` (+ `url`, `newTab`, `focus`, `tab`, `maxSteps`, `elements`, `strategy`, `model`, `json`). Runs the [jev-ultrafast](https://github.com/browser-use/jev-ultrafast) loop in the caller's tab until DONE, BLOCKED, or the budget runs out. Answers with the trace, the page and the indexed element table. |
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
`COPPER_ROUTER_KEY` instead ([headless.md](headless.md#model-keys)). There
is no Claude account sign-in on Linux yet: the API key is the only lane.
DONE is the model's own claim, so check the page.

### Faster runs (`strategy`)

*Fast mode* is on by default (`jevFast` in `intelligence.json`; set it to
`false` for the original loop): a run takes `turbo`, with the pointer glide
kept for a tab that is on the user's screen. `jev_run` / `jev_step` take
`strategy`, comma-separated speed-ups, to choose for one run
(`copper run --strategy …`):

| word | what changes |
|---|---|
| `fast` | no cosmetic pointer glide before a click; the trail still draws |
| `literal` | each value the goal quotes (`"maya@northwind.test"`, email addresses) becomes one Jev question in the same request — *which field holds this?* — and a TYPE_TEXT types the value Jev matched. No text-model call; a field no quoted value fits still asks the text model |
| `burst` | `literal`, and every other value Jev matched surely (≥ 60%) is typed in the same breath, before the next read: a sign-in form is one decision, not three |
| `lean` | the state Jev reads is trimmed (page text to 1,200 characters, five recent actions) |
| `paste` | a field's value is set in one go (native setter, input and change events) instead of a key event per character |
| `watch` | WAIT ends when the page changes (capped at 1.5 s) instead of after 100 ms |
| `turbo` | all of the above |
| `watched` | like `fast`, but the glide stays when the tab is on the user's screen (Fast mode is `turbo` + `watched`) |
| `classic` | none of them: the original loop |
| `llm`, `llm=<model>` | a small LLM (the text model, else Haiku) decides instead of Jev: one chat call returns the operation, the target and any text |

`model` asks another Jev for the run (`jev-preview`, `jev-1.14.0`).
`json: true` (`copper run --json-run`) returns the run as one object —
`status`, `elapsed_ms`, `totals` (`strategy`, Jev's ms and tokens, stale
reads, bursts) and one entry per step with where its time went
(`observe_ms`, `decide_ms`, `jev_ms`, `text_ms`, `act_ms`, `settle_ms`;
`burst: true` for a field typed without a decision of its own) — for a
harness.

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
copper signin --otp
copper call browser_navigate '{"url":"https://example.com"}'
```

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
- the CLI's `setup`, `session`, `claude`, `bitwarden`, `crashes`, `listen`
  and `transcript` commands

## What it is not

It is not a sandbox. The agent acts as you, in your sessions. Turn the server
off when you don't need it. There is no server-to-client event stream (a GET
on `/mcp` answers 405); every tool answers in the reply to its POST.
