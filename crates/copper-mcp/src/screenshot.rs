//! `browser_take_screenshot` (and `copper shot`): any section of a tab — an
//! element, a rectangle, the viewport or the whole page — at 1x to 4x,
//! optionally saved to a file. The same arguments, rules and answer as
//! Copper on the Mac.
//!
//! This module is the pure part: reading the arguments, the rectangle
//! maths (padding, clamping, the size cap), the answer sentence and where a
//! file goes. `tools.rs` measures the page and asks the engine for pixels
//! (`PageView::capture`); `shot.rs` crops, scales and encodes them.
//!
//! Every rectangle here is in view pixels: CSS pixels times the page zoom,
//! the coordinates of a scale-1 viewport picture (CSS pixels at 100 %).

use std::path::PathBuf;

use copper_engine::{CaptureArea, ViewRect};
use serde_json::{Map, Value};

/// No side of a picture above this many pixels.
pub const MAX_SIDE: u64 = 16384;
/// And no more pixels than this in all (64 megapixels).
pub const MAX_PIXELS: u64 = 64_000_000;
/// The scales an agent may ask for.
pub const MIN_SCALE: f64 = 1.0;
pub const MAX_SCALE: f64 = 4.0;

pub const ONE_SUBJECT: &str = "give one of ref, selector or region";
pub const SCALE_RANGE: &str = "scale must be from 1 to 4";
pub const NEGATIVE_PADDING: &str = "padding must be 0 or more";
pub const BAD_REGION: &str = "region needs x, y, width and height: numbers in CSS pixels";
pub const EMPTY_REGION: &str = "region width and height must be more than 0";
pub const RELATIVE_FILENAME: &str = "filename must be an absolute path or start with ~/";
pub const NO_FRAMES: &str =
    "frame isn't supported here yet: give a selector in the page itself, a ref or a region";
/// The tallest (and widest) page a picture covers, in CSS pixels, as on the
/// Mac (the full page has always stopped here).
pub const TALLEST: f64 = 20000.0;

/// What the picture is of.
#[derive(Clone, Debug, PartialEq)]
pub enum Subject {
    /// The viewport, or the whole page with `fullPage`.
    Page,
    /// An element by ref, as given (`e12`).
    Ref(String),
    /// An element by CSS selector, as given.
    Selector(String),
    /// A rectangle, as given (before padding and clamping).
    Region(ViewRect),
}

impl Subject {
    /// The answer's words for it: `the viewport`, `the whole page`, `e12`,
    /// `#box-b`, `the region 100,100 200×100`.
    pub fn words(&self, full_page: bool) -> String {
        match self {
            Subject::Page if full_page => "the whole page".into(),
            Subject::Page => "the viewport".into(),
            Subject::Ref(r) => r.clone(),
            Subject::Selector(css) => css.clone(),
            Subject::Region(r) => format!(
                "the region {},{} {}×{}",
                number(r.x),
                number(r.y),
                number(r.width),
                number(r.height)
            ),
        }
    }
}

/// `browser_take_screenshot`'s arguments, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct ShotArgs {
    pub jpeg: bool,
    pub full_page: bool,
    pub subject: Subject,
    /// CSS pixels around an element or region (0 or more).
    pub padding: f64,
    /// Picture pixels per CSS pixel asked for (1 to 4).
    pub scale: f64,
    /// `filename`, absolute ([`absolute`]); a trailing separator says a
    /// folder.
    pub filename: Option<String>,
}

/// A non-empty string argument.
fn text<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// A number argument: `Ok(None)` when absent (or null), `Err(())` when it
/// is something else or not finite.
fn optional_number(args: &Map<String, Value>, key: &str) -> Result<Option<f64>, ()> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().filter(|n| n.is_finite()).map(Some).ok_or(()),
    }
}

impl ShotArgs {
    /// Read and check the arguments; the error is the agent's to read.
    pub fn parse(args: &Map<String, Value>) -> Result<ShotArgs, String> {
        let reference = text(args, "ref");
        let selector = text(args, "selector");
        let region = args.get("region").filter(|v| !v.is_null());
        let given = [reference.is_some(), selector.is_some(), region.is_some()];
        if given.iter().filter(|g| **g).count() > 1 {
            return Err(ONE_SUBJECT.into());
        }
        let subject = if let Some(r) = reference {
            Subject::Ref(r.to_string())
        } else if let Some(css) = selector {
            if text(args, "frame").is_some() {
                return Err(NO_FRAMES.into());
            }
            Subject::Selector(css.to_string())
        } else if let Some(region) = region {
            Subject::Region(parse_region(region)?)
        } else {
            Subject::Page
        };
        let padding = optional_number(args, "padding")
            .map_err(|()| NEGATIVE_PADDING.to_string())?
            .unwrap_or(0.0);
        if padding < 0.0 {
            return Err(NEGATIVE_PADDING.into());
        }
        let scale = optional_number(args, "scale")
            .map_err(|()| SCALE_RANGE.to_string())?
            .unwrap_or(1.0);
        if !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
            return Err(SCALE_RANGE.into());
        }
        let filename = text(args, "filename").map(absolute).transpose()?;
        Ok(ShotArgs {
            jpeg: args.get("type").and_then(Value::as_str) == Some("jpeg"),
            full_page: args.get("fullPage").and_then(Value::as_bool) == Some(true),
            subject,
            padding,
            scale,
            filename,
        })
    }

    /// The picture's format, as the answer says it.
    pub fn format(&self) -> &'static str {
        if self.jpeg { "JPEG" } else { "PNG" }
    }

    pub fn mime(&self) -> &'static str {
        if self.jpeg { "image/jpeg" } else { "image/png" }
    }
}

