#!/usr/bin/env python3
"""WebKitGTK engine probe (not Copper): WebRTC (webrtc samples pc1 with mock
capture devices) and H.264 playback from a local mp4.
  python3 docs/engine-probe.py default|on BASE_URL   (see docs/engine.md)"""
import json, sys
import gi
gi.require_version("Gtk", "4.0")
gi.require_version("WebKit", "6.0")
from gi.repository import Gtk, WebKit, GLib

mode, base = sys.argv[1], sys.argv[2]
PC1 = "https://webrtc.github.io/samples/src/content/peerconnection/pc1/"
STEPS = [
  (PC1, """
    const r = {engine: '%s', RTCPeerConnection: typeof RTCPeerConnection,
      getUserMedia: typeof (navigator.mediaDevices && navigator.mediaDevices.getUserMedia),
      EME: typeof navigator.requestMediaKeySystemAccess, PublicKeyCredential: typeof PublicKeyCredential};
    if (r.RTCPeerConnection !== 'function') return JSON.stringify(r);
    const sleep = ms => new Promise(f => setTimeout(f, ms));
    document.getElementById('startButton').click();
    for (let i = 0; i < 50 && document.getElementById('callButton').disabled; i++) await sleep(200);
    r.localVideo = document.getElementById('localVideo').videoWidth;
    document.getElementById('callButton').click();
    const rv = document.getElementById('remoteVideo');
    for (let i = 0; i < 75 && !(rv.videoWidth > 0 && rv.currentTime > 1); i++) await sleep(200);
    r.remoteVideo = rv.videoWidth + 'x' + rv.videoHeight; r.remoteTime = rv.currentTime;
    try { const s = await window.pc1.getStats(); s.forEach(x => { if (x.type === 'outbound-rtp' && x.kind === 'video') r.outCodec = x.codecId; if (x.type === 'codec' && /video/.test(x.mimeType)) (r.codecs = r.codecs || []).push(x.mimeType); }); } catch (e) { r.stats = String(e); }
    const caps = RTCRtpSender.getCapabilities && RTCRtpSender.getCapabilities('video');
    r.senderVideoCodecs = caps ? [...new Set(caps.codecs.map(c => c.mimeType))] : null;
    return JSON.stringify(r);
  """ % WebKit.get_minor_version()),
  (base + "/video.html", """
    const v = document.getElementById('v'); const r = {};
    r.canPlay = {h264: v.canPlayType('video/mp4; codecs="avc1.42E01E, mp4a.40.2"'),
      hevc: v.canPlayType('video/mp4; codecs="hvc1.1.6.L93.B0"'), av1: v.canPlayType('video/webm; codecs="av01.0.05M.08, opus"')};
    r.mse = {h264: MediaSource.isTypeSupported('video/mp4; codecs="avc1.42E01E"'), aac: MediaSource.isTypeSupported('audio/mp4; codecs="mp4a.40.2"'),
      opus: MediaSource.isTypeSupported('audio/webm; codecs="opus"'), vp9: MediaSource.isTypeSupported('video/webm; codecs="vp9"')};
    const sleep = ms => new Promise(f => setTimeout(f, ms));
    for (const f of ['h264_aac.mp4', 'hevc_aac.mp4', 'av1_opus.webm']) {
      v.src = f; let err = null; v.onerror = () => err = v.error && v.error.code;
      try { await v.play(); } catch (e) { err = String(e.name); }
      await sleep(2500);
      r[f] = {err, time: +v.currentTime.toFixed(2), size: v.videoWidth + 'x' + v.videoHeight};
      v.pause();
    }
    return JSON.stringify(r);
  """),
]

def main():
    app = Gtk.Application(application_id=None)
    def activate(app):
        win = Gtk.ApplicationWindow(application=app)
        s = WebKit.Settings()
        if mode == "on":
            for prop in ("enable-webrtc", "enable-media-stream", "enable-encrypted-media",
                         "enable-mock-capture-devices", "enable-media-capabilities"):
                s.set_property(prop, True)
        else:
            s.set_property("enable-mock-capture-devices", True)
        s.set_property("media-playback-requires-user-gesture", False)
        print("mode", mode, {p: s.get_property(p) for p in ("enable-webrtc", "enable-media-stream", "enable-encrypted-media")}, flush=True)
        v = WebKit.WebView(settings=s)
        v.connect("permission-request", lambda view, req: (req.allow(), True)[1])
        win.set_child(v); win.set_default_size(1000, 700); win.present()
        steps = list(STEPS)
        def next_step():
            if not steps:
                app.quit(); return
            url, js = steps.pop(0)
            def loaded(view, ev):
                if ev != WebKit.LoadEvent.FINISHED:
                    return
                view.disconnect(h[0])
                def done(view, res):
                    try:
                        print(url, view.call_async_javascript_function_finish(res).to_string(), flush=True)
                    except Exception as e:
                        print(url, "eval error", e, flush=True)
                    next_step()
                v.call_async_javascript_function(js, -1, None, None, None, None, done)
            h = [v.connect("load-changed", loaded)]
            v.load_uri(url)
        next_step()
        GLib.timeout_add_seconds(90, lambda: app.quit())
    app.connect("activate", activate)
    app.run([])

main()
