#!/usr/bin/env python3
"""The agent's page tools that reach into the engine, end to end, on Linux
(copper-mcp `dialogs.rs`, `page_tools.rs`; the Mac's
docs/fixtures/agent-state-tools-e2e.py and agent-input-e2e.sh, copper-browser/Copper#181).

Each in a background tab of the agent's own, the user's tab left where it was:

  1. Dialogs: an alert left open is reported, and moving on closes it; a
     confirm answered by browser_handle_dialog; an armed dismiss and an armed
     prompt; a prompt inside browser_evaluate never hangs it.
  2. browser_file_upload: click then upload, armed then click, bad paths
     refused, cancel, and files dropped on a drop zone (ref).
  3. browser_pdf_save: a long page as a PDF of several pages, and one into
     the downloads folder.
  4. browser_network_requests: a fetch GET (200), a POST and a 404.
  5. browser_cookies: an HttpOnly cookie listed with its value hidden; set,
     a foreign domain refused, HttpOnly delete refused, a plain delete.
  6. browser_permissions: geolocation needs a place; granted, the page gets
     the agent's position; camera granted and revoked; notifications refused.
  7. The CLI verbs (copper pdf, upload, dialog, select, hover, scroll, wait,
     back, forward) reach the same tools.
  8. With every account shared with agents, the HttpOnly value shows.

A throwaway HOME and COPPER_WORLD on a private headless weston; pages from a
local server; nothing of a real profile touched.

    cargo build -p copper-app && docs/fixtures/agent-page-tools-e2e.py [--bin target/debug/copper]
"""
import http.server
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
args = sys.argv[1:]
binary = os.path.abspath(args[args.index("--bin") + 1]) if "--bin" in args else os.path.join(root, "target/debug/copper")
tmp = tempfile.mkdtemp(prefix="page-tools-")
home = os.path.join(tmp, "home")
runtime = os.path.join(tmp, "rt")
os.makedirs(home)
os.makedirs(runtime, mode=0o700)
world = "pagetools-e2e"
data = os.path.join(home, ".local/share", f"copper-{world}")
config = os.path.join(home, ".config", f"copper-{world}")
mcp_port = int(os.environ.get("MCP_PORT", "4195"))
token = os.urandom(16).hex()
log = os.path.join(tmp, "copper.log")
procs = []

env = dict(os.environ, HOME=home, XDG_RUNTIME_DIR=runtime, COPPER_WORLD=world,
           XDG_DATA_HOME=os.path.join(home, ".local/share"), XDG_CONFIG_HOME=os.path.join(home, ".config"),
           XDG_CACHE_HOME=os.path.join(home, ".cache"), WAYLAND_DISPLAY="pt-wl", GDK_BACKEND="wayland",
           NO_AT_BRIDGE="1", GTK_A11Y="none", WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1",
           DBUS_SESSION_BUS_ADDRESS=f"unix:path={runtime}/bus")
env.pop("DISPLAY", None)


def cleanup():
    for p in reversed(procs):
        if p.poll() is None:
            p.send_signal(signal.SIGTERM)
            try:
                p.wait(10)
            except subprocess.TimeoutExpired:
                p.kill()
    if "--keep" not in args:
        shutil.rmtree(tmp, ignore_errors=True)


def fail(why):
    print(f"FAIL {why}")
    if os.path.exists(log):
        print("".join(open(log, errors="replace").readlines()[-25:]))
    cleanup()
    sys.exit(1)


def expect(cond, why):
    if not cond:
        fail(why)


def until(what, check, seconds=10):
    limit = time.time() + seconds
    while time.time() < limit:
        value = check()
        if value:
            return value
        time.sleep(0.25)
    fail(f"timed out waiting for {what}")