fn parse_region(raw: &Value) -> Result<ViewRect, String> {
    let n = |k: &str| {
        raw.get(k)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .ok_or_else(|| BAD_REGION.to_string())
    };
    let rect = ViewRect::new(n("x")?, n("y")?, n("width")?, n("height")?);
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return Err(EMPTY_REGION.into());
    }
    Ok(rect)
}

/// A number the way the answer prints it: at most two decimals, no
/// trailing zeros (`1`, `1.5`, `2.37`).
pub fn number(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

// MARK: - rectangles

/// What the page measured, in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// The viewport (the view's size over the page zoom).
    pub viewport: (f64, f64),
    /// The document's scrollable size, as the page says it.
    pub document: (f64, f64),
    /// How far the page is scrolled.
    pub scroll: (f64, f64),
}

impl Geometry {
    /// The whole page from its top-left: the document, at least the
    /// viewport, at most [`TALLEST`] each way.
    pub fn page(&self) -> (f64, f64) {
        let (vw, vh) = self.viewport;
        (
            self.document.0.min(TALLEST).max(vw),
            self.document.1.min(TALLEST).max(vh),
        )
    }
}

/// What to picture: the area to capture and the rectangle within it, both
/// in CSS pixels (from the viewport's top-left, or the page's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// Whether `rect` is measured from the page's top-left (else the
    /// viewport's).
    pub in_document: bool,
    pub rect: ViewRect,
    /// The whole viewport (no cut needed).
    pub whole: bool,
}

impl Plan {
    /// The engine's area for this plan, in view pixels (`zoom` times CSS).
    /// The whole page is asked for as its rectangle: from the page's top,
    /// wherever it is scrolled, and no more than [`TALLEST`] of it.
    pub fn area(&self, zoom: f64) -> CaptureArea {
        let r = ViewRect::new(
            self.rect.x * zoom,
            self.rect.y * zoom,
            self.rect.width * zoom,
            self.rect.height * zoom,
        );
        match (self.in_document, self.whole) {
            (false, true) => CaptureArea::Viewport,
            (false, false) => CaptureArea::ViewportRect(r),
            (true, _) => CaptureArea::DocumentRect(r),
        }
    }
}

/// What `a` and `b` have in common; `None` when nothing.
pub fn intersect(a: ViewRect, b: ViewRect) -> Option<ViewRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    (x1 > x0 && y1 > y0).then(|| ViewRect::new(x0, y0, x1 - x0, y1 - y0))
}

/// `r` grown by `by` on every side, then cut to (0, 0)–`bounds`; `None`
/// when nothing is left (`Shots.area`).
pub fn area(r: ViewRect, by: f64, bounds: (f64, f64)) -> Option<ViewRect> {
    let grown = ViewRect::new(r.x - by, r.y - by, r.width + 2.0 * by, r.height + 2.0 * by);
    intersect(grown, ViewRect::new(0.0, 0.0, bounds.0, bounds.1))
}

/// Within this much of the viewport's edge an element counts as inside it.
const EDGE: f64 = 0.5;

/// What to picture for `args`. `element` is the element's border box from
/// the viewport's top-left (CSS pixels) once it was scrolled into view;
/// `label` names it in errors.
pub fn plan(
    args: &ShotArgs,
    g: &Geometry,
    element: Option<ViewRect>,
    label: &str,
) -> Result<Plan, String> {
    let page = g.page();
    let whole = |(w, h): (f64, f64), in_document: bool| Plan {
        in_document,
        rect: ViewRect::new(0.0, 0.0, w, h),
        whole: !in_document,
    };
    let part = |rect, in_document| Plan {
        in_document,
        rect,
        whole: false,
    };
    match (&args.subject, element) {
        (Subject::Page, _) if args.full_page => Ok(whole(page, true)),
        (Subject::Page, _) => Ok(whole(g.viewport, false)),
        (Subject::Region(r), _) if args.full_page => area(*r, args.padding, page)
            .map(|rect| part(rect, true))
            .ok_or_else(|| "the region is outside the page".into()),
        (Subject::Region(r), _) => area(*r, args.padding, g.viewport)
            .map(|rect| part(rect, false))
            .ok_or_else(|| "the region is outside the viewport".into()),
        (_, Some(e)) => {
            if e.width <= 0.0 || e.height <= 0.0 {
                return Err(format!("{label} has no size: there is nothing to picture"));
            }
            // Its whole box on the page, wherever the viewport ends.
            let (sx, sy) = g.scroll;
            let on_page = ViewRect::new(e.x + sx, e.y + sy, e.width, e.height);
            let rect = area(on_page, args.padding, page)
                .ok_or_else(|| format!("{label} is outside the page"))?;
            let seen = ViewRect::new(rect.x - sx, rect.y - sy, rect.width, rect.height);
            let (vw, vh) = g.viewport;
            let inside = seen.x >= -EDGE
                && seen.y >= -EDGE
                && seen.x + seen.width <= vw + EDGE
                && seen.y + seen.height <= vh + EDGE;
            // In view: a picture of the viewport is enough. Else the page's.
            Ok(match area(seen, 0.0, g.viewport).filter(|_| inside) {
                Some(seen) => part(seen, false),
                None => part(rect, true),
            })
        }
        (_, None) => Err(format!("{label} has no box to picture")),
    }
}

