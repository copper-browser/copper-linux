//! The wake cover (Mac `Stage.swift` 22-42, 69-94; `Tab.swift` "wake-cover").
//!
//! A tab put to sleep gives its page back, and a page built again paints
//! nothing for a moment. So just before it goes, the page is pictured, and
//! when the tab wakes the picture stands in for it — at the size it was
//! taken, in the stage's top-left corner, on the page's own ground colour
//! (read off the picture's right and bottom edges) — until the page has
//! something better to show. Then it fades, over [`FADE`].
//!
//! The picture comes off a beat after the new document commits
//! ([`AFTER_COMMIT`], the fallback for "first paint", which no engine here
//! reports), at once when the load finishes or fails, and never stays more
//! than [`AT_MOST`]. The platform draws it: [`crate::Shell::on_cover`] is
//! told when a tab's cover goes up (`Some`) and comes down (`None`).

use crate::main_loop::MainLoop;
use crate::{PageOf, Platform, Shell};
use copper_core::TabId;
use copper_engine::{Bitmap, PageView};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// The cover's fade as it comes down (`.easeOut(duration: 0.2)`).
pub const FADE: Duration = Duration::from_millis(200);
/// After the new document commits: long enough that a page which paints
/// is never undercut (Mac `didCommit` → `uncover(after: 1.2)`).
pub const AFTER_COMMIT: Duration = Duration::from_millis(1200);
/// Whatever happens to the page, the picture doesn't outstay it.
pub const AT_MOST: Duration = Duration::from_secs(4);
/// How long picturing a page may take before it sleeps without one.
const TAKE_LIMIT: Duration = Duration::from_secs(3);
/// JPEG quality (the Mac's 0.55).
const QUALITY: u8 = 55;

/// A page's last picture.
#[derive(Clone, PartialEq, Eq)]
pub struct Cover {
    /// The picture, as a JPEG.
    pub jpeg: Vec<u8>,
    /// Its size in device pixels.
    pub pixels: (u32, u32),
    /// The size it was taken at, in view points (the page's own size then):
    /// drawn at exactly this size, never scaled to fill. `(0, 0)` when the
    /// engine could not say: the pixels over the window's scale, then.
    pub size: (u32, u32),
    /// The page's background, as best the picture can tell (RGB; [`ground`]
    /// of the decoded picture when `None`).
    pub ground: Option<[u8; 3]>,
}

impl std::fmt::Debug for Cover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cover")
            .field("bytes", &self.jpeg.len())
            .field("pixels", &self.pixels)
            .field("size", &self.size)
            .field("ground", &self.ground)
            .finish()
    }
}

/// The page's background: the commonest colour along the picture's right
/// and bottom edges, where a page runs out of content and shows its ground
/// (Mac `Tab.ground(of:)`): 32 runs down the right edge and 32 across the
/// bottom, each averaged, bucketed to sixteen levels a channel; the fullest
/// bucket's mean wins.
pub fn ground_of(b: &Bitmap) -> Option<[u8; 3]> {
    ground(b.width, b.height, b.stride, &b.rgba, [0, 1, 2])
}

/// [`ground_of`] over raw 4-byte pixels, rows `stride` bytes apart, with
/// red, green and blue at byte offsets `rgb` (`[2, 1, 0]` for BGRA).
pub fn ground(
    width: u32,
    height: u32,
    stride: usize,
    pixels: &[u8],
    rgb: [usize; 3],
) -> Option<[u8; 3]> {
    let (w, h) = (width as usize, height as usize);
    if w <= 2 || h <= 2 {
        return None;
    }
    const RUNS: usize = 32;
    let px = |x: usize, y: usize| -> Option<[u32; 3]> {
        let p = pixels.get(y * stride + x * 4..y * stride + x * 4 + 4)?;
        Some(rgb.map(|i| u32::from(p[i])))
    };
    let mut samples: Vec<[u32; 3]> = Vec::with_capacity(RUNS * 2);
    let mut run = |points: &mut dyn Iterator<Item = (usize, usize)>| {
        let mut sum = [0u32; 3];
        let mut n = 0u32;
        for c in points.filter_map(|(x, y)| px(x, y)) {
            for (s, v) in sum.iter_mut().zip(c) {
                *s += v;
            }
            n += 1;
        }
        let mean = sum.map(|s| s.checked_div(n));
        if let [Some(r), Some(g), Some(b)] = mean {
            samples.push([r, g, b]);
        }
    };
    for i in 0..RUNS {
        let (y0, y1) = (i * h / RUNS, ((i + 1) * h / RUNS).max(i * h / RUNS + 1));
        run(&mut (y0..y1.min(h)).map(|y| (w - 1, y)));
    }
    for i in 0..RUNS {
        let (x0, x1) = (i * w / RUNS, ((i + 1) * w / RUNS).max(i * w / RUNS + 1));
        run(&mut (x0..x1.min(w)).map(|x| (x, h - 1)));
    }
    let mut buckets: HashMap<u32, Vec<[u32; 3]>> = HashMap::new();
    for s in &samples {
        let key = (s[0] >> 4) << 8 | (s[1] >> 4) << 4 | (s[2] >> 4);
        buckets.entry(key).or_default().push(*s);
    }
    // Ties go to the lighter bucket, so the answer never depends on the
    // map's order.
    let best = buckets
        .into_iter()
        .max_by_key(|(key, v)| (v.len(), *key))?
        .1;
    let n = best.len() as u32;
    let mean = |i: usize| (best.iter().map(|s| s[i]).sum::<u32>() / n) as u8;
    Some([mean(0), mean(1), mean(2)])
}

