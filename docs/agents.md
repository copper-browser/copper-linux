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
  page.

## Connect a client

### Claude Code, phi, Cursor (HTTP)

Settings › Agents › *Claude Code, phi, Cursor* › **Copy config**. Paste it
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

Use the second **Copy config**. It runs `copper --mcp-stdio`, a pipe to the
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

**Rotate token…** makes a new token. Every client config goes stale at once,
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
| `canvas_*` | Read and draw on your canvases: `canvas_list`, `canvas_open`, `canvas_read`, `canvas_apply`, `canvas_select`, `canvas_focus`, `canvas_create`, `canvas_invite`, `canvas_share_link`, `canvas_join`, `canvas_screenshot`, `canvas_chat`. A big canvas reads in pages: `canvas_read` with `overview` first, then `frame` or `region`, or follow `next` as `cursor`; `ids`, `types` and `query` narrow any read. A canvas that is closed, asleep or still loading is opened behind your tab and waited for (up to a minute for a huge one). |

Each tool takes an optional `reason`: one short sentence on what the agent is
doing and why. Copper never acts on it; it only shows it to you. While an
agent works in a tab, that tab gets a small dot in the tab row, and a capsule
over the page says who is driving and offers **Stop**. The capsule tucks
itself away when your pointer comes near. You can turn these announcements
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
**Agent access**. You can share everything, or turn individual items on.
Items in a Bitwarden folder or 1Password vault named `Agents` are shared automatically. A custom
field `copper-agent: deny` means never, even when sharing everything. Private
tabs are always refused. Agents can't change any of this.

## Jev mode: hand over a goal

Settings › Agents › **Let the agent hand Copper a goal** adds four tools, and
the server's instructions tell the agent to reach for them first:

| tool | does |
|---|---|
| `jev_run` | Takes a `goal` (+ `url`, `newTab`, `focus`, `tab`, `maxSteps`, `elements`). Runs the [jev-ultrafast](https://github.com/browser-use/jev-ultrafast) loop in the caller's tab until DONE, BLOCKED, or the budget runs out. Answers with the trace, the page and the indexed element table. |
| `jev_step` | One decision and its action; calling again with the same `goal` continues the session. |
| `jev_observe` | What Jev sees: every visible control as `[3] combobox  Where to? · London`, plus the visible text. |
| `jev_extract` | Takes an `instruction` (+ `schema`, `full`) and returns one JSON object drawn from the page. |

Jev needs a Jev key (`ts-…`). It comes from `intelligence.json` in Copper's
data folder (the same file the Mac uses), or from your Copper Cloud if it
provides one. Typing and `jev_extract` also need a text model, configured in
the same file. DONE is the model's own claim, so check the page.

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

`copper help` lists everything. `--json` prints the raw result, and
`--tab ID` works in another tab for one call. With the Flatpak, the same
commands are `flatpak run com.copperbrowser.Copper tabs` and so on.

## Not on Linux yet

These Mac features aren't in the Linux build:

- the agent pane (⌘E on the Mac) and its chat
- the driver timeline
- **Your agents** links to an agents app (`copper link`)
- the Listen transcript tools (`voice_status`, `voice_transcript`): there is
  no Voice on Linux
- the CLI's `setup`, `session`, `intelligence`, `claude`, `bitwarden`,
  `crashes`, `listen` and `transcript` commands

## What it is not

It is not a sandbox. The agent acts as you, in your sessions. Turn the server
off when you don't need it. There is no server-to-client event stream (a GET
on `/mcp` answers 405); every tool answers in the reply to its POST.