// MARK: - the size cap

/// Which cap made the scale go down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// [`MAX_SIDE`] pixels on a side.
    Side,
    /// [`MAX_PIXELS`] in all.
    Pixels,
}

/// The scale a picture of `width` x `height` view pixels gets, and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub scale: f64,
    pub size: (u32, u32),
    /// The cap that lowered the asked scale, if one did.
    pub lowered: Option<Limit>,
}

/// The picture's pixel size at `scale`: each side rounded, at least 1.
pub fn size_at(width: f64, height: f64, scale: f64) -> (u64, u64) {
    let side = |v: f64| (v * scale).round().max(1.0) as u64;
    (side(width), side(height))
}

fn over(size: (u64, u64)) -> bool {
    size.0 > MAX_SIDE || size.1 > MAX_SIDE || size.0 * size.1 > MAX_PIXELS
}

/// `asked`, or the largest two-decimal scale under the caps.
pub fn fit(width: f64, height: f64, asked: f64) -> Fit {
    let width = width.max(f64::MIN_POSITIVE);
    let height = height.max(f64::MIN_POSITIVE);
    let size = |s: f64| {
        let (w, h) = size_at(width, height, s);
        (
            w.min(u64::from(u32::MAX)) as u32,
            h.min(u64::from(u32::MAX)) as u32,
        )
    };
    if !over(size_at(width, height, asked)) {
        return Fit {
            scale: asked,
            size: size(asked),
            lowered: None,
        };
    }
    let by_side = (MAX_SIDE as f64 / width).min(MAX_SIDE as f64 / height);
    let by_pixels = (MAX_PIXELS as f64 / (width * height)).sqrt();
    let mut scale = (asked.min(by_side).min(by_pixels) * 100.0).floor() / 100.0;
    // Rounding the sides may still pass a cap by a pixel.
    while scale > 0.01 && over(size_at(width, height, scale)) {
        scale = ((scale * 100.0).round() - 1.0) / 100.0;
    }
    let scale = scale.max(0.01);
    Fit {
        scale,
        size: size(scale),
        lowered: Some(if by_side <= by_pixels {
            Limit::Side
        } else {
            Limit::Pixels
        }),
    }
}

/// The longest side of the image an answer carries (models refuse bigger).
pub const MAX_VIEW_SIDE: u64 = 8000;

/// The scale and size of the answer's own image for a picture of `width` x
/// `height` CSS pixels at `scale`: scale 1 when the picture was saved at a
/// higher one (the file has it; the model gets clickable coordinates), and
/// never more than [`MAX_VIEW_SIDE`] on its longest side (the largest
/// two-decimal scale that fits).
pub fn view_fit(width: f64, height: f64, scale: f64, saved: bool) -> (f64, (u32, u32)) {
    let mut s = if saved && scale > 1.0 { 1.0 } else { scale };
    let longest = |s: f64| {
        let (w, h) = size_at(width, height, s);
        w.max(h)
    };
    if longest(s) > MAX_VIEW_SIDE {
        // A side under 8000.5 rounds to 8000 at most; the loop takes off
        // the hundredth that lands past it (`Shots.inline`).
        let fits = (MAX_VIEW_SIDE as f64 + 0.5) / width.max(height).max(f64::MIN_POSITIVE);
        s = (s.min(fits) * 100.0).floor() / 100.0;
        while s > 0.01 && longest(s) > MAX_VIEW_SIDE {
            s = ((s * 100.0).round() - 1.0) / 100.0;
        }
        s = s.max(0.01);
    }
    let (w, h) = size_at(width, height, s);
    (
        s,
        (
            w.min(u64::from(u32::MAX)) as u32,
            h.min(u64::from(u32::MAX)) as u32,
        ),
    )
}

// MARK: - the answer

/// Whether and where the picture was saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Saved {
    To(PathBuf),
    /// The path tried, and why it failed.
    Failed(PathBuf, String),
}

