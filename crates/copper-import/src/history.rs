//! Chromium's `History` (`Chromium.places`): the visited, unhidden pages,
//! newest first, read from a copy.

use std::path::{Path, PathBuf};

use copper_core::date::SwiftDate;
use copper_core::history::History;
use rusqlite::{Connection, OpenFlags};

use crate::{Copied, ImportError, chromium_to_unix};

/// One page another browser has been to.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub url: String,
    pub title: String,
    /// At least 1.
    pub count: i64,
    pub last: SwiftDate,
}

impl Place {
    /// Into Copper's history, counted as it was counted there.
    pub fn take_into(&self, history: &mut History) {
        history.take(&self.url, &self.title, self.count, self.last);
    }
}

/// Every profile's places, newest first, at most `limit`.
pub fn read(profile_dirs: &[PathBuf], limit: usize) -> Vec<Place> {
    let mut out = Vec::new();
    for profile in profile_dirs {
        let file = profile.join("History");
        if file.is_file() {
            match rows(&file, limit) {
                Ok(rows) => out.extend(rows),
                Err(e) => tracing::warn!("import: {}: {e}", file.display()),
            }
        }
    }
    out.sort_by(|a, b| {
        b.last
            .0
            .partial_cmp(&a.last.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(limit);
    out
}

/// One `History` file.
pub fn rows(file: &Path, limit: usize) -> Result<Vec<Place>, ImportError> {
    let unreadable = |_| ImportError::Unreadable(file.display().to_string());
    let copy = Copied::of(file).map_err(unreadable)?;
    let db = Connection::open_with_flags(&copy.path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| ImportError::Unreadable(file.display().to_string()))?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut st = db
        .prepare(
            "SELECT url, title, visit_count, last_visit_time FROM urls \
             WHERE hidden = 0 AND visit_count > 0 ORDER BY last_visit_time DESC LIMIT ?1",
        )
        .map_err(|_| ImportError::Unreadable(file.display().to_string()))?;
    let now = SwiftDate::now();
    let rows = st
        .query_map([limit], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(|_| ImportError::Unreadable(file.display().to_string()))?;
    let mut out = Vec::new();
    for row in rows.flatten() {
        let (Some(url), title, count, stamp) = row else {
            continue;
        };
        if url::Url::parse(&url).is_err() {
            continue;
        }
        out.push(Place {
            url,
            title: title.unwrap_or_default(),
            count: count.unwrap_or(1).max(1),
            last: chromium_to_unix(stamp.unwrap_or(0))
                .map(SwiftDate::from_unix)
                .unwrap_or(now),
        });
    }
    Ok(out)
}
