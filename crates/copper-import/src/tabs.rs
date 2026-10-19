//! Chromium's open tabs (`FlowChromeTabs.swift`): an append-only SNSS
//! command stream in `Sessions/Session_*` (or the legacy `Current Session`).
//! The replay keeps windows, tabs, navigation entries, pins, groups and
//! last-active times. `Tabs_*` is the recently-closed list, whose ids
//! restart every launch: it is never replayed as open tabs.
//!
//! A running browser keeps its session file open. When it won't share it
//! (Windows) and nothing else could be read, the read is
//! [`ImportError::InUse`]: close the browser and try again.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use copper_core::groups::TabGroup;
use copper_core::session::Space;

use crate::{ImportError, copy_read, profile_name, profiles};

/// One open page.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenTab {
    pub url: String,
    pub title: String,
    pub pinned: bool,
    /// The id of a group in the owning [`ImportedSpace::groups`].
    pub group: Option<String>,
    /// When it was last in front, Unix seconds.
    pub seen: Option<f64>,
    /// The tab that was on screen in its window.
    pub active: bool,
}

/// One window, as a Copper space to be: "Chrome", "Chrome 2", or
/// "Chrome · Work" for another profile (whose own cookie jar it wears).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedSpace {
    pub space: Space,
    pub groups: Vec<TabGroup>,
    pub tabs: Vec<OpenTab>,
}

const SESSION_PREFIX: &str = "Session_";
const TABS_PREFIX: &str = "Tabs_";

fn is_session_file(name: &str) -> bool {
    name == "Current Session"
        || name == "Current Tabs"
        || name.starts_with(SESSION_PREFIX)
        || name.starts_with(TABS_PREFIX)
}

