//! Pictures as the agent gets them: cropped out of what the engine painted
//! and scaled to the size asked for (`screenshot.rs` decides which), PNG or
//! JPEG (quality 0.8). Plain RGBA arithmetic — no image framework.

use copper_engine::Bitmap;

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
    encode_jpeg_quality(img, 80)
}

/// Encode as JPEG at `quality` (1–100), alpha flattened onto white.
pub fn encode_jpeg_quality(img: &Rgba, quality: u8) -> Result<Vec<u8>, String> {
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
    jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100))
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

/// One output pixel's source pixels along one axis, with their weights.
type Taps = Vec<(usize, f32)>;

/// Catmull-Rom: sharp, no ringing to speak of on text.
fn cubic(x: f64) -> f64 {
    let x = x.abs();
    if x < 1.0 {
        1.5 * x * x * x - 2.5 * x * x + 1.0
    } else if x < 2.0 {
        -0.5 * x * x * x + 2.5 * x * x - 4.0 * x + 2.0
    } else {
        0.0
    }
}

/// The taps of `out` pixels covering source pixels `start .. start + len`
/// (fractional) of a line `limit` pixels long. Shrinking (or 1:1) averages
/// the source area each output pixel covers, so whole pixels copy exactly;
/// enlarging interpolates with [`cubic`]. Past the edges, the edge pixel.
fn taps(start: f64, len: f64, out: u32, limit: u32) -> Vec<Taps> {
    let n = out.max(1);
    let ratio = len / f64::from(n);
    let last = i64::from(limit.max(1) - 1);
    let at = |j: i64| j.clamp(0, last) as usize;
    (0..n)
        .map(|i| {
            let a = start + f64::from(i) * ratio;
            let b = a + ratio;
            let mut taps: Vec<(usize, f64)> = Vec::new();
            if ratio >= 1.0 - 1e-9 {
                for j in (a.floor() as i64)..(b.ceil() as i64) {
                    let cover = b.min(j as f64 + 1.0) - a.max(j as f64);
                    if cover > 1e-9 {
                        taps.push((at(j), cover));
                    }
                }
            } else {
                // The output pixel's centre, in source pixel-centre terms.
                let c = a + ratio / 2.0 - 0.5;
                let j0 = c.floor() as i64;
                for j in j0 - 1..=j0 + 2 {
                    taps.push((at(j), cubic(c - j as f64)));
                }
            }
            let sum: f64 = taps.iter().map(|(_, w)| w).sum();
            if taps.is_empty() || sum.abs() < 1e-12 {
                return vec![(at(a.floor() as i64), 1.0)];
            }
            taps.into_iter()
                .map(|(j, w)| (j, (w / sum) as f32))
                .collect()
        })
        .collect()
}

/// The rectangle `src` = (x, y, width, height) of `bitmap`, in its pixels
/// (fractions allowed), as an `out` = (width, height) picture: shrunk by
/// area averaging (1:1 copies exactly), enlarged with Catmull-Rom. Slow for
/// a big picture: run it off the UI thread.
pub fn resample(bitmap: &Bitmap, src: (f64, f64, f64, f64), out: (u32, u32)) -> Rgba {
    let (ow, oh) = (out.0.max(1), out.1.max(1));
    let mut pixels = vec![0u8; ow as usize * oh as usize * 4];
    if bitmap.width == 0 || bitmap.height == 0 {
        return Rgba {
            width: ow,
            height: oh,
            pixels,
        };
    }
    let xs = taps(src.0, src.2, ow, bitmap.width);
    let ys = taps(src.1, src.3, oh, bitmap.height);
    let first = xs.iter().flatten().map(|(j, _)| *j).min().unwrap_or(0);
    let end = xs.iter().flatten().map(|(j, _)| *j + 1).max().unwrap_or(1);
    // One row of the needed columns, summed down the source rows.
    let mut acc = vec![0f32; (end - first) * 4];
    for (oy, row_taps) in ys.iter().enumerate() {
        acc.fill(0.0);
        for &(row, wy) in row_taps {
            let Some(line) = bitmap.row(row as u32) else {
                continue;
            };
            let line = &line[first * 4..end * 4];
            for (a, &v) in acc.iter_mut().zip(line) {
                *a += wy * f32::from(v);
            }
        }
        let o = oy * ow as usize * 4;
        for (ox, col_taps) in xs.iter().enumerate() {
            let mut px = [0f32; 4];
            for &(col, wx) in col_taps {
                let p = (col - first) * 4;
                for (c, v) in px.iter_mut().enumerate() {
                    *v += wx * acc[p + c];
                }
            }
            let at = o + ox * 4;
            for (c, v) in px.iter().enumerate() {
                pixels[at + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Rgba {
        width: ow,
        height: oh,
        pixels,
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

    fn at(img: &Rgba, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn round_trip_scale_crop() {
        let img = solid(8, 6, [10, 20, 30, 255]);
        let png = encode_png(&img).unwrap();
        assert_eq!(decode_png(&png).unwrap(), img);
        let half = scale(&img, 4, 3);
        assert_eq!((half.width, half.height), (4, 3));
        assert_eq!(&half.pixels[..4], &[10, 20, 30, 255]);
        let cut = crop(&img, 2.0, 1.0, 3.0, 2.0).unwrap();
        assert_eq!((cut.width, cut.height), (3, 2));
        let jpeg = encode_jpeg(&img).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert!(crop(&img, 20.0, 20.0, 2.0, 2.0).is_none());
    }

    #[test]
    fn resample_copies_whole_pixels_exactly() {
        let b = bitmap(10, 8, |x, y| [x as u8 * 10, y as u8 * 10, 7, 255]);
        let r = resample(&b, (2.0, 3.0, 4.0, 2.0), (4, 2));
        assert_eq!((r.width, r.height), (4, 2));
        assert_eq!(at(&r, 0, 0), [20, 30, 7, 255]);
        assert_eq!(at(&r, 3, 1), [50, 40, 7, 255]);
    }

    #[test]
    fn resample_shrinks_by_area() {
        // A 2x picture of a 2-wide red | blue split, shrunk to 1x.
        let b = bitmap(4, 2, |x, _| {
            if x < 2 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            }
        });
        let r = resample(&b, (0.0, 0.0, 4.0, 2.0), (2, 1));
        assert_eq!(at(&r, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&r, 1, 0), [0, 0, 255, 255]);
        // Half a pixel in: the left output pixel straddles the seam.
        let r = resample(&b, (1.0, 0.0, 2.0, 2.0), (1, 1));
        assert_eq!(at(&r, 0, 0), [128, 0, 128, 255]);
    }

    #[test]
    fn resample_enlarges_smoothly() {
        let b = bitmap(2, 1, |x, _| if x == 0 { [0; 4] } else { [255; 4] });
        let r = resample(&b, (0.0, 0.0, 2.0, 1.0), (8, 4));
        assert_eq!((r.width, r.height), (8, 4));
        // Flat at the ends, a ramp in between.
        assert_eq!(at(&r, 0, 0)[0], 0);
        assert_eq!(at(&r, 7, 3)[0], 255);
        let mid: Vec<u8> = (0..8).map(|x| at(&r, x, 1)[0]).collect();
        assert!(mid.windows(2).all(|w| w[0] <= w[1]), "{mid:?}");
        assert!(mid[3] > 40 && mid[4] < 215, "{mid:?}");
    }
}
