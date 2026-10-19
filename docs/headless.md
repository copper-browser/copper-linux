# Headless: Copper with no display, for an agent

Copper can run with no screen: in a container, a microVM or a CI job, as an
agent's own browser. It still needs a Wayland compositor to draw into, so it
gets a headless one, and the agent drives it over MCP on a loopback port.
This is how the FluxBot runtime on AgentCore runs it (Copper ≥ 0.3.0).

The outline:

1. A runtime folder, a session D-Bus and a headless weston.
2. A profile of its own (`COPPER_WORLD`) with a pre-written `agent.json`.
3. Optionally, `copper cloud pair` while Copper is not running yet.
4. Start `copper`, wait for `copper health`, then drive it.

## What to install

On Ubuntu 24.04 or Debian 13 (arm64 or amd64):

```sh
apt-get install -y ./copper_0.3.0_arm64.deb    # pulls WebKitGTK 6.0, GTK 4, libadwaita
apt-get install -y weston dbus fonts-dejavu-core fonts-noto-color-emoji \
  bubblewrap xdg-dbus-proxy
```

`bubblewrap` and `xdg-dbus-proxy` are what WebKit's own sandbox runs in
(WebKitGTK needs bubblewrap 0.3.1 or later). No GPU is needed: weston draws
with pixman and WebKit paints in software.

## 1. Compositor and session bus

```sh
export XDG_RUNTIME_DIR=/run/copper            # any folder, mode 0700, owned by the user
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"

export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
dbus-daemon --session --address="$DBUS_SESSION_BUS_ADDRESS" --fork --print-pid \
  </dev/null >"$XDG_RUNTIME_DIR/dbus.pid"

weston --backend=headless --renderer=pixman --socket=copper-wl \
  --width=1440 --height=900 --idle-time=0 </dev/null >"$XDG_RUNTIME_DIR/weston.log" 2>&1 &
# wait for "$XDG_RUNTIME_DIR/copper-wl" to appear

export WAYLAND_DISPLAY=copper-wl
export GDK_BACKEND=wayland
export NO_AT_BRIDGE=1                         # no accessibility bus here; quiets GTK
```

`--renderer=pixman` is weston 12 and later (Ubuntu 24.04 has 13). On older
weston, use `--use-pixman`. The size is the screen's; a background tab paints
at the window's size whatever it is.

### The WebKit sandbox

WebKit starts every web process inside bubblewrap, which needs an
unprivileged user namespace. Check once in the image:

```sh
unshare --user --map-root-user true && echo "the sandbox can run"
```

- **It can** (a microVM with its own kernel, a VM, a desktop): leave it on.
  On Ubuntu 23.10+ hosts that restrict user namespaces, the `.deb`'s AppArmor
  profile grants `copper` one.
- **It can't** (most Docker/OCI containers: the default seccomp profile or
  `kernel.apparmor_restrict_unprivileged_userns=1` with no profile for the
  binary): web processes die at start with `bwrap: setting up uid map:
  Permission denied`. If the container itself is the isolation boundary —
  single-tenant, thrown away after use — turn WebKit's inner sandbox off:

  ```sh
  export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
  ```

  Never do this on a desktop or a shared machine: it is the only thing
  between a hostile page and the rest of the account.

If pages come up blank, also try `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
`libEGL warning: …` and `MESA: error: ZINK: failed to choose pdev` at start
are harmless with pixman.

## 2. A profile and `agent.json`

`COPPER_WORLD=<name>` gives Copper a profile of its own: its data folder is
`$XDG_DATA_HOME/copper-<name>` (default `~/.local/share/copper-<name>`), its
config folder `$XDG_CONFIG_HOME/copper-<name>`. The name is lowercased and
keeps only `a-z`, `0-9` and `-`. A Copper with a world never joins another
running Copper, so several bots can run side by side, each on its own port.

Write `agent.json` in the data folder **before** the first start (folder
0700, file 0600):

```json
{
  "enabled": true,
  "port": 4212,
  "token": "6f1c…64 hex characters…",
  "jev": true,
  "announces": false
}
```

| key | type | default | meaning |
|---|---|---|---|
| `enabled` | bool | `false` | Run the MCP server. Must be `true` for an agent. |
| `port` | integer 1–65535 | `4123` | Loopback port (`http://127.0.0.1:<port>/mcp`). `SEARCH_MCP_PORT` overrides it for one run. |
| `token` | string | minted | The bearer token every request carries (`Authorization: Bearer <token>`). Any non-empty string works; Copper mints 64 hex characters (`openssl rand -hex 32`) when it is missing or empty, and saves the file. Never log it. |
| `jev` | bool | `false` | Jev mode: adds `jev_run`, `jev_step`, `jev_observe` and `jev_extract`. |
| `announces` | bool | `true` | Say each tool call in the window's status line. Nobody watches a headless window: `false`. |
| `links` | array | absent | Agent links (bots that drive this browser remotely); leave out. |
| `agentLink` | object | absent | The legacy single link; leave out. |

Other keys are kept as they are. `/health` needs no token; everything on
`/mcp` does.

### Model keys

Jev mode needs a Jev key, and typing and `jev_extract` also need a text model
behind an OpenAI-compatible router. Copper merges three places, in this
order of strength:

