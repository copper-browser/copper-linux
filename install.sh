#!/bin/sh
#
# Copper installer for Linux — a small, fast WebKit browser.
#
#   curl -fsSL https://github.com/copper-browser/copper-linux/releases/latest/download/install.sh | sh
#
# Debian and Ubuntu (with WebKitGTK 6.0 in apt: Ubuntu 24.04+, Debian 13+):
# downloads the .deb for this machine from the latest GitHub release, checks
# its SHA-256, and installs it with apt (sudo), which pulls in WebKitGTK, GTK 4
# and libadwaita and loads Copper's AppArmor profile.
#
# Everything else (Fedora, Arch, openSUSE, older Debian/Ubuntu): downloads the
# .flatpak bundle, checks it, adds Flathub for this user if it is missing (the
# GNOME runtime comes from there), and installs it for this user (no sudo,
# unless flatpak itself has to be installed first).
#
# Re-running upgrades in place. x86_64 and aarch64 only.
#
# Flags:
#   --deb          install the .deb even if detection would pick Flatpak
#   --flatpak      install the Flatpak even on Debian/Ubuntu
#   --dry-run      say what would be done, download nothing, change nothing
#
# Environment:
#   COPPER_FEED     where release assets come from (default
#                   https://github.com/copper-browser/copper-linux/releases/latest/download;
#                   pin a version with …/releases/download/v0.1.0)
#   COPPER_METHOD   deb | flatpak (same as the flags)
#
# This file is attached verbatim to every GitHub release by
# .github/workflows/release.yml — keep it dependency-free POSIX sh (curl or
# wget, sha256sum, and the system's own package tools only).

set -eu

FEED="${COPPER_FEED:-https://github.com/copper-browser/copper-linux/releases/latest/download}"
FEED="${FEED%/}"
METHOD="${COPPER_METHOD:-}"
DRY=0
APP_ID=com.copperbrowser.Copper
FLATHUB=https://dl.flathub.org/repo/flathub.flatpakrepo

for arg in "$@"; do
	case "$arg" in
	--deb) METHOD=deb ;;
	--flatpak) METHOD=flatpak ;;
	--dry-run) DRY=1 ;;
	-h | --help)
		sed -n '2,33p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*)
		printf 'copper-install: unknown flag: %s\n' "$arg" >&2
		exit 2
		;;
	esac
done

# Everything said goes to stderr; stdout carries only download()'s path.
say() { printf 'copper-install: %s\n' "$*" >&2; }
die() {
	printf 'copper-install: %s\n' "$*" >&2
	exit 1
}
run() {
	if [ "$DRY" = 1 ]; then
		say "would run: $*"
	else
		"$@"
	fi
}

# Root runs things directly; anyone else goes through sudo (or doas).
SUDO=
if [ "$(id -u)" -ne 0 ]; then
	if command -v sudo >/dev/null 2>&1; then
		SUDO=sudo
	elif command -v doas >/dev/null 2>&1; then
		SUDO=doas
	fi
fi
as_root() {
	if [ "$(id -u)" -ne 0 ] && [ -z "$SUDO" ]; then
		die "need root for: $* (no sudo or doas found)"
	fi
	run $SUDO "$@"
}

fetch() { # url dest
	if command -v curl >/dev/null 2>&1; then
		curl -fsSL --retry 3 -o "$2" "$1"
	elif command -v wget >/dev/null 2>&1; then
		wget -q -O "$2" "$1"
	else
		die "need curl or wget"
	fi
}

[ "$(uname -s)" = Linux ] || die "this installer is for Linux; on a Mac use https://github.com/copper-browser/Copper"

case "$(uname -m)" in
x86_64 | amd64)
	DEB_ARCH=amd64
	FP_ARCH=x86_64
	;;
aarch64 | arm64)
	DEB_ARCH=arm64
	FP_ARCH=aarch64
	;;
*) die "unsupported architecture $(uname -m); Copper ships for x86_64 and aarch64" ;;
esac

# /etc/os-release: ID=ubuntu, ID_LIKE="debian" on derivatives (Mint, Pop!_OS…).
OS_ID=
OS_LIKE=
OS_NAME=Linux
if [ -r /etc/os-release ]; then
	# shellcheck disable=SC1091
	. /etc/os-release
	OS_ID="${ID:-}"
	OS_LIKE="${ID_LIKE:-}"
	OS_NAME="${PRETTY_NAME:-${NAME:-Linux}}"
