//! Icons for rows that have no page (Mac: `Fork/Marks.swift`).
//!
//! A tab only learns its favicon from its own page, so a pin or a Saved row
//! that came from Copper Cloud, or that has slept since the last launch,
//! showed a letter until it was clicked. Each such site is asked once a
//! launch, without a page and without JavaScript, the way the Mac asks:
//!
//!   1. `/favicon.ico`;
//!   2. the front page's own `<link rel="icon">`, read from its first 64 KB;
//!   3. `/apple-touch-icon.png`;
//!   4. the parent site's `/favicon.ico` (`calendar.google.com` → `google.com`).
//!
//! The first answer that is a picture is kept as the site's front-page icon
//! in the favicon cache (as PNG: an `.ico` is unpacked), where every row on
//! that site finds it.

use std::time::Duration;

/// A browser's user agent: some sites answer a bare client with a 403.
const AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

/// How many sites are asked at once.
const LANE: usize = 6;

/// Ask every origin (`https://host[:port]`) for its icon. Blocking: run on
/// a thread of its own. Returns each origin that answered with a PNG.
pub fn fetch_all(origins: Vec<String>) -> Vec<(String, Vec<u8>)> {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Vec::new();
    };
    let Ok(client) = reqwest::Client::builder()
        .user_agent(AGENT)
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(5))
        .build()
    else {
        return Vec::new();
    };
    rt.block_on(async move {
        let mut out = Vec::new();
        for slice in origins.chunks(LANE) {
            let asks = slice.iter().map(|o| {
                let client = client.clone();
                let o = o.clone();
                async move { (o.clone(), ask(&client, &o).await) }
            });
            let mut handles = Vec::new();
            for a in asks {
                handles.push(tokio::spawn(a));
            }
            for h in handles {
                if let Ok((o, Some(png))) = h.await {
                    out.push((o, png));
                }
            }
        }
        out
    })
}

async fn ask(client: &reqwest::Client, origin: &str) -> Option<Vec<u8>> {
    if let Some(hit) = image(client, &format!("{origin}/favicon.ico")).await {
        return Some(hit);
    }
    for href in declared(client, origin).await {
        if let Some(hit) = image(client, &href).await {
            return Some(hit);
        }
    }
    if let Some(hit) = image(client, &format!("{origin}/apple-touch-icon.png")).await {
        return Some(hit);
    }
    let host = url::Url::parse(origin).ok()?.host_str()?.to_string();
    let parent = parent_of(&host)?;
    image(client, &format!("https://{parent}/favicon.ico")).await
}

async fn get(client: &reqwest::Client, url: &str, limit: usize) -> Option<Vec<u8>> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.bytes().await.ok()?;
    (body.len() <= limit).then(|| body.to_vec())
}

/// One candidate, fetched, and kept only if it is a picture we can draw:
/// a site that answers its own 404 page with a 200 is common.
async fn image(client: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let body = get(client, url, 2_000_000).await?;
    if body.len() <= 60 {
        return None;
    }
    as_png(&body)
}

/// What the front page says its icon is, best guess first.
async fn declared(client: &reqwest::Client, origin: &str) -> Vec<String> {
    let Some(body) = get(client, &format!("{origin}/"), 4_000_000).await else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&body[..body.len().min(64_000)]).into_owned();
    declared_in(&text, origin)
}

