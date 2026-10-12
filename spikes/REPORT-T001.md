# T-001 report — WebKitGTK 6.0 from Rust

**Verdict: A-001 holds.** All nine risky calls work through the safe `webkit6`
bindings; nothing needed `webkit6-sys` or hand-written FFI. D-001 stays closed.

Spike: `spikes/hello` (`cargo run -p hello -- --selftest`; without the flag it
opens a window on the built-in page or the URL given). The selftest serves its
own pages from a tiny HTTP server thread on `127.0.0.1:<random>`, runs each
check with a 20 s timeout, and a watchdog thread exits the process after 150 s.

## Environment

| Thing | Version |
|---|---|
| Host | Ubuntu 24.04.5 LTS, aarch64 (devbox), headless weston 13.0.0 (`--backend=headless --renderer=pixman`) via `bin/dbx` |
| Toolchain | rustc / cargo 1.99.0 stable, edition 2024 |
| System libs | WebKitGTK 6.0 2.52.6, GTK 4.14.5, libadwaita 1.5.0, libsoup 3.4.4 |
| `webkit6` | 0.6.1 (feature `v2_52`) |
| `webkit6-sys` | 0.6.0 (transitive only, never called directly) |
| `javascriptcore6` | 0.6.0 |
| `soup3` | 0.9.0 |
| `gtk4` / `gdk4` | 0.11.5 (feature `v4_14`) |
| `glib` / `gio` | 0.22.10 |
| `libadwaita` | 0.9.2 (feature `v1_5`; workspace only, not used by the spike) |

`webkit6 0.6.1` requires the gtk-rs 0.11 / glib 0.22 line; workspace feature
ceilings match the oldest supported system (Ubuntu 24.04: GTK 4.14,
libadwaita 1.5, WebKitGTK 2.52).

## Selftest output

```
PASS load-url: uri=http://127.0.0.1:46881/ title="Copper spike"
PASS eval-js-json: {"height":3300,"nested":{"ok":true},"sum":6,"title":"Copper spike"} async={"later":"/"}
PASS script-message-handler: received {"n":42,"kind":"hello"}
PASS content-filter: 3 rules compiled; blocked.js + tracker.gif never reached server, allowed.js did; .ad display=none; page={"adDisplay":"none","allowed":true,"blocked":false,"gifLoaded":false}
PASS ephemeral-session-cookies: normal sees "spike=normal" (CookieManager ["spike=normal"]), ephemeral sees "spike=private" (CookieManager ["spike=private"])
PASS snapshot-visible: 1024x611 texture, 554 distinct colors (view height 611), saved /tmp/copper-hello/snapshot-visible.png
PASS snapshot-full-document: 1024x3300 texture, 1001 distinct colors (view height 611), saved /tmp/copper-hello/snapshot-full.png
PASS download: download-started=true finished, 47 bytes at /tmp/copper-hello-34635/downloads/copper-spike.bin (received_data_length=47)
PASS inspector-open: signal=attach attached=true inspected_uri=127.0.0.1
hello selftest: 9/9 passed
```

Both snapshot PNGs were inspected by eye: the visible one shows the first
(orange) screen with its heading; the full-document one shows all three
1100 px sections (3300 px tall).

## The nine calls

### 1. Load a URL — works
`WebView::load_uri`, completion via `connect_load_changed` (`LoadEvent::Finished`)
and `connect_load_failed`; `title()`, `uri()`.
```rust
view.connect_load_changed(move |_, ev| if ev == LoadEvent::Finished { done.set(true) });
view.connect_load_failed(move |_, _, uri, err| { /* record */ false });
view.load_uri(url);
```

### 2. Evaluate JS returning JSON — works
`WebViewExt::evaluate_javascript_future(script, world_name, source_uri)` →
`javascriptcore::Value`; `Value::to_json(indent)` gives a JSON string for
objects, strings, numbers. `call_async_javascript_function_future(body,
arguments: Option<&glib::Variant>, world, source_uri)` runs an async function
body (await works) — the right primitive for Copper's page tools. A named world
(`world_name: Some("copper")`) is available on both for isolation from page JS.
```rust
let v = view.evaluate_javascript_future("({title: document.title, sum: 6})", None, None).await?;
let json: serde_json::Value = serde_json::from_str(&v.to_json(0).unwrap())?;
let v2 = view.call_async_javascript_function_future(
    "await new Promise(r => setTimeout(r, 10)); return {later: location.pathname};",
    None, None, None).await?;
```

### 3. Script message handler — works
`UserContentManager::register_script_message_handler(name, world)` +
`connect_script_message_received(Some(name), |ucm, value| …)`; the page posts
with `window.webkit.messageHandlers.<name>.postMessage(obj)` (same as WKWebView).
`register_script_message_handler_with_reply` / `connect_script_message_with_reply_received`
(→ `ScriptMessageReply`) also exist for request/response messages.
```rust
ucm.register_script_message_handler("copper", None);
ucm.connect_script_message_received(Some("copper"), move |_, value| {
    *got.borrow_mut() = value.to_json(0).map(|s| s.to_string());
});
```

