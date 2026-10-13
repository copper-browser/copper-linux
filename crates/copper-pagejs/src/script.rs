//! The script table: every curated asset under `js/`, what it is, where it runs.

/// The JavaScript world a script runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum World {
    /// The page's own world (`WKContentWorld.page`; `world_name = None` in WebKitGTK).
    Page,
    /// A named isolated world (`WKContentWorld.world(name:)`; `world_name = Some(name)`).
    Isolated(&'static str),
}

impl World {
    /// The WebKitGTK `world_name` argument: `None` for the page world.
    pub fn name(self) -> Option<&'static str> {
        match self {
            World::Page => None,
            World::Isolated(name) => Some(name),
        }
    }
}

/// When a user script goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Injection {
    DocumentStart,
    DocumentEnd,
}

/// Which frames a user script goes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Frames {
    MainFrame,
    AllFrames,
}

/// How a script is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Installs a `window.__…` object; idempotent. Run before the calls that
    /// [`Script::requires`] it (a [`crate::Call`] does that for you).
    Installer,
    /// A synchronous call: `evaluate_javascript` or `call_async_javascript_function`.
    Call,
    /// Returns a promise: run it with `call_async_javascript_function`, which awaits it.
    AsyncCall,
    /// Registered as a user script (`WKUserScript` / `WebKitUserScript`).
    UserScript(Injection, Frames),
}

/// The canvas frame reporter's isolated world (`CanvasHost.frameWorld`).
pub const FRAME_WORLD: &str = "CopperCanvasFrames";