# --- the pages ------------------------------------------------------------------
PAGES = {
    "/home.html": "<!doctype html><title>HC home</title><h1>The user's tab</h1>",
    "/dialogs.html": """<!doctype html><title>HC dialogs</title>
<button id=alert onclick="alert('Saved!'); said.push('alert done')">Alert</button>
<button id=confirm onclick="said.push('confirm ' + confirm('Delete it?'))">Confirm</button>
<button id=prompt onclick="said.push('prompt ' + prompt('Your name?'))">Prompt</button>
<script>window.said = [];</script>""",
    "/upload.html": """<!doctype html><title>HC upload</title>
<input type=file id=file multiple><select id=pick><option>one</option><option>two</option></select>
<button id=hover onmouseenter="document.title='hovered'">hover me</button>
<div id=zone style="width:200px;height:80px;border:1px dashed">drop here</div>
<script>
window.got = null;
file.addEventListener('change', async () => {
  const f = file.files[0];
  window.got = {names: [...file.files].map(x => x.name), text: f ? await f.text() : null};
});
file.addEventListener('cancel', () => { window.cancelled = true; });
zone.addEventListener('dragover', e => e.preventDefault());
zone.addEventListener('drop', async e => {
  e.preventDefault();
  const f = e.dataTransfer.files[0];
  window.dropped = {names: [...e.dataTransfer.files].map(x => x.name), text: await f.text(), type: f.type};
});
</script>""",
    "/long.html": "<!doctype html><title>HC long page</title>" + "".join(
        f"<p style='height:120px'>Paragraph {i}: the quick brown fox.</p>" for i in range(60)),
    "/net.html": """<!doctype html><title>HC net</title><button id=go>go</button><script>
go.onclick = async () => {
  const a = await fetch('/api/json?from=page'); await a.json();
  await fetch('/api/json', {method: 'POST', body: 'x=1'});
  await fetch('/api/missing');
  window.fetched = 3;
};
</script>""",
    "/cookie.html": "<!doctype html><title>HC cookies</title><p>set</p>",
    "/geo.html": """<!doctype html><title>HC geo</title><script>
window.where = null;
function locate() {
  navigator.geolocation.getCurrentPosition(
    p => { window.where = [p.coords.latitude, p.coords.longitude]; },
    e => { window.where = 'error ' + e.code; }, {timeout: 5000});
}
</script>""",
}


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def reply(self, code, body, kind="text/html", extra=()):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        for k, v in extra:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = self.path.split("?")[0]
        if path in PAGES:
            extra = ()
            if path == "/cookie.html":
                extra = (("Set-Cookie", "hc_session=s3cret; Path=/; HttpOnly"), ("Set-Cookie", "hc_plain=visible; Path=/"))
            return self.reply(200, PAGES[path], extra=extra)
        if path == "/api/json":
            return self.reply(200, '{"answer":42}', "application/json")
        return self.reply(404, "nope", "text/plain")

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", "0") or 0))
        return self.reply(200, '{"ok":true}', "application/json")


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
# `localhost`, not 127.0.0.1: cookies and permissions want a host name.
site = f"http://localhost:{server.server_address[1]}"

# --- MCP ------------------------------------------------------------------------
session = None


def call(name, arguments=None, error=False):
    """A tool call; its text. error=True expects isError and returns the text."""
    global session
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": name, "arguments": arguments or {}}}).encode()
    headers = {"Authorization": f"Bearer {token}", "Content-Type": "application/json",
               "Accept": "application/json, text/event-stream"}
    if session:
        headers["Mcp-Session-Id"] = session
    req = urllib.request.Request(f"http://127.0.0.1:{mcp_port}/mcp", body, headers)
    with urllib.request.urlopen(req, timeout=90) as r:
        session = r.headers.get("Mcp-Session-Id") or session
        raw = r.read().decode()
    if raw.startswith("event:") or raw.startswith("data:"):
        raw = next(line[5:] for line in raw.splitlines() if line.startswith("data:"))
    result = json.loads(raw)["result"]
    text = "\n".join(c.get("text", "") for c in result["content"] if c["type"] == "text")
    if bool(result.get("isError")) != error:
        fail(f"{name} {arguments}: expected {'an error' if error else 'success'}, got: {text[:400]}")
    return text


def ev(js):
    return call("browser_evaluate", {"function": f"() => {js}"})


def js(expr):
    """`expr`'s value in the agent's tab, as JSON (however evaluate renders a string)."""
    raw = ev(f"JSON.stringify({expr} ?? null)")
    value = json.loads(raw)
    if isinstance(value, str):
        try:
            return json.loads(value)
        except ValueError:
            return value
    return value


def show(label, text):
    print(f"  {label}: " + text.replace("\n", "\n    ")[:900])


def health():
    try:
        urllib.request.urlopen(f"http://127.0.0.1:{mcp_port}/health", timeout=2)
        return True
    except OSError:
        return False


def front():
    """The tab on the user's screen (`browser_tabs list`'s current one)."""
    said = call("browser_tabs", {"action": "list"})
    m = re.search(r"^- \S+: \(on screen[^)]*\).*?\{tab ([0-9a-z]+)\}", said, re.M)
    return m.group(1) if m else said


