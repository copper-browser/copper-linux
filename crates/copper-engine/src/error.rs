use serde::{Deserialize, Serialize};

/// Result alias used across the engine API.
pub type Result<T, E = EngineError> = std::result::Result<T, E>;

/// Errors an engine call can report. Messages come from the engine and are
/// meant for logs and agent replies, not for matching on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum EngineError {
    /// A caller-supplied value was rejected before reaching the engine.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Script threw, failed to parse, or returned something without a JSON form.
    #[error("javascript error: {0}")]
    JavaScript(String),
    /// The thing asked for does not exist (no element at a point, no stored filter…).
    #[error("not found: {0}")]
    NotFound(String),
    /// Content-blocker JSON failed to compile, load or remove.
    #[error("content filter: {0}")]
    ContentFilter(String),
    /// The engine could not paint the page into an image.
    #[error("snapshot failed: {0}")]
    Snapshot(String),
    /// Cookie store read/write failed.
    #[error("cookies: {0}")]
    Cookies(String),
    /// Website-data fetch/remove failed.
    #[error("website data: {0}")]
    WebsiteData(String),
    /// A download could not be started.
    #[error("download: {0}")]
    Download(String),
    /// Filesystem error (profile or cache directories).
    #[error("i/o: {0}")]
    Io(String),
    /// Anything else the engine reported.
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        EngineError::Io(e.to_string())
    }
}