/// The answer's sentence, ahead of the page line:
/// `Screenshot 2880×1800 PNG at scale 2 of the viewport. Saved to /x.png.`
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub size: (u32, u32),
    pub format: &'static str,
    /// The scale the picture has (after any lowering).
    pub scale: f64,
    pub subject: String,
    pub saved: Option<Saved>,
    /// The scale asked for and the cap, when it was lowered.
    pub lowered: Option<(f64, Limit)>,
    /// The scale the engine could render at, when the picture was enlarged
    /// from it.
    pub upscaled: Option<f64>,
    /// The answer's own image, when it is not the picture: its size and
    /// scale ([`view_fit`]).
    pub view: Option<((u32, u32), f64)>,
}

impl Answer {
    pub fn text(&self, page_line: &str) -> String {
        let mut out = format!(
            "Screenshot {}×{} {} at scale {} of {}.",
            self.size.0,
            self.size.1,
            self.format,
            number(self.scale),
            self.subject
        );
        match &self.saved {
            Some(Saved::To(path)) => out.push_str(&format!(" Saved to {}.", path.display())),
            Some(Saved::Failed(path, why)) => {
                let why = why.strip_suffix('.').unwrap_or(why);
                out.push_str(&format!(" Couldn't save to {}: {why}.", path.display()));
            }
            None => {}
        }
        if let Some(((w, h), scale)) = self.view {
            let file = if matches!(self.saved, Some(Saved::To(_))) {
                "the file has the full picture"
            } else {
                "pass filename to keep the full picture"
            };
            out.push_str(&format!(
                " The image here is {w}×{h} at scale {}; {file}.",
                number(scale)
            ));
        }
        if let Some((asked, limit)) = self.lowered {
            let cap = match limit {
                Limit::Side => "16384 px",
                Limit::Pixels => "64 megapixels",
            };
            out.push_str(&format!(
                " Scale lowered from {} to {} to stay under {cap}.",
                number(asked),
                number(self.scale)
            ));
        }
        if let Some(rendered) = self.upscaled {
            out.push_str(&format!(
                " Upscaled: this page renders at most at scale {} here.",
                number(rendered)
            ));
        }
        out.push(' ');
        out.push_str(page_line);
        out
    }
}

// MARK: - files

/// `~` and `~/…` (on Windows `~\…` too) from the home folder; anything
/// else as given.
pub fn expand(path: &str) -> PathBuf {
    if path == "~" {
        return copper_core::platform::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    copper_core::platform::expand_tilde(path)
}

/// Whether `path` ends in a separator: a folder, even one not made yet.
fn names_a_folder(path: &str) -> bool {
    path.ends_with('/') || (cfg!(windows) && path.ends_with('\\'))
}

/// `filename` with `~` expanded, or an error when it is relative: Copper's
/// working folder is nobody's (the CLI makes a shell's relative path
/// absolute before it asks). A trailing separator is kept.
pub fn absolute(path: &str) -> Result<String, String> {
    let expanded = expand(path);
    if !expanded.is_absolute() {
        return Err(RELATIVE_FILENAME.into());
    }
    let mut out = expanded.to_string_lossy().into_owned();
    if names_a_folder(path) && !names_a_folder(&out) {
        out.push(std::path::MAIN_SEPARATOR);
    }
    Ok(out)
}

/// The file a picture asked for at `path` (absolute) goes to: `path`
/// itself, or for a folder (an existing directory, or a path ending in a
/// separator) `copper-<stamp>.png` (`.jpg` for JPEG) inside it, `-2`, `-3`…
/// when that is taken, so two pictures in one second both stay.
pub fn destination(path: &str, jpeg: bool, stamp: &str) -> PathBuf {
    let given = PathBuf::from(path);
    if !(names_a_folder(path) || given.is_dir()) {
        return given;
    }
    let ext = if jpeg { "jpg" } else { "png" };
    let first = given.join(format!("copper-{stamp}.{ext}"));
    let mut candidate = first.clone();
    let mut n = 1;
    while candidate.exists() && n < 1000 {
        n += 1;
        candidate = given.join(format!("copper-{stamp}-{n}.{ext}"));
    }
    candidate
}

/// Write a picture where `path` says ([`destination`]), making the folders
/// on the way; the file it went to, or why not. Slow on a hung volume: run
/// it off the UI thread.
pub fn save(bytes: &[u8], path: &str, jpeg: bool) -> Saved {
    let file = destination(path, jpeg, &stamp());
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&file, bytes));
    match written {
        Ok(()) => Saved::To(file),
        Err(e) => Saved::Failed(PathBuf::from(path), e.to_string()),
    }
}

