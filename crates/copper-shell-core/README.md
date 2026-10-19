# copper-shell-core

Copper's browser shell without a UI toolkit. It holds everything the browser
*is* apart from its widgets and its web engine. Linux (`copper-app`: GTK4 +
libadwaita + WebKitGTK) and Windows (`copper-win`: Win32 + DirectComposition +
WebView2) run the same code on top of it.

There are no `gtk`, `glib`, `gdk`, `gio` or `webkit` dependencies, and none
may be added. The crate builds for Windows on its own:

```sh
XWIN_ACCEPT_LICENSE=1 cargo xwin clippy -p copper-shell-core --all-targets \
    --target x86_64-pc-windows-msvc
# aarch64: ring's build script needs clang, not clang-cl.
XWIN_ACCEPT_LICENSE=1 cargo xwin clippy -p copper-shell-core --all-targets \
    --cross-compiler clang --target aarch64-pc-windows-msvc
```

Everything here runs on the main thread: it uses `Rc`, and engine pages are
`!Send`. Other threads reach the shell through `control::Control`.

## How an app plugs in

### 1. One `Platform` type names the three pieces

```rust
pub enum Gtk {}                       // a marker type; never constructed

impl copper_shell_core::Platform for Gtk {
    type Engine = WebKitEngine;       // copper-engine's traits; Clone (an Rc inside)
    type Loop = Glib;                 // main_loop::MainLoop
    type View = GtkView;              // view::ShellView<Gtk>: the window
    type Icon = gdk::Texture;         // a decoded favicon, ready to draw
    type Frame = GtkFrame;            // view::PageFrame: a tab's page on the stage

    fn user_name() -> String { /* full name, else login */ }
    // preferred_languages() has a default (LANGUAGE/LC_*/LANG, as GLib reads them).
}

pub type Shell = copper_shell_core::Shell<Gtk>;   // and so on for other generic types
```

| Piece | Trait | What the app supplies |
|---|---|---|
| Engine | `copper_engine::{Engine, Profile, PageView}` | Pages, profiles, filters and scripts. The shell also needs these additions: `PageView::{page_id, downgrade, view_size, is_mapped, snapshot_bitmap}`, trusted input, the file chooser, `Engine::register_scheme` (`copper://canvas`) and `Engine::set_preferred_languages`. |
| Main loop | `main_loop::MainLoop` | `spawn_local`, `sleep`, `post` (from any thread), `idle` and `is_main_thread`. GTK provides these through the GLib default context; Windows provides them through `copper-win-rt`. |
| Window | `view::ShellView<P>` | Notifications the window acts on at once: pages added or removed, the tab shown, `refresh(Refresh)`, layout, progress, hover, look, toasts and questions. It also answers small questions: `is_dark`, `is_online`, `showing_downloads`, `screenshot`. It can open a Settings page, resize the window, close the side panel (`hide_panel`) and decode icons. |
| Page frame | `view::PageFrame` | Shows or hides a `PageStatus` (crashed, offline and so on) over a tab's page. |

### 2. Lanes that draw something take a small trait of their own

| Lane | Trait / hook | The app's part |
|---|---|---|
| `agents` | `agents::AgentsApp<P>` | Backstage pages (`holds` / `prepare` / `release` / `release_all`), the marks drawn over pages (`announce`, `drive`, `clear_marks`), and passwords for `browser_sign_in` / `browser_autofill` (usually `forms::mcp::{sign_in, autofill}`). |
| `forms` | `forms::FormsUi` (`OfferView`, `PickerView`) plus `forms::vault::Look` | The save-offer capsule and the account list under a sign-in box. `Look` checks the secret store without opening it. GTK reads the Secret Service over D-Bus; Windows passes `vault::always_there`, because Credential Manager never prompts. |
| `updates` | `updates::UpdatesApp<P>` plus `updates::Channel` | Where releases live, one HTTP/1.1 exchange over the platform's sockets and TLS, the "Copper X is available" toast, and a redraw hook for Settings › Updates. |
| `curtain` | `curtain::on_change` | A listener that refreshes the Hidden panel. |
| `format` | `format::Calendar` | Local civil time and month names, so dates read the way the OS formats them. |

### 3. Construction order

```rust
let shell = Shell::new(options, Engine::new, |options| View::new(options));
// The view finishes itself; GTK's widgets need the shell (`set_ui`).
// The app registers its actions and commands, then starts the lanes:
shield::start(&shell);            // the block list compiles off the main loop
curtain::start(&shell);           // hidden.json
updates::start(&shell, app);      // updates.json; looks when due
forms::start::<U>(&shell, look);  // vaults, then watching every page
cloud::start(&shell);             // copper-cloud on its own runtime
canvas::start(&shell);            // after cloud: canvases sync through it
agents::start(&shell, app);       // the MCP server
shell.start();                    // the first pages are made here
// On quit: shell.shutdown() writes everything; agents::shutdown::<P>()
// stops the MCP server when the application exits.
```

