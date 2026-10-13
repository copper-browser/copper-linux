#!/usr/bin/env python3
"""Two Coppers, one local copper-cloud: link, pair, sync both ways, share a canvas.

Run on Linux with a headless Wayland display (`bin/dbx`), against a LOCAL
copper-cloud (native PostgreSQL; never Docker, never a real instance):

    LINK_CODE_FILE=~/cloudui-e2e/link-code.txt \
    COPPER_BIN=$CARGO_TARGET_DIR/debug/copper OUT=/tmp/cloudui-e2e \
    python3 crates/copper-app/src/cloud/qa/e2e.py

Each Copper runs in its own COPPER_WORLD (`cua`, `cub`, wiped first) with
COPPER_CLOUD_BENCH set, and is driven through that socket: the real
Settings › Cloud page is typed into and pressed, local edits go through the
shell, and the other side is waited on until it shows them. Screenshots of
both windows land in $OUT.
"""

import http.server
import json
import os
import shutil
import socket
import socketserver
import subprocess
import sys
import threading
import time

BIN = os.environ.get("COPPER_BIN", os.path.expanduser("~/target-cloudui/debug/copper"))
OUT = os.environ.get("OUT", "/tmp/cloudui-e2e")
CODE = open(os.path.expanduser(os.environ["LINK_CODE_FILE"])).read().strip()
WWW_PORT = int(os.environ.get("WWW_PORT", "18900"))
WWW = f"http://127.0.0.1:{WWW_PORT}"

failures = []


def check(ok, what):
    print(("PASS " if ok else "FAIL ") + what, flush=True)
    if not ok:
        failures.append(what)
    return ok


def serve_pages():
    root = os.path.join(OUT, "www")
    os.makedirs(root, exist_ok=True)
    for name, title, colour in [
        ("a1", "Pinned on A", "#c8743c"), ("a2", "Kept on A", "#a95a28"), ("a3", "Open on A", "#e2a06a"),
        ("b1", "Kept on B", "#3e63dd"), ("b3", "Open on B", "#12a594"), ("ahist", "Visited on A", "#888"),
        ("bhist", "Visited on B", "#888"), ("abm", "Bookmarked on A", "#888"), ("bbm", "Bookmarked on B", "#888"),
    ]:
        with open(os.path.join(root, f"{name}.html"), "w") as f:
            f.write(f"<!doctype html><title>{title}</title><body style='font:20px system-ui;margin:3em;"
                    f"border-left:8px solid {colour};padding-left:1em'><h1>{title}</h1><p>{name}.html</p>")

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **k):
            super().__init__(*a, directory=root, **k)

        def log_message(self, *a):
            pass

    socketserver.TCPServer.allow_reuse_address = True
    httpd = socketserver.ThreadingTCPServer(("127.0.0.1", WWW_PORT), Quiet)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


class Copper:
    def __init__(self, world):
        self.world = world
        home = os.path.expanduser("~")
        for base in (".local/share", ".config", ".cache"):
            shutil.rmtree(os.path.join(home, base, f"copper-{world}"), ignore_errors=True)
        self.sock_path = f"/tmp/cloudui-{world}.sock"
        if os.path.exists(self.sock_path):
            os.remove(self.sock_path)
        env = dict(os.environ, COPPER_WORLD=world, COPPER_CLOUD_BENCH=self.sock_path,
                   COPPER_LOG=os.environ.get("COPPER_LOG", "warn,copper_app=info,copper_cloud=info"))
        self.log = open(os.path.join(OUT, f"{world}.log"), "w")
        self.proc = subprocess.Popen([BIN], env=env, stdout=self.log, stderr=subprocess.STDOUT)
        for _ in range(300):
            if os.path.exists(self.sock_path):
                break
            time.sleep(0.1)
        time.sleep(1.0)
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.connect(self.sock_path)
        self.f = self.s.makefile("rw")

    def __call__(self, op, **kw):
        kw["op"] = op
        self.f.write(json.dumps(kw) + "\n")
        self.f.flush()
        line = self.f.readline()
        out = json.loads(line) if line else {"error": "no answer"}
        short = json.dumps(out)
        print(f"[{self.world}] {op} -> {short[:600]}", flush=True)
        return out

    def shot(self, name):
        return self("screenshot", path=os.path.join(OUT, f"{self.world}-{name}.png"))

    def wait(self, what, value, timeout=60):
        return check(self("wait", what=what, value=value, timeout=timeout).get("ok") is True,
                     f"{self.world} sees {what} {value}")

    def canvas(self, tool, **args):
        return self("canvas", tool=tool, args=args)

    def quit(self):
        try:
            self("quit")
        except Exception:
            pass
        try:
            self.proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def text_of(answer):
    items = answer.get("items") or []
    return "\n".join(i if isinstance(i, str) else json.dumps(i) for i in items)