### 4. Content filter via `UserContentFilterStore` — works
`UserContentFilterStore::new(dir)`, `save_future(id, &glib::Bytes)` compiles
WebKit content-blocker JSON (the same format `Shield.swift` emits for
`WKContentRuleListStore`), `load_future(id)` reloads the compiled filter,
`UserContentManager::add_filter` / `remove_all_filters` / `remove_filter_by_id`
apply it. Proven with three rules: `block` on `blocked\.js`, `block` on
`tracker\.gif` (neither request reached the server; `allowed.js` did) and
`css-display-none` on `.ad` (computed `display: none`).
```rust
let store = UserContentFilterStore::new(work.join("filters").to_str().unwrap());
let filter = store.save_future("copper-spike", &glib::Bytes::from_static(RULES.as_bytes())).await?;
ucm.add_filter(&filter);
```

### 5. Ephemeral `NetworkSession` cookie isolation — works
`NetworkSession::new(Some(data_dir), Some(cache_dir))` for a persistent
profile, `NetworkSession::new_ephemeral()` for private; bind per view with
`WebView::builder().network_session(&s)`. Each session's
`cookie_manager().cookies_future(url)` returns only its own cookies
(`soup3::Cookie`); `document.cookie` agrees; `is_ephemeral()` reports
correctly. One `NetworkSession` per Copper profile/space is therefore the model
for T-010.
```rust
let private = NetworkSession::new_ephemeral();
let private_view = WebView::builder().network_session(&private).build();
let names = private.cookie_manager().unwrap().cookies_future(url).await?; // ["spike=private"]
```

### 6. `get_snapshot`, visible region — works
`WebViewExt::snapshot_future(SnapshotRegion::Visible, SnapshotOptions::NONE)`
→ `gdk::Texture` at view size; `Texture::save_to_png(path)` /
`save_to_png_bytes()`; `Texture::download(&mut buf, stride)` for raw pixels.
```rust
let tex = view.snapshot_future(SnapshotRegion::Visible, SnapshotOptions::NONE).await?;
tex.save_to_png("/tmp/copper-hello/snapshot-visible.png")?;
```

### 7. `get_snapshot`, full document — works
Same call with `SnapshotRegion::FullDocument`: 1024×3300 for a 3300 px page
while the view is 611 px tall. Snapshots work under headless weston with the
pixman renderer (software GL fallback; see risks).

### 8. Download — works
`WebViewExt::download_uri(url)` (or `NetworkSession::download_uri`) → `Download`;
`NetworkSession::connect_download_started` fires for any download in the
session (including navigation-triggered ones). On the `Download`:
`connect_decide_destination(|d, suggested| { d.set_destination(path); true })`
— in WebKitGTK 6.0 the destination is a **file path, not a URI** —
`connect_finished`, `connect_failed`, `connect_received_data`,
`received_data_length()`, `estimated_progress()`, `cancel()`.
```rust
let dl = view.download_uri(&format!("{base}/file.bin")).unwrap();
dl.connect_decide_destination(move |dl, _suggested| { dl.set_destination(dest); true });
dl.connect_failed(...); dl.connect_finished(...);
```
Note `finished` is also emitted after `failed`; track failure first.

### 9. Inspector open — works
`Settings::set_enable_developer_extras(true)` (via `WebViewExt::settings()`),
then `WebViewExt::inspector()` → `WebInspector::show()`. Signals
`connect_open_window` / `connect_attach` / `connect_closed`; `is_attached()`,
`web_view()` (the inspector's own `WebViewBase`), `detach()`, `close()`. In a
window the default is to attach (docked); returning `false` from the handlers
keeps WebKit's default behaviour.
```rust
let inspector = view.inspector().unwrap();
inspector.connect_attach(|_| false);
inspector.show(); // → attach signal, is_attached() == true
```

## Findings that matter for later tasks

- **Ubuntu 24.04 AppArmor blocks the WebKit sandbox.** With
  `kernel.apparmor_restrict_unprivileged_userns=1` (the 24.04 default) the web
  process fails at start (`bwrap: setting up uid map: Permission denied`,
  then `Failed to fully launch dbus-proxy`, abort). Ubuntu ships an
  `flags=(unconfined) { userns, }` profile for `/usr/bin/epiphany` for exactly
  this. Fixed on the devbox with `/etc/apparmor.d/copper-dev` covering
  `/home/farce/src/copper-linux/target/**`; **the .deb (T-023) must install an
  equivalent profile for `/usr/bin/copper`** (Flatpak is unaffected). Do not
  "fix" it with `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS`.
- Headless weston has no GPU: Mesa prints EGL/ZINK warnings and WebKit falls
  back to software rendering. Harmless for tests; snapshots render correctly.
- Every async WebKit call has a `*_future` variant usable from
  `glib::MainContext::spawn_local`; they must run on the GTK main thread (the
  bindings assert main-context ownership). T-005's Tokio side must hop to the
  main loop via a channel, as §6 already plans.
- `glib::future_with_timeout` gives cheap per-call timeouts on the main loop.
- The spike polls `Rc<Cell<bool>>` flags for signal completion; T-005 should
  turn signals into proper channels/streams.

## Not covered here (later spikes / tasks)

Off-screen agent tabs and trusted input (T-002), `UserScript` injection at
document start and isolated worlds for page JS (T-006), custom URI schemes,
permission requests, web-process-crash handling.
