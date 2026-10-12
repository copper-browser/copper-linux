//! The sync engine and its pure parts (Mac: `CloudSync.swift`,
//! `CloudMerge.swift`, `CloudApply.swift`, `CloudHistory.swift`,
//! `CloudHistoryDelete.swift`).

pub mod adapter;
pub mod delete;
pub mod engine;
pub mod history;
pub mod merge;

pub use adapter::{BrowserState, LiveKept, LocalVisit, MemoryBrowser, MemoryState};
pub use delete::Selector;
pub use engine::{DeviceTabs, Domain, SyncEngine, SyncState, SyncStatus};