def main():
    os.makedirs(OUT, exist_ok=True)
    httpd = serve_pages()
    a = Copper("cua")
    b = Copper("cub")
    try:
        run(a, b)
    finally:
        a.quit()
        b.quit()
        httpd.shutdown()
    print("\n%d failure(s)" % len(failures))
    for f in failures:
        print("  - " + f)
    sys.exit(1 if failures else 0)


ONLY_CANVAS = os.environ.get("ONLY_CANVAS") == "1"


def trust_flow():
    """A third Copper: a wrong fingerprint is refused; a code without one shows
    the certificate's fingerprint and links only once it is trusted."""
    c = Copper("cuc")
    try:
        c("settings")
        head, frag = CODE.split("#", 1)
        parts = dict(p.split("=", 1) for p in frag.split("&"))
        wrong = f"{head}#k={parts['k']}&fp={'0' * 64}"
        r = c("connect", code=wrong, timeout=20)
        check(r["status"]["linked"] is False and bool(r.get("error")), "a wrong fingerprint is refused")
        c.shot("01-wrong-fingerprint")
        nofp = f"{head}#k={parts['k']}"
        said = c("type-code", code=nofp)
        check("No fingerprint" in (said.get("said") or ""), "a code without a fingerprint says so before sending")
        r = c("connect", keep=True, timeout=20)
        seen = (r.get("trust") or "").replace(":", "")
        check(seen == parts["fp"], "the untrusted flow shows the certificate's fingerprint")
        c.shot("02-untrusted-fingerprint")
        r = c("trust", timeout=20)
        check(r["status"]["linked"] is True, "Trust and connect links, pinned to that certificate")
        check((r["status"].get("fingerprint") or "").replace(":", "") == parts["fp"], "the link is pinned to the trusted fingerprint")
        c.shot("03-trusted-linked")
    finally:
        c.quit()


def run(a, b):
    trust_flow()
    link_and_pair(a, b)
    if not ONLY_CANVAS:
        browser_sync(a, b)
    canvas(a, b)


def link_and_pair(a, b):
    # ---- A: link code, account, sync.
    a("settings")
    a.shot("01-unlinked")
    said = a("type-code", code=CODE)
    check("Link code" in (said.get("said") or ""), "A reads the link code (host + fingerprint shown before sending)")
    check("127.0.0.1:18443" in (said.get("said") or ""), "A shows the host")
    a.shot("02-link-code-read")
    r = a("connect", keep=True)
    check(r["status"]["linked"] is True, "A linked")
    a.shot("03-linked-account-form")
    r = a("signup", email="ann@example.test", password="a long test password", name="Ann")
    check(r["status"]["signedIn"] is True, "A signed up and in")
    a.shot("04-signed-in-choose")
    r = a("turn-on")
    check(r["sync"]["on"] is True, "A sync on")
    a.wait("synced", "", 30)
    a.shot("05-sync-on")

    if not ONLY_CANVAS:
        a_local_data(a)
    pair(a, b)


