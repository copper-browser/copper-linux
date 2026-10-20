//! Which tabs sleep, and when (Mac `Sleep.swift` 29-151; spec chrome.md
//! §10).
//!
//! A tab left alone for `sleep.after` (half an hour) only becomes
//! *eligible*: tabs sleep, longest-left first, only while more pages are
//! loaded than this computer keeps easily ([`budget`]: two for every three
//! gigabytes, 6 to 40), or once left for twelve hours ([`CEILING`]). When
//! memory runs short the wait shrinks — to five minutes, to nothing when it
//! is critical — and the budget no longer protects anyone.
//!
//! Some tabs never sleep, because waking them could not give back what
//! they were doing ([`keep`]). Private tabs are among them here: a private
//! tab's cookies live only as long as its page, so sleeping it would sign
//! it out of everything (the Mac keeps its shy tabs' data in memory
//! elsewhere).
//!
//! Everything here is pure; [`crate::Shell`] gathers the facts, asks the
//! page whether it holds anything typed, pictures it and lets it go.

use std::time::Duration;

/// Left this long, a page goes whatever the budget says.
pub const CEILING: f64 = 12.0 * 3600.0;
/// Memory short (Windows' "low memory", Linux under 15 % available): tabs
/// left five minutes may go.
pub const SHORT_WAIT: f64 = 5.0 * 60.0;

/// Why a tab stays awake (Mac `awake(because:)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    OnScreen,
    Pinned,
    Agent,
    Blank,
    Private,
    Asleep,
    NoPage,
    Loading,
    Audio,
    Call,
    Downloading,
    Opener,
}

impl Keep {
    /// In the Mac's words.
    pub fn reason(self) -> &'static str {
        match self {
            Keep::OnScreen => "on screen",
            Keep::Pinned => "pinned",
            Keep::Agent => "an agent is working in it",
            Keep::Blank => "blank",
            Keep::Private => "private",
            Keep::Asleep => "already asleep",
            Keep::NoPage => "no page",
            Keep::Loading => "still loading",
            Keep::Audio => "playing sound",
            Keep::Call => "on a call",
            Keep::Downloading => "downloading",
            Keep::Opener => "the page on screen came from it",
        }
    }
}

/// What the shell knows about one tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    /// Selected, or the other half of the selected split.
    pub on_screen: bool,
    pub pinned: bool,
    pub agent: bool,
    pub blank: bool,
    pub private: bool,
    pub asleep: bool,
    /// It has a live page.
    pub page: bool,
    pub loading: bool,
    pub audio: bool,
    pub call: bool,
    pub downloading: bool,
    /// The tab on screen was opened by this one (a sign-in window hands its
    /// answer back to it).
    pub opener: bool,
}

/// Why `f` must stay awake, `None` when nothing keeps it. The clock is the
/// caller's business; this is everything else. (Not here: a page holding
/// something typed — the page is asked, just before it would go.)
pub fn keep(f: &Facts) -> Option<Keep> {
    let checks = [
        (f.on_screen, Keep::OnScreen),
        (f.pinned, Keep::Pinned),
        (f.agent, Keep::Agent),
        (f.blank, Keep::Blank),
        (f.private, Keep::Private),
        (f.asleep, Keep::Asleep),
        (!f.page, Keep::NoPage),
        (f.loading, Keep::Loading),
        (f.audio, Keep::Audio),
        (f.call, Keep::Call),
        (f.downloading, Keep::Downloading),
        (f.opener, Keep::Opener),
    ];
    checks.into_iter().find(|(on, _)| *on).map(|(_, k)| k)
}

/// How many pages stay loaded however long they are left: two for every
/// three gigabytes (24 on 36 GB, 10 on 16), at least 6, at most 40 — or
/// `sleep.keep`. Memory unknown: the least.
pub fn budget(memory_bytes: Option<u64>, keep_setting: Option<usize>) -> usize {
    if let Some(n) = keep_setting.filter(|n| *n > 0) {
        return n;
    }
    let gib = memory_bytes.unwrap_or(0) / (1 << 30);
    (gib as usize * 2 / 3).clamp(6, 40)
}