Each new page goes through the shell's `on_page_created` hooks. The app calls
`shield::protect`, `curtain::dress` and its own per-page features from there,
and passes the page's script messages and loads to `curtain::{message,
navigated}`. When the page ends, the app calls `curtain::page_gone(page_id)`.

### Conventions

- **Process-wide slots.** A lane with process-wide state keeps it as
  `Rc<dyn Any>` / `Box<dyn Any>` in a thread-local and downcasts it with the
  platform type: `shell::current::<P>()`, `agents::get::<P>()`,
  `forms::get::<U>()`, and the shield, curtain and updates state. Each
  process has one platform, so the downcast cannot fail in a real app.
- **Page identity is `PageId`**, not a widget. It never repeats within a
  run, so a tab's new page (after a crash or a sleep) is never mistaken for
  the old one.
- **Timers are loops.** `glib::timeout_add_local` became
  `spawn_local` + `sleep`. A timer that must be cancellable is a
  `main_loop::spawn` `Task`, stopped with `abort`. Dropping a `Task` does
  not stop it.
- **Log targets follow the module path.** Moved code logs under
  `copper_shell_core::…`. Canvas is the exception: it keeps the explicit
  `copper_app::canvas` target it had before the move. The app's default
  filter is `warn,copper_app=info,copper_shell_core=info`, and the QA
  scripts (`qa/selftest.sh`, `cloud/qa/e2e.py`) pass the same. What is
  logged by default is therefore unchanged, but a hand-written `COPPER_LOG`
  that names only `copper_app` no longer covers the moved modules.
- **History-friendly moves.** Files were moved whole, keeping their
  `Mac: X.swift` references and their behaviour. `copper-app` re-exports each
  moved module from where it used to live (`pub use
  copper_shell_core::shield::*;`) and adds only its GTK half. Generic call
  sites in the app name the platform explicitly (`shield::trouble::<Gtk>()`).

## What is in the crate

| Module | What |
|---|---|
| `shell` | `Shell<P>`: copper-core's tabs, spaces, history, bookmarks and settings, plus one live engine page per awake tab (`pages`) and what its events do. `languages` holds the preferred-languages logic. |
| `main_loop` | `MainLoop`, `Task` / `spawn` / `timeout`, `spawn_blocking`, and `TestLoop` for tests. |
| `view` | `ShellView`, `PageFrame`, `PageStatus`, `Refresh`, `Question`. |
| `control` | The thread-safe control API that automation and the CLI drive the shell with. |
| `session`, `options`, `private`, `crash`, `downloads` | Session files and data and cache locations, launch options, private tabs, crash records, and the downloads model. |
| `spaces` | Space colour and tint (`colour`), and how a space is named and marked. |
| `cloud` | The copper-cloud lane: `adapter`, the browser view the sync engine reads and writes (`AppBrowser<P>`), and `mcp`, the agents' cloud history tool. |
| `canvas` | Canvas: `host`, `mcp`, `registry`, `store`, `poster`, `chatprefs`, `scheme` (`copper://canvas`) and the page bundle in `resources/`. The bundle and wire messages are the same as before the move. |
| `agents` | The agents' MCP server: `host`, `cli` (the stdio bridge), `crashes`, `groups` and `heat`. |
| `forms` | Passwords and form fill: `watch` (every page's forms relay, sign-ins held until they take), `vault` (the platform store, Bitwarden, 1Password), `mcp` (`browser_sign_in` / `browser_autofill`), `rt` (the lane's Tokio runtime) and `view` (`FormsUi`). |
| `shield` | The ad blocker: compiling the list, protecting pages, pausing per site, and following Settings. |
| `curtain` | Hide anything: what is hidden per site, the user style sheets on every page, the picker's messages, undo and peek. |
| `updates` | The update check: the feed, its six-hour cadence, what was seen and announced, redirects, and semver order. |
| `palette` | `complete`: the palette's matching and ranking. |
| `format` | Times, sizes and speeds in the shell's words (`Calendar` for local dates). |
| `util` | Curated page scripts in engine form, `run` / `fire` for curated calls, and `site_key`. |

## What stays in copper-app

`copper-app` is the GTK view layer plus glue:

- `platform.rs` (`Gtk`, `Glib`, `GtkView`, `GtkFrame`, the `Calendar` impl);
- `shell.rs` (aliases and the `GtkShell` accessors);
- `window/*`, `style.rs`, `actions.rs` (GIO actions, the command, panel and
  settings-page registry);
- the palette's widgets, `spaces/ui.rs` and every Settings page;
- the panels (History, Bookmarks, Downloads, Hidden);
- the forms widgets (`offer`, `picker`), the Secret Service probe
  (`keyring`), Settings › Passwords and the "Bring in from Chrome" sheet
  (`import`; its logic is in `copper-import`);
- the agents' backstage, marks, QA and Settings › Agents;
- Settings › Cloud (`cloud/page.rs`) and the cloud QA bench (`cloud/bench.rs`,
  a Unix socket);
- the GTK self-tests.

## Still to move

These are toolkit-free in substance but still live in `copper-app`. They are
listed in the order they would come over:

1. **`features/reader.rs`** (reading mode, 83 lines). It needs `glib::spawn`
   → `P::Loop::spawn_local` and its page type made generic.
2. **The per-page feature wiring in `features/mod.rs`** (`page_created`):
   which lanes a new page goes through, and the page's event pump. It
   compares widgets to tell whether a page is current, and should compare
   `PageId` instead. The panels' `page_moved` would become a hook.
3. **The command registry in `actions.rs`** (`Command`, `register_command`,
   `commands`, `run_command`, `space_commands`). Every platform's palette
   needs it; today it lives beside the GIO actions.

## Platform notes for Windows

- `agents/cli.rs` `detach()`: on Windows the bridge-launched browser starts
  with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`, so it outlives the
  client's console.
- `agents/heat.rs` reads `/proc`. On Windows it reports nothing until a
  process walk (Toolhelp32) is added.
- `canvas/store.rs` sets 0600/0700 modes on Unix only. On Windows it relies
  on the profile folder's ACL.
- `session::cache_dir` on Windows is `Cache\` beside the store's `Data\`
  (`%LOCALAPPDATA%\Copper\Cache`, per CONTRACT §4). `home()` reads
  `USERPROFILE`.
- `forms` builds on Windows because `copper-creds` uses Credential Manager
  there (`store::wincred`). Secret Service, oo7 and ashpd are Linux-only.
