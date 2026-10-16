#!/usr/bin/env python3
"""Re-capture the macOS MCP catalogue the Linux build serves word for word.

    packaging/capture-mac-mcp.py --port 4199 --token-file <probe agent.json>

POSTs `initialize` and `tools/list` to a running macOS Copper (a PROBE
world, never someone's everyday app; Jev mode on, so every tool is listed)
and writes `fixtures/mac/mcp-initialize.json` and
`fixtures/mac/tools-list.json` in the form they are kept in (sorted keys,
two-space indent, UTF-8, trailing newline). Prints the wire length
`crates/copper-mcp/tests/contract.rs` pins, and reminds you to bring the
canvas sentence of `catalogue::CANVAS_NOTE` in line with the instructions.
Standard library only.
"""
import argparse
import json
import os
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def post(port, token, body):
    req = urllib.request.Request(f"http://127.0.0.1:{port}/mcp", data=json.dumps(body).encode(), method="POST")
    req.add_header("Content-Type", "application/json")
    req.add_header("Authorization", f"Bearer {token}")
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.loads(r.read())


def keep(path, value):
    with open(path, "w", encoding="utf-8") as f:
        f.write(json.dumps(value, indent=2, ensure_ascii=False, sort_keys=True) + "\n")
    print("wrote", os.path.relpath(path, ROOT))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--token-file", required=True, help="the probe's agent.json")
    a = ap.parse_args()
    token = json.load(open(a.token_file))["token"]
    init = post(a.port, token, {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "capture", "version": "1"}}})
    tools = post(a.port, token, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
    tools["result"]["tools"].sort(key=lambda t: t["name"])
    keep(os.path.join(ROOT, "fixtures/mac/mcp-initialize.json"), init)
    keep(os.path.join(ROOT, "fixtures/mac/tools-list.json"), tools)
    wire = json.dumps({"jsonrpc": "2.0", "id": 2, "result": {"tools": tools["result"]["tools"]}},
                      separators=(",", ":"), ensure_ascii=False, sort_keys=True).replace("/", "\\/")
    print(f"contract.rs: assert_eq!(wire.len(), {len(wire.encode())});  ({len(tools['result']['tools'])} tools)")
    print("check catalogue::CANVAS_NOTE against the new instructions (Linux serves them minus the voice sentence)")


if __name__ == "__main__":
    main()
