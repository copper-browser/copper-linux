# copper-pagejs

The JavaScript macOS Copper puts into pages, curated for Linux (WebKitGTK 6.0),
as `include_str!` assets with a typed Rust API (PLAN T-006).

- `js/extracted/` + `js/INVENTORY.md` — the raw, regenerable extraction of every
  Swift literal (reference only; `python3 js/extract.py`). Not used at runtime.
- `js/<feature>/*.js` — the curated assets below. Each starts with
  `// from Sources/Search/<path>:<line> <symbol>` (the Swift literal it came from,
  for diffing after Mac changes; extra `// also …` lines name other call sites
  of the same script) and is **one JavaScript function expression**. Bodies are
  verbatim from Swift; only the wrapper changed.
- `src/` — `Script` (the table: file, kind, world, installer), `Call<T>` (a
  script + its JSON arguments + its result type), `UserScript`, typed builders
  per feature (`page`, `jev`, `trail`, `probe`, `forms`, `curtain`, `tab`,
  `canvas`), the message-handler names and payloads (`messages`), and Rust
  ports of the Swift formatting MCP results depend on (`render::render` =
  `Tools.Page.render`, `jev::table` = `Ultrafast.table`, `curtain::css`).

## Parameters instead of interpolation

Swift built scripts by splicing values into source (`\(quote(text))`,
`` `\(escape(css))` ``) or by `callAsyncJavaScript(arguments:)`. Here every
script takes one JSON object: `(<function>)(<json>)`. Page text and agent input
never become JavaScript source; the only exception is `browser_evaluate`, whose
input *is* the agent's function.

```rust
let call = copper_pagejs::page::snapshot(None, false, 600);   // Call<String>
let body = call.function_body();       // installer (window.__copper) + invocation
// WebKitGTK: view.call_async_javascript_function_future(&body, None, call.world().name(), None)
let text: String = call.decode(value.to_json(0).as_deref())?;
```

- **`Call::function_body()`** — for `call_async_javascript_function` (awaits a
  returned promise). Use it for every call. The required installer
  (`page/helper.js`, `jev/fast.js`, `trail/layer.js`) runs first in the same
  turn; installers are idempotent, so this replaces the Mac's
  "probe `typeof window.__copper`, then install" round trip.
- **`Call::expression()`** — one expression for `evaluate_javascript`;
  synchronous calls only.
- **`UserScript::new` / `forms::relay()` …** — `source` is a complete
  statement plus injection time, frames and world for `WebKitUserScript`.
- `source(script) -> (&str, World)` returns the raw asset and its world.

Message handlers (`messages::HANDLERS`): `officeScroll`, `officeVeil`,
`officeImages`, `officeForms`, `canvas` in the page world, and `copperFrame` in
the isolated world `CopperCanvasFrames`. WebKitGTK exposes them exactly as
WKWebView does (`window.webkit.messageHandlers.<name>.postMessage`), so the
scripts post unchanged.

## Byte-for-byte with the Mac

MCP text produced in the page (snapshot with refs, find, get_text, console,
select/fill results) comes from the verbatim scripts. The Swift formatting
around it is ported and unit-tested against Swift's own output:
`render::render` (JSONSerialization pretty/sorted-keys quirks: `" : "`, `\/`,
`%.17g` numbers, `1`/`0` booleans, Foundation key collation) and `jev::table`
(checked against `fixtures/mac/mcp-examples/jev_observe.json`).

`tests/golden/mac/*.txt` are real answers from macOS Copper
`1.0.20261012.49` (headless instance, `SEARCH_PROBE=headless`, 1024×768) for
`tests/pages/mcp.html` and `jev.html`, captured with the same call sequence the
`mcp_sequence_matches_mac` / `jev_observe_matches_mac` tests replay. The
`### Page` block (tab id, background note) is Swift's `pageLine` and is not
compared. To refresh: serve `tests/pages` on `127.0.0.1:18765`, start a
headless Copper (`SEARCH_PROBE=headless SEARCH_MCP_PORT=4193`), and replay the
calls in that test with `copper --json call <tool> '<args>'`.

