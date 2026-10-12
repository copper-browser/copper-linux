//! Screenshots as the agent gets them (`Tools.Page.screenshot`): one image
//! pixel per view point (so a point read off the picture can be clicked),
//! optionally cropped to an element, PNG or JPEG (quality 0.8).
//!
//! The engine paints at the device scale; this scales the picture down to
//! view points and crops it. Plain RGBA arithmetic — no image framework.

/// Decoded RGBA8 pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Decode a PNG to RGBA8.
pub fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|[r, g, b]| [*r, *g, *b, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|[g, a]| [*g, *g, *g, *a])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("indexed PNG after expansion".into()),
    };
    Ok(Rgba {
        width: w,
        height: h,
        pixels,
    })
}

/// Encode RGBA8 as PNG.
pub fn encode_png(img: &Rgba) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, img.width, img.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&img.pixels)
            .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Encode as JPEG, quality 80, alpha flattened onto white.
pub fn encode_jpeg(img: &Rgba) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = img
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|[r, g, b, a]| {
            let a = u32::from(*a);
            let over = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
            [over(*r), over(*g), over(*b)]
        })
        .collect();
    let w = u16::try_from(img.width).map_err(|_| "picture too wide for JPEG")?;
    let h = u16::try_from(img.height).map_err(|_| "picture too tall for JPEG")?;
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 80)
        .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Box-filter scale to `w` × `h` (downscaling; upscaling falls back to
/// nearest neighbour).
pub fn scale(img: &Rgba, w: u32, h: u32) -> Rgba {
    let (w, h) = (w.max(1), h.max(1));
    if w == img.width && h == img.height {
        return img.clone();
    }
    let sx = f64::from(img.width) / f64::from(w);
    let sy = f64::from(img.height) / f64::from(h);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let y0 = (f64::from(y) * sy).floor() as u32;
        let y1 = (((f64::from(y) + 1.0) * sy).ceil() as u32).clamp(y0 + 1, img.height);
        for x in 0..w {
            let x0 = (f64::from(x) * sx).floor() as u32;
            let x1 = (((f64::from(x) + 1.0) * sx).ceil() as u32).clamp(x0 + 1, img.width);
            let mut acc = [0u64; 4];
            let mut n = 0u64;
            for yy in y0..y1.min(img.height) {
                for xx in x0..x1.min(img.width) {
                    let i = ((yy * img.width + xx) * 4) as usize;
                    for (c, slot) in acc.iter_mut().enumerate() {
                        *slot += u64::from(img.pixels[i + c]);
                    }
                    n += 1;
                }
            }
            let o = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                pixels[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    Rgba {
        width: w,
        height: h,
        pixels,
    }
}

/// Crop to a rectangle (clamped to the picture). `None` when it misses.
pub fn crop(img: &Rgba, x: f64, y: f64, w: f64, h: f64) -> Option<Rgba> {
    let x0 = x.max(0.0).floor() as u32;
    let y0 = y.max(0.0).floor() as u32;
    let x1 = ((x + w).ceil().max(0.0) as u32).min(img.width);
    let y1 = ((y + h).ceil().max(0.0) as u32).min(img.height);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut pixels = Vec::with_capacity((cw * ch * 4) as usize);
    for row in y0..y1 {
        let start = ((row * img.width + x0) * 4) as usize;
        pixels.extend_from_slice(&img.pixels[start..start + (cw * 4) as usize]);
    }
    Some(Rgba {
        width: cw,
        height: ch,
        pixels,
    })
}

/// The agent's picture from the engine's PNG.
///
/// * `view_width` — the view's width in points; the picture is scaled so
///   one pixel is one point (the device scale comes back out).
/// * `rect` — crop to this box, in view points, after scaling.
pub fn agent_picture(
    png_bytes: &[u8],
    view_width: f64,
    rect: Option<(f64, f64, f64, f64)>,
    jpeg: bool,
) -> Result<Vec<u8>, String> {
    let mut img = decode_png(png_bytes)?;
    let mut touched = false;
    if view_width >= 1.0 && (f64::from(img.width) - view_width).abs() >= 1.0 {
        let factor = view_width / f64::from(img.width);
        let h = (f64::from(img.height) * factor).round() as u32;
        img = scale(&img, view_width.round() as u32, h);
        touched = true;
    }
    if let Some((x, y, w, h)) = rect {
        img = crop(&img, x, y, w, h).ok_or("the element is outside the view")?;
        touched = true;
    }
    if jpeg {
        encode_jpeg(&img)
    } else if !touched {
        // Re-encoding costs time; an untouched picture goes out as it came.
        Ok(png_bytes.to_vec())
    } else {
        encode_png(&img)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Rgba {
        Rgba {
            width: w,
            height: h,
            pixels: c
                .iter()
                .copied()
                .cycle()
                .take((w * h * 4) as usize)
                .collect(),
        }
    }

    #[test]
    fn round_trip_scale_crop() {
        let img = solid(8, 6, [10, 20, 30, 255]);
        let png = encode_png(&img).unwrap();
        assert_eq!(decode_png(&png).unwrap(), img);
        let half = agent_picture(&png, 4.0, None, false).unwrap();
        let half = decode_png(&half).unwrap();
        assert_eq!((half.width, half.height), (4, 3));
        assert_eq!(&half.pixels[..4], &[10, 20, 30, 255]);
        let cut = agent_picture(&png, 8.0, Some((2.0, 1.0, 3.0, 2.0)), false).unwrap();
        let cut = decode_png(&cut).unwrap();
        assert_eq!((cut.width, cut.height), (3, 2));
        let jpeg = agent_picture(&png, 8.0, None, true).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert!(crop(&img, 20.0, 20.0, 2.0, 2.0).is_none());
    }
}