macro_rules! scripts {
    ($( $(#[$doc:meta])* $variant:ident = $file:literal, $kind:expr, $world:expr, $requires:expr; )*) => {
        /// One curated script asset (`crates/copper-pagejs/js/<file>`).
        ///
        /// Every asset is a header (`// from Sources/Search/<path>:<line> <symbol>`)
        /// followed by exactly one JavaScript function expression; it is run as
        /// `(<source>)(<json args>)`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Script {
            $( $(#[$doc])* $variant, )*
        }

        impl Script {
            /// Every curated script, in table order.
            pub const ALL: &'static [Script] = &[ $( Script::$variant, )* ];

            /// Path relative to `crates/copper-pagejs/js/`.
            pub fn file(self) -> &'static str {
                match self { $( Script::$variant => $file, )* }
            }

            /// The whole asset: header comments and the function expression.
            pub fn source(self) -> &'static str {
                match self { $( Script::$variant => include_str!(concat!("../js/", $file)), )* }
            }

            /// How the script is used.
            pub fn kind(self) -> Kind {
                match self { $( Script::$variant => $kind, )* }
            }

            /// The world it runs in.
            pub fn world(self) -> World {
                match self { $( Script::$variant => $world, )* }
            }

            /// The installer that must have run in this document first, if any.
            pub fn requires(self) -> Option<Script> {
                match self { $( Script::$variant => $requires, )* }
            }
        }
    };
}

use Frames::{AllFrames, MainFrame};
use Injection::{DocumentEnd, DocumentStart};
use Kind::{AsyncCall, Call, Installer, UserScript};
use World::Page;

const HELPER: Option<Script> = Some(Script::PageHelper);
const JEV: Option<Script> = Some(Script::JevFast);
const TRAIL: Option<Script> = Some(Script::TrailLayer);

scripts! {
    // ---- MCP page tools (Fork/MCP/Page.swift, Tools.swift) ----
    /// `window.__copper`: snapshot with refs, find, rect, setValue, select, fill, click, … and console capture.
    PageHelper = "page/helper.js", Installer, Page, None;
    /// `browser_snapshot`: the Playwright-shaped accessibility tree with `[ref=eN]`.
    Snapshot = "page/snapshot.js", Call, Page, HELPER;
    /// Scroll a target into view; its rect in viewport CSS pixels (`Page.prepare`).
    Rect = "page/rect.js", Call, Page, HELPER;
    /// Document height for a full-page screenshot, capped at 20000.
    DocumentHeight = "page/document_height.js", Call, Page, None;
    /// DOM click (no real input available).
    Click = "page/click.js", Call, Page, HELPER;
    /// DOM hover.
    Hover = "page/hover.js", Call, Page, HELPER;
    /// Focus a target.
    Focus = "page/focus.js", Call, Page, HELPER;
    /// Native-setter typing + input/change events.
    SetValue = "page/set_value.js", Call, Page, HELPER;
    /// Submit the target's form (or Enter).
    Submit = "page/submit.js", Call, Page, HELPER;
    /// `browser_fill_form`, one field.
    Fill = "page/fill.js", Call, Page, HELPER;
    /// `browser_press_key` without real input.
    Key = "page/key.js", Call, Page, HELPER;
    /// `browser_select_option`.
    Select = "page/select.js", Call, Page, HELPER;
    /// `browser_drag` without real input.
    Drag = "page/drag.js", Call, Page, HELPER;
    /// `browser_wait_for` text / textGone poll.
    HasText = "page/has_text.js", Call, Page, None;
    /// `browser_scroll`.
    Scroll = "page/scroll.js", Call, Page, HELPER;
    /// `browser_get_text` (and Agent/jev_extract's whole-page text).
    Text = "page/text.js", Call, Page, HELPER;
    /// `browser_console_messages`: the captured log as a JSON string.
    Console = "page/console.js", Call, Page, HELPER;
    /// `browser_find`.
    FindText = "page/find_text.js", Call, Page, HELPER;
    /// The element behind a ref, for `browser_evaluate` with `ref`.
    Element = "page/element.js", Call, Page, HELPER;
    /// The helper's console capture from document start (`window.__copperLogs`),
    /// so `browser_console_messages` also has what the page logged while loading.
    ConsoleEarly = "page/console_early.js", UserScript(DocumentStart, MainFrame), Page, None;

    // ---- Jev (Fork/MCP/Ultrafast.swift) ----
    /// `window.__jevFast`: observe, marker, check, resolve, selectAll, domClick, setValue, settle.
    JevFast = "jev/fast.js", Installer, Page, None;
    /// The page as Jev sees it (`jev_observe`).
    JevObserve = "jev/observe.js", Call, Page, JEV;
    /// Whole-page freshness marker.
    JevMarker = "jev/marker.js", Call, Page, JEV;
    /// `[page_key, guard]` for one node.
    JevCheck = "jev/check.js", Call, Page, JEV;
    /// Where to put the input for an action, now (or null); does a select in full.
    JevResolve = "jev/resolve.js", Call, Page, JEV;
    /// DOM click on a node.
    JevDomClick = "jev/dom_click.js", Call, Page, JEV;
    /// Focus a node and select its text.
    JevSelectAll = "jev/select_all.js", Call, Page, JEV;
    /// Native-setter value on a node.
    JevSetValue = "jev/set_value.js", Call, Page, JEV;
    /// Wait for the page to settle after an action (two frames / 50–200 ms).
    JevSettle = "jev/settle.js", AsyncCall, Page, None;
    /// Scroll by a delta, instantly.
    JevScrollBy = "jev/scroll_by.js", Call, Page, None;

    // ---- Trails (Fork/MCP/Trail.swift) ----
    /// `window.__jevTrail`: the cosmetic SVG layer that draws the agent's moves.
    TrailLayer = "trail/layer.js", Installer, Page, None;
    /// Glide the pointer to a point; resolves on arrival.
    TrailGlide = "trail/glide.js", AsyncCall, Page, TRAIL;
    /// One fire-and-forget call into the layer (target, click, typed, scroll, seen, wait).
    TrailFire = "trail/fire.js", Call, Page, TRAIL;
    /// Take the layer off the page.
    TrailClear = "trail/clear.js", Call, Page, None;

    // ---- Perf probe (Fork/MCP/Probe.swift) ----
    /// `browser_perf_probe`: samples rAF, DOM churn, animations, filters, … for N seconds.
    PerfProbe = "probe/perf.js", AsyncCall, Page, None;

    // ---- Forms and passwords (Forms.swift, Tab.swift) ----
    /// `window.__officeForms` + the officeForms relay (sign-in found/sent/settled, focus, fullscreen).
    FormsRelay = "forms/relay.js", UserScript(DocumentEnd, MainFrame), Page, None;
    /// Hides `PublicKeyCredential` so sites fall back to the password.
    WithoutPasskeys = "forms/without_passkeys.js", UserScript(DocumentStart, AllFrames), Page, None;
    /// Fill the sign-in pair.
    FormsFillLogin = "forms/fill_login.js", Call, Page, None;
    /// Submit the sign-in.
    FormsSubmit = "forms/submit.js", Call, Page, None;
    /// Fill a one-time code (one box or one box per digit).
    FormsFillOtp = "forms/fill_otp.js", Call, Page, None;
    /// Fill identity/card values by field kind; answers how many were filled.
    FormsFillValues = "forms/fill_values.js", Call, Page, None;
    /// Fill the focused box.
    FormsFillFocused = "forms/fill_focused.js", Call, Page, None;
    /// Fill the box whose label matches.
    FormsFillField = "forms/fill_field.js", Call, Page, None;
    /// Whether a field of a group (login/card/identity/other) is present.
    FormsHasFields = "forms/has_fields.js", Call, Page, None;
    /// The field kinds present, first occurrence order.
    FormsFieldsPresent = "forms/fields_present.js", Call, Page, None;
    /// Whether a one-time-code box is present.
    FormsHasOtp = "forms/has_otp.js", Call, Page, None;
    /// Whether a visible password box is present.
    FormsHasPassword = "forms/has_password.js", Call, Page, None;
    /// Whether typed text would be lost by putting the tab to sleep.
    FormsUnsaved = "forms/unsaved.js", Call, Page, None;

    // ---- Hide anything (Curtain.swift, Tab.swift) ----
    /// `window.__officeVeil`: the element picker; posts to officeVeil.
    CurtainPicker = "curtain/picker.js", UserScript(DocumentStart, MainFrame), Page, None;
    /// The persisted hiding stylesheet (`#office-veil`); also run as a call on update.
    CurtainStyle = "curtain/style.js", UserScript(DocumentStart, MainFrame), Page, None;
    /// Start picking.
    CurtainStart = "curtain/start.js", Call, Page, None;
    /// Stop picking.
    CurtainStop = "curtain/stop.js", Call, Page, None;
    /// Show one hidden element while its row is hovered.
    CurtainPeek = "curtain/peek.js", Call, Page, None;
    /// End a peek.
    CurtainUnpeek = "curtain/unpeek.js", Call, Page, None;

    // ---- Reading mode (Reader.swift) ----
    /// Replace the page with its article; answers "read" or "none".
    ReaderArticle = "reader/article.js", Call, Page, None;

    // ---- Tab plumbing (Tab.swift, Swipe.swift, ImageMenu.swift, Icons.swift, Browser.swift, StorageImport.swift) ----
    /// Scroll position relay to officeScroll.
    ScrollRelay = "tab/scroll_relay.js", UserScript(DocumentEnd, MainFrame), Page, None;
    /// `overscroll-behavior-y: none` on the root.
    SwipeCalm = "tab/swipe_calm.js", UserScript(DocumentStart, MainFrame), Page, None;
    /// Whether a horizontal wheel belongs to the page (officeScroll `side`).
    SwipeWatch = "tab/swipe_watch.js", UserScript(DocumentStart, AllFrames), Page, None;
    /// Image right-click relay to officeImages.
    ImageRelay = "tab/image_relay.js", UserScript(DocumentStart, AllFrames), Page, None;
    /// The page's declared icons.
    Favicons = "tab/favicons.js", Call, Page, None;
    /// Clear the selection (closing find).
    ClearSelection = "tab/clear_selection.js", Call, Page, None;
    /// `document.readyState`.
    ReadyState = "tab/ready_state.js", Call, Page, None;
    /// `window.scrollTo`.
    ScrollTo = "tab/scroll_to.js", Call, Page, None;
    /// Smart zoom target for a double-tap (JSON string or null).
    SmartZoom = "tab/smart_zoom.js", Call, Page, None;
    /// `location.origin`.
    Origin = "tab/origin.js", Call, Page, None;
    /// Put localStorage items; answers the keys that failed.
    StoragePut = "tab/storage_put.js", Call, Page, None;

    // ---- Canvas bridge (Fork/Canvas/*) ----
    /// Live web frames on the board report their address/title/icon to copperFrame.
    CanvasFrameReporter = "canvas/frame_reporter.js", UserScript(DocumentStart, AllFrames), World::Isolated(FRAME_WORLD), None;
    /// The lean board's typing relay (officeForms `focus`).
    CanvasTyping = "canvas/typing.js", UserScript(DocumentStart, MainFrame), Page, None;
    CanvasApplyUpdate = "canvas/apply_update.js", AsyncCall, Page, None;
    CanvasReady = "canvas/ready.js", AsyncCall, Page, None;
    CanvasTheme = "canvas/theme.js", AsyncCall, Page, None;
    CanvasIsLive = "canvas/is_live.js", Call, Page, None;
    CanvasApply = "canvas/apply.js", AsyncCall, Page, None;
    CanvasRead = "canvas/read.js", AsyncCall, Page, None;
    CanvasSelect = "canvas/select.js", AsyncCall, Page, None;
    CanvasZoomTo = "canvas/zoom_to.js", AsyncCall, Page, None;
    CanvasPresence = "canvas/presence.js", AsyncCall, Page, None;
    CanvasExportState = "canvas/export_state.js", AsyncCall, Page, None;
    CanvasWsClosed = "canvas/ws_closed.js", AsyncCall, Page, None;
    CanvasWsOpen = "canvas/ws_open.js", AsyncCall, Page, None;
    CanvasWsMessage = "canvas/ws_message.js", AsyncCall, Page, None;
    CanvasSetShare = "canvas/set_share.js", AsyncCall, Page, None;
    CanvasSetStatus = "canvas/set_status.js", AsyncCall, Page, None;
    CanvasRename = "canvas/rename.js", AsyncCall, Page, None;
    CanvasFrameEvent = "canvas/frame_event.js", AsyncCall, Page, None;
    CanvasFrameHost = "canvas/frame_host.js", AsyncCall, Page, None;
    CanvasSetChat = "canvas/set_chat.js", AsyncCall, Page, None;
    CanvasShowChat = "canvas/show_chat.js", AsyncCall, Page, None;
    CanvasRevealChat = "canvas/reveal_chat.js", AsyncCall, Page, None;
    CanvasChatSend = "canvas/chat_send.js", AsyncCall, Page, None;
}

impl Script {
    /// The Swift origin from the header: `Sources/Search/<path>:<line>`.
    pub fn origin(self) -> &'static str {
        let first = self.source().lines().next().unwrap_or_default();
        let rest = first.strip_prefix("// from ").unwrap_or_default();
        rest.split_whitespace().next().unwrap_or_default()
    }

    /// The Swift symbol from the header (e.g. `Page.helper`).
    pub fn symbol(self) -> &'static str {
        let first = self.source().lines().next().unwrap_or_default();
        first
            .strip_prefix("// from ")
            .and_then(|rest| rest.split_once(' '))
            .map(|(_, symbol)| symbol)
            .unwrap_or_default()
    }

    /// The function expression without the header comment lines.
    pub fn function(self) -> &'static str {
        let source = self.source();
        let mut offset = 0;
        for line in source.split_inclusive('\n') {
            if line.starts_with("//") {
                offset += line.len();
            } else {
                break;
            }
        }
        source[offset..].trim_end()
    }

    /// Whether the result is a promise (run it with `call_async_javascript_function`).
    pub fn is_async(self) -> bool {
        self.kind() == Kind::AsyncCall
    }
}
