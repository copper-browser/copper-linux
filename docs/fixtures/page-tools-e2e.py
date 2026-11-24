#!/usr/bin/env python3
"""Mac #181 on Linux, end to end: a real copper (debug build) on a private
headless weston, a fresh COPPER_WORLD in a temp HOME, pages from a local
server, every check through MCP (and `copper eval` through the CLI).

    cargo build -p copper-app && docs/fixtures/page-tools-e2e.py [<worktree>]
"""
import http.server, json, os, secrets, shutil, socket, subprocess, sys, tempfile, threading, time, urllib.request

tree = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
binary = os.path.join(tree, "target/debug/copper")
home = tempfile.mkdtemp(prefix="cl-tpe-home-")
pages = tempfile.mkdtemp(prefix="cl-tpe-pages-")
rt = tempfile.mkdtemp(prefix="cl-tpe-rt-"); os.chmod(rt, 0o700)
world = "tpe-e2e"
token = secrets.token_hex(32)


def free_port():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p


mcp_port, web_port = free_port(), free_port()
open(os.path.join(pages, "page.html"), "w").write("""<!doctype html><title>Page tools</title>
<script>console.log('pt-early-log at parse time')</script>
<h1>Page tools</h1>
<button id=under onclick="window.clicks=(window.clicks||0)+1">Under the wall</button>
<button id=hid style="display:none">Hidden</button>
<input id=ro readonly value="2026-11-24" aria-label="Date">
<select id=sel aria-label="Colour"><option>Red</option><option>Blue</option></select>
<ul id=list style="height:60px;overflow:auto">""" + "".join(f"<li style='height:20px'>Row {i}</li>" for i in range(20)) + """</ul>
<div id=wall style="position:fixed;left:0;top:0;width:100%;height:120px;background:#fff">We use cookies <button>Accept all</button></div>
""")

env = dict(os.environ, HOME=home, XDG_RUNTIME_DIR=rt, COPPER_WORLD=world, COPPER_HEADLESS="1",
           XDG_DATA_HOME=os.path.join(home, ".local/share"), XDG_CONFIG_HOME=os.path.join(home, ".config"),
           NO_AT_BRIDGE="1", GDK_BACKEND="wayland", WAYLAND_DISPLAY="tpe-wl",
           WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1", COPPER_LOG="warn")
data = os.path.join(home, ".local/share", f"copper-{world}")
os.makedirs(data, mode=0o700)
with open(os.path.join(data, "agent.json"), "w") as f:
    json.dump({"enabled": True, "port": mcp_port, "token": token, "jev": False, "announces": False}, f)
os.chmod(os.path.join(data, "agent.json"), 0o600)

procs = []
failures = []


def cleanup():
    for p in reversed(procs):
        p.terminate()
        try: p.wait(10)
        except Exception: p.kill()
    for d in (home, pages, rt): shutil.rmtree(d, ignore_errors=True)


handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory=pages, **k)
http.server.SimpleHTTPRequestHandler.log_message = lambda *a: None
server = http.server.ThreadingHTTPServer(("127.0.0.1", web_port), handler)
threading.Thread(target=server.serve_forever, daemon=True).start()

