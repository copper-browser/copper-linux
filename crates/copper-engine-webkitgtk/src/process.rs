//! Which web process is a page's, and what it holds (`PageView::weight`).
//!
//! WebKitGTK does not say which process hosts a view. What it does: a view
//! with no process gets one inside its first load call, started from this
//! thread before the call returns; a cross-site navigation moves the page
//! to a new process before WebKit says the load started; a related view (a
//! popup that keeps `window.opener`) shares its opener's. So a page's
//! process is the one web process that appeared during its load call
//! ([`spawned_by`]) or, at a load to another host, the one that appeared
//! since anyone last looked and nobody has claimed ([`appeared`]). Two
//! pages moving to new processes in the same instant claim neither: they
//! go unweighed (and so never sleep for their weight) until their next
//! load. Memory is the process's proportional set size (pages it shares
//! with other processes split between them), else its resident size.
//!
//! With WebKit's sandbox the process Copper starts is `bwrap`, and the web
//! process runs a level or two below it: that one is weighed.

use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    /// Web processes already seen (claimed, or passed over).
    static SEEN: RefCell<HashSet<u32>> = RefCell::default();
}

/// Run `load`; the one web process it started, if exactly one.
pub(crate) fn spawned_by(load: impl FnOnce()) -> Option<u32> {
    let before = web_children();
    load();
    let after = web_children();
    let new: Vec<u32> = after.difference(&before).copied().collect();
    SEEN.with_borrow_mut(|seen| {
        seen.retain(|p| after.contains(p));
        seen.extend(&new);
    });
    only(new)
}

/// The one web process that appeared since anyone last looked, if exactly
/// one; every one there is counts as seen from now.
pub(crate) fn appeared() -> Option<u32> {
    let now = web_children();
    let new: Vec<u32> = SEEN.with_borrow_mut(|seen| {
        let new = now.difference(seen).copied().collect();
        *seen = now;
        new
    });
    only(new)
}

fn only(v: Vec<u32>) -> Option<u32> {
    match v[..] {
        [one] => Some(one),
        _ => None,
    }
}

/// The web process at or below `pid` (the sandbox's `bwrap` runs it a level
/// or two down) and its memory in bytes.
pub(crate) fn weigh(pid: u32) -> Option<(u32, u64)> {
    let web = web_below(pid, 4)?;
    Some((web, bytes(web)?))
}

/// Copper's web processes, as started: each child of this process that is
/// a WebKit web process or the sandbox that will run one.
fn web_children() -> HashSet<u32> {
    children(std::process::id())
        .into_iter()
        .filter(|p| {
            std::fs::read(format!("/proc/{p}/cmdline")).is_ok_and(|c| {
                c.split(|b| *b == 0)
                    .any(|arg| arg.ends_with(b"WebKitWebProcess"))
            })
        })
        .collect()
}

/// `pid`'s children (every thread's).
fn children(pid: u32) -> Vec<u32> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    tasks
        .flatten()
        .filter_map(|t| std::fs::read_to_string(t.path().join("children")).ok())
        .flat_map(|s| {
            s.split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect::<Vec<u32>>()
        })
        .collect()
}

/// `pid` if it is the web process itself, else the first one below it.
fn web_below(pid: u32, depth: u32) -> Option<u32> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    // `comm` is cut at 15 bytes.
    if comm.trim_end().starts_with("WebKitWebProces") {
        return Some(pid);
    }
    if depth == 0 {
        return None;
    }
    children(pid)
        .into_iter()
        .find_map(|c| web_below(c, depth - 1))
}

/// What `pid` holds: its proportional set size, else its resident size.
fn bytes(pid: u32) -> Option<u64> {
    if let Some(pss) = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
        .ok()
        .and_then(|s| pss(&s))
    {
        return Some(pss);
    }
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

/// `Pss:` from `smaps_rollup`, in bytes.
fn pss(rollup: &str) -> Option<u64> {
    rollup
        .lines()
        .find_map(|l| l.strip_prefix("Pss:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        .map(|kb| kb * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pss() {
        let rollup = "55d0-7ffd ---p 00000000 00:00 0  [rollup]\nRss:  120480 kB\nPss:   98304 kB\nPss_Anon: 1 kB\n";
        assert_eq!(pss(rollup), Some(98304 * 1024));
        assert_eq!(pss("Rss: 1 kB\n"), None);
    }

    #[test]
    fn only_one_new_process_is_claimed() {
        assert_eq!(only(vec![]), None);
        assert_eq!(only(vec![7]), Some(7));
        assert_eq!(only(vec![7, 8]), None);
    }

    #[test]
    fn this_process_weighs_itself_by_its_own_memory() {
        // Not a web process: nothing to weigh by that name.
        assert_eq!(weigh(std::process::id()), None);
        assert!(bytes(std::process::id()).is_some_and(|b| b > 0));
    }
}