/// A moment as `YYYYMMDD-HHMMSS`.
pub fn format_stamp(parts: (i64, u32, u32, u32, u32, u32)) -> String {
    let (y, mo, d, h, mi, s) = parts;
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// Now on the local clock as `YYYYMMDD-HHMMSS` (UTC where the local time
/// zone can't be read).
pub fn stamp() -> String {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    format_stamp(local::parts(unix).unwrap_or_else(|| utc_parts(unix)))
}

/// `unix` seconds as UTC (year, month, day, hour, minute, second).
pub fn utc_parts(unix: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400) as u32;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, secs / 3600, secs / 60 % 60, secs % 60)
}

/// The local wall clock, from the C library (Unix) or Windows.
mod local {
    /// `unix` on the local clock, `None` where it can't be read.
    #[cfg(all(unix, target_pointer_width = "64"))]
    pub fn parts(unix: i64) -> Option<(i64, u32, u32, u32, u32, u32)> {
        use std::ffi::c_int;
        /// `struct tm`'s standard fields, then room for the platform's own
        /// (`tm_gmtoff`, `tm_zone`).
        #[repr(C)]
        struct Tm {
            sec: c_int,
            min: c_int,
            hour: c_int,
            mday: c_int,
            mon: c_int,
            year: c_int,
            wday: c_int,
            yday: c_int,
            isdst: c_int,
            rest: [u64; 4],
        }
        unsafe extern "C" {
            fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
        }
        let mut tm = Tm {
            sec: 0,
            min: 0,
            hour: 0,
            mday: 0,
            mon: 0,
            year: 0,
            wday: 0,
            yday: 0,
            isdst: 0,
            rest: [0; 4],
        };
        // SAFETY: both pointers are valid for the call; `Tm` is at least as
        // big as the C library's `struct tm` on 64-bit Unix.
        let done = unsafe { localtime_r(&unix, &mut tm) };
        if done.is_null() {
            return None;
        }
        let field = |v: c_int| u32::try_from(v).ok();
        Some((
            i64::from(tm.year) + 1900,
            field(tm.mon)? + 1,
            field(tm.mday)?,
            field(tm.hour)?,
            field(tm.min)?,
            field(tm.sec)?.min(59),
        ))
    }

    #[cfg(windows)]
    pub fn parts(_unix: i64) -> Option<(i64, u32, u32, u32, u32, u32)> {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        // SAFETY: SYSTEMTIME is plain data; GetLocalTime fills it.
        let t: SYSTEMTIME = unsafe {
            let mut t = std::mem::zeroed();
            GetLocalTime(&mut t);
            t
        };
        Some((
            i64::from(t.wYear),
            u32::from(t.wMonth),
            u32::from(t.wDay),
            u32::from(t.wHour),
            u32::from(t.wMinute),
            u32::from(t.wSecond),
        ))
    }

    #[cfg(not(any(windows, all(unix, target_pointer_width = "64"))))]
    pub fn parts(_unix: i64) -> Option<(i64, u32, u32, u32, u32, u32)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    fn parse(v: Value) -> Result<ShotArgs, String> {
        ShotArgs::parse(&args(v))
    }

    #[test]
    fn old_calls_read_as_before() {
        let a = parse(json!({})).unwrap();
        assert_eq!(
            a,
            ShotArgs {
                jpeg: false,
                full_page: false,
                subject: Subject::Page,
                padding: 0.0,
                scale: 1.0,
                filename: None,
            }
        );
        let a = parse(json!({"fullPage": true, "type": "jpeg", "ref": "e3"})).unwrap();
        assert!(a.full_page && a.jpeg);
        assert_eq!(a.subject, Subject::Ref("e3".into()));
        assert_eq!((a.format(), a.mime()), ("JPEG", "image/jpeg"));
        // Empty strings and nulls are not there.
        let a = parse(json!({"ref": "", "selector": null, "region": null})).unwrap();
        assert_eq!(a.subject, Subject::Page);
    }

    #[test]
    fn new_arguments() {
        let a = parse(json!({
            "selector": "#box-b", "padding": 10, "scale": 1.5, "filename": "/tmp/x.png"
        }))
        .unwrap();
        assert_eq!(a.subject, Subject::Selector("#box-b".into()));
        assert_eq!((a.padding, a.scale), (10.0, 1.5));
        assert_eq!(a.filename.as_deref(), Some("/tmp/x.png"));
        let a =
            parse(json!({"region": {"x": 100, "y": 100.5, "width": 200, "height": 100}})).unwrap();
        assert_eq!(
            a.subject,
            Subject::Region(ViewRect::new(100.0, 100.5, 200.0, 100.0))
        );
        // frame only matters with a selector.
        assert!(parse(json!({"ref": "e1", "frame": "f2"})).is_ok());
    }