fn names(folder: &Path) -> Vec<String> {
    std::fs::read_dir(folder)
        .map(|r| {
            r.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default()
}

fn has_session_files(folder: &Path) -> bool {
    names(folder).iter().any(|n| is_session_file(n))
}

/// Every profile of the browser at `root`. `source` names the spaces.
/// An error only when session files exist and none could be read; a fresh
/// browser with none at all has no tabs open.
pub fn read(root: &Path, source: &str) -> Result<Vec<ImportedSpace>, ImportError> {
    let profiles = profiles(root);
    if profiles.is_empty() {
        return Err(ImportError::Unreadable(format!("{source}'s session files")));
    }
    let mut spaces = Vec::new();
    let (mut read_any, mut any_files, mut in_use) = (false, false, false);
    for profile in &profiles {
        let profile_root = root.join(profile);
        let sessions = profile_root.join("Sessions");
        let folder = if has_session_files(&sessions) {
            sessions
        } else {
            profile_root.clone()
        };
        any_files |= has_session_files(&folder);
        if let Some((found, any)) =
            parse(&folder, &profile_root, Some(profile), source, &mut in_use)
        {
            read_any |= any;
            spaces.extend(found);
        }
    }
    if !read_any && any_files {
        return Err(unreadable(format!("{source}'s session files"), in_use));
    }
    Ok(spaces)
}

/// One profile folder (or its `Sessions`).
pub fn read_profile(profile_dir: &Path, source: &str) -> Result<Vec<ImportedSpace>, ImportError> {
    let profile = profile_dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string);
    let sessions = profile_dir.join("Sessions");
    let folder = if has_session_files(&sessions) {
        sessions
    } else {
        profile_dir.to_path_buf()
    };
    let mut in_use = false;
    match parse(
        &folder,
        profile_dir,
        profile.as_deref(),
        source,
        &mut in_use,
    ) {
        Some((spaces, true)) => Ok(spaces),
        _ => Err(unreadable("session files".into(), in_use)),
    }
}

/// Nothing could be read: because the browser holds the files, or not.
fn unreadable(what: String, in_use: bool) -> ImportError {
    if in_use {
        ImportError::InUse(what)
    } else {
        ImportError::Unreadable(what)
    }
}

/// A session file's bytes from a copy, noting when the browser holds it.
fn read_noting(file: &Path, in_use: &mut bool) -> Option<Vec<u8>> {
    copy_read(file)
        .map_err(|e: io::Error| *in_use |= crate::copy::is_in_use(&e))
        .ok()
}

fn candidates(folder: &Path, prefix: &str, legacy: &str) -> Vec<PathBuf> {
    let all = names(folder);
    let mut modern: Vec<&String> = all.iter().filter(|n| n.starts_with(prefix)).collect();
    modern.sort_by_key(|n| std::cmp::Reverse(n[prefix.len()..].parse::<i64>().unwrap_or(0)));
    if !modern.is_empty() {
        return modern.into_iter().map(|n| folder.join(n)).collect();
    }
    all.iter()
        .filter(|n| *n == legacy)
        .map(|n| folder.join(n))
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Session,
    Tabs,
}

struct Command {
    id: u8,
    payload: Vec<u8>,
    kind: Kind,
}

#[derive(Clone)]
struct Nav {
    index: i32,
    url: String,
    title: String,
}

#[derive(Default)]
struct TabState {
    id: i32,
    window: Option<i32>,
    index: Option<i32>,
    navs: HashMap<i32, Nav>,
    selected: Option<i32>,
    pinned: bool,
    group: Option<(u64, u64)>,
    last_active: Option<f64>,
}

struct WindowState {
    id: i32,
    selected_tab: i32,
    kind: i32,
}

impl WindowState {
    fn new(id: i32) -> Self {
        WindowState {
            id,
            selected_tab: -1,
            kind: 0,
        }
    }
}

#[derive(Default)]
struct GroupState {
    title: String,
    hue: Option<f64>,
}

fn parse(
    folder: &Path,
    profile_root: &Path,
    profile: Option<&str>,
    source: &str,
    in_use: &mut bool,
) -> Option<(Vec<ImportedSpace>, bool)> {
    let sessions = candidates(folder, SESSION_PREFIX, "Current Session");
    let closed = candidates(folder, TABS_PREFIX, "Current Tabs");
    if sessions.is_empty() && closed.is_empty() {
        return Some((Vec::new(), false));
    }
    let mut commands = None;
    for file in &sessions {
        if let Some(parsed) =
            read_noting(file, in_use).and_then(|d| parse_commands(&d, Kind::Session))
        {
            commands = Some(parsed);
            break;
        }
    }
    let Some(commands) = commands else {
        // Readable, but only a closed-tabs list: nothing open to bring.
        let readable = closed
            .first()
            .and_then(|f| read_noting(f, in_use))
            .and_then(|d| parse_commands(&d, Kind::Tabs))
            .is_some();
        return Some((Vec::new(), readable));
    };

    let mut tabs: HashMap<i32, TabState> = HashMap::new();
    let mut windows: HashMap<i32, WindowState> = HashMap::new();
    let mut groups: HashMap<(u64, u64), GroupState> = HashMap::new();
    for c in &commands {
        replay(c, &mut tabs, &mut windows, &mut groups);
    }

    let everyday = matches!(profile, None | Some("") | Some("Default"));
    let label = profile_name(profile_root);
    let base = if everyday {
        source.to_string()
    } else {
        format!(
            "{source} · {}",
            label
                .or(profile.map(str::to_string))
                .unwrap_or_else(|| source.into())
        )
    };
    let worn = if everyday {
        None
    } else {
        profile.map(str::to_string)
    };

    let mut ordered: Vec<&WindowState> = windows.values().filter(|w| w.kind == 0).collect();
    ordered.sort_by_key(|w| w.id);
    let mut spaces = Vec::new();
    for (number, window) in ordered.iter().enumerate() {
        let mut states: Vec<&TabState> = tabs
            .values()
            .filter(|t| t.window == Some(window.id))
            .collect();
        states.sort_by(|a, b| {
            let (l, r) = (a.index.unwrap_or(i32::MAX), b.index.unwrap_or(i32::MAX));
            l.cmp(&r).then(a.id.cmp(&b.id))
        });
        let mut out: Vec<(OpenTab, Option<(u64, u64)>)> = Vec::new();
        for tab in states {
            let chosen = tab
                .selected
                .and_then(|s| tab.navs.get(&s))
                .map(|n| n.index)
                .or_else(|| tab.navs.keys().max().copied());
            let Some(nav) = chosen.and_then(|i| tab.navs.get(&i)) else {
                continue;
            };
            let Ok(parsed) = url::Url::parse(&nav.url) else {
                continue;
            };
            if !matches!(parsed.scheme(), "http" | "https") {
                continue;
            }
            out.push((
                OpenTab {
                    url: nav.url.clone(),
                    title: nav.title.clone(),
                    pinned: tab.pinned,
                    group: None,
                    seen: tab.last_active,
                    active: tab.index == Some(window.selected_tab),
                },
                tab.group,
            ));
        }
        let mut model_groups: Vec<TabGroup> = Vec::new();
        let mut ids: HashMap<(u64, u64), String> = HashMap::new();
        for (_, key) in &out {
            let Some(key) = key else { continue };
            if ids.contains_key(key) {
                continue;
            }
            let state = groups.get(key);
            let name = state
                .map(|s| s.title.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Tab Group".into());
            let group = TabGroup::new(name, state.and_then(|s| s.hue));
            ids.insert(*key, group.id.clone());
            model_groups.push(group);
        }
        let final_tabs = out
            .into_iter()
            .map(|(mut tab, key)| {
                tab.group = key.and_then(|k| ids.get(&k).cloned());
                tab
            })
            .collect();
        let mut space = Space::new(if number == 0 {
            base.clone()
        } else {
            format!("{base} {}", number + 1)
        });
        space.profile = worn.clone();
        spaces.push(ImportedSpace {
            space,
            groups: model_groups,
            tabs: final_tabs,
        });
    }
    Some((spaces, true))
}

fn u16_at(d: &[u8], at: usize) -> usize {
    usize::from(d[at]) | (usize::from(d[at + 1]) << 8)
}

fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

fn u64_at(d: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[at..at + 8]);
    u64::from_le_bytes(b)
}

fn parse_commands(data: &[u8], kind: Kind) -> Option<Vec<Command>> {
    if data.len() < 8 || &data[..4] != b"SNSS" {
        return None;
    }
    // 1: the old cleartext stream; 3: cleartext with a marker. 4/5 are
    // OS-crypt encrypted and skipped.
    let version = u32_at(data, 4);
    if version != 1 && version != 3 {
        return None;
    }
    let valid_at = |at: usize| {
        at + 2 <= data.len() && {
            let size = u16_at(data, at);
            size >= 1 && size <= data.len() - at - 2
        }
    };
    let mut offset = 8;
    let mut out = Vec::new();
    while offset + 2 <= data.len() {
        let size = u16_at(data, offset);
        if size < 1 || size > data.len() - offset - 2 {
            break;
        }
        let id = data[offset + 2];
        let end = offset + 2 + size;
        if id != 255 {
            out.push(Command {
                id,
                payload: data[offset + 3..end].to_vec(),
                kind,
            });
        }
        let mut next = end;
        // A few old writers padded records to four bytes.
        if !next.is_multiple_of(4) {
            let aligned = (next + 3) & !3;
            if !valid_at(next) && valid_at(aligned) {
                next = aligned;
            }
        }
        offset = next;
    }
    Some(out)
}

fn replay(
    c: &Command,
    tabs: &mut HashMap<i32, TabState>,
    windows: &mut HashMap<i32, WindowState>,
    groups: &mut HashMap<(u64, u64), GroupState>,
) {
    let p = &c.payload;
    fn state(tabs: &mut HashMap<i32, TabState>, id: i32) -> &mut TabState {
        tabs.entry(id).or_insert_with(|| TabState {
            id,
            ..Default::default()
        })
    }
    if c.kind == Kind::Tabs {
        match c.id {
            1 => {
                if let Some((t, nav)) = read_navigation(p) {
                    state(tabs, t).navs.insert(nav.index, nav);
                }
            }
            4 => {
                if let Some((t, i)) = read_pair(p) {
                    state(tabs, t).selected = Some(i);
                }
            }
            5 => {
                if let Some((t, pinned)) = read_pinned(p) {
                    state(tabs, t).pinned = pinned;
                }
            }
            _ => {}
        }
        return;
    }
    match c.id {
        0 => {
            if let Some((w, t)) = read_pair(p) {
                state(tabs, t).window = Some(w);
                windows.entry(w).or_insert_with(|| WindowState::new(w));
            }
        }
        2 => {
            if let Some((t, i)) = read_pair(p) {
                state(tabs, t).index = Some(i);
            }
        }
        5 => {
            if let Some((t, i)) = read_pair(p).filter(|(_, i)| *i >= 0) {
                prune(state(tabs, t), i, i32::MAX, false);
            }
        }
        11 => {
            if let Some((t, i)) = read_pair(p).filter(|(_, i)| *i > 0) {
                prune(state(tabs, t), 0, i, true);
            }
        }
        6 => {
            if let Some((t, nav)) = read_navigation(p) {
                state(tabs, t).navs.insert(nav.index, nav);
            }
        }
        7 => {
            if let Some((t, i)) = read_pair(p) {
                state(tabs, t).selected = Some(i);
            }
        }
        8 => {
            if let Some((w, i)) = read_pair(p) {
                windows
                    .entry(w)
                    .or_insert_with(|| WindowState::new(w))
                    .selected_tab = i;
            }
        }
        9 => {
            if let Some((w, k)) = read_pair(p) {
                windows.entry(w).or_insert_with(|| WindowState::new(w)).kind = k;
            }
        }
        12 => {
            if let Some((t, pinned)) = read_pinned(p) {
                state(tabs, t).pinned = pinned;
            }
        }
        16 => {
            if let Some(t) = read_closed_id(p) {
                tabs.remove(&t);
            }
        }
        17 => {
            if let Some(w) = read_closed_id(p) {
                windows.remove(&w);
            }
        }
        21 => {
            if let Some((t, micros)) = read_last_active(p) {
                state(tabs, t).last_active = crate::chromium_to_unix(micros);
            }
        }
        24 => {
            if let Some((t, i, n)) = read_triple(p).filter(|(_, i, n)| *i >= 0 && *n > 0) {
                prune(state(tabs, t), i, n, true);
            }
        }
        25 => {
            if let Some((t, g)) = read_tab_group(p) {
                state(tabs, t).group = g;
            }
        }
        27 => {
            if let Some((key, title, hue)) = read_group_metadata(p) {
                groups.insert(key, GroupState { title, hue });
            }
        }
        _ => {}
    }
}

fn prune(state: &mut TabState, index: i32, count: i32, shifts: bool) {
    let start = i64::from(index);
    let amount = i64::from(count);
    let end = if count == i32::MAX {
        i64::MAX
    } else {
        start + amount
    };
    let mut kept = HashMap::new();
    for (old, nav) in state.navs.drain() {
        let old = i64::from(old);
        if old >= start && old < end {
            continue;
        }
        let candidate = if shifts && old >= end {
            old - amount
        } else {
            old
        };
        let Ok(new) = i32::try_from(candidate) else {
            continue;
        };
        let mut moved = nav;
        moved.index = new;
        kept.insert(new, moved);
    }
    if let Some(sel) = state.selected {
        let s = i64::from(sel);
        if s >= start && s < end {
            state.selected = (index > 0).then(|| index - 1);
        } else if shifts && s >= end {
            state.selected = i32::try_from(s - amount).ok();
        }
    }
    state.navs = kept;
}

/// A Chromium `Pickle`: a u32 payload size, then 4-byte aligned fields.
struct Pickle<'a> {
    b: &'a [u8],
    at: usize,
    end: usize,
}