def a_local_data(a):
    # ---- A: local data.
    a("space-add", name="Work A")
    a("open", url=f"{WWW}/a1.html", space="Work A", pin="A", foreground=False)
    a("open", url=f"{WWW}/a2.html", space="Work A", saved=True)
    a("open", url=f"{WWW}/a3.html", space="Work A")
    a("bookmark-add", url=f"{WWW}/abm.html", title="Bookmarked on A")
    a("visit", url=f"{WWW}/ahist.html", title="Visited on A")
    time.sleep(4)
    a("sync-now")
    a("settings-close")
    a.shot("06-main-work-a")


def pair(a, b):
    # ---- A: pairing code.
    a("settings")
    code = a("make-code").get("code")
    check(bool(code) and "#p=cp_" in code, "A made a pairing code")
    a("settings-scroll", fraction=1.0)
    a.shot("07-pairing-card")

    # ---- B: paste the pairing code.
    b("settings")
    b.shot("01-unlinked")
    said = b("type-code", code=code)
    check("Pairing code" in (said.get("said") or ""), "B reads the pairing code")
    b.shot("02-pairing-code-read")
    r = b("connect", keep=True, timeout=60)
    check(r["status"]["signedIn"] is True, "B paired and signed in")
    b.wait("phase", "Syncing", 60)
    b.shot("03-sync-on")


def browser_sync(a, b):
    # ---- B sees A's data.
    b.wait("space", "Work A", 90)
    b.wait("pin", f"{WWW}/a1.html", 60)
    b.wait("tab", f"{WWW}/a2.html", 60)
    b.wait("bookmark", f"{WWW}/abm.html", 60)
    b.wait("history", f"{WWW}/ahist.html", 90)
    b("sync-now")
    b.wait("device-tab", f"{WWW}/a3.html", 60)
    b("settings-scroll", fraction=0.35)
    b.shot("04-other-devices")
    b("settings-close")
    b("space-switch", name="Work A")
    b.shot("05-main-work-a-from-a")
    pairing = a("pairing")
    check("Used" in json.dumps(pairing), "A's pairing card says the code was used")

    # ---- B: local edits, A sees them.
    b("space-add", name="Work B")
    b("open", url=f"{WWW}/b1.html", space="Work B", saved=True)
    b("open", url=f"{WWW}/b3.html", space="Work B")
    b("bookmark-add", url=f"{WWW}/bbm.html", title="Bookmarked on B")
    b("visit", url=f"{WWW}/bhist.html", title="Visited on B")
    time.sleep(4)
    b("sync-now")
    a.wait("space", "Work B", 90)
    a.wait("tab", f"{WWW}/b1.html", 60)
    a.wait("bookmark", f"{WWW}/bbm.html", 60)
    a.wait("history", f"{WWW}/bhist.html", 90)
    a("sync-now")
    a.wait("device-tab", f"{WWW}/b3.html", 60)
    a("settings-close")
    a("space-switch", name="Work B")
    a.shot("08-main-work-b-from-b")

    # ---- Settings sync: dark on A shows up on B.
    a("look", look="dark")
    time.sleep(3)
    a("sync-now")
    b.wait("look", "dark", 60)
    a("settings")
    a.shot("09-settings-dark")
    a("settings-scroll", fraction=0.5)
    a.shot("10-settings-dark-middle")
    a("settings-scroll", fraction=1.0)
    a.shot("11-settings-dark-bottom")
    a("settings-close")
    b("settings")
    b.shot("06-settings-dark")
    b("settings-close")

    # ---- cloud_history_delete (MCP) from A.
    r = a("history-delete", args={"page": [f"{WWW}/ahist.html"]})
    t = r.get("text") or ""
    check('"deleted"' in t and '"what"' in t, "cloud_history_delete answers {deleted, …, what}")



