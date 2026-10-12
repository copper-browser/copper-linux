//! `downloads.json` and the live downloads list (Mac: `Loot.swift`,
//! `Fork/Downloads.swift`, `docs/downloads.md`).
//!
//! The file is the memory of finished downloads ([`Keep`], newest first, at
//! most [`KEEP_CAP`]). [`Downloads`] adds this session's live rows (active,
//! finished, failed) on top, for the door and the panel. It holds no engine
//! objects: the shell maps engine download events onto [`DownloadId`]s.

use crate::date::SwiftDate;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::Instant;

/// The file keeps the newest fifty.
pub const KEEP_CAP: usize = 50;

/// A finished download as the file keeps it (`Keep`). `date` is a Swift date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keep {
    /// The file name shown.
    pub name: String,
    /// Where it came from (page or host); may be empty.
    #[serde(default)]
    pub from: String,
    /// Absolute path on disk; also the identity of the row.
    pub path: String,
    pub date: SwiftDate,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Keep {
    /// Whether the file is still on disk.
    pub fn still_there(&self) -> bool {
        std::path::Path::new(&self.path).exists()
    }
}

/// A live download's id for this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DownloadId(pub u64);

/// Where a live download is.
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadState {
    Active,
    Finished,
    Failed { reason: String },
    Cancelled,
}

/// One row of this session's downloads.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveDownload {
    pub id: DownloadId,
    pub name: String,
    pub source_url: String,
    /// Destination path once known.
    pub path: Option<String>,
    pub state: DownloadState,
    pub received: u64,
    /// Total bytes when the server said.
    pub total: Option<u64>,
    /// Bytes per second, sampled every half second.
    pub speed: f64,
    /// Finished but not looked at yet (the door's dot).
    pub unseen: bool,
    started: Instant,
    sample: (Instant, u64),
}

impl LiveDownload {
    /// 0…1 when the size is known.
    pub fn fraction(&self) -> Option<f64> {
        self.total
            .filter(|t| *t > 0)
            .map(|t| (self.received as f64 / t as f64).min(1.0))
    }
    /// Seconds left at the current speed, when both are known.
    pub fn seconds_left(&self) -> Option<f64> {
        let total = self.total?;
        (self.speed > 0.0).then(|| total.saturating_sub(self.received) as f64 / self.speed)
    }
    /// When it started.
    pub fn started(&self) -> Instant {
        self.started
    }
}

/// The downloads list model: live rows plus the kept file.
#[derive(Debug, Clone, Default)]
pub struct Downloads {
    kept: Vec<Keep>,
    live: Vec<LiveDownload>,
    next: u64,
    generation: u64,
}

impl Downloads {
    /// From the file.
    pub fn new(kept: Vec<Keep>) -> Self {
        Downloads {
            kept,
            ..Default::default()
        }
    }

    /// What to write to `downloads.json`.
    pub fn kept(&self) -> &[Keep] {
        &self.kept
    }

    /// This session's rows, newest first.
    pub fn live(&self) -> &[LiveDownload] {
        &self.live
    }

    /// Bumped on every change (the kept list or a live row).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A live row by id.
    pub fn get(&self, id: DownloadId) -> Option<&LiveDownload> {
        self.live.iter().find(|d| d.id == id)
    }

    /// A download began.
    pub fn start(&mut self, name: &str, source_url: &str, total: Option<u64>) -> DownloadId {
        self.next += 1;
        let id = DownloadId(self.next);
        let now = Instant::now();
        self.live.insert(
            0,
            LiveDownload {
                id,
                name: name.to_string(),
                source_url: source_url.to_string(),
                path: None,
                state: DownloadState::Active,
                received: 0,
                total,
                speed: 0.0,
                unseen: false,
                started: now,
                sample: (now, 0),
            },
        );
        self.generation += 1;
        id
    }

    /// The engine chose (or the user picked) where it goes.
    pub fn set_destination(&mut self, id: DownloadId, path: &str, name: Option<&str>) {
        if let Some(d) = self.live.iter_mut().find(|d| d.id == id) {
            d.path = Some(path.to_string());
            if let Some(n) = name {
                d.name = n.to_string();
            }
            self.generation += 1;
        }
    }