bus = f"unix:path={rt}/bus"
env["DBUS_SESSION_BUS_ADDRESS"] = bus
procs.append(subprocess.Popen(["dbus-daemon", "--session", f"--address={bus}", "--nofork"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
procs.append(subprocess.Popen(["weston", "--backend=headless", "--renderer=pixman", "--socket=tpe-wl", "--width=1440", "--height=900", "--idle-time=0"],
                              env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
for _ in range(100):
    if os.path.exists(os.path.join(rt, "tpe-wl")): break
    time.sleep(0.1)
log = open(os.path.join(rt, "copper.log"), "w")
procs.append(subprocess.Popen([binary], env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True))
try:
    for _ in range(240):
        try:
            if json.load(urllib.request.urlopen(f"http://127.0.0.1:{mcp_port}/health", timeout=1)).get("running"): break
        except Exception: time.sleep(0.25)
    else:
        print(open(os.path.join(rt, "copper.log")).read()[-3000:]); raise SystemExit("copper never came up")

    def call(name, arguments):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}).encode()
        req = urllib.request.Request(f"http://127.0.0.1:{mcp_port}/mcp", body, {"Content-Type": "application/json",
            "Accept": "application/json, text/event-stream", "Authorization": f"Bearer {token}"})
        r = json.load(urllib.request.urlopen(req, timeout=60))["result"]
        return "\n".join(c.get("text", "") for c in r["content"]), r.get("isError", False)

    def check(what, ok, said):
        print(("PASS " if ok else "FAIL ") + what + "\n     → " + said.split("\n### Page")[0].replace("\n", "\n       ")[:400])
        if not ok: failures.append(what)

    said, err = call("browser_tabs", {"action": "new", "url": f"http://127.0.0.1:{web_port}/page.html"})
    check("open the fixture in an agent tab", not err and "Page tools" in said, said)
    tab = said.split("{tab ")[1].split("}")[0]

    said, err = call("browser_evaluate", {"function": "document.title"})
    check("evaluate: a plain expression", not err and said == "Page tools", said)
    said, err = call("browser_evaluate", {"function": "async () => { await new Promise(r => setTimeout(r, 50)); return {n: 1}; }"})
    check("evaluate: a Promise is awaited, an object is JSON", not err and said == '{"n":1}', said)
    said, err = call("browser_evaluate", {"function": "() => { throw new Error('pt-boom-17') }"})
    check("evaluate: a page error is its own message", err and said.startswith("JavaScript:") and said.endswith("pt-boom-17"), said)
    said, err = call("browser_evaluate", {"function": "() => {"})
    check("evaluate: a syntax error says how to write it", err and "The function did not parse" in said, said)

    snap, _ = call("browser_snapshot", {"interactive": True})
    ref = lambda label: snap.split(f'"{label}" [ref=')[1].split("]")[0]
    said, err = call("browser_click", {"ref": ref("Under the wall"), "element": "Under the wall"})
    clicks, _ = call("browser_evaluate", {"function": "window.clicks || 0"})
    check("click under a cookie wall: an error naming the wall, no click", err and "covered by div#wall" in said and clicks == "0", said)
    said, err = call("browser_click", {"selector": "#hid"})
    check("click on a hidden button: not visible", err and "not visible (display: none)" in said, said)
    call("browser_evaluate", {"function": "document.getElementById('wall').remove()"})
    said, err = call("browser_click", {"ref": ref("Under the wall"), "element": "Under the wall"})
    clicks, _ = call("browser_evaluate", {"function": "window.clicks || 0"})
    check("with the wall gone the click lands", not err and said.startswith("Clicked Under the wall") and clicks == "1", said)

    said, err = call("browser_type", {"ref": ref("Date"), "text": "x"})
    check("typing into a read-only box: an error", err and "read-only" in said, said)
    said, err = call("browser_select_option", {"ref": ref("Colour"), "values": ["Purple"]})
    check("an unmatched option: an error", err and said.startswith('no option matched ["Purple"]'), said)
    said, err = call("browser_fill_form", {"fields": [{"name": "Ghost", "ref": "e999", "type": "textbox", "value": "x"}]})
    check("a form field not filled: an error", err and said.startswith("1 of 1 fields not filled"), said)
    said, err = call("browser_press_key", {"key": "NotAKey"})
    check("an unknown key: an error", err and said.startswith('Unknown key "NotAKey"'), said)
    said, err = call("browser_wait_for", {"time": -1})
    check("a negative wait: an error", err and said.startswith("time must be a number of seconds"), said)
    said, err = call("browser_wait_for", {"textGone": "Page tools"})
    check("a wait that runs out: an error", err and said.startswith('Timed out after 15 s: "Page tools" is still on the page'), said)
    said, err = call("browser_scroll", {"deltaY": 1e100})
    check("deltaY 1e100: refused", err and said == "deltaY must be a number from -1000000 to 1000000; got 1e+100", said)
    row = call("browser_find", {"text": "Row 2"})[0].split("[ref=")[1].split("]")[0]
    said, err = call("browser_scroll", {"ref": row, "deltaY": 40})
    top, _ = call("browser_evaluate", {"function": "document.getElementById('list').scrollTop + ',' + scrollY"})
    check("scrolling a row scrolls its list", not err and top == "40,0", said + f" (list,page = {top})")
    said, err = call("browser_navigate_back", {})
    check("back with no history: an error", err and said.startswith("No page to go back to"), said)
    said, err = call("browser_groups", {"action": "assign"})
    check("a bad browser_groups call: an error", err and said.startswith("group (a name) is required"), said)
    said, err = call("browser_console_messages", {})
    check("console logs from page load are kept", not err and "pt-early-log at parse time" in said, said)

    cli = subprocess.run([binary, "--tab", tab, "eval", "document.title"], env=env, capture_output=True, text=True, timeout=60)
    check("copper eval document.title", cli.returncode == 0 and cli.stdout.strip() == "Page tools", cli.stdout + cli.stderr)
finally:
    server.shutdown()
    cleanup()
print(f"\n{'ALL PASS' if not failures else 'FAILED: ' + ', '.join(failures)}")
sys.exit(1 if failures else 0)
