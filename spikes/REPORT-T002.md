# T-002 report — off-screen agent tabs and trusted input

**Verdict: DONE.** One rendering approach passes every check, and input
needs no JavaScript for pointer moves, left clicks, text and editing keys.

- **Rendering → rung (a), in the form that works: `a2-ghost-host`.** The
  agent's page stays a child of a Copper window that is on screen, under a
  1×1 widget that keeps it *mapped* and snapshots it into a `GtkSnapshot` that
  is thrown away. WebKit counts the page as visible: rAF runs at 60/s,
  snapshots are live, and the user's window shows 0 changed pixels in a
  compositor screenshot. The literal (a), a non-visible `GtkStack` page,
  fails: WebKit treats an unmapped view as a hidden page (rAF 0/s), and no
  setting changes that.
- **Input → rung (2), plus (3) for the rest.** Emitting signals on WebKit's own
  GTK event controllers produces **trusted** hover, click, double click and
  click-to-focus, ≤ 2 ms after the emit. WebKit editing commands produce
  **trusted** `beforeinput`/`input` (typing, Backspace), and `InsertNewline`
  produces a trusted `submit` (Enter). Everything else is DOM events
  dispatched from script, which are untrusted. Rung (1) is rejected: in
  Ubuntu's `libwebkitgtk-6.0` build every automation input command returns
  `NotImplemented`, and even where it works it forces an ephemeral session
  and sets `navigator.webdriver = true`.
- **Trust gap, written down:** none of these paths can make a trusted
  `keydown`/`keyup`/`keypress`, right or middle click, click with modifiers,
  drag, or wheel event.

