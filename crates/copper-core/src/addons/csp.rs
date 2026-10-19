//! The Content-Security-Policy every addon HTML page gets (contract §5.4),
//! as a `<meta>` element injected as the first element of `<head>` — the
//! scheme handlers carry no headers on every platform, so the policy rides
//! in the document. All network goes through `copper.fetch`.

/// The policy.
pub const CSP: &str = "default-src copper-addon:; script-src copper-addon:; style-src copper-addon: 'unsafe-inline'; img-src copper-addon: https: data: blob:; font-src copper-addon: data:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";

/// `<meta http-equiv="Content-Security-Policy" content="…">`.
pub fn meta_tag() -> String {
    format!(r#"<meta http-equiv="Content-Security-Policy" content="{CSP}">"#)
}

/// `html` with [`meta_tag`] as the first element of its `<head>`. With no
/// `<head>` start tag before the first other element (comments, a doctype
/// and `<html>` are skipped), a `<head>` holding the tag is inserted there.
pub fn inject_csp(html: &str) -> String {
    let out = inject_csp_bytes(html.as_bytes());
    // Insertion happens next to ASCII bytes only, so this is still UTF-8.
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// [`inject_csp`] on bytes (a page in any ASCII-compatible encoding).
pub fn inject_csp_bytes(html: &[u8]) -> Vec<u8> {
    let meta = meta_tag();
    let (at, wrap) = insertion_point(html);
    let mut out = Vec::with_capacity(html.len() + meta.len() + 13);
    out.extend_from_slice(&html[..at]);
    if wrap {
        out.extend_from_slice(b"<head>");
        out.extend_from_slice(meta.as_bytes());
        out.extend_from_slice(b"</head>");
    } else {
        out.extend_from_slice(meta.as_bytes());
    }
    out.extend_from_slice(&html[at..]);
    out
}

/// Where to insert, and whether a `<head>` must be made.
fn insertion_point(b: &[u8]) -> (usize, bool) {
    let mut i = 0;
    // A byte-order mark stays first.
    if b.starts_with(&[0xef, 0xbb, 0xbf]) {
        i = 3;
    }
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            return (i, true);
        }
        if b[i] != b'<' {
            // Text: the body has begun.
            return (i, true);
        }
        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            match find(b, i + 4, b"-->") {
                Some(end) => {
                    i = end + 3;
                    continue;
                }
                None => return (b.len(), true),
            }
        }
        if rest.starts_with(b"<!") || rest.starts_with(b"<?") {
            match tag_end(b, i) {
                Some(end) => {
                    i = end + 1;
                    continue;
                }
                None => return (b.len(), true),
            }
        }
        let name = tag_name(&b[i + 1..]);
        if name.eq_ignore_ascii_case(b"html") {
            match tag_end(b, i) {
                Some(end) => {
                    i = end + 1;
                    continue;
                }
                None => return (b.len(), true),
            }
        }
        if name.eq_ignore_ascii_case(b"head") {
            return match tag_end(b, i) {
                Some(end) => (end + 1, false),
                None => (b.len(), true),
            };
        }
        // Any other tag (or an end tag): insert a head before it.
        return (i, true);
    }
}

/// The tag name right after `<`: ASCII letters and digits.
fn tag_name(b: &[u8]) -> &[u8] {
    let n = b.iter().take_while(|c| c.is_ascii_alphanumeric()).count();
    let name = &b[..n];
    // `<header>` is not `<head>`: the name must end at whitespace, `/` or `>`.
    match b.get(n) {
        None | Some(b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r' | b'\x0c') => name,
        _ => &[],
    }
}

/// The index of the `>` closing the tag that starts at `start`, honouring
/// quoted attribute values.
fn tag_end(b: &[u8], start: usize) -> Option<usize> {
    let mut quote = None;
    for (j, &c) in b.iter().enumerate().skip(start + 1) {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if c == b'>' => return Some(j),
            None => {}
        }
    }
    None
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> String {
        meta_tag()
    }

    #[test]
    fn into_an_existing_head() {
        let html = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>x</title></head><body></body></html>";
        let out = inject_csp(html);
        assert_eq!(
            out,
            format!(
                "<!doctype html><html lang=\"en\"><head>{}<meta charset=\"utf-8\"><title>x</title></head><body></body></html>",
                m()
            )
        );
    }

    #[test]
    fn uppercase_head_and_attributes() {
        let out = inject_csp("<HTML><HEAD data-x=\"a>b\">\n<TITLE>t</TITLE></HEAD></HTML>");
        assert_eq!(
            out,
            format!(
                "<HTML><HEAD data-x=\"a>b\">{}\n<TITLE>t</TITLE></HEAD></HTML>",
                m()
            )
        );
    }

    #[test]
    fn creates_a_head_when_absent() {
        assert_eq!(
            inject_csp("<!DOCTYPE html>\n<body><p>x</p></body>"),
            format!("<!DOCTYPE html>\n<head>{}</head><body><p>x</p></body>", m())
        );
        assert_eq!(
            inject_csp("<html><body>x</body></html>"),
            format!("<html><head>{}</head><body>x</body></html>", m())
        );
        assert_eq!(
            inject_csp("<p>bare</p>"),
            format!("<head>{}</head><p>bare</p>", m())
        );
        assert_eq!(inject_csp(""), format!("<head>{}</head>", m()));
        assert_eq!(inject_csp("hello"), format!("<head>{}</head>hello", m()));
    }

    #[test]
    fn not_fooled_by_header_or_comments() {
        assert_eq!(
            inject_csp("<!-- <head> --><header>x</header>"),
            format!("<!-- <head> --><head>{}</head><header>x</header>", m())
        );
        // A `<head>` mentioned later (in a script) is not the head.
        assert_eq!(
            inject_csp("<body><script>'<head>'</script></body>"),
            format!("<head>{}</head><body><script>'<head>'</script></body>", m())
        );
    }

    #[test]
    fn bom_stays_first() {
        let out = inject_csp("\u{feff}<head></head>");
        assert!(out.starts_with('\u{feff}'));
        assert_eq!(out, format!("\u{feff}<head>{}</head>", m()));
    }

    #[test]
    fn policy_shape() {
        assert!(CSP.contains("connect-src 'none'"));
        assert!(m().starts_with("<meta http-equiv=\"Content-Security-Policy\""));
    }
}