def canvas(a, b):
    # ---- Canvas: shared board, both ways.
    r = a.canvas("canvas_create", name="Shared board")
    created = json.loads(text_of(r) or "{}")
    check(created.get("kind") == "shared", "A made a shared canvas")
    cid = created.get("id")
    r = a.canvas("canvas_open", id=cid, foreground=True)
    check("Shared board" in text_of(r), "A opened the shared canvas")
    r = a.canvas("canvas_apply", id=cid, ops=[{"op": "add", "shape": {"type": "sticky", "id": "s_a", "x": 0, "y": 0,
                                                                    "text": "Hello from A", "color": "yellow"}}],
                 **{"as": "e2e on A"})
    check('"applied":1' in text_of(r), "A applied a sticky")
    found = False
    for _ in range(30):
        if "Shared board" in text_of(b.canvas("canvas_list")):
            found = True
            break
        time.sleep(1)
    check(found, "B lists the shared canvas")
    r = b.canvas("canvas_open", id=cid, foreground=True)
    seen = False
    for _ in range(30):
        if "Hello from A" in text_of(b.canvas("canvas_read", id=cid)):
            seen = True
            break
        time.sleep(1)
    check(seen, "B reads A's sticky on the shared canvas")
    b.canvas("canvas_apply", id=cid, ops=[{"op": "add", "shape": {"type": "sticky", "id": "s_b", "x": 260, "y": 0,
                                                                 "text": "Reply from B", "color": "blue"}}],
             **{"as": "e2e on B"})
    seen = False
    for _ in range(30):
        if "Reply from B" in text_of(a.canvas("canvas_read", id=cid)):
            seen = True
            break
        time.sleep(1)
    check(seen, "A reads B's sticky on the shared canvas (live room)")
    probe = ("return JSON.stringify({wk: typeof window.webkit, h: !!(window.webkit && window.webkit.messageHandlers"
             " && window.webkit.messageHandlers.canvas), q: (window.__copperHost||{messages:[]}).messages.length,"
             " types: (window.__copperHost||{messages:[]}).messages.slice(-10).map(m => m.type)});")
    print("B probe:", json.dumps(b("canvas-eval", id=cid, js=probe)), flush=True)
    print("A probe:", json.dumps(a("canvas-eval", id=cid, js=probe)), flush=True)
    print("B read:", text_of(b.canvas("canvas_read", id=cid))[:800], flush=True)
    print("A hosts:", json.dumps(a("canvas-hosts")), flush=True)
    print("B hosts:", json.dumps(b("canvas-hosts")), flush=True)
    a.canvas("canvas_focus", id=cid, shapeIds=["s_a", "s_b"])
    b.canvas("canvas_focus", id=cid, shapeIds=["s_a", "s_b"])
    time.sleep(1.5)
    a.shot("12-canvas-shared-dark")
    b.shot("07-canvas-shared-dark")
    print(json.dumps(a("canvas-hosts")), flush=True)

    # ---- Personal canvas: local.
    r = a.canvas("canvas_apply", id="personal", ops=[{"op": "add", "shape": {"type": "text", "id": "p1", "x": 0, "y": 0,
                                                                           "text": "Only on A"}}])
    check('"applied":1' in text_of(r), "A wrote on its Personal canvas")
    r = a.canvas("canvas_read", id="personal")
    check("Only on A" in text_of(r), "A reads its Personal canvas back")
    r = a.canvas("canvas_screenshot", id=cid)
    check('"image"' in json.dumps(r), "canvas_screenshot returns a PNG")
    r = a.canvas("canvas_share_link", id=cid)
    check("copper://canvas/join/" in text_of(r), "canvas_share_link makes a link")

    # ---- Light again.
    a("look", look="light")
    time.sleep(3)
    a("sync-now")
    b.wait("look", "light", 60)
    a.canvas("canvas_open", id=cid, foreground=True)
    a.shot("13-canvas-shared-light")
    b.shot("08-canvas-shared-light")
    a("settings")
    a.shot("14-settings-light")
    a("settings-scroll", fraction=0.5)
    a.shot("15-settings-light-middle")
    a("settings-scroll", fraction=1.0)
    a.shot("16-settings-light-bottom")


if __name__ == "__main__":
    main()