    #[test]
    fn errors_say_what_is_wrong() {
        let err = |v| parse(v).unwrap_err();
        let region = json!({"x": 5000, "y": 5000, "width": 10, "height": 10});
        assert_eq!(err(json!({"ref": "e1", "region": region})), ONE_SUBJECT);
        assert_eq!(err(json!({"ref": "e1", "selector": "p"})), ONE_SUBJECT);
        assert_eq!(err(json!({"scale": 9})), SCALE_RANGE);
        assert_eq!(err(json!({"scale": 0.5})), SCALE_RANGE);
        assert_eq!(err(json!({"scale": "2"})), SCALE_RANGE);
        assert_eq!(err(json!({"padding": -1})), NEGATIVE_PADDING);
        assert_eq!(err(json!({"filename": "rel/x.png"})), RELATIVE_FILENAME);
        assert_eq!(err(json!({"filename": "~bob/x.png"})), RELATIVE_FILENAME);
        assert_eq!(err(json!({"region": {"x": 1, "y": 1}})), BAD_REGION);
        assert_eq!(err(json!({"region": [1, 2, 3, 4]})), BAD_REGION);
        let flat = json!({"x": 1, "y": 1, "width": 0, "height": 4});
        assert_eq!(err(json!({"region": flat})), EMPTY_REGION);
        assert_eq!(err(json!({"selector": "p", "frame": "f2"})), NO_FRAMES);
    }

    #[test]
    fn subjects_in_words() {
        let r = ViewRect::new(100.0, 100.0, 200.0, 100.0);
        assert_eq!(
            Subject::Region(r).words(false),
            "the region 100,100 200×100"
        );
        let r = ViewRect::new(-2.5, 0.126, 1.0 / 3.0, 7.0);
        assert_eq!(
            Subject::Region(r).words(true),
            "the region -2.5,0.13 0.33×7"
        );
        assert_eq!(Subject::Page.words(false), "the viewport");
        assert_eq!(Subject::Page.words(true), "the whole page");
        assert_eq!(Subject::Ref("f2:e7".into()).words(false), "f2:e7");
        assert_eq!(Subject::Selector("#box-b".into()).words(true), "#box-b");
        assert_eq!(number(2.0), "2");
        assert_eq!(number(1.5), "1.5");
        assert_eq!(number(2.375), "2.38");
        assert_eq!(number(-0.001), "0");
    }

    /// grid.html in a 947 x 746 viewport, scrolled to y 1000.
    const G: Geometry = Geometry {
        viewport: (947.0, 746.0),
        document: (1000.0, 3200.0),
        scroll: (0.0, 1000.0),
    };

    fn with(subject: Subject, full_page: bool, padding: f64) -> ShotArgs {
        ShotArgs {
            jpeg: false,
            full_page,
            subject,
            padding,
            scale: 1.0,
            filename: None,
        }
    }

    fn plan_of(args: &ShotArgs, element: Option<ViewRect>) -> Result<Plan, String> {
        plan(args, &G, element, "#x")
    }

    #[test]
    fn whole_viewport_and_page() {
        let p = plan_of(&with(Subject::Page, false, 9.0), None).unwrap();
        assert_eq!(p.area(1.0), CaptureArea::Viewport);
        assert_eq!(p.rect, ViewRect::new(0.0, 0.0, 947.0, 746.0));
        // The whole document, wider than the viewport too.
        let p = plan_of(&with(Subject::Page, true, 0.0), None).unwrap();
        let page = ViewRect::new(0.0, 0.0, 1000.0, 3200.0);
        assert_eq!(p.area(1.0), CaptureArea::DocumentRect(page));
        assert_eq!(p.rect, page);
        // A short page is still the viewport's size; a very long one stops.
        let short = Geometry {
            document: (10.0, 10.0),
            ..G
        };
        assert_eq!(short.page(), (947.0, 746.0));
        let long = Geometry {
            document: (947.0, 90000.0),
            ..G
        };
        assert_eq!(long.page(), (947.0, TALLEST));
    }

    #[test]
    fn regions_pad_and_clamp() {
        let r = |x, y, w, h| Subject::Region(ViewRect::new(x, y, w, h));
        let p = plan_of(&with(r(100.0, 100.0, 200.0, 100.0), false, 0.0), None).unwrap();
        assert_eq!(
            p.area(1.0),
            CaptureArea::ViewportRect(ViewRect::new(100.0, 100.0, 200.0, 100.0))
        );
        // In view pixels at 150 %.
        assert_eq!(
            p.area(1.5),
            CaptureArea::ViewportRect(ViewRect::new(150.0, 150.0, 300.0, 150.0))
        );
        // Padding, then clamped to the viewport.
        let p = plan_of(&with(r(5.0, 700.0, 100.0, 40.0), false, 10.0), None).unwrap();
        assert_eq!(p.rect, ViewRect::new(0.0, 690.0, 115.0, 56.0));
        let e = plan_of(&with(r(5000.0, 5000.0, 10.0, 10.0), false, 0.0), None);
        assert_eq!(e.unwrap_err(), "the region is outside the viewport");
        // With fullPage, in the page's coordinates.
        let p = plan_of(&with(r(0.0, 2000.0, 300.0, 300.0), true, 0.0), None).unwrap();
        assert_eq!(
            p.area(1.0),
            CaptureArea::DocumentRect(ViewRect::new(0.0, 2000.0, 300.0, 300.0))
        );
        let e = plan_of(&with(r(0.0, 5000.0, 10.0, 10.0), true, 0.0), None);
        assert_eq!(e.unwrap_err(), "the region is outside the page");
    }

