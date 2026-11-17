//! Agent screenshots (`PageView::capture`): the viewport or the whole
//! document, at the display's scale or sharper.
//!
//! WebKitGTK paints a snapshot at the view's device scale and has no scale
//! option, so a sharper picture needs the page itself to render bigger. For
//! a page hanging in the backstage's ghost host (an agent's tab nobody is
//! looking at) the [`Lens`] gives its view a `k` times larger allocation and
//! `k` times the page zoom for the moment of the snapshot: the CSS viewport,
//! and so the layout, stay what they were while every CSS pixel becomes `k`
//! device pixels. Then both go back. Nothing is drawn meanwhile (the host
//! never draws its children). A page on screen is never resized or zoomed:
//! it is painted at the display's scale, and the caller enlarges it and
//! says so.
//!
//! The page does notice: a few `resize` events (the CSS viewport is the
//! same size again by the time it is pictured), and its scroll position is
//! put back explicitly (a taller viewport can clamp it).

use std::time::{Duration, Instant};

use copper_engine::{
    Capture, CaptureArea, CaptureRequest, EngineError, LocalFuture, PageView, Result, World,
};
use glib::prelude::*;
use gtk4::prelude::*;
use serde_json::Value;
use webkit6::prelude::*;
use webkit6::{SnapshotOptions, SnapshotRegion};

use crate::convert;
use crate::ghost::GhostHost;
use crate::page::WebKitPage;

/// The isolated world the lens reads and scrolls the page from, out of the
/// page's own scripts' reach.
const WORLD: &str = "CopperCapture";
/// How long each step of the lens may take to reach the web process.
const STEP: Duration = Duration::from_millis(1500);
/// Pause between two reads while waiting.
const POLL: Duration = Duration::from_millis(8);
/// The biggest picture the lens lets WebKit paint: per side, and in all
/// (device pixels; the snapshot is one bitmap in the web process).
const MAX_SIDE: f64 = 16384.0;
const MAX_PIXELS: f64 = 64e6;
/// Below this the lens is not worth a relayout.
const MIN_FACTOR: f64 = 1.01;

/// What the page says about its viewport (CSS pixels).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Probe {
    width: f64,
    height: f64,
    scroll_x: f64,
    scroll_y: f64,
    /// The document's scrollable size.
    doc_width: f64,
    doc_height: f64,
}

const PROBE_JS: &str = "(() => { const d = document.documentElement, b = document.body; \
     return { w: innerWidth, h: innerHeight, x: scrollX, y: scrollY, \
     dw: Math.max(d.scrollWidth, b ? b.scrollWidth : 0), dh: Math.max(d.scrollHeight, b ? b.scrollHeight : 0) }; })()";

impl Probe {
    fn from_json(v: &Value) -> Option<Probe> {
        let n = |k: &str| v.get(k).and_then(Value::as_f64);
        Some(Probe {
            width: n("w")?,
            height: n("h")?,
            scroll_x: n("x")?,
            scroll_y: n("y")?,
            doc_width: n("dw").unwrap_or(0.0),
            doc_height: n("dh").unwrap_or(0.0),
        })
    }
}

async fn probe(page: &WebKitPage) -> Option<Probe> {
    let v = page.eval(PROBE_JS, &World::isolated(WORLD)).await.ok()?;
    Probe::from_json(&v)
}

/// Scroll back to (`x`, `y`) CSS pixels at once, whatever the page's
/// `scroll-behavior` says.
async fn scroll_to(page: &WebKitPage, x: f64, y: f64) {
    let js = format!("scrollTo({{ left: {x}, top: {y}, behavior: 'instant' }}); true");
    let _ = page.eval(&js, &World::isolated(WORLD)).await;
}

/// Read the page until `done` holds, for at most [`STEP`].
async fn wait_for(page: &WebKitPage, done: impl Fn(&Probe) -> bool) -> bool {
    let deadline = Instant::now() + STEP;
    loop {
        if probe(page).await.is_some_and(|p| done(&p)) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        glib::timeout_future(POLL).await;
    }
}

/// The view's device scale: the surface's (fractional) scale where GDK has
/// one, else the widget's integer scale factor.
fn device_scale(view: &webkit6::WebView) -> f64 {
    view.native()
        .and_then(|n| n.surface())
        .map(|s| s.scale())
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or_else(|| f64::from(view.scale_factor().max(1)))
}

/// How much bigger the page can render: `want`, within [`MAX_SIDE`] and
/// [`MAX_PIXELS`] for a picture of `width` x `height` device pixels at 1x.
fn bounded_factor(want: f64, width: f64, height: f64) -> f64 {
    if !(width > 0.0 && height > 0.0) {
        return 1.0;
    }
    want.min(MAX_SIDE / width)
        .min(MAX_SIDE / height)
        .min((MAX_PIXELS / (width * height)).sqrt())
}

/// A page rendering bigger than its stage for one snapshot.
struct Lens {
    page: WebKitPage,
    ghost: GhostHost,
    /// The page zoom it had.
    zoom: f64,
    /// The CSS viewport and scroll position it had.
    before: Probe,
    /// The page zoom and view size are this many times what they were.
    factor: f64,
}

