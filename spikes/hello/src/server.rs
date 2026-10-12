//! A tiny HTTP/1.0-style server on 127.0.0.1 for the selftest. One thread per
//! connection, `Connection: close`, records every request path.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

pub type Hits = Arc<Mutex<Vec<String>>>;

pub const DOWNLOAD_BODY: &[u8] = b"copper spike download payload\n0123456789abcdef\n";

pub struct Server {
    pub base: String,
    pub hits: Hits,
}

const INDEX: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Copper spike</title>
<style>
  body { margin: 0; font: 32px/1.3 sans-serif; }
  section { height: 1100px; padding: 40px; box-sizing: border-box; color: #fff; }
  #a { background: linear-gradient(135deg, #e8613c, #f6b042); }
  #b { background: linear-gradient(135deg, #1f6f8b, #63c3d1); }
  #c { background: linear-gradient(135deg, #3a2f6b, #9a6fd0); }
</style></head>
<body>
  <section id="a"><h1>Copper for Linux</h1><p>T-001 spike: WebKitGTK 6.0 from Rust.</p></section>
  <section id="b"><h2>Second screen</h2><p>Only in the full-document snapshot.</p></section>
  <section id="c"><h2>Third screen</h2><p>Bottom of the page.</p></section>
</body></html>"#;

const FILTER_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>filter</title></head><body>
<div class="ad">AD</div>
<img id="gif" src="/tracker.gif">
<script src="/allowed.js"></script>
<script src="/blocked.js"></script>
</body></html>"#;

/// 1x1 transparent GIF.
const GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
];

pub fn start() -> std::io::Result<Server> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let base = format!("http://{}", listener.local_addr()?);
    let hits: Hits = Arc::default();
    let h = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let h = h.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &h);
            });
        }
    });
    Ok(Server { base, hits })
}

fn handle(mut stream: TcpStream, hits: &Hits) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    loop {
        let mut l = String::new();
        if reader.read_line(&mut l)? == 0 || l == "\r\n" || l == "\n" {
            break;
        }
    }
    hits.lock().unwrap().push(path.clone());

    let (route, query) = path.split_once('?').unwrap_or((&path, ""));
    let mut extra = String::new();
    let (ctype, body): (&str, Vec<u8>) = match route {
        "/" => ("text/html; charset=utf-8", INDEX.into()),
        "/filter" => ("text/html; charset=utf-8", FILTER_PAGE.into()),
        "/allowed.js" => ("text/javascript", b"window.allowedRan = true;".to_vec()),
        "/blocked.js" => ("text/javascript", b"window.blockedRan = true;".to_vec()),
        "/tracker.gif" => ("image/gif", GIF.to_vec()),
        "/setcookie" => {
            let v = query.strip_prefix("v=").unwrap_or("x");
            extra = format!("Set-Cookie: spike={v}; Path=/\r\n");
            (
                "text/html; charset=utf-8",
                format!("<title>cookie {v}</title>set").into(),
            )
        }
        "/file.bin" => {
            extra = "Content-Disposition: attachment; filename=\"copper-spike.bin\"\r\n".into();
            ("application/octet-stream", DOWNLOAD_BODY.to_vec())
        }
        _ => {
            let body = b"not found".to_vec();
            write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )?;
            return stream.write_all(&body);
        }
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)
}