Known engine difference (not a script one): Safari's `innerText` includes a
`<select>`'s option texts and WebKitGTK 2.52's does not, so `browser_get_text`
over a form with a `<select>` lacks those lines on Linux. The test pins this
exact gap and nothing else.

Two deliberate Linux changes to the helper, both marked `// Linux:` in
`page/helper.js`:

- **Console from document start.** On the Mac the console capture starts when
  the helper is installed (first page-tool call), so what a page logged while
  loading is lost. `page/console_early.js` (`page::console_early()`) is the
  same capture as a document-start user script; the helper adopts its
  `window.__copperLogs` instead of hooking `console` again. Entries, text and
  the 200-line cap are unchanged, so `browser_console_messages` reads the same.
- **Instant scrolling into view.** `rect` and `click` scroll the target into
  view with `behavior: 'instant'`; the Mac's default follows CSS
  `scroll-behavior: smooth`, so on such pages the box was measured before the
  scroll and the real click landed on whatever had been there.

## Tests

`timeout 900 bin/dbx cargo test -p copper-pagejs -- --test-threads=1`

- unit tests: table completeness (every `js/**/*.js` is in `Script::ALL`, has a
  Swift header, is one function, has no `/*INTERP` left), call building,
  decoding, `render`, `jev::table`, `curtain::css`;
- `tests/webview.rs` (own `main`, GTK on the main thread): every script parses
  in WebKit, and every feature runs in a real WebKitGTK view against
  `tests/pages/*.html` served from a local HTTP server, with all handlers
  registered and their messages asserted. `cargo test -p copper-pagejs --test
  webview -- jev` runs only matching cases.

`login.html`, `autofill.html` and `canvas-fixture.html` are copied from
`copper-ref/docs/fixtures/`; the rest are written for these tests.

## Not ported (and why)

| Mac script | Why not on Linux v1 |
|---|---|
| `Page.swift:38` probe `typeof window.__copper`, `Ultrafast.swift:320`, `Trail.swift:126` install probes | replaced by idempotent installers run with each call |
| `Passkeys.swift:22` passkey shim | passkeys are out of scope (PLAN §1) |
| `StoreRelay.swift:40`, `Tab.swift:901` extension store button | no web extensions on WebKitGTK |
| `ExtensionPopup.swift`, `ExtensionShims.swift` | no web extensions on WebKitGTK |
| `Float.swift` picture-in-picture isolate | no always-on-top PiP on Wayland |
| `AnimatedBackdrop.swift`, `Backdrop/scene.js`, `three.module.min.js` | animated backdrop is out of scope |
| `Bench.swift`, `CanvasBench.swift`, `CanvasChecklistBench.swift`, `CanvasChatUI.swift` bench literals | Mac test benches |
| `CanvasImport.swift:243` legacy canvas migration | one-time Mac data migration |
| `Canvas/Resources/canvas.html` inline module | a bundled page T-022 hosts as is, not injected JS |

## Script index