1. **The environment** (this run only), for the router:
   `COPPER_ROUTER_URL` (`http://` or `https://`; Copper adds
   `/v1/chat/completions`) and `COPPER_ROUTER_KEY` — both or neither — and
   optionally `COPPER_ROUTER_MODEL`, sent for every size (haiku, sonnet, opus)
   and for Jev's text writer. Without it Copper sends the size names. These
   beat the cloud's router and `intelligence.json`.
2. **Copper Cloud**: a signed-in Copper gets the keys the cloud provides
   (`cloud-intelligence.json`, written at sign-in and refreshed after). The
   cloud's router key wins over `intelligence.json`'s.
3. **`intelligence.json`** in the data folder (0600): `jevKey`, `jevModel`,
   `jevEndpoint`, `routerKey`, `routerURL`, `routerModel`, `routerModels`,
   `textModel`.

So when the cloud's own gateway is out of reach (it is office-only, say),
point Copper at a router you can reach with the environment, not with
`intelligence.json`: a cloud that provides a router would win over the file.
`copper cloud status` says where each key comes from (`env`, `cloud`,
`local`, `none`) without showing one.

## 3. Pair with Copper Cloud (optional)

With Copper **not yet running**, pair the profile with a pairing code (from
a signed-in Copper, or a service that mints them):

```sh
COPPER_WORLD=bot-1a2b3c4d copper cloud pair "$PAIRING_LINK" \
  --sync tabs --name "FluxBot · Vera" --json
```

```json
{"ok": true, "email": "vera@example.com", "device_id": "0193…", "device_name": "FluxBot · Vera",
 "host": "cloud.example.com", "sync": ["tabs"], "provided": ["jev", "router"]}
```

It writes `cloud.json` and the sync switches for that profile only, and
fetches the keys the cloud provides; the browser syncs as soon as it starts.
It refuses while a Copper window owns the profile (`"error":"running"`,
exit 2). On failure it prints `{"ok":false,"error":"<code>","message":"…"}`
and exits 1. It never prints the session token, the code or a key. See
[cloud.md](cloud.md#from-the-command-line-with-no-window) for `--sync`
and the rest; `copper cloud status --json` reads the result back.

## 4. Start, wait, drive

```sh
export COPPER_WORLD=bot-1a2b3c4d
export COPPER_HEADLESS=1          # reported by /health as "headless": true
setsid copper </dev/null >"$XDG_RUNTIME_DIR/copper.log" 2>&1 &

until copper health >/dev/null 2>&1; do sleep 0.25; done
copper health
# Copper health: running: true (port 4212) · version: 0.3.0
# Jev mode: on
# Headless: yes
```

Without the CLI: `GET http://127.0.0.1:4212/health` answers
`{"name":"copper","version":"0.3.0","mcp":"/mcp","running":true,"headless":true,"port":4212}`
with no token once the server is up.

Then any MCP client (Streamable HTTP, one POST per message, bearer token) or
the CLI drives it:

```sh
copper open https://example.com
copper text
copper run "find the pricing page and stop there"
```

Stop it with `SIGTERM`: Copper quits cleanly and saves its session. To move a
microVM to another bot, stop this Copper and start one with the other
`COPPER_WORLD`.

## Environment

| variable | for | meaning |
|---|---|---|
| `COPPER_WORLD` | browser, CLI | The profile: `copper-<world>` data and config folders. |
| `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `HOME` | browser, CLI | Where those folders are (absolute XDG paths only). |
| `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY`, `GDK_BACKEND=wayland` | browser | The headless compositor. |
| `DBUS_SESSION_BUS_ADDRESS` | browser | The session bus (portals, the keyring, GApplication). |
| `NO_AT_BRIDGE=1` | browser | No accessibility bus. |
| `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1` | browser | Only in a disposable container that is itself the sandbox (above). |
| `COPPER_HEADLESS=1` (or `SEARCH_HEADLESS=1`) | browser | Reported by `/health` and `copper health`. |
| `SEARCH_MCP_PORT` | browser, CLI | Listen on (connect to) this port instead of `agent.json`'s. |
| `COPPER_AGENT_PORT` | CLI | Connect to this port. |
| `COPPER_ROUTER_URL`, `COPPER_ROUTER_KEY`, `COPPER_ROUTER_MODEL` | browser | The router, beating the cloud's and `intelligence.json`'s. |
| `COPPER_LAUNCH=1` | CLI | Let a CLI command start Copper when it is down. A headless runtime starts Copper itself. |

## Troubleshooting

- **`Failed to initialize GTK`**: `WAYLAND_DISPLAY` or `XDG_RUNTIME_DIR` is
  wrong, or weston isn't up yet (wait for the socket).
- **`bwrap: setting up uid map: Permission denied`**, pages never load: the
  sandbox can't make a user namespace; see above.
- **`copper health` says Copper isn't running**: `enabled` isn't `true`, the
  port is taken (the log says `Couldn't listen on port …: another program is
  using it`), or `COPPER_WORLD` differs
  between the browser and the CLI.
- **`401 bearer token required`**: the token in the request isn't
  `agent.json`'s. Copper reads `agent.json` at start; restart it after
  changing the file.
