//! Copper's models and persistence: no GTK, no WebKit.
//!
//! Everything a Mac Copper keeps in `~/Library/Application Support/Copper/`
//! is modelled here with the same JSON shape, so a folder copied from a Mac
//! loads and a file written here reads on a Mac. Every struct carries an
//! `extra` map (`#[serde(flatten)]`) that keeps keys this build does not
//! know; optional fields stay absent when absent. Dates use the exact Swift
//! encodings (see [`date`]).
//!
//! | File | Type | Module |
//! |---|---|---|
//! | `session.json` | [`session::SessionShape`] | [`session`], live state in [`browser`] |
//! | `history.json` | `Vec<`[`history::Visit`]`>` | [`history`] |
//! | `bookmarks.json` | `Vec<`[`bookmarks::Bookmark`]`>` | [`bookmarks`] |
//! | `windows.json` | `Vec<`[`groups::SavedWindow`]`>` | [`groups`] |
//! | `groups.json` | [`groups::GroupsFile`] | [`groups`] |
//! | `hidden.json` | [`crashes::HiddenFile`] | [`crashes`] |
//! | `downloads.json` | `Vec<`[`downloads::Keep`]`>` | [`downloads`] |
//! | `agent.json` (0600) | [`agent::AgentFile`] | [`agent`] |
//! | `chat.json` | [`agent::ChatConfig`] | [`agent`] |
//! | `mcp.json` (0600) | [`agent::McpServersFile`] | [`agent`] |
//! | `trails.json` | [`trails::TrailsFile`] | [`trails`] |
//! | `cloud.json` (0600) | [`cloud::CloudFile`] | [`cloud`] |
//! | `cloud-sync.json` (0600) | [`cloud::CloudSyncFile`] | [`cloud`] |
//! | `cloud-intelligence.json` (0600) | [`cloud::CloudProvided`] | [`cloud`] |
//! | `canvas/canvases.json` | `Vec<`[`canvas::CanvasEntry`]`>` | [`canvas`] |
//! | `intelligence.json` (0600) | [`intelligence::IntelligenceKeys`] | [`intelligence`] |
//! | `claude.json` (0600) | [`intelligence::ClaudeCredential`] | [`intelligence`] |
//! | `updates.json` | [`updates::UpdatesFile`] | [`updates`] |
//! | `crashes/crashes.json` | [`crashes::CrashIndex`] | [`crashes`] |
//! | `settings.json` (config dir) | `serde_json::Map` → [`settings::Settings`] | [`settings`] |
//!
//! Cloud sync documents (base64 payloads in `cloud-sync.json`):
//! [`cloud::SpacesDoc`], [`cloud::SettingsDoc`], [`cloud::BookmarksDoc`],
//! [`cloud::TabsDoc`].
//!
//! Persistence: [`store::Store`] (atomic write + `.bak` + quarantine) and
//! [`saver::Saver`] (debounced background writes with explicit flush).

pub mod address;
pub mod agent;
pub mod bookmarks;
pub mod browser;
pub mod canvas;
pub mod cloud;
pub mod crashes;
pub mod date;
pub mod downloads;
pub mod groups;
pub mod history;
pub mod ids;
pub mod intelligence;
pub mod json;
pub mod saver;
pub mod session;
pub mod settings;
pub mod store;
pub mod trails;
pub mod updates;

pub use address::{SearchEngine, url_from_typed};
pub use bookmarks::{Bookmark, Bookmarks};
pub use browser::{Browser, BrowserEvent, CloseOutcome, OpenOptions, Tab, TabId};
pub use date::{IsoDate, SwiftDate, UnixTime};
pub use downloads::{Downloads, Keep};
pub use history::{History, Suggestion, SuggestionKind, Visit};
pub use saver::Saver;
pub use session::{SessionEntry, SessionShape, Space};
pub use settings::Settings;
pub use store::{DataFile, FileModel, LoadSource, Loaded, Paths, Store, StoreError};
