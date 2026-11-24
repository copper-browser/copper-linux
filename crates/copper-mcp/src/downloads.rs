//! `browser_downloads`: the downloads the caller's tab started (or a popup
//! it opened), and where the files are (Mac `StateTools.downloads`, `line`).

use copper_core::downloads::bytes;

/// Where one download is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadState {
    Running,
    Finished,
    Failed(String),
    Cancelled,
}

/// One download, as the tool lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadRow {
    pub name: String,
    pub state: DownloadState,
    pub done: u64,
    /// When the server said.
    pub total: Option<u64>,
    /// The file, once it has a place.
    pub path: Option<String>,
    /// The host it came from; may be empty.
    pub from: String,
}

/// A tab's downloads, newest first, and where a download lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabDownloads {
    pub rows: Vec<DownloadRow>,
    pub folder: String,
}

impl Default for TabDownloads {
    fn default() -> Self {
        TabDownloads {
            rows: Vec::new(),
            folder: "~/Downloads".into(),
        }
    }
}

impl TabDownloads {
    /// Any still downloading (what `wait` waits for).
    pub fn running(&self) -> bool {
        self.rows.iter().any(|r| r.state == DownloadState::Running)
    }

    /// The tool's answer.
    pub fn answer(&self) -> String {
        if self.rows.is_empty() {
            return format!(
                "No downloads from this tab. A click that downloads lands the file in {}.",
                self.folder
            );
        }
        let lines: Vec<String> = self.rows.iter().map(line).collect();
        format!("### Downloads (newest first)\n{}", lines.join("\n"))
    }
}

fn line(row: &DownloadRow) -> String {
    let size = bytes(row.done);
    let state = match &row.state {
        DownloadState::Running => format!(
            "downloading {size}{}",
            row.total
                .map(|t| format!(" of {}", bytes(t)))
                .unwrap_or_default()
        ),
        DownloadState::Finished => format!("finished, {size}"),
        DownloadState::Failed(why) => format!("failed: {why}"),
        DownloadState::Cancelled => "cancelled".into(),
    };
    let place = row
        .path
        .as_ref()
        .map(|p| format!(" → {p}"))
        .unwrap_or_default();
    let from = if row.from.is_empty() { "?" } else { &row.from };
    format!("- {} — {state}{place} (from {from})", row.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: DownloadState, done: u64, total: Option<u64>, path: Option<&str>) -> DownloadRow {
        DownloadRow {
            name: "a.zip".into(),
            state,
            done,
            total,
            path: path.map(String::from),
            from: "x.test".into(),
        }
    }

    #[test]
    fn lines_say_each_state() {
        let p = Some("/home/u/Downloads/a.zip");
        assert_eq!(
            line(&row(DownloadState::Running, 5_700_000, Some(10_500_000), p)),
            "- a.zip — downloading 5.7 MB of 10.5 MB → /home/u/Downloads/a.zip (from x.test)"
        );
        // Unknown total, no place yet.
        assert_eq!(
            line(&row(DownloadState::Running, 588_000, None, None)),
            "- a.zip — downloading 588 KB (from x.test)"
        );
        assert_eq!(
            line(&row(DownloadState::Finished, 1_230_000_000, None, p)),
            "- a.zip — finished, 1.23 GB → /home/u/Downloads/a.zip (from x.test)"
        );
        assert_eq!(
            line(&row(
                DownloadState::Failed("Network error".into()),
                4,
                None,
                None
            )),
            "- a.zip — failed: Network error (from x.test)"
        );
        let mut cancelled = row(DownloadState::Cancelled, 0, None, None);
        cancelled.from.clear();
        assert_eq!(line(&cancelled), "- a.zip — cancelled (from ?)");
    }

    #[test]
    fn answers_none_or_the_list() {
        let mut d = TabDownloads {
            rows: Vec::new(),
            folder: "/home/u/Downloads".into(),
        };
        assert_eq!(
            d.answer(),
            "No downloads from this tab. A click that downloads lands the file in /home/u/Downloads."
        );
        assert!(!d.running());
        d.rows.push(row(DownloadState::Running, 4, None, None));
        d.rows.push(row(DownloadState::Finished, 4, None, None));
        assert!(d.running());
        assert_eq!(
            d.answer(),
            "### Downloads (newest first)\n- a.zip — downloading 4 bytes (from x.test)\n- a.zip — finished, 4 bytes (from x.test)"
        );
    }
}