A-002 holds once it is restated as "mapped but never drawn", not "unmapped".
A-003 is false. The D-006 text for the plan is in [§ D-006](#d-006--decision-text-for-the-plan).

Spike: `spikes/backstage` (`cargo run -p backstage -- --selftest`). Each rung
runs in its own child process (`backstage --rung NAME`). The child builds the
same user window (a focused `GtkEntry` over a `GtkStack` showing the user's
tab), hosts an agent page in the rung's way, takes measurements, and prints
one JSON line. The selftest starts a private `weston --debug` so rungs can
take real compositor screenshots. It prints the table, writes
`/tmp/copper-backstage/results.json` and the PNGs, and exits 0 if a
rendering rung passes. Run time is about 96 s.

## Environment

| Thing | Version |
|---|---|
| Host | devbox, Ubuntu 24.04.5 aarch64, via `bin/dbx`; rungs run on a private `weston 13.0.0 --backend=headless --renderer=pixman --debug` (1440×900, no panel) |
| System libs | WebKitGTK 6.0 **2.52.6** (`2.52.6-0ubuntu0.24.04.1`), GTK 4.14.5 |
| Crates | `webkit6` 0.6.1 (`v2_52`), `gtk4`/`gdk4` 0.11.5 (`v4_14`), `glib` 0.22 |
| Rendering | software only (Mesa/llvmpipe and pixman; no GPU) |
| Source read | WebKit tag `webkitgtk-2.52.6`: `WebKitWebViewBase.cpp`, `AcceleratedBackingStore.cpp`, `WebAutomationSessionGtk.cpp`, `WebAutomationSession.cpp`, `WebKitAutomationSession.cpp`, `WebKitWebView.cpp`, `WebKitWebContext.cpp`, `WebKitInitialize.cpp`, `RemoteInspectorServer.cpp`, `RemoteInspectorGlib.cpp`, `SocketConnection.cpp`, `SessionHostGlib.cpp`, `Page.cpp`, `OptionsGTK.cmake`; Ubuntu's `debian/rules` for 2.52.6 |

## Selftest output (final run)

```
RENDERING (agent page in the hidden host; pass = rAF>=50/s, snapshot non-blank+live, user tab/focus/pixels unchanged, not shown to the user)
rung                     rAF/s  timers/s visibility snapshot colors/live   user-same  on-screen user rAF  verdict
z-baseline-visible        39.0      83.4 visible    811 / yes              no         yes            0.0  REF
a1-stack-hidden            0.0      83.4 hidden     811 / no               yes        no            62.4  FAIL
a2-ghost-host             60.0      83.5 visible    811 / yes              yes        no            62.5  PASS
b1-hidden-toplevel         0.0       1.0 hidden     ERR snapshot: There was an error creating the snapshot yes        no            62.4  FAIL
b2-second-toplevel        39.5      83.5 visible    811 / yes              no         yes           61.9  FAIL
b3-minimized-toplevel      0.0      83.4 visible    811 / no               yes        yes           62.4  FAIL
c-nested-compositor       39.5      83.0 visible    811 / yes              yes        no            62.3  FAIL
c2-nested-ghost           60.0      83.5 visible    811 / yes              yes        no            62.0  PASS
x-ghost-stalled            0.0      83.4 visible    811 / no               yes        no            61.9  FAIL
x-ghost-selfdrive-stalled    62.5      83.5 visible    811 / yes              yes        no            62.5  PASS
x-handoff-ghost           60.0      83.5 visible    811 / yes              yes        no            62.4  PASS
x-handoff-nested          59.9      83.4 visible    811 / yes              yes        no            62.4  PASS
  x-handoff-ghost hand-off to the user's stack: shown=yes rAF/s 60.0 -> 39.5 (visible baseline 39.0), snapshot live=yes
  x-handoff-nested hand-off to the user's stack: shown=yes rAF/s 59.9 -> 39.4 (visible baseline 39.0), snapshot live=yes

INPUT (agent page hosted a2-style; t = every event isTrusted, f = some untrusted, - = no event)
method                       hover   click   dblclk  focus   keys    input   submit  field value      user-same
(1) automation session       -       -       -       -       -       -       -                        yes
                             right-click contextmenu trusted=- | navigator.webdriver=true | session ephemeral=true | contexts=["http://127.0.0.1:38763/probe"]
(2a) controller signals      t       t       t       t       -       -       -       -                yes
(2b) editing InsertText      -       -       -       -       -       t       -       hello            yes
                             click pointerType=mouse | DeleteBackward value=hell input=t | InsertNewline submit=t
(3) JS-dispatched            f       f       -       -       f       f       t       via-js           yes

RENDER DECISION: a2-ghost-host (first rung in ladder order that passes every check)
details: /tmp/copper-backstage/results.json, snapshots /tmp/copper-backstage/*.png
EXIT=0
```

Across the full runs made while building the spike, the numbers stayed
within ±0.5 rAF/s and every rung got the same verdict each time.

### How each check is measured

- **rAF/s and timers/s.** The probe page counts `requestAnimationFrame` ticks
  and a 10 ms `setInterval`. The spike reads both counters plus
  `performance.now()`, waits 2.0 s, and reads them again. The 10 ms timer's
  ceiling is about 83/s, because WebKit clamps nested timers.
- **Snapshot.** Two `snapshot_future(Visible)` calls 500 ms apart.
  *Non-blank* means at least 16 distinct colours and more than 5 % non-white
  pixels. *Live* means the pixel at (100,100), a box whose hue changes every
  rAF tick, differs between the two snapshots. Each snapshot takes 5–8 ms.
- **User-same.** The spike captures all of the following before the agent
  page exists and again after all agent work, and every item must be equal:
  - the stack's visible child is still `user`;
  - `gtk_window_get_focus` is still the user's field;
  - `gtk_widget_pick` at the centre of the tab area still lands in the user's
    `WebView`;
  - the user page's `document.activeElement` is unchanged;
  - the GTK-rendered tab area has 0 changed pixels (overlay children
    included);
  - a `weston-screenshooter` capture of the whole output has 0 changed pixels
    (caret blink disabled).

  b2's second toplevel changed 750 674 screen pixels.
- **Limit of this environment.** Headless weston has no `wl_seat`, so no
  window ever becomes *active*. Keyboard focus is therefore checked as GTK's
  focus widget; window activation cannot be observed here.
- **Software rendering ceiling.** Every page that is actually composited
  runs at about 39 rAF/s on this box, because GSK and weston both render in
  software. `z-baseline-visible` (the probe shown as the user's own tab)
  gives that ceiling. That is why b2, c and the shown page after hand-off read
  about 39. The GhostHost rungs never composite the page, so they run at
  WebKit's 60 Hz.

## Rendering ladder

### Why the literal (a) cannot work: what WebKit means by "visible"

- **Visibility is decided in the UI process.**
  `webkitWebViewBaseUpdateVisibility` (WebKitWebViewBase.cpp) computes
  `ActivityState::IsVisible` as
  `gtk_widget_get_mapped(view) && toplevel->isInMonitor() && !isMinimized() && !isSuspended()`.
- **A hidden page loses rAF unconditionally.** `Page::setIsVisibleInternal(false)`
  (Page.cpp) calls `suspendScriptedAnimations()` without checking any
  setting.
- **No setting or env var reaches rAF.** `Settings::all_features()` was
  searched for anything about hidden pages, throttling or visibility. The
  matches were `DOMTimersThrottling`, `HiddenPageDOMTimerThrottling`,
  `HiddenPageDOMTimerThrottlingAutoIncreases`,
  `HiddenPageCSSAnimationSuspension` (all Embedder status),
  `PageVisibilityBasedProcessSuppression`,
  `ShouldTakeNearSuspendedAssertions` (Internal), and the audio/video
  visibility flags. Only timer throttling and CSS animation suspension can be
  changed. a1 turned them off: timers then ran at 83/s instead of 1/s
  (compare b1), but rAF stayed at 0. `set_is_muted` only
  affects audio. None of the `WEBKIT_*` env vars in the library
  (`strings libwebkitgtk-6.0.so`) touches hidden-page behaviour.
- **WebKit only acknowledges a frame when GTK draws the view.**
  `AcceleratedBackingStore::frameDone()` runs only from
  `AcceleratedBackingStore::snapshot()`, which is the widget's `snapshot`
  vfunc (AcceleratedBackingStore.cpp). A view that is mapped but never
  snapshotted renders one frame and stalls.

The GhostHost follows from these facts. Keep the view mapped so WebKit calls
it visible, and run its `snapshot` vfunc every frame so the frames are
acknowledged, but never hand the resulting render node to the renderer.

### Rung by rung

| Rung | What it is | rAF/s | timers/s | snapshot | user-same | Verdict |
|---|---|---|---|---|---|---|
| a1-stack-hidden | agent `WebView` as a non-visible `GtkStack` page; throttling features off | **0.0** (hidden) | 83.4 (1.0 with throttling on, see b1) | 811 colours, **stale** (frozen at the last frame) | yes | FAIL (rAF) |
| **a2-ghost-host** | agent view mapped in the user's window under a 1×1 `GhostHost` (overlay child) | **60.0** | 83.5 | 811, **live** | **yes**: 0 GTK px, 0 screen px | **PASS** |
| b1-hidden-toplevel | second `GtkWindow`, never presented | 0.0 | **1.0** (hidden-page timer throttling at its default) | **error** ("There was an error creating the snapshot", never realized) | yes | FAIL |
| b2-second-toplevel | second window, presented | 39.5 | 83.5 | live | **no**: 750 674 screen px changed | FAIL (visible to the user; Wayland gives no off-screen placement) |
| b3-minimized-toplevel | presented, then `minimize()` | **0.0** | 83.4 | stale | n/a | FAIL (frame callbacks stop; on a real desktop it still sits in the dock and alt-tab) |
| c-nested-compositor | window on a private headless weston, opened with `gdk::Display::open` | 39.5 (composited in software) | 83.0 | live | yes | FAIL (< 50; the cause is the software ceiling, not the approach) |
| **c2-nested-ghost** | GhostHost inside a window on the private weston | **60.0** | 83.5 | live | yes | **PASS**: the fallback |

The first passing rung in ladder order is **a2**. c2 also passes and is the
fallback for when no Copper window can host the page (risks R1, R2).

### Experiments that shape the design

- **Stalled frame clock (`x-ghost-stalled`).** This simulates a window that
  keeps committing frames while the compositor withholds frame callbacks, as
  happens to an occluded window. The test host sits in a minimized window
  with a spinning `GtkSpinner`. With nothing else done, the GhostHost was
  snapshotted once and the agent's **rAF stayed at 0**. Without the spinner
  the agent kept painting at 60/s: a window that never commits never waits
  for frame callbacks.
- **Self-drive (`x-ghost-selfdrive-stalled`).** Every 8 ms the host
  re-allocates its children and calls `gtk_widget_snapshot_child` into a
  discarded snapshot. **rAF went from 0 to 62.5/s.** Two details mattered:
  - Re-allocating is required. Without it GTK printed "Trying to snapshot
    WebKitWebView without a current allocation" every tick, because WebKit
    had queued a resize that the stalled layout phase never ran.
  - A `GtkWidgetPaintable` does **not** work as the driver. It only refreshes
    when the window draws the widget; measured rAF stayed at 0 with 638
    ticks.

  The timer costs nearly nothing when idle: the child's render node is cached
  until WebKit queues a draw, and the host allocated once in the a2 run (no
  relayout loop).
- **Hand-off.** This is the user picking the agent's tab. The test calls
  `unparent()` on the view, adds it to the user's stack and makes it the
  visible child. Starting from the GhostHost: shown, rAF 39.5 (equal to the
  visible baseline), live. Starting from the nested display: also shown,
  39.4, live. **Moving a `WebView` between two `GdkDisplay`s works.**
- **Page focus without GTK focus.** Emitting `enter` on the view's
  `GtkEventControllerFocus` makes `document.hasFocus()` go from false to true.
  GTK's focus widget stays on the user's field, and every user-state check
  still passed. `leave` turns it back off. This is the Linux equivalent of
  macOS Backstage's "says it is key". It also calls the IM filter's
  `notifyFocusedIn` (see risk R5).

## Input ladder

### (1) WebKitAutomationSession: rejected

- **The client side has no injection API.** `WebKitAutomationSession` exposes
  only `get_id`, `set/get_application_info`, `create-web-view` and
  `will-close`. Commands arrive over WebKit's **remote-inspector socket**, the
  channel `WebKitWebDriver` uses.
- **The spike speaks that protocol from inside the same process**
  (`src/automation.rs`, about 230 lines):
  - **Framing.** u32 big-endian body length, then one flags byte, then the
    message name, a NUL, and the GVariant body.
  - **Message sequence.** `StartAutomationSession (sa{sv})` →
    `DidStartAutomationSession` + `SetTargetList (ta(tsssb))` →
    `Setup (tt)` → `SendMessageToBackend (tts)` carrying JSON `Automation.*`
    commands.
  - **Start-up requirement.** `WEBKIT_INSPECTOR_SERVER=127.0.0.1:<port>` must
    be in the environment before WebKit initializes
    (`webkitInitialize → initializeRemoteInspectorServer`), and port 0 is
    rejected. The selftest passes the variable to the child process.
- **Measured.** The session starts (`automation-started` fired; connecting
  took 5 ms) and `Automation.getBrowsingContexts` answers. Then:
  1. **Every input command returns `NotImplemented`.**
     `performMouseInteraction` (Move, SingleClick, DoubleClick, Right),
     `performKeyboardInteractions` (`InsertByKey` text and Enter) and
     `performInteractionSequence` all fail. Ubuntu's `debian/rules` builds
     the 6.0 flavour with `-DENABLE_WEBDRIVER=OFF` (WebDriver is enabled only
     on the 4.1 build). `OptionsGTK.cmake` turns on
     `ENABLE_WEBDRIVER_{MOUSE,KEYBOARD,WHEEL}_INTERACTIONS` only when
     `ENABLE_WEBDRIVER` is on.
  2. **Only views built with `is-controlled-by-automation` can be driven.**
     `getBrowsingContexts` filters on it; the user's tab was not listed. The
     property is construct-only, so an agent could never drive a tab the user
     opened.
  3. **Those views are always ephemeral.** `WebKitWebView::constructed`
     replaces their network session with
     `webkit_web_context_get_network_session_for_automation()`, an ephemeral
     session (measured: `is_ephemeral() == true`). An agent tab could never
     be signed in as the user.
  4. **`navigator.webdriver === true`** on those pages (measured). Bot walls
     and Google sign-in check for it.
  5. **The inspector server is an unauthenticated TCP listener on loopback.**
     It exposes every page as a remote-inspector target, and a process can
     have only one automation session.

Points 2–5 hold on every distro, so (1) is rejected even where a build turns
WebDriver on.

### (2) Native paths

| Path | Result |
|---|---|
| Building a `GdkEvent` | Not possible in GTK4: the `gdk_*_event_new` constructors are private. `gtk_widget_activate` only emits a widget's `activate` signal, which is not a pointer event. |
| **Signal emission on WebKit's `GtkEventController`s**, found with the public `gtk_widget_observe_controllers()` (12 on a view) | **Works for the pointer.** `GtkGestureClick` (touch-only)`::released(n, x, y)` → `webkitWebViewBaseTouchRelease` synthesizes Motion+Press+Release through `webkitWebViewBaseSynthesizeMouseEvent` with the signal's coordinates and needs no `GdkEvent` (modifiers come from the current event, which is NULL, so 0). `GtkEventControllerMotion::enter(x, y)` → `NativeWebMouseEvent(point)`, a mouse move. **Measured:** hover, click, dblclick (`released(1)` then `released(2)`) and click-to-focus are all `isTrusted`; click at (400,140) has `detail` 1, `pointerType` `"mouse"`, mousedown button 0; latency from emit to DOM event 0.7–2.0 ms. **Does not work:** keys (`key-pressed` dereferences `gtk_event_controller_get_current_event`); the mouse `GestureClick` and `motion` handlers (they need the event too); scroll (same); touch drag (needs the gesture's start point); right click by long press (it sets `isLongPressed`, which only a real touch press resets, and that press grabs GTK focus; every later click would be a right click). |
| **WebKit editing commands** (`execute_editing_command[_with_argument]`, public API) | `InsertText("hello")` → trusted `beforeinput` + `input` (`insertText`), value "hello". `DeleteBackward` → trusted `deleteContentBackward`, value "hell". `InsertNewline` in an `<input>` inside a form → trusted `submit` (implicit submission). No `keydown`/`keyup`. Latency 0.7–1.3 ms. |
| Wayland virtual pointer/keyboard | Not on weston 13: `wayland-info` lists no `zwlr_virtual_pointer_manager_v1` or `zwp_virtual_keyboard_manager_v1`, and the headless backend has no `wl_seat` at all. Mutter does not offer them to clients either. Where they do exist (wlroots) they move the user's real pointer and keyboard focus, which breaks R-012. Rejected. |
| Web-process extension + `send_message_to_page` | No trusted path. The 6.0 web-process extension API has no DOM or event API (removed in the 2022 GLib API), and events created by JS in the extension are untrusted. Not built. |

### (3) JS-dispatched (`input.js` from T-005 today)

All measured **untrusted**: mousemove, mousedown, `click()`, keydown, and
`input` after the native value setter (value "via-js" was set). One
exception: `form.requestSubmit()` fires a **trusted** `submit`, because the
user agent fires it.

### Chosen combination and what reaches the page trusted

| `InputEvent` | Mechanism | `isTrusted` |
|---|---|---|
| `Move` (no modifiers) | `motion.emit("enter", x, y)` | yes |
| `Click` left, no modifiers, `count` n | `tap.emit("released", i, x, y)` for i in 1..=n | yes (incl. `dblclick`, focus) |
| `Click` right/middle or with modifiers | JS (`input.js`) | no |
| `Type(text)` | per character: JS `keydown` → unless prevented, `InsertText(ch)` → JS `keyup` | input/beforeinput yes; key events no |
| `Key` Enter / Backspace / Delete / arrows / Home / End / Ctrl+A/C/X/V/Z | JS `keydown` → unless prevented, editing command (`InsertNewline`, `DeleteBackward`, `DeleteForward`, `MoveLeft`…, `SelectAll`, `Copy`/`Cut`/`Paste`, `Undo`/`Redo`) → JS `keyup` | edit/input/submit yes; key events no |
| `Key` Tab, Escape, F-keys, other shortcuts | JS keydown/keyup (+ JS focus-next for Tab) | no |
| `Drag`, `Scroll` | JS (pointer events; `scrollBy`, whose `scroll` events are trusted) | no |

Only `InsertText`, `DeleteBackward` and `InsertNewline` were measured. The
other editing-command names come from WebKit's editor command table, and
T-005 must confirm each one.

## D-006 — decision text for the plan

> **D-006 — Off-screen agent tabs: a GhostHost in a Copper window, input
> through WebKit's own handlers.** When an agent works in a tab nobody is
> looking at, its `WebView` stays a child of a Copper window that is mapped
> and not minimized or suspended. It sits under a 1×1 widget that cannot be
> targeted, focused or seen by accessibility. That widget keeps the view
> mapped and snapshots it into a discarded `GtkSnapshot` every frame. An 8 ms
> timer also re-allocates and snapshots it, so a stalled window frame clock
> never stalls the agent. WebKit therefore counts the page as visible. T-002
> measured rAF at 60/s, live snapshots in 5 ms, and the user's tab, focus and
> pixels unchanged (0 changed screen pixels). When no Copper window can host
> the page, the same host goes into a window on a private headless weston
> opened as a second `GdkDisplay` (measured 60/s). If weston is not
> available, the page is held degraded: eval, snapshot and input still work,
> rAF is paused. Input: pointer moves and left clicks are emitted on the
> view's own GTK event controllers (trusted, ≤ 2 ms). Text and editing keys
> go through WebKit editing commands (trusted `beforeinput`/`input`; Enter
> submits). Every other key, right, middle and modified clicks, drag and
> wheel are DOM events from script (untrusted). `keydown`/`keyup` are never
> trusted. Rejected: a hidden `GtkStack` page or a window that was never
> presented (WebKit hides unmapped views: rAF 0/s); a second shown toplevel
> (the user sees it; Wayland cannot place windows off screen);
> `WebKitAutomationSession` (input is `NotImplemented` in Debian/Ubuntu's 6.0
> build, controlled views are forced onto an ephemeral session and report
> `navigator.webdriver`); Wayland virtual input (not offered, and it would
> move the user's real pointer). *Revisit when:* WebKitGTK gains a public
> input-synthesis API; WebKit changes the controller layout this relies on
> (watched by a start-up self-check and T-024); or GNOME's
> suspended-window behaviour (risk R1) makes hosting in a Copper window
> unreliable, in which case make the nested display the default.

## API sketch for T-005 / T-017

### `copper-engine` (engine-neutral additions)

```rust
/// R-012 / D-006: pages an agent works in while nobody looks at them.
pub trait Backstage {
    type Page: PageView;
    /// Keep `page` laid out at stage size, rendering and frame-acknowledged
    /// while it is not shown. Idempotent: refreshes the linger clock. A page
    /// shown in a window is left where it is (and only gets page focus).
    fn hold(&self, page: &Self::Page) -> Result<()>;
    /// Back to an ordinary background tab (hidden, throttled, free to sleep).
    fn release(&self, page: &Self::Page);
    fn holds(&self, page: &Self::Page) -> bool;
    /// For `copper tabs` / bench, like macOS `Backstage.report`.
    fn status(&self) -> BackstageStatus;
}

#[derive(Clone, Debug, Serialize)]
pub struct BackstageStatus { pub held: usize, pub mode: BackstageMode, pub stage: (u32, u32) }

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum BackstageMode { Idle, InWindow, Nested, Degraded }

// Engine gains:
//   type Backstage: Backstage<Page = Self::Page>;
//   fn backstage(&self) -> &Self::Backstage;

/// How an input action reached the page (for trails/probe; MCP tools ignore it).
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum Trust { Trusted, Mixed, Untrusted }

// PageView::input keeps its shape but reports how it was delivered:
//   fn input(&self, event: InputEvent) -> LocalFuture<Result<Trust>>;
```

`InputEvent` (T-005's enum) needs no change. Its doc comment should state the
trust table above instead of "dispatch DOM events from script".

### `copper-engine-webkitgtk`

```rust
/// `GhostHost` is spikes/backstage/src/ghost.rs: a 1x1 widget, can_target=false,
/// can_focus=false, accessible Hidden. `snapshot` snapshots each child into a
/// throwaway GtkSnapshot. `self_drive(8 ms)` re-allocates and snapshots
/// children from a timer.
pub struct WebKitBackstage { inner: Rc<Inner> }
struct Inner {
    host: GhostHost,
    slots: RefCell<Vec<(glib::WeakRef<gtk4::Window>, glib::WeakRef<gtk4::Overlay>)>>,
    held: RefCell<HashMap<PageKey, Held>>,       // last touch, page-focus emitted
    drive: Cell<Option<glib::SourceId>>,          // only while `held` is non-empty
    nested: RefCell<Option<NestedDisplay>>,       // started lazily: weston + gdk::Display::open
}
impl WebKitBackstage {
    /// The shell registers each window's root overlay. The host lives in the
    /// first registered window that is mapped and whose `gdk::Toplevel::state()`
    /// has neither MINIMIZED nor SUSPENDED, re-picked on `notify::state` and
    /// when windows close. With no such window it moves to the nested display
    /// (if `weston` is on PATH), else `Degraded`.
    pub fn add_window(&self, window: &gtk4::Window, slot: &gtk4::Overlay);
    /// Follow the size an active tab has (macOS `Backstage.stageSize`).
    pub fn set_stage_size(&self, w: i32, h: i32);
}
impl copper_engine::Backstage for WebKitBackstage { /* hold: host.hang(page.widget()); emit focus enter */ }

/// Found once per page in `WebKitPage::new` via `widget.observe_controllers()`:
/// the `EventControllerMotion`, the touch-only `GestureClick`, the
/// `EventControllerFocus`. A missing one → that event kind falls back to JS
/// and one log line (a start-up self-check also runs this against a scratch view).
struct Hands {
    motion: Option<gtk4::EventControllerMotion>,
    tap: Option<gtk4::GestureClick>,
    focus: Option<gtk4::EventControllerFocus>,
}

impl PageView for WebKitPage {
    fn input(&self, ev: InputEvent) -> LocalFuture<Result<Trust>> {
        // 1. Install a one-shot capture listener (copper-input world) for the
        //    event that ends this action (click / mouseup / input / keyup).
        // 2. Deliver:
        //    Move{no mods}            -> hands.motion.emit_by_name::<()>("enter", &[&x, &y])
        //    Click{Left, n, no mods}  -> for i in 1..=n { hands.tap.emit_by_name::<()>("released", &[&(i as i32), &x, &y]) }
        //    Type(s)                  -> per char: js keydown; if !defaultPrevented { view.execute_editing_command_with_argument("InsertText", ch) }; js keyup
        //    Key{key, text, mods}     -> js keydown; if !prevented { editing_command(&key, mods) (or InsertText(text)) else js default (Tab focus move) }; js keyup
        //    everything else          -> input.js (untrusted)
        // 3. Resolve when the listener fires (spike: ≤ 2 ms), time out at 1 s.
    }
}
```

The agent-facing views in the backstage must not open anything over the
user's window. T-005 or T-017 should handle these signals for held pages:
`context-menu` (return true; the spike does this), `show-option-menu`
(`<select>`; render it in the page or pick the option through script),
`run-file-chooser`, `run-color-chooser`, `script-dialog` (answer it for the
agent), `enter-fullscreen` (deny), `print`, `permission-request` (policy) and
`show-notification`.

### `copper-app/src/agents/backstage.rs` (T-017)

This file is the policy layer. It mirrors `Fork/MCP/Backstage.swift`:
- `AgentTabs.prepare(tab)`: wake the tab, then `engine.backstage().hold(page)`.
  Wait about 120 ms after a fresh hold, as macOS does.
- Hold for 5 min after the agent's last call (`Backstage.linger`). A 0.5 s
  sweep releases expired holds and holds of closed tabs.
- When the user selects a held tab, the shell moves its widget into the stack
  (hand-off is measured to work). When the user leaves it while the hold is
  live, the page goes back to the host.

## Residual risks

- **R1 — GNOME suspended windows.** Mutter (45 and later) sends
  `GDK_TOPLEVEL_STATE_SUSPENDED` for minimized windows and windows on other
  workspaces, and possibly for fully covered ones. WebKit then hides *every*
  page in that window, including held ones. This cannot be reproduced on
  headless weston, because weston never sends suspended or minimized state.
  The mitigation is the host-window policy plus the c2 fallback; T-017 must
  test it on a real GNOME session.
- **R2 — Shipping weston for the c2 fallback.** It means a `.deb`
  `Recommends: weston` and bundling it in the Flatpak. If neither is
  acceptable, the fallback is `Degraded`. Decide in T-023.
- **R3 — WebKit internals.** The controller trick relies on
  WebKitWebViewBase's controller layout and handlers, which are not API.
  They were verified in the 2.52.6 source and by measurement. Mitigations:
  the start-up self-check, automatic fallback to JS, and a T-024 watch-list
  item.
- **R4 — The trust gap.** Sites that require a trusted `keydown` (some
  editors, games, key-capture widgets) or a trusted right click or drag will
  see untrusted events. This is the same limit macOS Copper avoided with
  NSEvents.
- **R5 — IME.** Page focus by emitting focus `enter` also calls the IM
  filter's `notifyFocusedIn` for the agent view. On a desktop with IBus or
  Fcitx this might move the IM's focus away from the user's field. It is
  untested here (no seat: one `gdk_seat_get_keyboard` critical was logged).
  Gate page focus behind a check in T-017. Click-to-focus works without it
  (trusted `focus` event measured with `hasFocus()` false).
- **R6 — Popups from held pages.** The signals listed under the API sketch
  must be intercepted, or `<select>` menus and file choosers will open over
  the user's window.
- **R7 — Accessibility.** The host is marked `Hidden`, but whether Orca skips
  the subtree is not verified.
- **R8 — Cost.** A held page renders at full rate, on the GPU on real
  machines. GTK never composites it, and the self-drive timer costs almost
  nothing when there is no new frame.
- **R9 — Environment.** Everything here is from software rendering on
  headless weston, aarch64. X11 is untested; the approach does not depend on
  Wayland.

## Files and commands

- **Files changed:**
  - `Cargo.toml` (one line: `"spikes/backstage"` in `members`)
  - `spikes/backstage/Cargo.toml`
  - `spikes/backstage/src/{main,rung,ghost,automation,server}.rs` (2 010 lines)
  - this report
  - `Cargo.lock` was refreshed by `bin/dbx` (adds the `backstage` package).
- **Commands:**
  - `bin/dbx cargo run -p backstage -- --selftest` → exit 0
  - `bin/dbx cargo clippy -p backstage --all-targets -- -D warnings` → clean
  - `bin/dbx cargo fmt -p backstage --check` → clean
- **Devbox side effect:** installed `wayland-utils` (`wayland-info`) to list
  weston's globals.