impl<'a> Pickle<'a> {
    fn new(b: &'a [u8]) -> Option<Self> {
        if b.len() < 4 || u32_at(b, 0) as usize != b.len() - 4 {
            return None;
        }
        Some(Pickle {
            b,
            at: 4,
            end: b.len(),
        })
    }

    fn align(v: usize) -> usize {
        (v + 3) & !3
    }

    fn take(&mut self, n: usize) -> Option<usize> {
        if self.at + n > self.end {
            return None;
        }
        let here = self.at;
        self.at = Self::align(self.at + n);
        (self.at <= self.end).then_some(here)
    }

    fn i32(&mut self) -> Option<i32> {
        self.take(4).map(|at| u32_at(self.b, at) as i32)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|at| u32_at(self.b, at))
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|at| u64_at(self.b, at))
    }

    fn i64(&mut self) -> Option<i64> {
        self.u64().map(|v| v as i64)
    }

    fn bool(&mut self) -> Option<bool> {
        if self.at >= self.end {
            return None;
        }
        let v = self.b[self.at] != 0;
        self.at = Self::align(self.at + 1);
        (self.at <= self.end).then_some(v)
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > self.end - self.at {
            return None;
        }
        let at = self.take(n)?;
        Some(&self.b[at..at + n])
    }

    fn string(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }

    fn string16(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        let bytes = n.checked_mul(2)?;
        if bytes > self.end - self.at {
            return None;
        }
        let at = self.take(bytes)?;
        let units: Vec<u16> = (0..n)
            .map(|i| u16::from(self.b[at + 2 * i]) | (u16::from(self.b[at + 2 * i + 1]) << 8))
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}