    #[test]
    fn elements_come_out_whole() {
        let el = |padding| with(Subject::Selector("#x".into()), false, padding);
        // #box-b scrolled into view (y 1000): a picture of the viewport.
        let b = ViewRect::new(400.0, 500.0, 240.0, 120.0);
        let p = plan_of(&el(10.0), Some(b)).unwrap();
        assert_eq!(
            p.area(1.0),
            CaptureArea::ViewportRect(ViewRect::new(390.0, 490.0, 260.0, 140.0))
        );
        // fullPage changes nothing for an element.
        let mut full = el(10.0);
        full.full_page = true;
        assert_eq!(plan_of(&full, Some(b)).unwrap(), p);
        // #tall, taller than the viewport: the page's picture, all of it.
        let tall = ViewRect::new(700.0, -600.0, 120.0, 1800.0);
        let p = plan_of(&el(0.0), Some(tall)).unwrap();
        assert_eq!(
            p.area(1.0),
            CaptureArea::DocumentRect(ViewRect::new(700.0, 400.0, 120.0, 1800.0))
        );
        // Padding off the viewport's edge: still whole, stopped by the page.
        let top = ViewRect::new(0.0, -1000.0, 50.0, 50.0);
        let p = plan_of(&el(20.0), Some(top)).unwrap();
        assert_eq!(p.rect, ViewRect::new(0.0, 0.0, 70.0, 70.0));
        assert!(p.in_document);
        let none = ViewRect::new(10.0, 10.0, 0.0, 0.0);
        assert_eq!(
            plan_of(&el(0.0), Some(none)).unwrap_err(),
            "#x has no size: there is nothing to picture"
        );
        let gone = ViewRect::new(5000.0, 10.0, 10.0, 10.0);
        assert_eq!(
            plan_of(&el(0.0), Some(gone)).unwrap_err(),
            "#x is outside the page"
        );
    }

    #[test]
    fn intersections() {
        let a = ViewRect::new(0.0, 0.0, 100.0, 50.0);
        let b = ViewRect::new(90.0, 10.0, 30.0, 100.0);
        assert_eq!(intersect(a, b), Some(ViewRect::new(90.0, 10.0, 10.0, 40.0)));
        assert_eq!(intersect(a, ViewRect::new(100.0, 0.0, 5.0, 5.0)), None);
        assert_eq!(
            area(b, 5.0, (110.0, 60.0)),
            Some(ViewRect::new(85.0, 5.0, 25.0, 55.0))
        );
    }

    #[test]
    fn the_cap_lowers_the_scale() {
        let f = fit(1280.0, 800.0, 2.0);
        assert_eq!((f.scale, f.size, f.lowered), (2.0, (2560, 1600), None));
        let f = fit(420.0, 220.0, 2.0);
        assert_eq!(f.size, (840, 440));
        let f = fit(1.0 / 3.0, 7.0, 1.0);
        assert_eq!(f.size, (1, 7));
        // 6000 px tall at 4 would be 24000: down to 2.73.
        let f = fit(300.0, 6000.0, 4.0);
        assert_eq!(f.lowered, Some(Limit::Side));
        assert_eq!(f.scale, 2.73);
        assert!(f.size.1 <= 16384);
        // 1280 x 3200 at 4 is 65.5 MP: under 64 MP.
        let f = fit(1280.0, 3200.0, 4.0);
        assert_eq!(f.lowered, Some(Limit::Pixels));
        assert_eq!(f.scale, 3.95);
        assert!(u64::from(f.size.0) * u64::from(f.size.1) <= MAX_PIXELS);
        // Even scale 1 can be too big: a very long page goes below it.
        let f = fit(1280.0, 40000.0, 1.0);
        assert_eq!((f.scale, f.lowered), (0.4, Some(Limit::Side)));
    }

    #[test]
    fn the_answer_image() {
        // Saved in HD: scale 1 in the answer.
        assert_eq!(view_fit(420.0, 220.0, 2.0, true), (1.0, (420, 220)));
        // Not saved: the picture itself, if it fits.
        assert_eq!(view_fit(420.0, 220.0, 2.0, false), (2.0, (840, 440)));
        assert_eq!(view_fit(420.0, 220.0, 1.0, true), (1.0, (420, 220)));
        // Never more than 8000 px on a side: 1280 x 20000 at 1 is 0.4.
        assert_eq!(view_fit(1280.0, 20000.0, 1.0, false), (0.4, (512, 8000)));
        assert_eq!(view_fit(1640.0, 20000.0, 2.0, true), (0.4, (656, 8000)));
        let (s, (w, h)) = view_fit(3001.0, 7001.0, 3.0, false);
        assert!(w.max(h) <= 8000 && s == 1.14, "{s} {w}x{h}");
        // A side that rounds down to 8000 is in: 16000.8 at 0.5 is 8000.4.
        assert_eq!(view_fit(100.0, 16000.8, 1.0, false), (0.5, (50, 8000)));
        // The Mac's example: 1640 x 20000 lowered to 0.81, shown at 0.4.
        assert_eq!(view_fit(1640.0, 20000.0, 0.81, false), (0.4, (656, 8000)));
    }

