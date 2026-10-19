//! Live frames' posters (canvas-perf contract A.3): the page asks for a
//! picture of a live web frame while it is on screen, and shows that picture
//! instead of the frame when it unloads the frame (off screen, too small, or
//! over the live budget).
//!
//! * page → host: `{type: 'framePoster', name, id, url, rect: {x, y, w, h},
//!   maxWidth}` — `rect` in CSS px of the board page's viewport (the iframe's
//!   content box);
//! * host → page: `copperCanvas.framePoster(name, dataUrl | null)` — a
//!   `data:image/jpeg;base64,…` no wider than `maxWidth`, aspect kept; `null`
//!   when there is no picture to be had.
//!
//! Requests that arrive together share one snapshot of the board; the
//! texture is read on the main thread (a copy), and cropping, scaling and
//! JPEG encoding happen on a worker thread. A frame asking again within
//! [`MIN_GAP`] is answered `null` (the page itself asks at most every 30 s).

use copper_mcp::shot::{self, Rgba};
use serde_json::{Map, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The shortest time between two pictures of one frame.
pub const MIN_GAP: Duration = Duration::from_secs(5);
/// Requests waiting at most (the oldest are answered `null`).
pub const MAX_QUEUE: usize = 24;
/// JPEG quality (the contract's ~0.7).
pub const QUALITY: u8 = 70;
/// `maxWidth` when the page gives none, and the bounds it is held to.
pub const DEFAULT_WIDTH: u32 = 640;
const WIDTH_BOUNDS: (u32, u32) = (32, 1600);

/// `(x, y, w, h)`.
pub type Rect = (f64, f64, f64, f64);

/// One poster request, as the page sent it.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub name: String,
    pub id: String,
    pub url: String,
    /// CSS px in the page's viewport.
    pub rect: Rect,
    pub max_width: u32,
}

/// The page's message as a request; `Err(Some(name))` when it is unusable
/// but names a frame (that frame is answered `null`), `Err(None)` when it
/// names none.
pub fn parse(body: &Map<String, Value>) -> Result<Request, Option<String>> {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .ok_or(None)?;
    let rect = body.get("rect").and_then(Value::as_object);
    let num = |k: &str| rect.and_then(|r| r.get(k)).and_then(Value::as_f64);
    let (Some(x), Some(y), Some(w), Some(h)) = (num("x"), num("y"), num("w"), num("h")) else {
        return Err(Some(name));
    };
    if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) || w < 4.0 || h < 4.0 {
        return Err(Some(name));
    }
    let max_width = body
        .get("maxWidth")
        .and_then(Value::as_f64)
        .filter(|w| w.is_finite() && *w > 0.0)
        .map(|w| (w as u32).clamp(WIDTH_BOUNDS.0, WIDTH_BOUNDS.1))
        .unwrap_or(DEFAULT_WIDTH);
    let text = |k: &str| {
        body.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    Ok(Request {
        id: text("id"),
        url: text("url"),
        name,
        rect: (x, y, w, h),
        max_width,
    })
}

/// A CSS-px rect in device pixels of a `tex_w` × `tex_h` snapshot, clamped
/// to it; `None` when less than half of it is in the picture (the frame is
/// mostly off screen: a poster of it would be wrong).
pub fn device_rect(rect: Rect, scale: f64, tex_w: u32, tex_h: u32) -> Option<Rect> {
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let (x, y, w, h) = (
        rect.0 * scale,
        rect.1 * scale,
        rect.2 * scale,
        rect.3 * scale,
    );
    let x0 = x.max(0.0);
    let y0 = y.max(0.0);
    let x1 = (x + w).min(f64::from(tex_w));
    let y1 = (y + h).min(f64::from(tex_h));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    if (x1 - x0) * (y1 - y0) < 0.5 * w * h {
        return None;
    }
    Some((x0, y0, x1 - x0, y1 - y0))
}