fn zero(b: &[u8], from: usize, through: usize) -> bool {
    through < b.len() && from <= through && b[from..=through].iter().all(|&x| x == 0)
}

fn read_pair(p: &[u8]) -> Option<(i32, i32)> {
    if let Some(mut k) = Pickle::new(p)
        && let (Some(a), Some(b)) = (k.i32(), k.i32())
    {
        return Some((a, b));
    }
    (p.len() >= 8).then(|| (u32_at(p, 0) as i32, u32_at(p, 4) as i32))
}

fn read_pinned(p: &[u8]) -> Option<(i32, bool)> {
    if p.len() == 8 && zero(p, 5, 7) {
        return Some((u32_at(p, 0) as i32, p[4] != 0));
    }
    if let Some(mut k) = Pickle::new(p)
        && let (Some(t), Some(b)) = (k.i32(), k.bool())
    {
        return Some((t, b));
    }
    (p.len() >= 5).then(|| (u32_at(p, 0) as i32, p[4] != 0))
}

fn read_closed_id(p: &[u8]) -> Option<i32> {
    if p.len() >= 16 && zero(p, 4, 7) {
        return Some(u32_at(p, 0) as i32);
    }
    if let Some(mut k) = Pickle::new(p) {
        return k.i32();
    }
    (p.len() >= 4).then(|| u32_at(p, 0) as i32)
}