/// Make a [`Cover`] of `bitmap` (taken at `size` view points): the ground,
/// and the JPEG. Pure; slow for a big picture, so run off the UI thread.
pub fn make(bitmap: &Bitmap, size: (u32, u32)) -> Option<Cover> {
    let ground = ground_of(bitmap);
    let row = bitmap.width as usize * 4;
    let mut pixels = Vec::with_capacity(row * bitmap.height as usize);
    for y in 0..bitmap.height {
        pixels.extend_from_slice(bitmap.row(y)?);
    }
    let rgba = copper_mcp::shot::Rgba {
        width: bitmap.width,
        height: bitmap.height,
        pixels,
    };
    let jpeg = copper_mcp::shot::encode_jpeg_quality(&rgba, QUALITY).ok()?;
    Some(Cover {
        jpeg,
        pixels: (bitmap.width, bitmap.height),
        size,
        ground,
    })
}

/// Picture `page` for its cover: the picture the engine kept when the page
/// was last hidden ([`Platform::kept_picture`]; its ground is then left to
/// whoever decodes it), else the page as it is now. `None` when the engine
/// can't (a page off screen that it won't draw) or takes too long.
pub async fn take<P: Platform>(page: &PageOf<P>) -> Option<Cover> {
    let size = page.view_size();
    if let Some((jpeg, w, h)) = P::kept_picture(page) {
        return Some(Cover {
            jpeg,
            pixels: (w, h),
            size,
            ground: None,
        });
    }
    if size.0 == 0 || size.1 == 0 {
        return None;
    }
    let shot = page.snapshot_bitmap();
    let bitmap = crate::main_loop::timeout::<P::Loop, _>(TAKE_LIMIT, shot)
        .await?
        .ok()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let worker = std::thread::Builder::new()
        .name("copper-cover".into())
        .spawn(move || {
            let _ = tx.send(make(&bitmap, size));
        });
    if worker.is_err() {
        return None;
    }
    rx.await.ok().flatten()
}

type CoverHook<P> = Rc<dyn Fn(&Shell<P>, &TabId, Option<&Rc<Cover>>)>;

/// The pictures of sleeping tabs and the covers up now.
pub(crate) struct Covers<P: Platform> {
    /// A sleeping tab's last picture.
    pictures: RefCell<HashMap<TabId, Rc<Cover>>>,
    /// Covers on show, each with the turn it went up on (so an older
    /// timer leaves a newer cover alone).
    showing: RefCell<HashMap<TabId, (Rc<Cover>, u64)>>,
    turn: Cell<u64>,
    hooks: RefCell<Vec<CoverHook<P>>>,
}

impl<P: Platform> Default for Covers<P> {
    fn default() -> Self {
        Covers {
            pictures: RefCell::default(),
            showing: RefCell::default(),
            turn: Cell::new(0),
            hooks: RefCell::default(),
        }
    }
}

impl<P: Platform> Shell<P> {
    /// Run `hook` when a tab's cover goes up (`Some`) or comes down
    /// (`None`; the platform fades it over [`FADE`]).
    pub fn on_cover(&self, hook: impl Fn(&Shell<P>, &TabId, Option<&Rc<Cover>>) + 'static) {
        self.covers().hooks.borrow_mut().push(Rc::new(hook));
    }