/// How often the sleeper looks: a quarter of the wait, 5 s to a minute.
pub fn every(after: f64) -> Duration {
    Duration::from_secs_f64((after / 4.0).clamp(5.0, 60.0))
}

/// How short memory is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pressure {
    #[default]
    None,
    /// A warning: tabs left five minutes go.
    Warning,
    /// Critical: every eligible tab goes now.
    Critical,
}

impl Pressure {
    /// From total and available bytes: under 5 % free is critical, under
    /// 15 % a warning.
    pub fn of(total: u64, available: u64) -> Pressure {
        if total == 0 {
            return Pressure::None;
        }
        let free = available as f64 / total as f64;
        if free < 0.05 {
            Pressure::Critical
        } else if free < 0.15 {
            Pressure::Warning
        } else {
            Pressure::None
        }
    }
}

/// One tab, for [`choose`].
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate<K> {
    pub id: K,
    /// Seconds since it was last looked at.
    pub idle: f64,
    /// It has a page loaded (counts against the budget).
    pub loaded: bool,
    pub keep: Option<Keep>,
    /// A space comes back to it: it waits for memory to run short (Mac
    /// "space-keepalive").
    pub remembered: bool,
}

/// The rules in force.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// `sleep.after`.
    pub after: f64,
    /// `sleep.after` was set by hand: no budget.
    pub after_set: bool,
    /// [`budget`].
    pub budget: usize,
    pub pressure: Pressure,
}

/// The tabs to put to sleep now, the one left longest first (Mac
/// `sleepIdle`).
pub fn choose<K: Clone>(tabs: &[Candidate<K>], rules: &Rules) -> Vec<K> {
    let wait = match rules.pressure {
        Pressure::None => rules.after,
        Pressure::Warning => rules.after.min(SHORT_WAIT),
        Pressure::Critical => 0.0,
    };
    let mut idle: Vec<&Candidate<K>> = tabs
        .iter()
        .filter(|t| t.keep.is_none() && t.idle >= wait)
        .filter(|t| rules.pressure != Pressure::None || !t.remembered)
        .collect();
    idle.sort_by(|a, b| b.idle.total_cmp(&a.idle));
    if rules.pressure != Pressure::None || rules.after_set {
        return idle.into_iter().map(|t| t.id.clone()).collect();
    }
    let loaded = tabs.iter().filter(|t| t.loaded).count();
    let mut over = loaded as i64 - rules.budget as i64;
    let mut chosen = Vec::new();
    for t in idle {
        if over > 0 || t.idle >= CEILING {
            chosen.push(t.id.clone());
            over -= 1;
        }
    }
    chosen
}