fn read_last_active(p: &[u8]) -> Option<(i32, i64)> {
    if p.len() == 16 && u32_at(p, 0) != 12 && zero(p, 4, 7) {
        return Some((u32_at(p, 0) as i32, u64_at(p, 8) as i64));
    }
    if let Some(mut k) = Pickle::new(p)
        && let (Some(t), Some(m)) = (k.i32(), k.i64())
    {
        return Some((t, m));
    }
    (p.len() >= 16).then(|| (u32_at(p, 0) as i32, u64_at(p, 8) as i64))
}

fn read_triple(p: &[u8]) -> Option<(i32, i32, i32)> {
    if let Some(mut k) = Pickle::new(p)
        && let (Some(a), Some(b), Some(c)) = (k.i32(), k.i32(), k.i32())
    {
        return Some((a, b, c));
    }
    (p.len() >= 12).then(|| {
        (
            u32_at(p, 0) as i32,
            u32_at(p, 4) as i32,
            u32_at(p, 8) as i32,
        )
    })
}

fn read_navigation(p: &[u8]) -> Option<(i32, Nav)> {
    let mut k = Pickle::new(p)?;
    let tab = k.i32()?;
    let index = k.i32()?;
    let url = k.string()?;
    let title = k.string16()?;
    k.bytes()?; // encoded page state: opaque, not necessarily UTF-8
    k.i32()?;
    Some((tab, Nav { index, url, title }))
}

type GroupKey = (u64, u64);

fn read_tab_group(p: &[u8]) -> Option<(i32, Option<GroupKey>)> {
    let pick = |tab: i32, high: u64, low: u64, has: bool| {
        Some((tab, (has && (high != 0 || low != 0)).then_some((high, low))))
    };
    if p.len() == 32 && zero(p, 4, 7) {
        return pick(u32_at(p, 0) as i32, u64_at(p, 8), u64_at(p, 16), p[24] != 0);
    }
    if let Some(mut k) = Pickle::new(p)
        && let (Some(t), Some(h), Some(l)) = (k.i32(), k.u64(), k.u64())
    {
        let has = k.bool().unwrap_or(true);
        return pick(t, h, l, has);
    }
    if p.len() < 24 {
        return None;
    }
    pick(
        u32_at(p, 0) as i32,
        u64_at(p, 8),
        u64_at(p, 16),
        p.len() < 25 || p[24] != 0,
    )
}

fn read_group_metadata(p: &[u8]) -> Option<(GroupKey, String, Option<f64>)> {
    let mut k = Pickle::new(p)?;
    let high = k.u64()?;
    let low = k.u64()?;
    let title = k.string16()?;
    let color = k.u32()?;
    let hue = (color != 0).then(|| (f64::from(color) / 8.0).min(1.0));
    Some(((high, low), title, hue))
}
