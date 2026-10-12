#!/usr/bin/env python3
"""Write copper-version.json for a Linux release.

    packaging/version-manifest.py --dist dist/ --version 0.1.0 --tag v0.1.0 \
        --commit <sha> --repo copper-browser/copper-linux \
        --published-at 2026-10-12T00:00:00Z > dist/copper-version.json

Same top-level shape as the macOS manifest (copper-browser/Copper
.github/workflows/release.yml, read by Fork/Updates.swift): schemaVersion,
product, version, releaseId, commit, source, status, publishedAt, installer,
latestUrl, notes. The macOS per-archive fields (arch, zip, sha256, archiveUrl,
hasCli) describe one zip; a Linux release has four artifacts, so they live in
`assets` instead, one entry per file, and `platform` says "linux". Swift's
decoder treats the per-archive fields as optional, so a Mac reading this file
by mistake does not break; it is never served from the Mac feed anyway.

Every asset named below must be in --dist together with its .sha256 file.
"""

import argparse
import hashlib
import json
import os
import re
import sys

# kind, arch (as the package format names it), stable name, versioned name.
ASSETS = [
    ("deb", "amd64", "copper_amd64.deb", "copper_{v}_amd64.deb"),
    ("deb", "arm64", "copper_arm64.deb", "copper_{v}_arm64.deb"),
    ("flatpak", "x86_64", "copper-x86_64.flatpak", "copper-{v}-x86_64.flatpak"),
    ("flatpak", "aarch64", "copper-aarch64.flatpak", "copper-{v}-aarch64.flatpak"),
]


def first_paragraph(path: str) -> str:
    """NOTES.md's first paragraph, joined into one line (the Mac rule)."""
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
    except OSError:
        return ""
    for para in re.split(r"\n\s*\n", text):
        if para.strip():
            return " ".join(line.strip() for line in para.splitlines() if line.strip())
    return ""


def sha256(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--dist", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--tag", required=True)
    p.add_argument("--commit", required=True)
    p.add_argument("--repo", required=True)
    p.add_argument("--published-at", required=True)
    p.add_argument("--notes", default="NOTES.md")
    p.add_argument("--status", default="lkg")
    a = p.parse_args()

    base = f"https://github.com/{a.repo}/releases"
    assets = []
    for kind, arch, stable, versioned in ASSETS:
        # Asset names use the Cargo (semver) version, not the Debian one
        # (0.1.0~rc.1-1): GitHub rewrites "~" in asset names.
        name = versioned.format(v=a.version)
        path = os.path.join(a.dist, name)
        if not os.path.isfile(path):
            print(f"version-manifest: missing {path}", file=sys.stderr)
            return 1
        digest = sha256(path)
        with open(path + ".sha256", encoding="utf-8") as f:
            recorded = f.read().split()[0]
        if recorded != digest:
            print(f"version-manifest: {name}.sha256 says {recorded}, file is {digest}", file=sys.stderr)
            return 1
        assets.append(
            {
                "kind": kind,
                "arch": arch,
                "name": name,
                "size": os.path.getsize(path),
                "sha256": digest,
                "url": f"{base}/download/{a.tag}/{name}",
                "latestName": stable,
                "latestUrl": f"{base}/latest/download/{stable}",
            }
        )

    manifest = {
        "schemaVersion": 1,
        "product": "copper",
        "platform": "linux",
        "version": a.version,
        "releaseId": a.tag,
        "commit": a.commit,
        "source": f"https://github.com/{a.repo}/commit/{a.commit}",
        "status": a.status,
        "publishedAt": a.published_at,
        "installer": "install.sh",
        "latestUrl": f"{base}/latest",
        "notes": first_paragraph(a.notes),
        "assets": assets,
    }
    json.dump(manifest, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