/// Total and available memory, from `/proc/meminfo`'s text.
pub fn meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|rest| {
                rest.trim()
                    .trim_end_matches("kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .map(|kb| kb * 1024)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

/// This computer's memory, total and available, in bytes, where the
/// standard library can't say: `/proc/meminfo` on Linux; `None` elsewhere
/// (a platform says for itself, [`crate::Platform::memory`]).
pub fn system_memory() -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    {
        meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: u32, idle: f64, loaded: bool) -> Candidate<u32> {
        Candidate {
            id,
            idle,
            loaded,
            keep: None,
            remembered: false,
        }
    }

    fn rules(budget: usize) -> Rules {
        Rules {
            after: 1800.0,
            after_set: false,
            budget,
            pressure: Pressure::None,
        }
    }

    #[test]
    fn eligible_tabs_sleep_only_over_the_budget_longest_left_first() {
        let tabs = vec![
            tab(1, 4000.0, true),
            tab(2, 9000.0, true),
            tab(3, 100.0, true),
            tab(4, 2000.0, true),
        ];
        // Under budget: nothing.
        assert!(choose(&tabs, &rules(6)).is_empty());
        // One over: the one left longest.
        assert_eq!(choose(&tabs, &rules(3)), vec![2]);
        // Two over: the two left longest; the fresh one never.
        assert_eq!(choose(&tabs, &rules(2)), vec![2, 1]);
        assert_eq!(choose(&tabs, &rules(0)), vec![2, 1, 4]);
    }

    #[test]
    fn a_spaces_own_tab_waits_for_short_memory() {
        let mut kept = tab(1, CEILING * 2.0, true);
        kept.remembered = true;
        let tabs = vec![kept];
        assert!(choose(&tabs, &rules(0)).is_empty());
        let mut r = rules(0);
        r.pressure = Pressure::Warning;
        assert_eq!(choose(&tabs, &r), vec![1]);
    }

    #[test]
    fn twelve_hours_sleeps_whatever_the_budget() {
        let tabs = vec![tab(1, CEILING + 1.0, true), tab(2, 4000.0, true)];
        assert_eq!(choose(&tabs, &rules(40)), vec![1]);
    }

    #[test]
    fn kept_tabs_never_sleep_but_count_against_the_budget() {
        let mut playing = tab(1, CEILING * 2.0, true);
        playing.keep = Some(Keep::Audio);
        let tabs = vec![playing, tab(2, 4000.0, true)];
        assert_eq!(choose(&tabs, &rules(1)), vec![2]);
    }

    #[test]
    fn a_hand_set_wait_or_short_memory_skips_the_budget() {
        let tabs = vec![tab(1, 400.0, true), tab(2, 30.0, true)];
        let mut r = rules(40);
        r.after = 20.0;
        r.after_set = true;
        assert_eq!(choose(&tabs, &r), vec![1, 2]);
        let mut r = rules(40);
        r.pressure = Pressure::Warning;
        assert_eq!(choose(&tabs, &r), vec![1]);
        r.pressure = Pressure::Critical;
        assert_eq!(choose(&tabs, &r), vec![1, 2]);
    }

    #[test]
    fn what_keeps_a_tab_awake() {
        assert_eq!(
            keep(&Facts {
                page: true,
                ..Facts::default()
            }),
            None
        );
        assert_eq!(keep(&Facts::default()), Some(Keep::NoPage));
        let all = |f: fn(&mut Facts)| {
            let mut x = Facts {
                page: true,
                ..Facts::default()
            };
            f(&mut x);
            keep(&x)
        };
        assert_eq!(all(|f| f.on_screen = true), Some(Keep::OnScreen));
        assert_eq!(all(|f| f.pinned = true), Some(Keep::Pinned));
        assert_eq!(all(|f| f.agent = true), Some(Keep::Agent));
        assert_eq!(all(|f| f.blank = true), Some(Keep::Blank));
        assert_eq!(all(|f| f.private = true), Some(Keep::Private));
        assert_eq!(all(|f| f.asleep = true), Some(Keep::Asleep));
        assert_eq!(all(|f| f.loading = true), Some(Keep::Loading));
        assert_eq!(all(|f| f.audio = true), Some(Keep::Audio));
        assert_eq!(all(|f| f.call = true), Some(Keep::Call));
        assert_eq!(all(|f| f.downloading = true), Some(Keep::Downloading));
        assert_eq!(all(|f| f.opener = true), Some(Keep::Opener));
        assert_eq!(Keep::Call.reason(), "on a call");
    }

    #[test]
    fn the_budget_follows_memory() {
        let gib = |n: u64| Some(n << 30);
        assert_eq!(budget(gib(36), None), 24);
        assert_eq!(budget(gib(16), None), 10);
        assert_eq!(budget(gib(4), None), 6);
        assert_eq!(budget(gib(128), None), 40);
        assert_eq!(budget(None, None), 6);
        assert_eq!(budget(gib(16), Some(3)), 3);
    }

    #[test]
    fn the_timer_is_a_quarter_of_the_wait() {
        assert_eq!(every(1800.0), Duration::from_secs(60));
        assert_eq!(every(60.0), Duration::from_secs(15));
        assert_eq!(every(4.0), Duration::from_secs(5));
    }

    #[test]
    fn pressure_and_meminfo() {
        assert_eq!(Pressure::of(100, 50), Pressure::None);
        assert_eq!(Pressure::of(100, 10), Pressure::Warning);
        assert_eq!(Pressure::of(100, 4), Pressure::Critical);
        assert_eq!(Pressure::of(0, 0), Pressure::None);
        let text = "MemTotal:       16314884 kB\nMemFree:  100 kB\nMemAvailable:    8157442 kB\n";
        assert_eq!(meminfo(text), Some((16314884 * 1024, 8157442 * 1024)));
        assert_eq!(meminfo("nothing"), None);
    }
}
