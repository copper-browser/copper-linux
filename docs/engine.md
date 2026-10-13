# The engine on Linux — what WebKitGTK can and can't do

Copper for Linux renders pages with **WebKitGTK 6.0**. The engine comes
either from the distribution (the `.deb`, through apt) or from the GNOME
runtime on Flathub (the Flatpak). Copper turns on whatever the engine has. It
cannot add what the engine was built without.

This page records what each engine was measured to do, and why `install.sh`
picks the `.deb` on Debian and Ubuntu and the Flatpak everywhere else.

## Measured

Measured on 2026-10-13 on an Ubuntu 24.04 aarch64 machine with no GPU.

- **Ubuntu (`.deb`):** `libwebkitgtk-6.0-4` 2.52.6, GStreamer 1.24.2. Results
  from the W5 QA run.
- **Flatpak:** the `org.gnome.Platform//51` runtime with WebKitGTK 2.54.1 and
  `org.freedesktop.Platform.codecs-extra` 26.08-extra. These results come
  from a standalone WebKitGTK probe running inside that runtime, under the
  same sandbox permissions the Copper Flatpak gets (network, Wayland, DRI,
  PulseAudio). The probe loads the WebRTC samples'
  [pc1](https://webrtc.github.io/samples/src/content/peerconnection/pc1/)
  page with mock capture devices, then plays local files: H.264/AAC `.mp4`,
  HEVC/AAC `.mp4`, AV1/Opus `.webm`.

| | Ubuntu 24.04 `.deb`, stock | `.deb` + its Recommends | Flatpak (GNOME 51) |
|---|---|---|---|
| H.264 / AAC `<video>` | no (`NotSupportedError`) | plays | plays (640×360, advancing) |
| HEVC, AV1 | no | plays | plays |
| VP8 / VP9 | plays | plays | plays |
| MSE: mp4/H.264, mp4/AAC, webm/Opus | no / no / no | yes | yes |
| YouTube-style MSE, 4K DASH | no | plays (DASH-IF 2160p) | not measured; same codecs present |
| `RTCPeerConnection` (WebRTC) | undefined | undefined | **undefined**, even with `enable-webrtc` on |
| EME (`requestMediaKeySystemAccess`) | undefined | undefined | undefined |
| WebAuthn (`PublicKeyCredential`) | undefined | undefined | undefined |
| `getUserMedia` | present | present | present |

"Stock" means what `libwebkitgtk-6.0-4` pulls in by itself:
`gstreamer1.0-plugins-base`, `-good`, `-gl` and `-x`. The Recommends that the
`.deb` adds are `gstreamer1.0-libav`, `-plugins-bad`, `-plugins-good`, `-gl`,
an audio sink (`-pipewire`, `-pulseaudio` or `-alsa`), `libgl1-mesa-dri`, a
portal backend and `fonts-noto-color-emoji`.

The GNOME 51 runtime ships `webrtcbin`, `nicesrc`/`nicesink`, `dtls*`, `srtp*`
and, through codecs-extra, `avdec_h264` and `x264enc`. The GStreamer side of
WebRTC is therefore all there. WebKitGTK itself was built without it:
`WebKit.Settings.get_all_features()` lists no PeerConnection feature in either
engine, and `RTCPeerConnection` stays undefined after `enable-webrtc`,
`enable-media-stream` and `enable-encrypted-media` are switched on.

## What that means for you

- **Video works** with either package, as long as the `.deb`'s Recommends are
  installed. `install.sh` installs them and lists what it added. On a system
  where apt was told to skip Recommends, run
  `sudo apt install gstreamer1.0-libav gstreamer1.0-plugins-bad`. The Flatpak
  gets the same codecs from the `codecs-extra` extension, which Flatpak
  installs with the runtime.
- **Calls don't work** in either package. Google Meet, Teams, Slack huddles
  and Discord voice need WebRTC. Use their desktop apps, or another browser
  for calls.
- **DRM doesn't work.** There is no Widevine for WebKitGTK, so Netflix,
  Spotify, Disney+ and similar sites can't play.
- **Passkeys and security keys don't work.** Sites that offer a passkey fall
  back to a password.

## Which package `install.sh` picks

| System | Default | Why |
|---|---|---|
| Ubuntu 24.04+, Debian 13+ and derivatives (apt has `libwebkitgtk-6.0-4`) | `.deb` | The engine matches the Flatpak's in everything measured above. It updates with the system, has no ~1 GB runtime to download, uses the Secret Service directly rather than through the portal, and the AppArmor profile lets WebKit's own sandbox run. |
| Fedora, Arch, openSUSE, older Debian/Ubuntu, anything else | Flatpak | No WebKitGTK 6.0 `.deb` to depend on there. The runtime brings its own WebKitGTK and codecs. |

`--flatpak` or `--deb` (or `COPPER_METHOD`) overrides the choice. Switching
packages starts from an empty profile: the Flatpak keeps its data in
`~/.var/app/com.copperbrowser.Copper/`, the `.deb` in `~/.local/share/copper/`.

A Flatpak whose runtime ships WebRTC would change this choice. So would a
distribution WebKitGTK that ships it. To check an engine, run the probe
below with it.

## Measure it again

Prerequisites: the GNOME 51 runtime is installed, and a few media files are
being served on `http://127.0.0.1:8799`.

```sh
flatpak run --socket=wayland --share=network --device=dri --socket=pulseaudio \
  --filesystem=$PWD/docs:ro --command=python3 org.gnome.Platform//51 \
  $PWD/docs/engine-probe.py on http://127.0.0.1:8799
```

The server must also serve a `video.html` that holds `<video id=v muted>`,
next to `h264_aac.mp4`, `hevc_aac.mp4` and `av1_opus.webm`. On the
distribution's engine, run the same script with the system `python3` and
`gir1.2-webkit-6.0`. On Ubuntu 23.10 and later, WebKit's sandbox can't start
from `python3` until an AppArmor profile grants it `userns`, the same way
`packaging/debian/copper.apparmor` does for `copper`.

[`engine-probe.py`](engine-probe.py) makes a WebKitGTK `WebView` with `enable-webrtc`,
`enable-media-stream`, `enable-encrypted-media` and
`enable-mock-capture-devices` on. It runs pc1's Start and Call, reports
`typeof RTCPeerConnection` and the remote video size, and then plays each
media file for 2.5 s.
