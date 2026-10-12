//! Tiny HTTP/1.1 server on 127.0.0.1 for the integration test: one thread
//! per connection, `Connection: close`, records every request path.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Hits = Arc<Mutex<Vec<String>>>;

pub const DOWNLOAD_BODY: &[u8] = b"copper engine download payload\n0123456789abcdef\n";
/// Size of `/slow.bin`; served in 64 KiB chunks 25 ms apart (about 3 s).
pub const SLOW_SIZE: usize = 128 * 64 * 1024;

pub struct Server {
    pub base: String,
    pub hits: Hits,
}

impl Server {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn saw(&self, needle: &str) -> bool {
        self.hits.lock().unwrap().iter().any(|p| p.contains(needle))
    }

    pub fn clear(&self) {
        self.hits.lock().unwrap().clear();
    }
}

const INDEX: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Copper engine test</title>
<link rel="icon" type="image/png" href="/icon.png">
<style>
  body { margin: 0; font: 28px/1.3 sans-serif; }
  section { height: 900px; padding: 30px; box-sizing: border-box; color: #fff; }
  #a { background: linear-gradient(135deg, #e8613c, #f6b042); }
  #b { background: linear-gradient(135deg, #1f6f8b, #63c3d1); }
</style></head>
<body>
  <section id="a"><h1>Copper engine</h1><p>needle one, needle two.</p></section>
  <section id="b"><h2>Second screen</h2><p>A third needle lives here.</p></section>
</body></html>"#;

const PAGE2: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Page two</title></head>
<body style="background:#234;color:#fff"><h1>Page two</h1></body></html>"#;

const FILTER_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>filter</title></head><body>
<div class="ad">AD</div>
<img id="gif" src="/tracker.gif">
<script src="/allowed.js"></script>
<script src="/blocked.js"></script>
</body></html>"#;

const INPUT_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>input</title>
<style>
  body { margin: 0; font: 16px sans-serif; }
  #btn { position: absolute; left: 20px; top: 20px; width: 120px; height: 40px; }
  #field { position: absolute; left: 20px; top: 80px; width: 200px; height: 30px; }
  #check { position: absolute; left: 20px; top: 130px; width: 20px; height: 20px; margin: 0; }
  #pad { position: absolute; left: 20px; top: 170px; width: 200px; height: 90px; background: #ccc; }
  #scroller { position: absolute; left: 300px; top: 20px; width: 200px; height: 100px; overflow: auto; }
</style></head><body>
<button id="btn">Press</button>
<input id="field">
<input id="check" type="checkbox">
<div id="pad"></div>
<div id="scroller"><div style="height: 1000px">tall</div></div>
<script>
  window.log = { clicks: 0, trusted: null, inputs: 0, keys: [], drag: [] };
  const $ = (id) => document.getElementById(id);
  $('btn').addEventListener('click', (e) => { log.clicks++; log.trusted = e.isTrusted; });
  $('field').addEventListener('input', () => log.inputs++);
  $('field').addEventListener('keydown', (e) => log.keys.push(e.key));
  $('pad').addEventListener('mousedown', (e) => log.drag.push(['down', e.clientX | 0, e.clientY | 0]));
  $('pad').addEventListener('mousemove', (e) => { if (e.buttons & 1) log.drag.push(['move']); });
  document.addEventListener('mouseup', (e) => log.drag.push(['up', e.clientX | 0, e.clientY | 0]));
</script></body></html>"#;

/// 1x1 transparent GIF.
const GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
];

pub fn start(icon_png: Vec<u8>) -> std::io::Result<Server> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let base = format!("http://{}", listener.local_addr()?);
    let hits: Hits = Arc::default();
    let h = hits.clone();
    let icon: Arc<[u8]> = icon_png.into();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let h = h.clone();
            let icon = icon.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &h, &icon);
            });
        }
    });
    Ok(Server { base, hits })
}

fn handle(mut stream: TcpStream, hits: &Hits, icon: &[u8]) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
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
        "/page2" => ("text/html; charset=utf-8", PAGE2.into()),
        "/filter" => ("text/html; charset=utf-8", FILTER_PAGE.into()),
        "/input" => ("text/html; charset=utf-8", INPUT_PAGE.into()),
        "/allowed.js" => ("text/javascript", b"window.allowedRan = true;".to_vec()),
        "/blocked.js" => ("text/javascript", b"window.blockedRan = true;".to_vec()),
        "/tracker.gif" => ("image/gif", GIF.to_vec()),
        "/icon.png" => ("image/png", icon.to_vec()),
        "/setcookie" => {
            let v = query.strip_prefix("v=").unwrap_or("x");
            extra = format!("Set-Cookie: who={v}; Path=/\r\n");
            (
                "text/html; charset=utf-8",
                format!("<title>cookie {v}</title>set").into(),
            )
        }
        "/file.bin" => {
            extra = "Content-Disposition: attachment; filename=\"copper-test.bin\"\r\n".into();
            ("application/octet-stream", DOWNLOAD_BODY.to_vec())
        }
        "/slow.bin" => {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {SLOW_SIZE}\r\n\
                 Content-Disposition: attachment; filename=\"slow.bin\"\r\nConnection: close\r\n\r\n"
            )?;
            let chunk = vec![0x5au8; 64 * 1024];
            for _ in 0..SLOW_SIZE / chunk.len() {
                stream.write_all(&chunk)?;
                std::thread::sleep(Duration::from_millis(25));
            }
            return Ok(());
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