/// The `<link rel=…icon… href=…>`s in `html`, resolved against `origin`,
/// best first: a 24–192 px raster, then any raster, SVG never (it cannot
/// be drawn here).
pub fn declared_in(html: &str, origin: &str) -> Vec<String> {
    let Ok(base) = url::Url::parse(&format!("{origin}/")) else {
        return Vec::new();
    };
    let low = html.to_ascii_lowercase();
    let mut scored: Vec<(i32, String)> = Vec::new();
    let mut at = 0;
    while let Some(i) = low[at..].find("<link") {
        let start = at + i;
        let Some(len) = low[start..].find('>') else {
            break;
        };
        let tag = &html[start..start + len];
        let ltag = &low[start..start + len];
        at = start + len;
        let Some(rel) = attribute(ltag, "rel") else {
            continue;
        };
        if !rel.contains("icon") || rel.contains("mask-icon") {
            continue;
        }
        let Some(href) = attribute(tag, "href") else {
            continue;
        };
        let Ok(resolved) = base.join(href.trim()) else {
            continue;
        };
        if !resolved.scheme().starts_with("http") || resolved.path().ends_with(".svg") {
            continue;
        }
        let px = attribute(ltag, "sizes")
            .and_then(|s| {
                s.split_whitespace()
                    .filter_map(|p| p.split('x').next()?.parse::<i32>().ok())
                    .max()
            })
            .unwrap_or(0);
        let score = match px {
            0 => 30,
            1..24 => 12,
            24..=192 => 50 + px.min(64),
            _ => 20,
        } + if rel.contains("apple-touch") { -5 } else { 0 };
        scored.push((score, resolved.to_string()));
    }
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    let mut out: Vec<String> = Vec::new();
    for (_, u) in scored {
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

/// `name="value"`, `name='value'` or `name=value` in one tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    // ASCII lowercasing keeps every byte where it was.
    let low = tag.to_ascii_lowercase();
    let b = tag.as_bytes();
    let mut from = 0;
    while let Some(i) = low[from..].find(name).map(|i| from + i) {
        from = i + name.len();
        if i > 0 && !b[i - 1].is_ascii_whitespace() {
            continue;
        }
        let mut j = from;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if b.get(j) != Some(&b'=') {
            continue;
        }
        j += 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        return match b.get(j)? {
            q @ (b'"' | b'\'') => {
                let end = tag[j + 1..].find(*q as char)?;
                Some(&tag[j + 1..j + 1 + end])
            }
            _ => {
                let end = tag[j..]
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .map_or(tag.len(), |e| j + e);
                Some(tag[j..end].trim_end_matches('/'))
            }
        };
    }
    None
}

/// `calendar.google.com` → `google.com`; `www.bbc.co.uk` → `bbc.co.uk`;
/// a bare registrable host → `None` (`Marks.parent`).
pub fn parent_of(host: &str) -> Option<String> {
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 {
        return None;
    }
    let two = ["co", "com", "org", "net", "ac", "gov", "edu"];
    let n = labels.len();
    let take = if n > 3 && two.contains(&labels[n - 2]) && labels[n - 1].len() == 2 {
        3
    } else {
        2
    };
    let parent = labels[n - take..].join(".");
    (parent != host).then_some(parent)
}

/// The picture as a PNG: a PNG as it is, an `.ico` unpacked (its largest
/// PNG entry, or its largest 32- or 24-bit bitmap); anything else `None`.
pub fn as_png(bytes: &[u8]) -> Option<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(SIG) {
        return Some(bytes.to_vec());
    }
    if bytes.len() < 6 || bytes[0..4] != [0, 0, 1, 0] {
        return None;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let mut best: Option<(u32, &[u8])> = None;
    for i in 0..count {
        let e = bytes.get(6 + i * 16..6 + i * 16 + 16)?;
        let side = |b: u8| if b == 0 { 256 } else { u32::from(b) };
        let area = side(e[0]) * side(e[1]);
        let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let off = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let Some(data) = bytes.get(off..off.checked_add(size)?) else {
            continue;
        };
        if best.is_none_or(|(a, _)| area > a) {
            best = Some((area, data));
        }
    }
    let (_, data) = best?;
    if data.starts_with(SIG) {
        return Some(data.to_vec());
    }
    dib_to_png(data)
}

/// An `.ico` entry's bitmap (a BITMAPINFOHEADER, the pixels bottom-up at
/// twice the height, then the AND mask) as an RGBA PNG.
fn dib_to_png(d: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |o: usize| {
        d.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let header = u32_at(0)? as usize;
    let w = u32_at(4)? as i32;
    let h = (u32_at(8)? as i32) / 2;
    let bpp = u16::from_le_bytes([*d.get(14)?, *d.get(15)?]);
    if !(1..=256).contains(&w) || !(1..=256).contains(&h) || !(bpp == 32 || bpp == 24) {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let row = (w * bpp as usize / 8).div_ceil(4) * 4;
    let mask_row = w.div_ceil(32) * 4;
    let px = d.get(header..header + row * h)?;
    let mask = d.get(header + row * h..header + row * h + mask_row * h);
    let mut rgba = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        let src = &px[(h - 1 - y) * row..];
        for x in 0..w {
            let o = (y * w + x) * 4;
            let s = x * bpp as usize / 8;
            rgba[o] = src[s + 2];
            rgba[o + 1] = src[s + 1];
            rgba[o + 2] = src[s];
            rgba[o + 3] = if bpp == 32 { src[s + 3] } else { 255 };
            any_alpha |= bpp == 32 && src[s + 3] != 0;
        }
    }
    // A 32-bit entry with no alpha at all, or a 24-bit one: the AND mask
    // says what is transparent.
    if !any_alpha || bpp == 24 {
        for y in 0..h {
            for x in 0..w {
                let clear = mask
                    .map(|m| m[(h - 1 - y) * mask_row + x / 8] & (0x80 >> (x % 8)) != 0)
                    .unwrap_or(false);
                rgba[(y * w + x) * 4 + 3] = if clear { 0 } else { 255 };
            }
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(&rgba).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parents() {
        assert_eq!(
            parent_of("calendar.google.com").as_deref(),
            Some("google.com")
        );
        assert_eq!(parent_of("www.bbc.co.uk").as_deref(), Some("bbc.co.uk"));
        assert_eq!(parent_of("google.com"), None);
    }

    #[test]
    fn declared_links_best_first() {
        let html = r#"<head><link rel="mask-icon" href="/m.svg"><link rel="icon" href="/i.svg">
            <link rel='shortcut icon' href=/f16.png sizes=16x16>
            <link href="https://cdn.x.com/i32.png" rel="icon" sizes="32x32"/></head>"#;
        let got = declared_in(html, "https://x.com");
        assert_eq!(
            got,
            vec!["https://cdn.x.com/i32.png", "https://x.com/f16.png"]
        );
    }

    #[test]
    fn ico_with_a_bitmap_becomes_a_png() {
        // A 2×2 32-bit entry: header, 4 pixels, mask.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&2u32.to_le_bytes());
        dib.extend_from_slice(&4u32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        for _ in 0..4 {
            dib.extend_from_slice(&[0, 0, 255, 255]);
        }
        dib.extend_from_slice(&[0u8; 8]);
        let mut ico = vec![0, 0, 1, 0, 1, 0];
        ico.extend_from_slice(&[2, 2, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&dib);
        let png = as_png(&ico).expect("png");
        let dec = png::Decoder::new(std::io::Cursor::new(png));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size()];
        r.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..4], &[255, 0, 0, 255]);
        assert!(as_png(b"<html>not found</html>").is_none());
    }

    #[test]
    #[ignore = "network"]
    fn live_sites_answer() {
        let got = fetch_all(
            [
                "https://github.com",
                "https://mail.google.com",
                "https://www.youtube.com",
                "https://app.slack.com",
                "https://x.com",
                "https://www.amazon.com",
                "https://claude.ai",
            ]
            .map(String::from)
            .to_vec(),
        );
        let names: Vec<_> = got
            .iter()
            .map(|(o, p)| format!("{o} {}", p.len()))
            .collect();
        eprintln!("{names:#?}");
        assert!(got.len() >= 5);
    }
}
