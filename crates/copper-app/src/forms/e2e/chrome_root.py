#!/usr/bin/env python3
"""Build a Chrome-on-Linux user-data root for the creds-ui end-to-end run:
copper-import's bookmark fixtures (tests/fixtures/flow-chrome) plus a History
DB, SNSS session files and a Login Data DB with v10 and v11 rows — the same
shapes crates/copper-import/tests/chrome.rs builds.

    chrome_root.py <copper-import fixture dir> <root to create>

The v11 key password is printed on stdout for the caller to put in the
keyring as Chrome's "Chrome Safe Storage" item (application=chrome).
"""
import hashlib
import os
import shutil
import sqlite3
import struct
import sys

from cryptography.hazmat.primitives import padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

V11_PASSWORD = "c2VjcmV0LWNocm9tZS1rZXk="


def derive(pw: bytes) -> bytes:
    return hashlib.pbkdf2_hmac("sha1", pw, b"saltysalt", 1, 16)


def wrap(plain: bytes, key: bytes, tag: str) -> bytes:
    p = padding.PKCS7(128).padder()
    body = p.update(plain) + p.finalize()
    enc = Cipher(algorithms.AES(key), modes.CBC(b" " * 16)).encryptor()
    return tag.encode() + enc.update(body) + enc.finalize()


def u32(v):
    return struct.pack("<I", v)


def pad(b):
    return b + b"\0" * ((4 - len(b) % 4) % 4)


def field(s):
    b = s.encode()
    return u32(len(b)) + pad(b)


def field16(s):
    units = s.encode("utf-16-le")
    return u32(len(units) // 2) + pad(units)


def pickle(body):
    return u32(len(body)) + body


def command(cid, payload):
    return struct.pack("<H", len(payload) + 1) + bytes([cid]) + payload


def nav(tab, index, url, title):
    return pickle(u32(tab) + u32(index) + field(url) + field16(title) + u32(0) + u32(0))


def pair(a, b):
    return u32(a) + u32(b)


def session_file():
    out = b"SNSS" + u32(1)
    w1 = [
        (7, 0, [("https://mail.example/", "Mail")]),
        (8, 1, [("https://news.example/a", "A"), ("https://news.example/b", "B")]),
        (9, 2, [("chrome://settings/", "Settings")]),
        (10, 3, [("https://docs.example/", "Docs")]),
        (11, 4, [("https://gone.example/", "Gone")]),
    ]
    for tab, index, navs in w1:
        out += command(0, pair(1, tab))
        out += command(2, pair(tab, index))
        for i, (url, title) in enumerate(navs):
            out += command(6, nav(tab, i, url, title))
    out += command(7, pair(8, 0))
    out += command(12, u32(7) + bytes([1, 0, 0, 0]))
    closed = u32(11) + bytes(12)
    out += command(16, closed)
    out += command(8, pair(1, 3))
    out += command(0, pair(2, 20))
    out += command(2, pair(20, 0))
    out += command(6, nav(20, 0, "https://second.example/", "Second"))
    out += command(9, pair(3, 1))
    out += command(0, pair(3, 30))
    out += command(6, nav(30, 0, "https://app.example/", "App"))
    return out


def micros(unix):
    return (unix + 11_644_473_600) * 1_000_000


def main():
    fixture, root = sys.argv[1], sys.argv[2]
    if os.path.exists(root):
        shutil.rmtree(root)
    shutil.copytree(fixture, root)
    for prof, prefs in [
        ("Default", '{"profile":{"name":"Person 1"}}'),
        ("Profile 1", '{"profile":{"name":"Work"}}'),
        ("Profile 2", "{}"),
    ]:
        with open(os.path.join(root, prof, "Preferences"), "w") as f:
            f.write(prefs)
    for prof, rows in [
        ("Default", [("https://alpha.example/one", "One", 3, micros(1_700_000_300), 0),
                     ("https://hidden.example/", "Hidden", 5, micros(1_700_000_400), 1),
                     ("https://never.example/", "Never", 0, micros(1_700_000_500), 0)]),
        ("Profile 1", [("https://beta.example/", "Beta", 1, micros(1_700_000_600), 0)]),
    ]:
        db = sqlite3.connect(os.path.join(root, prof, "History"))
        db.execute("CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, "
                   "typed_count INTEGER, last_visit_time INTEGER, hidden INTEGER)")
        for url, title, count, last, hidden in rows:
            db.execute("INSERT INTO urls (url, title, visit_count, typed_count, last_visit_time, hidden) "
                       "VALUES (?, ?, ?, 0, ?, ?)", (url, title, count, last, hidden))
        db.commit()
        db.close()
    sessions = os.path.join(root, "Default", "Sessions")
    os.makedirs(sessions, exist_ok=True)
    with open(os.path.join(sessions, "Session_13400000000000001"), "wb") as f:
        f.write(session_file())
    work = os.path.join(root, "Profile 1", "Sessions")
    os.makedirs(work, exist_ok=True)
    with open(os.path.join(work, "Session_13400000000000009"), "wb") as f:
        f.write(b"SNSS" + u32(1) + command(0, pair(5, 1)) + command(6, nav(1, 0, "https://work.example/", "Work")))

    k10, k11 = derive(b"peanuts"), derive(V11_PASSWORD.encode())
    db = sqlite3.connect(os.path.join(root, "Default", "Login Data"))
    db.execute("CREATE TABLE logins (origin_url VARCHAR NOT NULL, action_url VARCHAR, "
               "username_element VARCHAR, username_value VARCHAR, password_element VARCHAR, "
               "password_value BLOB, signon_realm VARCHAR NOT NULL, date_created INTEGER NOT NULL, "
               "blacklisted_by_user INTEGER NOT NULL, scheme INTEGER NOT NULL, date_last_used INTEGER NOT NULL DEFAULT 0)")
    for origin, user, blob, never, used in [
        ("https://www.example.com/login", "ann", wrap(b"ann-v10-pw", k10, "v10"), 0, micros(1_700_000_000)),
        ("https://accounts.example.org/", "bob", wrap(b"bob-v11-pw", k11, "v11"), 0, 0),
        ("https://never.example/", "", b"", 1, 0),
    ]:
        db.execute("INSERT INTO logins (origin_url, username_value, password_value, signon_realm, "
                   "date_created, blacklisted_by_user, scheme, date_last_used) VALUES (?, ?, ?, ?, 0, ?, 0, ?)",
                   (origin, user, blob, origin, never, used))
    db.commit()
    db.close()
    print(V11_PASSWORD)


if __name__ == "__main__":
    main()