fi
debian_family=0
case " $OS_ID $OS_LIKE " in
*" debian "* | *" ubuntu "*) debian_family=1 ;;
esac

# The .deb needs WebKitGTK 6.0 from apt; without it, the Flatpak brings its own.
has_webkit6_in_apt() {
	command -v apt-cache >/dev/null 2>&1 || return 1
	cand=$(apt-cache policy libwebkitgtk-6.0-4 2>/dev/null | sed -n 's/^ *Candidate: *//p')
	[ -n "$cand" ] && [ "$cand" != "(none)" ]
}

if [ -z "$METHOD" ]; then
	if [ "$debian_family" = 1 ] && has_webkit6_in_apt; then
		METHOD=deb
	else
		METHOD=flatpak
	fi
fi
say "$OS_NAME, $(uname -m): installing the $METHOD"

TMP=$(mktemp -d 2>/dev/null || mktemp -d -t copper-install)
trap 'rm -rf "$TMP"' EXIT INT TERM
# apt's _apt user must be able to read the .deb.
chmod 755 "$TMP"

# Download an asset and its .sha256 and check one against the other.
download() { # asset-name → path
	asset=$1
	if [ "$DRY" = 1 ]; then
		say "would download $FEED/$asset (+ .sha256)"
		echo "$TMP/$asset"
		return
	fi
	say "downloading $FEED/$asset"
	fetch "$FEED/$asset" "$TMP/$asset" || die "download failed: $FEED/$asset"
	fetch "$FEED/$asset.sha256" "$TMP/$asset.sha256" || die "download failed: $FEED/$asset.sha256"
	want=$(cut -d' ' -f1 <"$TMP/$asset.sha256")
	have=$(sha256sum "$TMP/$asset" | cut -d' ' -f1)
	if [ -z "$want" ] || [ "$want" != "$have" ]; then
		die "checksum mismatch for $asset (expected $want, got $have)"
	fi
	chmod 644 "$TMP/$asset"
	echo "$TMP/$asset"
}

install_deb() {
	[ "$debian_family" = 1 ] || command -v apt-get >/dev/null 2>&1 || die "--deb needs apt (Debian/Ubuntu)"
	deb=$(download "copper_$DEB_ARCH.deb")
	as_root apt-get install -y "$deb"
	say "installed. Open Copper from your apps, or run: copper"
}

install_flatpak_tool() {
	command -v flatpak >/dev/null 2>&1 && return 0
	say "flatpak is not installed; installing it"
	if command -v apt-get >/dev/null 2>&1; then
		as_root apt-get install -y flatpak
	elif command -v dnf >/dev/null 2>&1; then
		as_root dnf install -y flatpak
	elif command -v zypper >/dev/null 2>&1; then
		as_root zypper --non-interactive install flatpak
	elif command -v pacman >/dev/null 2>&1; then
		as_root pacman -S --noconfirm --needed flatpak
	else
		die "install flatpak with your package manager (https://flatpak.org/setup/), then run this again"
	fi
	[ "$DRY" = 1 ] || command -v flatpak >/dev/null 2>&1 || die "flatpak still not found after installing it"
}

install_flatpak() {
	install_flatpak_tool
	run flatpak remote-add --user --if-not-exists flathub "$FLATHUB"
	bundle=$(download "copper-$FP_ARCH.flatpak")
	# A bundle cannot be installed over an installed copy (flatpak says
	# "already installed", even with --reinstall), so a re-run takes the old
	# one out first. Uninstall keeps ~/.var/app/$APP_ID — tabs, history,
	# settings stay. The runtime comes from Flathub.
	if flatpak info --user "$APP_ID" >/dev/null 2>&1; then
		say "replacing the installed Copper (your data in ~/.var/app/$APP_ID stays)"
		run flatpak uninstall --user -y --noninteractive "$APP_ID"
	fi
	run flatpak install --user -y --noninteractive "$bundle"
	say "installed. Open Copper from your apps, or run: flatpak run $APP_ID"
	say "(a desktop that started before flatpak was installed may need a log-out to show it)"
}

case "$METHOD" in
deb) install_deb ;;
flatpak) install_flatpak ;;
*) die "unknown method '$METHOD' (deb or flatpak)" ;;
esac