impl Lens {
    /// Magnify `page` by about `want` (at most), or say why not.
    async fn open(
        page: &WebKitPage,
        want: f64,
        dsf: f64,
        document: bool,
        before: Probe,
    ) -> std::result::Result<Lens, String> {
        let view = page.view();
        let ghost = view
            .parent()
            .and_downcast::<GhostHost>()
            .ok_or("the page is on screen")?;
        if !view.is_mapped() {
            return Err("the page is not mapped".into());
        }
        let (w, h) = (view.width(), view.height());
        if w <= 0 || h <= 0 {
            return Err("the page has no size".into());
        }
        let zoom = view.zoom_level();
        let (pw, ph) = if document {
            (
                before.doc_width.max(before.width) * zoom * dsf,
                before.doc_height.max(before.height) * zoom * dsf,
            )
        } else {
            (f64::from(w) * dsf, f64::from(h) * dsf)
        };
        let k = bounded_factor(want, pw, ph);
        let wk = (f64::from(w) * k).round() as i32;
        let hk = (f64::from(h) * k).round() as i32;
        let factor = f64::from(wk) / f64::from(w);
        // Each step must be visible to the page as a different viewport width.
        if factor < MIN_FACTOR || (f64::from(wk - w) / zoom) < 3.0 {
            return Err(format!("this page can render at most {k:.2} times bigger"));
        }
        let lens = Lens {
            page: page.clone(),
            ghost,
            zoom,
            before,
            factor,
        };
        // A wider viewport first (never a narrower one, which could switch
        // the page to its small-screen layout), then the zoom that brings
        // its CSS size back.
        let wide = before.width * factor;
        lens.ghost.set_child_size(view, Some((wk, hk)));
        if !wait_for(page, |p| (p.width - wide).abs() <= 1.0).await {
            lens.close().await;
            return Err("the page did not take the bigger size".into());
        }
        view.set_zoom_level(zoom * factor);
        if !wait_for(page, |p| (p.width - before.width).abs() <= 1.0).await {
            lens.close().await;
            return Err("the page did not take the zoom".into());
        }
        scroll_to(page, before.scroll_x, before.scroll_y).await;
        Ok(lens)
    }

    /// Put the page back as it was: zoom first (a wider viewport again for
    /// a moment), then the size, then the scroll position.
    async fn close(self) {
        let view = self.page.view();
        view.set_zoom_level(self.zoom);
        let wide = self.before.width * self.factor;
        let _ = wait_for(&self.page, |p| (p.width - wide).abs() <= 1.0).await;
        self.ghost.set_child_size(view, None);
        let width = self.before.width;
        if !wait_for(&self.page, |p| (p.width - width).abs() <= 1.0).await {
            tracing::debug!("capture: the page took long to get its size back");
        }
        scroll_to(&self.page, self.before.scroll_x, self.before.scroll_y).await;
    }
}

/// `PageView::capture` for WebKitGTK; see the module docs.
pub(crate) fn capture(page: &WebKitPage, request: CaptureRequest) -> LocalFuture<Result<Capture>> {
    let page = page.clone();
    Box::pin(async move {
        let view = page.view().clone();
        let document = request.area.in_document();
        let width = view.width();
        let zoom = view.zoom_level();
        let before = probe(&page).await;
        let dsf = device_scale(&view);
        let mut lens = None;
        if let Some(before) = before
            && request.scale > dsf * MIN_FACTOR
        {
            match Lens::open(&page, request.scale / dsf, dsf, document, before).await {
                Ok(l) => lens = Some(l),
                Err(why) => tracing::debug!("capture at the display's scale: {why}"),
            }
        }
        let region = if document {
            SnapshotRegion::FullDocument
        } else {
            SnapshotRegion::Visible
        };
        let shot = view.snapshot_future(region, SnapshotOptions::NONE).await;
        let factor = lens.as_ref().map_or(1.0, |l| l.factor);
        if let Some(lens) = lens {
            lens.close().await;
        }
        let texture = shot.map_err(|e| EngineError::Snapshot(e.message().to_owned()))?;
        // Each picture says its own scale: over the view's width, or the
        // document's (in view pixels); else the display's times the lens's.
        let across = if document {
            before.map_or(0.0, |p| p.doc_width.max(p.width) * zoom)
        } else {
            f64::from(width)
        };
        let scale = if across > 0.0 {
            f64::from(texture.width()) / across
        } else {
            dsf * factor
        };
        let shot = Capture {
            bitmap: convert::texture_to_bitmap(&texture),
            origin: (0.0, 0.0),
            scale,
        };
        Ok(match request.area {
            CaptureArea::ViewportRect(r) | CaptureArea::DocumentRect(r) => shot.cropped(r),
            CaptureArea::Viewport | CaptureArea::Document => shot,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lens_stays_within_bounds() {
        assert_eq!(bounded_factor(3.0, 1280.0, 800.0), 3.0);
        // 4x of a 1280 x 3200 document is 65.5 MP: a little less.
        let k = bounded_factor(4.0, 1280.0, 3200.0);
        assert!(k < 4.0 && (1280.0 * k) * (3200.0 * k) <= MAX_PIXELS + 1.0);
        // A page 20000 px tall cannot render bigger at all.
        assert!(bounded_factor(2.0, 1280.0, 20000.0) < 1.0);
        assert_eq!(bounded_factor(2.0, 0.0, 10.0), 1.0);
    }

    #[test]
    fn probe_reads_the_page() {
        let v = serde_json::json!({"w": 1280, "h": 800, "x": 0, "y": 40.5, "dw": 1280, "dh": 3200});
        let p = Probe::from_json(&v).unwrap();
        assert_eq!((p.width, p.scroll_y, p.doc_height), (1280.0, 40.5, 3200.0));
        assert!(Probe::from_json(&serde_json::json!({"w": 1})).is_none());
    }
}