    /// Bytes arrived. Speed is re-sampled at most every half second.
    pub fn progress(&mut self, id: DownloadId, received: u64, total: Option<u64>) {
        if let Some(d) = self.live.iter_mut().find(|d| d.id == id) {
            d.received = received;
            if total.is_some() {
                d.total = total;
            }
            let now = Instant::now();
            let dt = now.duration_since(d.sample.0).as_secs_f64();
            if dt >= 0.5 {
                d.speed = received.saturating_sub(d.sample.1) as f64 / dt;
                d.sample = (now, received);
            }
            self.generation += 1;
        }
    }

    /// It landed: the row finishes and the file remembers it (newest first,
    /// one row per path, capped).
    pub fn finish(&mut self, id: DownloadId, from: &str) {
        let Some(d) = self.live.iter_mut().find(|d| d.id == id) else {
            return;
        };
        d.state = DownloadState::Finished;
        d.unseen = true;
        d.speed = 0.0;
        if let Some(t) = d.total {
            d.received = d.received.max(t);
        }
        if let Some(path) = d.path.clone() {
            let keep = Keep {
                name: d.name.clone(),
                from: from.to_string(),
                path,
                date: SwiftDate::now(),
                extra: Map::new(),
            };
            self.add_kept(keep);
        }
        self.generation += 1;
    }

    /// It failed.
    pub fn fail(&mut self, id: DownloadId, reason: &str) {
        if let Some(d) = self.live.iter_mut().find(|d| d.id == id) {
            d.state = DownloadState::Failed {
                reason: reason.to_string(),
            };
            d.speed = 0.0;
            self.generation += 1;
        }
    }

    /// Cancelled by the user: the row goes.
    pub fn cancel(&mut self, id: DownloadId) {
        let before = self.live.len();
        self.live.retain(|d| d.id != id);
        if self.live.len() != before {
            self.generation += 1;
        }
    }

    /// The popover was opened: nothing is unseen any more.
    pub fn mark_seen(&mut self) {
        for d in &mut self.live {
            d.unseen = false;
        }
        self.generation += 1;
    }

    /// **Clear**: finished and failed rows go; active ones stay.
    pub fn clear_finished(&mut self) {
        self.live.retain(|d| d.state == DownloadState::Active);
        self.generation += 1;
    }

    /// Number of active downloads.
    pub fn active_count(&self) -> usize {
        self.live
            .iter()
            .filter(|d| d.state == DownloadState::Active)
            .count()
    }

    /// Total progress across active downloads with a known size.
    pub fn total_fraction(&self) -> Option<f64> {
        let active: Vec<&LiveDownload> = self
            .live
            .iter()
            .filter(|d| d.state == DownloadState::Active)
            .collect();
        if active.is_empty() || active.iter().any(|d| d.total.is_none()) {
            return None;
        }
        let total: u64 = active.iter().filter_map(|d| d.total).sum();
        let got: u64 = active.iter().map(|d| d.received).sum();
        (total > 0).then(|| (got as f64 / total as f64).min(1.0))
    }

    /// Remember a finished file (`Loot.add`).
    pub fn add_kept(&mut self, keep: Keep) {
        self.kept.retain(|k| k.path != keep.path);
        self.kept.insert(0, keep);
        self.kept.truncate(KEEP_CAP);
        self.generation += 1;
    }

    /// Remove one remembered row (never the file).
    pub fn forget(&mut self, path: &str) {
        self.kept.retain(|k| k.path != path);
        self.generation += 1;
    }

    /// Empty the remembered list (never the files).
    pub fn forget_all(&mut self) {
        self.kept.clear();
        self.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle() {
        let mut d = Downloads::new(vec![]);
        let id = d.start("a.bin", "https://x.test/a.bin", Some(100));
        d.set_destination(id, "/tmp/a.bin", None);
        d.progress(id, 50, None);
        assert_eq!(d.get(id).unwrap().fraction(), Some(0.5));
        assert_eq!(d.total_fraction(), Some(0.5));
        d.finish(id, "x.test");
        assert_eq!(d.kept().len(), 1);
        assert!(d.get(id).unwrap().unseen);
        d.clear_finished();
        assert!(d.live().is_empty());
        for i in 0..60 {
            d.add_kept(Keep {
                name: format!("{i}"),
                from: String::new(),
                path: format!("/tmp/{i}"),
                date: SwiftDate(0.0),
                extra: Map::new(),
            });
        }
        assert_eq!(d.kept().len(), KEEP_CAP);
        assert_eq!(d.kept()[0].name, "59");
    }
}
