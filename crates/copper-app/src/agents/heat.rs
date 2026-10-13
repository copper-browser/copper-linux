//! What the OS says about Copper's web processes, ahead of what
//! `browser_perf_probe`'s page script says about itself (Mac:
//! `Probe.processes`). Linux WebKitGTK does not say which web process
//! hosts which tab, so the line lists Copper's WebKit processes with their
//! CPU since the call began (the probe's own window).

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Clock ticks per second (`USER_HZ`, 100 on every Linux Copper runs on).
const HZ: f64 = 100.0;
/// How long a found set of processes is trusted before /proc is walked again.
const REFRESH: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct Heat {
    /// When the current call began, and each process's ticks then.
    base: RefCell<Option<(Instant, HashMap<i32, u64>)>>,
    /// Copper's WebKit processes: pid → name, and when they were found.
    found: RefCell<Option<(Instant, HashMap<i32, String>)>>,
}

impl Heat {
    /// A call is starting: remember where every process's CPU stands.
    pub fn mark(&self) {
        let pids = self.processes();
        let ticks = pids.keys().filter_map(|p| Some((*p, ticks(*p)?))).collect();
        *self.base.borrow_mut() = Some((Instant::now(), ticks));
    }

    /// `## Processes (% of one core)` with a line per process, or empty.
    pub fn line(&self) -> String {
        let Some((since, base)) = self.base.borrow().clone() else {
            return String::new();
        };
        let secs = since.elapsed().as_secs_f64();
        if secs < 0.2 {
            return String::new();
        }
        let mut rows: Vec<(f64, i32, String)> = self
            .processes()
            .into_iter()
            .filter_map(|(pid, name)| {
                let now = ticks(pid)?;
                let then = base.get(&pid).copied()?;
                let pct = now.saturating_sub(then) as f64 / HZ / secs * 100.0;
                Some((pct, pid, name))
            })
            .collect();
        if rows.is_empty() {
            return String::new();
        }
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut out = String::from("## Processes (% of one core)\n");
        for (pct, pid, name) in rows.into_iter().take(6) {
            out.push_str(&format!(
                "- {name} (pid {pid}): {}% over the last {:.0} s\n",
                pct.round() as i64,
                secs.max(1.0)
            ));
        }
        out.push_str("(WebKitGTK does not say which web process hosts which tab.)\n\n");
        out
    }

    fn processes(&self) -> HashMap<i32, String> {
        if let Some((at, pids)) = self.found.borrow().as_ref()
            && at.elapsed() < REFRESH
        {
            return pids.clone();
        }
        let pids = webkit_descendants(std::process::id() as i32);
        *self.found.borrow_mut() = Some((Instant::now(), pids.clone()));
        pids
    }
}

/// `(comm, ppid, utime + stime)` from `/proc/<pid>/stat`.
fn stat(pid: i32) -> Option<(String, i32, u64)> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat(&raw)
}

fn parse_stat(raw: &str) -> Option<(String, i32, u64)> {
    let open = raw.find('(')?;
    let close = raw.rfind(')')?;
    let comm = raw[open + 1..close].to_string();
    let rest: Vec<&str> = raw[close + 1..].split_whitespace().collect();
    // After the comm: state(0) ppid(1) … utime(11) stime(12).
    let ppid = rest.get(1)?.parse().ok()?;
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    Some((comm, ppid, utime + stime))
}

fn ticks(pid: i32) -> Option<u64> {
    stat(pid).map(|s| s.2)
}

/// Every `WebKit*` process below `root`, by pid.
fn webkit_descendants(root: i32) -> HashMap<i32, String> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return HashMap::new();
    };
    let all: Vec<(i32, String, i32)> = dir
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter_map(|pid| stat(pid).map(|(comm, ppid, _)| (pid, comm, ppid)))
        .collect();
    let parent: HashMap<i32, i32> = all.iter().map(|(p, _, pp)| (*p, *pp)).collect();
    let below = |mut pid: i32| {
        for _ in 0..16 {
            match parent.get(&pid) {
                Some(&pp) if pp == root => return true,
                Some(&pp) if pp > 1 => pid = pp,
                _ => return false,
            }
        }
        false
    };
    all.into_iter()
        .filter(|(pid, comm, _)| comm.starts_with("WebKit") && below(*pid))
        .map(|(pid, comm, _)| (pid, pretty(&comm)))
        .collect()
}

/// `comm` is cut at 15 bytes: `WebKitWebProces` → `WebKitWebProcess`.
fn pretty(comm: &str) -> String {
    match comm {
        "WebKitWebProces" => "WebKitWebProcess".into(),
        "WebKitNetworkPr" => "WebKitNetworkProcess".into(),
        "WebKitGPUProces" => "WebKitGPUProcess".into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_with_spaces_in_comm() {
        let raw =
            "1234 (Web Content) S 77 1234 1234 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 1 0 100 0 0";
        assert_eq!(parse_stat(raw), Some(("Web Content".into(), 77, 300)));
        assert_eq!(pretty("WebKitWebProces"), "WebKitWebProcess");
    }
}
