//! Chromium's `History` (`Chromium.places`): the visited, unhidden pages,
//! newest first, read from a copy.
//!
//! The browser keeps `History` open while it runs. On Windows the copy
//! shares the file every way it can; when the browser still won't let it be
//! read, [`read_all`] says so in [`Read::in_use`] (and [`rows`] returns
//! [`ImportError::InUse`]) so the person can close it and try again.

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

/// What a read of every profile's `History` found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Read {
    /// Newest first, at most the limit asked for.
    pub places: Vec<Place>,
    /// Files the browser holds without sharing (Windows): close it and try
    /// again.
    pub in_use: Vec<PathBuf>,
    /// Files that are there but couldn't be read as a history database.
    pub failed: Vec<PathBuf>,
}

impl Read {
    pub fn complete(&self) -> bool {
        self.in_use.is_empty() && self.failed.is_empty()
    }
}

/// Every profile's places, newest first, at most `limit`. A file that
/// can't be read is logged and left out; [`read_all`] says which.
pub fn read(profile_dirs: &[PathBuf], limit: usize) -> Vec<Place> {
    read_all(profile_dirs, limit).places
}

/// Every profile's places, newest first, at most `limit`, with the files
/// that couldn't be read and why.
pub fn read_all(profile_dirs: &[PathBuf], limit: usize) -> Read {
    let mut read = Read::default();
    for profile in profile_dirs {
        let file = profile.join("History");
        if file.is_file() {
            match rows(&file, limit) {
                Ok(rows) => read.places.extend(rows),
                Err(e) => {
                    tracing::warn!("import: {}: {e}", file.display());
                    match e {
                        ImportError::InUse(_) => read.in_use.push(file),
                        _ => read.failed.push(file),
                    }
                }
            }
        }
    }
    read.places.sort_by(|a, b| {
        b.last
            .0
            .partial_cmp(&a.last.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    read.places.truncate(limit);
    read
}

/// One `History` file. [`ImportError::InUse`] when its browser won't share
/// it (Windows).
pub fn rows(file: &Path, limit: usize) -> Result<Vec<Place>, ImportError> {
    let copy =
        Copied::of(file).map_err(|e| crate::copy::read_error(file.display().to_string(), &e))?;
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
