//! A PNG of the whole window, drawn by GTK itself (no compositor): the
//! window's paintable → a render node → the window's renderer → a texture.
//! Pages are included (WebKitGTK draws through the same snapshot). Used by
//! the debug `--screenshot` hook, the self-test and [`crate::control`].

use gtk4 as gtk;
use gtk4::prelude::*;
use std::path::Path;

/// The window (or any widget) as a texture at its current size.
pub fn texture(widget: &gtk::Widget) -> Option<gtk::gdk::Texture> {
    let (w, h) = (widget.width(), widget.height());
    if w <= 0 || h <= 0 {
        return None;
    }
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, w as f64, h as f64);
    let node = snapshot.to_node()?;
    let renderer = widget.native()?.renderer()?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32);
    Some(renderer.render_texture(&node, Some(&bounds)))
}

/// PNG bytes of `widget`.
pub fn window_png_bytes(widget: &gtk::Widget) -> Option<Vec<u8>> {
    texture(widget).map(|t| t.save_to_png_bytes().to_vec())
}

/// Save `widget` as a PNG at `path`.
pub fn save(widget: &gtk::Widget, path: &Path) -> anyhow::Result<()> {
    let t = texture(widget).ok_or_else(|| anyhow::anyhow!("the window has no size yet"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    t.save_to_png(path)?;
    Ok(())
}
