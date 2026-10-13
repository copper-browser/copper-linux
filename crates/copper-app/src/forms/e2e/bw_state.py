#!/usr/bin/env python3
"""The Bitwarden mock's state for the creds-ui run: its seed, with the
localhost item carrying a TOTP key and a PIN field (and no agent deny), so the
one-time-code and field fills can be exercised on the login fixture.

    bw_state.py <bw-mock script> <state.json>
"""
import importlib.machinery
import importlib.util
import json
import sys

loader = importlib.machinery.SourceFileLoader("bwmock", sys.argv[1])
spec = importlib.util.spec_from_loader("bwmock", loader)
mock = importlib.util.module_from_spec(spec)
loader.exec_module(mock)
st = mock.seed()
for item in st["items"]:
    if item["id"] == "i-local":
        item["login"]["totp"] = "otpauth://totp/dev?secret=JBSWY3DPEHPK3PXP"
        item["fields"] = [{"name": "PIN", "value": "9876", "type": 1}]
with open(sys.argv[2], "w") as f:
    json.dump(st, f, indent=1)