/// The poster for one request out of the whole board picture: cropped,
/// scaled to at most `max_width` (never up), JPEG, as a data URL.
pub fn render(board: &Rgba, rect: Rect, max_width: u32) -> Option<String> {
    let crop = shot::crop(board, rect.0, rect.1, rect.2, rect.3)?;
    let img = if crop.width > max_width {
        let h = (f64::from(crop.height) * f64::from(max_width) / f64::from(crop.width))
            .round()
            .max(1.0) as u32;
        shot::scale(&crop, max_width, h)
    } else {
        crop
    };
    let jpeg = shot::encode_jpeg_quality(&img, QUALITY).ok()?;
    Some(format!("data:image/jpeg;base64,{}", super::b64(&jpeg)))
}

/// RGBA8 from a downloaded texture (rows `stride` bytes apart).
pub fn rgba(width: u32, height: u32, stride: usize, bytes: &[u8]) -> Option<Rgba> {
    let row = width as usize * 4;
    if stride < row || bytes.len() < stride * (height as usize).saturating_sub(1) + row {
        return None;
    }
    let mut pixels = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        pixels.extend_from_slice(&bytes[y * stride..y * stride + row]);
    }
    Some(Rgba {
        width,
        height,
        pixels,
    })
}

/// One host's poster work: what waits, whether a batch is running, and when
/// each frame last got a picture.
#[derive(Default)]
pub struct Posters {
    pub queue: RefCell<Vec<Request>>,
    pub busy: Cell<bool>,
    pub last: RefCell<HashMap<String, Instant>>,
    /// (pictures, nulls) answered so far, for `describe`.
    pub counts: Cell<(u64, u64)>,
}

/// What [`Posters::offer`] decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Offer {
    /// Queued; start a batch if none is running.
    Queued,
    /// Answer these frames `null` now (asked too soon, or pushed out).
    Refuse(Vec<String>),
}

impl Posters {
    /// Take a request: queued (one per frame — a newer one replaces it), or
    /// refused when that frame had a picture under [`MIN_GAP`] ago.
    pub fn offer(&self, r: Request, now: Instant) -> Offer {
        if self
            .last
            .borrow()
            .get(&r.name)
            .is_some_and(|at| now.duration_since(*at) < MIN_GAP)
        {
            return Offer::Refuse(vec![r.name]);
        }
        let mut q = self.queue.borrow_mut();
        q.retain(|o| o.name != r.name);
        q.push(r);
        let mut out = Vec::new();
        while q.len() > MAX_QUEUE {
            out.push(q.remove(0).name);
        }
        if out.is_empty() {
            Offer::Queued
        } else {
            Offer::Refuse(out)
        }
    }

    /// Everything waiting, for one batch.
    pub fn take(&self) -> Vec<Request> {
        std::mem::take(&mut *self.queue.borrow_mut())
    }