    /// Whether anything draws covers ([`Shell::on_cover`]): no picture is
    /// taken otherwise.
    pub fn draws_covers(&self) -> bool {
        !self.covers().hooks.borrow().is_empty()
    }

    /// The cover on show for tab `id`, if any.
    pub fn cover(&self, id: &TabId) -> Option<Rc<Cover>> {
        self.covers()
            .showing
            .borrow()
            .get(id)
            .map(|(c, _)| c.clone())
    }

    /// Whether sleeping tab `id` has a picture waiting for its wake.
    pub fn has_picture(&self, id: &TabId) -> bool {
        self.covers().pictures.borrow().contains_key(id)
    }

    /// Keep `cover` for when tab `id` wakes.
    pub(crate) fn keep_picture(&self, id: &TabId, cover: Cover) {
        self.covers()
            .pictures
            .borrow_mut()
            .insert(id.clone(), Rc::new(cover));
    }

    /// Tab `id` is waking: put its picture up, if it has one.
    pub(crate) fn wake_cover(&self, id: &TabId) {
        let Some(cover) = self.covers().pictures.borrow_mut().remove(id) else {
            return;
        };
        let turn = self.covers().turn.get() + 1;
        self.covers().turn.set(turn);
        self.covers()
            .showing
            .borrow_mut()
            .insert(id.clone(), (cover.clone(), turn));
        tracing::debug!(tab = %id.short(), ?cover, "wake cover up");
        self.tell_cover(id, Some(&cover));
        self.uncover(id, AT_MOST);
    }

    /// Take tab `id`'s cover down after `after` (now when zero).
    pub fn uncover(&self, id: &TabId, after: Duration) {
        let Some(turn) = self.covers().showing.borrow().get(id).map(|(_, t)| *t) else {
            return;
        };
        if after.is_zero() {
            tracing::debug!(tab = %id.short(), "wake cover down");
            self.covers().showing.borrow_mut().remove(id);
            self.tell_cover(id, None);
            return;
        }
        let weak = self.downgrade();
        let id = id.clone();
        P::Loop::spawn_local(async move {
            P::Loop::sleep(after).await;
            let Some(s) = weak.upgrade() else { return };
            let current = s.covers().showing.borrow().get(&id).map(|(_, t)| *t);
            if current == Some(turn) {
                s.uncover(&id, Duration::ZERO);
            }
        });
    }

    /// Tab `id` is gone, or was sent somewhere new: no picture, no cover.
    pub(crate) fn forget_cover(&self, id: &TabId) {
        self.covers().pictures.borrow_mut().remove(id);
        self.uncover(id, Duration::ZERO);
    }

    fn tell_cover(&self, id: &TabId, cover: Option<&Rc<Cover>>) {
        let hooks = self.covers().hooks.borrow().clone();
        for hook in &hooks {
            hook(self, id, cover);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Bitmap {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&f(x, y));
            }
        }
        Bitmap {
            width: w,
            height: h,
            stride: w as usize * 4,
            rgba,
        }
    }

    #[test]
    fn the_ground_is_the_edges_commonest_colour() {
        // A white page with a dark scrollbar run at the top right and dark
        // text in the middle: the edges are mostly white.
        let b = bitmap(200, 100, |x, y| {
            if (x > 50 && x < 150 && y > 20 && y < 80) || (x == 199 && y < 10) {
                [20, 20, 20, 255]
            } else {
                [250, 248, 246, 255]
            }
        });
        assert_eq!(ground_of(&b), Some([250, 248, 246]));
        let dark = bitmap(64, 64, |_, _| [18, 18, 20, 255]);
        assert_eq!(ground_of(&dark), Some([18, 18, 20]));
        assert_eq!(ground_of(&bitmap(2, 2, |_, _| [0, 0, 0, 255])), None);
    }

    #[test]
    fn a_cover_is_a_jpeg_at_the_size_it_was_taken() {
        let b = bitmap(30, 20, |_, _| [255, 255, 255, 255]);
        let c = make(&b, (20, 13)).unwrap();
        assert!(c.jpeg.starts_with(&[0xFF, 0xD8]));
        assert_eq!(c.pixels, (30, 20));
        assert_eq!(c.size, (20, 13));
        assert_eq!(c.ground, Some([255, 255, 255]));
    }
}