def cli(*words):
    out = subprocess.run([binary, "--tab", mine, *words], env=env, capture_output=True, text=True, timeout=90, cwd=tmp)
    return out.returncode, out.stdout + out.stderr


def start(settings=None):
    os.makedirs(data, mode=0o700, exist_ok=True)
    path = os.path.join(data, "agent.json")
    json.dump({"enabled": True, "port": mcp_port, "token": token, "announces": False, "jev": False}, open(path, "w"))
    os.chmod(path, 0o600)
    if settings:
        os.makedirs(config, exist_ok=True)
        file = os.path.join(config, "settings.json")
        kept = json.load(open(file)) if os.path.exists(file) else {}
        kept.update(settings)
        json.dump(kept, open(file, "w"))
    p = subprocess.Popen([binary, f"{site}/home.html"], env=env, stdout=open(log, "a"), stderr=subprocess.STDOUT)
    procs.append(p)
    until("Copper's agent server", lambda: p.poll() is None and health() or (p.poll() is not None and fail("Copper quit")), 60)
    return p


def stop(p):
    p.send_signal(signal.SIGTERM)
    p.wait(20)


# --- the display -------------------------------------------------------------------
procs.append(subprocess.Popen(["dbus-daemon", "--session", f"--address=unix:path={runtime}/bus", "--nofork"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
procs.append(subprocess.Popen(["weston", "--backend=headless", "--renderer=pixman", "--socket=pt-wl",
                               "--width=1440", "--height=900", "--idle-time=0"], env=env,
                              stdout=open(os.path.join(tmp, "weston.log"), "w"), stderr=subprocess.STDOUT))
until("weston", lambda: os.path.exists(os.path.join(runtime, "pt-wl")), 20)

app = start()
try:
    user = until("the user's tab", lambda: (lambda f: f if re.fullmatch(r"[0-9a-z]{4,}", f) else None)(front()), 30)
    print(f"user's tab: {user}")

    # 1. Dialogs -----------------------------------------------------------------
    said = call("browser_tabs", {"action": "new", "url": f"{site}/dialogs.html"})
    mine = re.search(r"- Tab: ([0-9a-z]+)", call("browser_snapshot", {})).group(1)
    out = call("browser_click", {"selector": "#alert", "element": "Alert"})
    show("click alert", out)
    expect('- alert "Saved!" → open' in out, "the alert is not reported open")
    out = call("browser_snapshot", {"interactive": True})
    expect('- alert "Saved!" → OK (you moved on' in out, f"moving on didn't close it: {out[-300:]}")
    out = call("browser_click", {"selector": "#confirm", "element": "Confirm"})
    expect('- confirm "Delete it?" → open' in out, f"confirm not open: {out[-300:]}")
    out = call("browser_handle_dialog", {"accept": True})
    show("handle_dialog", out)
    expect(out.startswith('Accepted: the confirm "Delete it?"'), out)
    out = call("browser_handle_dialog", {"accept": False})
    expect(out.startswith("No dialog is open on this tab. The next alert, confirm or prompt will be dismissed"), out)
    out = call("browser_click", {"selector": "#confirm", "element": "Confirm"})
    expect(re.search(r'^- confirm "Delete it\?" → dismissed$', out, re.M), f"armed dismiss: {out[-300:]}")
    call("browser_handle_dialog", {"accept": True, "promptText": "Ada"})
    out = call("browser_click", {"selector": "#prompt", "element": "Prompt"})
    expect('- prompt "Your name?" → accepted with "Ada"' in out, f"armed prompt: {out[-300:]}")
    said_js = js("said")
    expect(said_js == ["alert done", "confirm true", "confirm false", "prompt Ada"], f"the page saw {said_js}")
    out = call("browser_evaluate", {"function": '() => String(prompt("Inside a script?"))'})
    expect(out == "null" or out == '"null"', f"evaluate with a prompt: {out}")
    out = call("browser_snapshot", {"interactive": True})
    expect('- prompt "Inside a script?" → dismissed — a script of Copper' in out, f"not reported: {out[-300:]}")
    expect(front() == user, "the user's tab changed")
    print("ok 1 dialogs: open + moved on, answered, armed dismiss, armed prompt, none hangs evaluate")

    # 2. Upload ------------------------------------------------------------------
    one = os.path.join(tmp, "hc-upload.txt")
    two = os.path.join(tmp, "hc-second.txt")
    open(one, "w").write("hello upload")
    open(two, "w").write("second file")
    call("browser_navigate", {"url": f"{site}/upload.html"})
    said = call("browser_click", {"selector": "#file", "element": "file input"})
    show("click file input", said)
    expect("file chooser" in said, f"the click's answer doesn't mention the chooser: {said[-200:]}")
    said = call("browser_file_upload", {"paths": [one]})
    show("upload", said)
    expect("Uploaded hc-upload.txt" in said, said)
    got = until("the page to read the file", lambda: js("window.got"))
    expect(got == {"names": ["hc-upload.txt"], "text": "hello upload"}, f"page got {got}")
    expect("No file chooser" in call("browser_file_upload", {}), "cancel with nothing open should say so")
    said = call("browser_file_upload", {"paths": [two, one]})
    expect(said.startswith("Armed"), said)
    call("browser_click", {"selector": "#file", "element": "file input"})
    until("the armed files", lambda: (lambda g: g if g and g["names"] == ["hc-second.txt", "hc-upload.txt"] else None)(
        js("window.got")))
    expect("no such file" in call("browser_file_upload", {"paths": [os.path.join(tmp, "nope.txt")]}, error=True), "missing file not refused")
    expect("absolute" in call("browser_file_upload", {"paths": ["hc-upload.txt"]}, error=True), "relative path not refused")
    call("browser_click", {"selector": "#file", "element": "file input"})
    expect("Cancelled" in call("browser_file_upload", {}), "cancel not answered")
    snap = call("browser_snapshot", {"selector": "#zone"})
    zone = re.search(r"\[ref=(e\d+)\]", snap)
    if not zone:
        snap = call("browser_find", {"text": "drop here"})
        zone = re.search(r"\[ref=(e\d+)\]", snap)
    expect(zone, f"no ref for the drop zone: {snap[:300]}")
    said = call("browser_file_upload", {"paths": [one], "ref": zone.group(1)})
    show("drop", said)
    dropped = until("the drop", lambda: js("window.dropped"))
    expect(dropped == {"names": ["hc-upload.txt"], "text": "hello upload", "type": "text/plain"}, f"dropped {dropped}")
    expect(front() == user, "the user's tab changed")
    print("ok 2 upload: click-then-upload, armed, bad paths refused, cancel, dropped on a zone")

    # 3. PDF ---------------------------------------------------------------------
    call("browser_navigate", {"url": f"{site}/long.html"})
    pdf = os.path.join(tmp, "hc.pdf")
    said = call("browser_pdf_save", {"filename": pdf})
    show("pdf", said)
    pages = int(re.search(r"a (\d+)-page PDF", said).group(1))
    expect(open(pdf, "rb").read(5) == b"%PDF-" and pages >= 2, said)
    said = call("browser_pdf_save", {})
    show("pdf default", said)
    m = re.search(r"PDF to (/.*?HC long page\.pdf)\.", said)
    expect(m and os.path.exists(m.group(1)), said)
    print(f"ok 3 pdf: {pages} pages, and one into the downloads folder ({m.group(1)})")

    # 4. Network -----------------------------------------------------------------
    call("browser_navigate", {"url": f"{site}/net.html"})
    call("browser_click", {"selector": "#go", "element": "go"})
    until("the fetches", lambda: js("window.fetched") == 3)
    said = call("browser_network_requests")
    show("network", said)
    for line in [f"[GET] {site}/net.html => [200] (document)", f"[GET] {site}/api/json?from=page => [200]",
                 f"[POST] {site}/api/json => [200]", f"[GET] {site}/api/missing => [404]"]:
        expect(line in said, f"missing {line!r} in:\n{said}")
    print("ok 4 network: the document, GET 200, POST 200, 404")

    # 5. Cookies -----------------------------------------------------------------
    call("browser_navigate", {"url": f"{site}/cookie.html"})
    said = call("browser_cookies")
    show("cookies", said)
    expect("hc_session=(hidden" in said and "s3cret" not in said and "hc_plain=visible" in said, said)
    call("browser_cookies", {"action": "set", "name": "hc_new", "value": "1"})
    expect("hc_new=1" in ev("document.cookie"), "set cookie not visible to the page")
    out = call("browser_cookies", {"action": "set", "name": "x", "value": "1", "domain": "evil.test"}, error=True)
    expect(out == "evil.test is not localhost or a parent domain of it", out)
    out = call("browser_cookies", {"action": "delete", "name": "hc_session"}, error=True)
    expect("Agent access" in out and out.startswith("not deleted"), out)
    expect("Deleted 1" in call("browser_cookies", {"action": "delete", "name": "hc_plain"}), "plain delete")
    print("ok 5 cookies: HttpOnly hidden; set; foreign domain and HttpOnly delete refused; plain delete")

    # 6. Permissions ---------------------------------------------------------------
    call("browser_navigate", {"url": f"{site}/geo.html"})
    out = call("browser_permissions", {"grant": ["geolocation"]}, error=True)
    expect(out.startswith("geolocation needs latitude and longitude"), out)
    out = call("browser_permissions", {"grant": ["geolocation"], "latitude": 25.76, "longitude": -80.19})
    show("permissions", out)
    port = server.server_address[1]
    expect(out.startswith(f"### Permissions for localhost:{port}") and "- geolocation: granted (at 25.76, -80.19)" in out, out)
    ev("locate()")
    where = until("the page's position", lambda: js("window.where"))
    show("page position", json.dumps(where))
    expect(where == [25.76, -80.19], f"the page got {where}")
    out = call("browser_permissions", {"grant": ["camera", "microphone"]})
    expect("- camera: granted" in out and "- microphone: granted" in out, out)
    out = call("browser_permissions", {"revoke": ["camera"], "site": f"localhost:{port}"})
    expect("- camera: ask" in out and "- microphone: granted" in out, out)
    out = call("browser_permissions", {"grant": ["notifications"]}, error=True)
    expect(out.startswith("Copper doesn't show web notifications"), out)
    out = call("browser_permissions", {"grant": ["midi"]}, error=True)
    expect(out == "Unknown permission midi; use one of geolocation, camera, microphone, notifications.", out)
    out = call("browser_permissions", {"grant": ["geolocation"], "latitude": 1e100, "longitude": 0}, error=True)
    expect(out == "latitude must be a number from -90 to 90; got 1e+100", out)
    print("ok 6 permissions: a place required, the page gets it; camera granted then revoked; notifications refused")

    # 7. CLI verbs ---------------------------------------------------------------
    said = call("browser_navigate", {"url": f"{site}/upload.html"})
    code, out = cli("pdf", "cli.pdf")
    expect(code == 0 and os.path.exists(os.path.join(tmp, "cli.pdf")), f"copper pdf: {out}")
    code, out = cli("upload", "hc-upload.txt")  # relative to the shell's folder
    expect(code == 0 and "Armed" in out, f"copper upload: {out}")
    code, out = cli("upload", "--cancel")
    expect(code == 0 and "Disarmed" in out, f"copper upload --cancel: {out}")
    code, out = cli("dialog", "accept", "Grace")
    expect(code == 0 and "will be accepted with \"Grace\"" in out, f"copper dialog: {out}")
    call("browser_handle_dialog", {"accept": False})  # disarmed by a later answer
    snap = call("browser_snapshot", {"interactive": True})
    ref = re.search(r'combobox[^\n]*\[ref=(e\d+)\]', snap)
    expect(ref, f"no combobox ref in {snap[:300]}")
    code, out = cli("select", ref.group(1), "two")
    expect(code == 0 and js("document.querySelector('#pick').value") == "two", f"copper select: {out}")
    code, out = cli("hover", "--selector", "#hover")
    expect(code == 0, f"copper hover: {out}")
    for words in [("scroll", "200"), ("wait", "--time", "0.2"), ("back",), ("forward",)]:
        code, out = cli(*words)
        expect(code == 0, f"copper {' '.join(words)}: {out}")
    expect(front() == user, "the user's tab changed")
    print("ok 7 CLI: pdf, upload, dialog, select, hover, scroll, wait, back, forward")

    # 8. Shared with agents: the HttpOnly value shows -------------------------------
    stop(app)
    app = start({"agent.credentials.shareAll": True})
    session = None
    call("browser_tabs", {"action": "new", "url": f"{site}/cookie.html"})
    said = until("the HttpOnly value with access", lambda: (lambda s: s if "hc_session=s3cret" in s else None)(
        call("browser_cookies", {"name": "hc_session"})))
    show("cookies shared", said)
    print("ok 8 cookies: the HttpOnly value shows once every account is shared with agents")
    print("PASS")
finally:
    cleanup()