    pub fn answered(&self, name: &str, picture: bool, now: Instant) {
        let (ok, none) = self.counts.get();
        if picture {
            self.last.borrow_mut().insert(name.to_string(), now);
            self.counts.set((ok + 1, none));
        } else {
            self.counts.set((ok, none + 1));
        }
        let mut last = self.last.borrow_mut();
        if last.len() > 256 {
            last.retain(|_, at| now.duration_since(*at) < MIN_GAP);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn parses_the_message() {
        let r = parse(&body(
            json!({"type": "framePoster", "name": "f1", "id": "s1", "url": "https://a.b/",
            "rect": {"x": 10, "y": 20.5, "w": 300, "h": 200}, "maxWidth": 480}),
        ))
        .unwrap();
        assert_eq!(r.name, "f1");
        assert_eq!(r.id, "s1");
        assert_eq!(r.rect, (10.0, 20.5, 300.0, 200.0));
        assert_eq!(r.max_width, 480);
        let d = parse(&body(
            json!({"name": "f2", "rect": {"x": 0, "y": 0, "w": 50, "h": 50}}),
        ))
        .unwrap();
        assert_eq!(d.max_width, DEFAULT_WIDTH);
        assert_eq!(
            parse(&body(
                json!({"name": "f3", "rect": {"x": 0, "y": 0, "w": 1, "h": 50}})
            )),
            Err(Some("f3".into()))
        );
        assert_eq!(parse(&body(json!({"name": "f4"}))), Err(Some("f4".into())));
        assert_eq!(parse(&body(json!({"rect": {}}))), Err(None));
    }

    #[test]
    fn rect_to_device_pixels() {
        assert_eq!(
            device_rect((10.0, 10.0, 100.0, 50.0), 2.0, 1000, 1000),
            Some((20.0, 20.0, 200.0, 100.0))
        );
        // Clamped when a little is off the edge, refused when most is.
        assert_eq!(
            device_rect((-10.0, 0.0, 100.0, 50.0), 1.0, 1000, 1000),
            Some((0.0, 0.0, 90.0, 50.0))
        );
        assert_eq!(
            device_rect((-80.0, 0.0, 100.0, 50.0), 1.0, 1000, 1000),
            None
        );
        assert_eq!(
            device_rect((2000.0, 0.0, 100.0, 50.0), 1.0, 1000, 1000),
            None
        );
        assert_eq!(device_rect((0.0, 0.0, 10.0, 10.0), 0.0, 1000, 1000), None);
    }

    #[test]
    fn renders_a_scaled_jpeg() {
        let board = Rgba {
            width: 400,
            height: 300,
            pixels: [200u8, 100, 50, 255].repeat(400 * 300),
        };
        let url = render(&board, (0.0, 0.0, 400.0, 300.0), 100).unwrap();
        let b64 = url.strip_prefix("data:image/jpeg;base64,").unwrap();
        let jpeg = super::super::unb64(b64);
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        // SOF0 carries height then width.
        let sof = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        let h = u16::from_be_bytes([jpeg[sof + 5], jpeg[sof + 6]]);
        let w = u16::from_be_bytes([jpeg[sof + 7], jpeg[sof + 8]]);
        assert_eq!((w, h), (100, 75));
        // Never scaled up.
        let small = render(&board, (0.0, 0.0, 40.0, 30.0), 640).unwrap();
        let jpeg = super::super::unb64(small.strip_prefix("data:image/jpeg;base64,").unwrap());
        let sof = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        assert_eq!(u16::from_be_bytes([jpeg[sof + 7], jpeg[sof + 8]]), 40);
        assert!(render(&board, (500.0, 0.0, 10.0, 10.0), 100).is_none());
    }

    #[test]
    fn rgba_honours_the_stride() {
        let bytes = [1u8, 2, 3, 4, 9, 9, 5, 6, 7, 8, 9, 9];
        let img = rgba(1, 2, 6, &bytes).unwrap();
        assert_eq!(img.pixels, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(rgba(2, 2, 6, &bytes).is_none());
    }

    #[test]
    fn throttles_and_bounds_the_queue() {
        let p = Posters::default();
        let now = Instant::now();
        let r = |n: &str| Request {
            name: n.into(),
            id: String::new(),
            url: String::new(),
            rect: (0.0, 0.0, 10.0, 10.0),
            max_width: 640,
        };
        assert_eq!(p.offer(r("a"), now), Offer::Queued);
        assert_eq!(p.offer(r("a"), now), Offer::Queued); // replaces
        assert_eq!(p.take().len(), 1);
        p.answered("a", true, now);
        assert_eq!(p.offer(r("a"), now), Offer::Refuse(vec!["a".into()]));
        assert_eq!(p.offer(r("a"), now + MIN_GAP), Offer::Queued);
        p.take();
        for i in 0..MAX_QUEUE {
            assert_eq!(p.offer(r(&format!("f{i}")), now), Offer::Queued);
        }
        assert_eq!(p.offer(r("last"), now), Offer::Refuse(vec!["f0".into()]));
        assert_eq!(p.counts.get(), (1, 0));
    }
}