    #[test]
    fn answer_sentences() {
        let mut a = Answer {
            size: (2880, 1800),
            format: "PNG",
            scale: 2.0,
            subject: "the viewport".into(),
            saved: Some(Saved::To(PathBuf::from("/Users/x/Desktop/shot.png"))),
            lowered: None,
            upscaled: None,
            view: None,
        };
        assert_eq!(
            a.text("### Page"),
            "Screenshot 2880×1800 PNG at scale 2 of the viewport. Saved to /Users/x/Desktop/shot.png. ### Page"
        );
        a.saved = Some(Saved::Failed(PathBuf::from("/no/x.png"), "denied".into()));
        a.scale = 2.37;
        a.lowered = Some((4.0, Limit::Side));
        a.upscaled = Some(1.0);
        a.subject = "#box-b".into();
        assert_eq!(
            a.text("P"),
            "Screenshot 2880×1800 PNG at scale 2.37 of #box-b. Couldn't save to /no/x.png: denied. \
             Scale lowered from 4 to 2.37 to stay under 16384 px. \
             Upscaled: this page renders at most at scale 1 here. P"
        );
        a.saved = None;
        a.lowered = Some((4.0, Limit::Pixels));
        a.upscaled = None;
        assert!(a.text("P").contains(" to stay under 64 megapixels. P"));
        // The answer's own image: after the save, before the notes.
        a.size = (840, 440);
        a.scale = 2.0;
        a.lowered = None;
        a.subject = "the region 100,100 420×220".into();
        a.saved = Some(Saved::To(PathBuf::from("/tmp/c3.png")));
        a.view = Some(((420, 220), 1.0));
        assert_eq!(
            a.text("P"),
            "Screenshot 840×440 PNG at scale 2 of the region 100,100 420×220. Saved to /tmp/c3.png. \
             The image here is 420×220 at scale 1; the file has the full picture. P"
        );
        a.saved = None;
        a.size = (1640, 20000);
        a.view = Some(((656, 8000), 0.4));
        a.upscaled = Some(1.0);
        assert!(a.text("P").contains(
            " of the region 100,100 420×220. The image here is 656×8000 at scale 0.4; \
             pass filename to keep the full picture. Upscaled: "
        ));
    }

    #[test]
    fn filenames() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        let stamp = "20261117-131500";
        // An existing folder: a new name in it, then -2 once that is taken.
        let first = destination(d, true, stamp);
        assert_eq!(first, dir.path().join("copper-20261117-131500.jpg"));
        std::fs::write(&first, b"x").unwrap();
        assert_eq!(
            destination(d, true, stamp),
            dir.path().join("copper-20261117-131500-2.jpg")
        );
        // A folder not made yet, named by its trailing separator.
        let fresh = absolute(&format!("{d}/new/")).unwrap();
        assert!(fresh.ends_with(std::path::MAIN_SEPARATOR));
        assert_eq!(
            destination(&fresh, false, stamp),
            dir.path().join("new").join("copper-20261117-131500.png")
        );
        // A file in folders that don't exist yet.
        let file = format!("{d}/a/b/shot.png");
        match save(b"png", &file, false) {
            Saved::To(path) => {
                assert_eq!(path, dir.path().join("a/b/shot.png"));
                assert_eq!(std::fs::read(&path).unwrap(), b"png");
            }
            other => panic!("{other:?}"),
        }
        // Under a file: the failure, with the path as asked.
        let under = format!("{}/x.png", first.display());
        match save(b"png", &under, false) {
            Saved::Failed(path, why) => {
                assert_eq!(path, PathBuf::from(&under));
                assert!(!why.is_empty());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(absolute("rel/x.png").unwrap_err(), RELATIVE_FILENAME);
        if let Some(home) = copper_core::platform::home_dir() {
            assert_eq!(expand("~/x.png"), home.join("x.png"));
            assert_eq!(expand("~"), home);
            assert_eq!(
                absolute("~/x.png").unwrap(),
                home.join("x.png").to_string_lossy()
            );
        }
    }

    #[test]
    fn stamps() {
        assert_eq!(utc_parts(0), (1970, 1, 1, 0, 0, 0));
        // 2026-11-17 18:15:09 UTC.
        assert_eq!(utc_parts(1_794_939_309), (2026, 11, 17, 18, 15, 9));
        assert_eq!(utc_parts(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(format_stamp((2026, 11, 7, 3, 4, 5)), "20261107-030405");
        let now = stamp();
        assert_eq!(now.len(), 15);
        assert_eq!(&now[8..9], "-");
    }
}