| File | `Script` | Swift origin | Kind | World | Installer |
|---|---|---|---|---|---|
| `page/helper.js` | `PageHelper` | `Fork/MCP/Page.swift:138` `Page.helper` | installer | page |  |
| `page/snapshot.js` | `Snapshot` | `Fork/MCP/Page.swift:73` `Page.snapshot` | call | page | `page/helper.js` |
| `page/rect.js` | `Rect` | `Fork/MCP/Page.swift:82` `Page.prepare` | call | page | `page/helper.js` |
| `page/document_height.js` | `DocumentHeight` | `Fork/MCP/Page.swift:110` `Page.screenshot` | call | page |  |
| `page/click.js` | `Click` | `Fork/MCP/Tools.swift:310` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/hover.js` | `Hover` | `Fork/MCP/Tools.swift:320` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/focus.js` | `Focus` | `Fork/MCP/Tools.swift:334` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/set_value.js` | `SetValue` | `Fork/MCP/Tools.swift:341` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/submit.js` | `Submit` | `Fork/MCP/Tools.swift:349` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/fill.js` | `Fill` | `Fork/MCP/Tools.swift:360` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/key.js` | `Key` | `Fork/MCP/Tools.swift:370` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/select.js` | `Select` | `Fork/MCP/Tools.swift:377` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/drag.js` | `Drag` | `Fork/MCP/Tools.swift:390` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/has_text.js` | `HasText` | `Fork/MCP/Tools.swift:418` `Tools.dispatch` | call | page |  |
| `page/scroll.js` | `Scroll` | `Fork/MCP/Tools.swift:439` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/text.js` | `Text` | `Fork/MCP/Tools.swift:444` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/console.js` | `Console` | `Fork/MCP/Tools.swift:447` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/find_text.js` | `FindText` | `Fork/MCP/Tools.swift:451` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/element.js` | `Element` | `Fork/MCP/Tools.swift:405` `Tools.dispatch` | call | page | `page/helper.js` |
| `page/console_early.js` | `ConsoleEarly` | `Fork/MCP/Page.swift:138` `Page.helper` (its console capture) | user script: start, main frame | page |  |
| `jev/fast.js` | `JevFast` | `Fork/MCP/Ultrafast.swift:132` `Ultrafast.script` | installer | page |  |
| `jev/observe.js` | `JevObserve` | `Fork/MCP/Ultrafast.swift:365` `Ultrafast.observe` | call | page | `jev/fast.js` |
| `jev/marker.js` | `JevMarker` | `Fork/MCP/Ultrafast.swift:377` `Ultrafast.fresh` | call | page | `jev/fast.js` |
| `jev/check.js` | `JevCheck` | `Fork/MCP/Ultrafast.swift:385` `Ultrafast.fresh` | call | page | `jev/fast.js` |
| `jev/resolve.js` | `JevResolve` | `Fork/MCP/Ultrafast.swift:691` `Ultrafast.act` | call | page | `jev/fast.js` |
| `jev/dom_click.js` | `JevDomClick` | `Fork/MCP/Ultrafast.swift:721` `Ultrafast.act` | call | page | `jev/fast.js` |
| `jev/select_all.js` | `JevSelectAll` | `Fork/MCP/Ultrafast.swift:725` `Ultrafast.act` | call | page | `jev/fast.js` |
| `jev/set_value.js` | `JevSetValue` | `Fork/MCP/Ultrafast.swift:731` `Ultrafast.act` | call | page | `jev/fast.js` |
| `jev/settle.js` | `JevSettle` | `Fork/MCP/Ultrafast.swift:354` `Ultrafast.observe` | async call | page |  |
| `jev/scroll_by.js` | `JevScrollBy` | `Fork/MCP/Ultrafast.swift:688` `Ultrafast.act` | call | page |  |
| `trail/layer.js` | `TrailLayer` | `Fork/MCP/Trail.swift:146` `Trail.script` | installer | page |  |
| `trail/glide.js` | `TrailGlide` | `Fork/MCP/Trail.swift:55` `Trail.glide` | async call | page | `trail/layer.js` |
| `trail/fire.js` | `TrailFire` | `Fork/MCP/Trail.swift:140` `Trail.fire` | call | page | `trail/layer.js` |
| `trail/clear.js` | `TrailClear` | `Fork/MCP/Trail.swift:106` `Trail.clear` | call | page |  |
| `probe/perf.js` | `PerfProbe` | `Fork/MCP/Probe.swift:22` `Probe.script` | async call | page |  |
| `forms/relay.js` | `FormsRelay` | `Forms.swift:102` `FormRelay.script` | user script: end, main frame | page |  |
| `forms/without_passkeys.js` | `WithoutPasskeys` | `Forms.swift:90` `FormRelay.withoutPasskeys` | user script: start, all frames | page |  |
| `forms/fill_login.js` | `FormsFillLogin` | `Tab.swift:566` `Tab.fill` | call | page |  |
| `forms/submit.js` | `FormsSubmit` | `Tab.swift:574` `Tab.submitSignIn` | call | page |  |
| `forms/fill_otp.js` | `FormsFillOtp` | `Tab.swift:582` `Tab.fillOTP` | call | page |  |
| `forms/fill_values.js` | `FormsFillValues` | `Tab.swift:599` `Tab.fillValues` | call | page |  |
| `forms/fill_focused.js` | `FormsFillFocused` | `Tab.swift:613` `Tab.fillFocused` | call | page |  |
| `forms/fill_field.js` | `FormsFillField` | `Tab.swift:627` `Tab.fillField` | call | page |  |
| `forms/has_fields.js` | `FormsHasFields` | `Tab.swift:637` `Tab.hasFields` | call | page |  |
| `forms/fields_present.js` | `FormsFieldsPresent` | `Tab.swift:647` `Tab.fieldsPresent` | call | page |  |
| `forms/has_otp.js` | `FormsHasOtp` | `Tab.swift:658` `Tab.hasOTPField` | call | page |  |
| `forms/has_password.js` | `FormsHasPassword` | `Tab.swift:668` `Tab.hasPasswordField` | call | page |  |
| `forms/unsaved.js` | `FormsUnsaved` | `Tab.swift:813` `Tab.unsaved` | call | page |  |
| `curtain/picker.js` | `CurtainPicker` | `Curtain.swift:163` `Veiling.picker` | user script: start, main frame | page |  |
| `curtain/style.js` | `CurtainStyle` | `Curtain.swift:148` `Veiling.style` | user script: start, main frame | page |  |
| `curtain/start.js` | `CurtainStart` | `Tab.swift:486` `Tab.startPicking` | call | page |  |
| `curtain/stop.js` | `CurtainStop` | `Tab.swift:487` `Tab.stopPicking` | call | page |  |
| `curtain/peek.js` | `CurtainPeek` | `Tab.swift:682` `Tab.peek` | call | page |  |
| `curtain/unpeek.js` | `CurtainUnpeek` | `Tab.swift:687` `Tab.unpeek` | call | page |  |
| `reader/article.js` | `ReaderArticle` | `Reader.swift:11` `Reader.script` | call | page |  |
| `tab/scroll_relay.js` | `ScrollRelay` | `Tab.swift:1597` `ScrollRelay.script` | user script: end, main frame | page |  |
| `tab/swipe_calm.js` | `SwipeCalm` | `Swipe.swift:33` `Swipe.calm` | user script: start, main frame | page |  |
| `tab/swipe_watch.js` | `SwipeWatch` | `Swipe.swift:45` `Swipe.watch` | user script: start, all frames | page |  |
| `tab/image_relay.js` | `ImageRelay` | `ImageMenu.swift:32` `ImageRelay.watch` | user script: start, all frames | page |  |
| `tab/favicons.js` | `Favicons` | `Icons.swift:238` `Favicons.probe` | call | page |  |
| `tab/clear_selection.js` | `ClearSelection` | `Browser.swift:198` `Browser.closeFind` | call | page |  |
| `tab/ready_state.js` | `ReadyState` | `Tab.swift:966` `Tab.loadAndVerify` | call | page |  |
| `tab/scroll_to.js` | `ScrollTo` | `Tab.swift:1376` `PageView.smartMagnify` | call | page |  |
| `tab/smart_zoom.js` | `SmartZoom` | `Tab.swift:1395` `PageView.smart` | call | page |  |
| `tab/origin.js` | `Origin` | `Fork/StorageImport.swift:162` `StorageImport.put` | call | page |  |
| `tab/storage_put.js` | `StoragePut` | `Fork/StorageImport.swift:169` `StorageImport.put` | call | page |  |
| `canvas/frame_reporter.js` | `CanvasFrameReporter` | `Fork/Canvas/CanvasFrames.swift:114` `CanvasHost.frameReporter` | user script: start, all frames | isolated `CopperCanvasFrames` |  |
| `canvas/typing.js` | `CanvasTyping` | `Fork/Canvas/CanvasLean.swift:78` `CanvasHost.typing` | user script: start, main frame | page |  |
| `canvas/apply_update.js` | `CanvasApplyUpdate` | `Fork/Canvas/CanvasHost.swift:614` `CanvasHost.handToSiblings` | async call | page |  |
| `canvas/ready.js` | `CanvasReady` | `Fork/Canvas/CanvasHost.swift:658` `CanvasHost.ready` | async call | page |  |
| `canvas/theme.js` | `CanvasTheme` | `Fork/Canvas/CanvasHost.swift:711` `CanvasHost.watchAppearance` | async call | page |  |
| `canvas/is_live.js` | `CanvasIsLive` | `Fork/Canvas/CanvasHost.swift:742` `CanvasHost.adoptIfLive` | call | page |  |
| `canvas/apply.js` | `CanvasApply` | `Fork/Canvas/CanvasHost.swift:788` `CanvasHost.apply` | async call | page |  |
| `canvas/read.js` | `CanvasRead` | `Fork/Canvas/CanvasHost.swift:801` `CanvasHost.read` | async call | page |  |
| `canvas/select.js` | `CanvasSelect` | `Fork/Canvas/CanvasHost.swift:821` `CanvasHost.select` | async call | page |  |
| `canvas/zoom_to.js` | `CanvasZoomTo` | `Fork/Canvas/CanvasHost.swift:827` `CanvasHost.zoom` | async call | page |  |
| `canvas/presence.js` | `CanvasPresence` | `Fork/Canvas/CanvasHost.swift:835` `CanvasHost.presence` | async call | page |  |
| `canvas/export_state.js` | `CanvasExportState` | `Fork/Canvas/CanvasHost.swift:902` `CanvasHost.checkpoint` | async call | page |  |
| `canvas/ws_closed.js` | `CanvasWsClosed` | `Fork/Canvas/CanvasHost.swift:984` `CanvasHost.roomChanged` | async call | page |  |
| `canvas/ws_open.js` | `CanvasWsOpen` | `Fork/Canvas/CanvasHost.swift:998` `CanvasHost.tellOpen` | async call | page |  |
| `canvas/ws_message.js` | `CanvasWsMessage` | `Fork/Canvas/CanvasHost.swift:1034` `CanvasHost.hand` | async call | page |  |
| `canvas/set_share.js` | `CanvasSetShare` | `Fork/Canvas/CanvasHost.swift:1062` `CanvasHost.shareNow` | async call | page |  |
| `canvas/set_status.js` | `CanvasSetStatus` | `Fork/Canvas/CanvasHost.swift:1109` `CanvasHost.statusNow` | async call | page |  |
| `canvas/rename.js` | `CanvasRename` | `Fork/Canvas/CanvasHost.swift:1144` `CanvasHost.renamed` | async call | page |  |
| `canvas/frame_event.js` | `CanvasFrameEvent` | `Fork/Canvas/CanvasFrames.swift:98` `CanvasHost.frameEvent` | async call | page |  |
| `canvas/frame_host.js` | `CanvasFrameHost` | `Fork/Canvas/CanvasFrames.swift:106` `CanvasHost.tellFrameHost` | async call | page |  |
| `canvas/set_chat.js` | `CanvasSetChat` | `Fork/Canvas/CanvasChat.swift:205` `CanvasChat.tell` | async call | page |  |
| `canvas/show_chat.js` | `CanvasShowChat` | `Fork/Canvas/CanvasChat.swift:272` `CanvasChat.received` | async call | page |  |
| `canvas/reveal_chat.js` | `CanvasRevealChat` | `Fork/Canvas/CanvasChat.swift:468` `CanvasChat.open` | async call | page |  |
| `canvas/chat_send.js` | `CanvasChatSend` | `Fork/Canvas/CanvasChatUI.swift:246` `CanvasChat.runTool` | async call | page |  |
| `canvas/frame_poster.js` | `CanvasFramePoster` | `Fork/Canvas/CanvasFrames.swift:1` `CanvasHost.framePoster` (canvas-perf contract A.3; Mac line in wave 2) | async call | page |  |
