//! A tiny HTTP server on 127.0.0.1 serving the two pages the spike needs:
//! `/user` (the page the user is looking at) and `/probe` (the agent's page,
//! which counts rAF ticks and timer ticks and records every input event with
//! its `isTrusted`). One thread per connection, `Connection: close`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

/// The user's page: static, with its own rAF counter as a baseline.
const USER: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>user page</title>
<style>body{margin:0;font:28px/1.4 sans-serif;background:#f4efe6;color:#2b2b2b}
main{padding:48px}</style></head>
<body><main><h1>The user's tab</h1><p>Nothing here should change while an agent works.</p></main>
<script>
window.uprobe = {raf: 0};
(function tick(){ uprobe.raf++; requestAnimationFrame(tick); })();
</script></body></html>"#;

/// The agent's page. Layout (CSS px, viewport origin):
///   #box   0,0     200x200  colour changes every rAF tick (liveness pixel)
///   #btn   300,100 200x80   centre (400,140)
///   #field 300,250 300x40   centre (450,270), inside a form (Enter submits)
const PROBE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>agent probe</title>
<style>
  body{margin:0;font:20px sans-serif;background:linear-gradient(90deg,#fff 0,#dfe9f3 60%,#c3d3e8 100%);height:100vh}
  #box{position:absolute;left:0;top:0;width:200px;height:200px;background:hsl(0 80% 50%)}
  #btn{position:absolute;left:300px;top:100px;width:200px;height:80px;font-size:22px}
  #field{position:absolute;left:300px;top:250px;width:300px;height:40px;font-size:20px;box-sizing:border-box}
  #txt{position:absolute;left:300px;top:340px}
</style></head>
<body>
<div id="box"></div>
<button id="btn">Press</button>
<form id="form" onsubmit="return false"><input id="field" autocomplete="off"></form>
<div id="txt">agent probe page</div>
<script>
window.probe = {raf:0, timers:0, clicks:[], downs:[], dbl:[], moves:0, movesTrusted:0,
                keys:[], inputs:[], focus:[], submits:[], ctx:[]};
const box = document.getElementById('box');
(function tick(){ probe.raf++; box.style.background = `hsl(${(probe.raf*13)%360} 80% 50%)`; requestAnimationFrame(tick); })();
setInterval(() => { probe.timers++; }, 10);
const btn = document.getElementById('btn');
btn.addEventListener('click', e => probe.clicks.push({t:e.isTrusted, x:e.clientX, y:e.clientY, d:e.detail, p:e.pointerType ?? null}));
btn.addEventListener('mousedown', e => probe.downs.push({t:e.isTrusted, b:e.button}));
btn.addEventListener('dblclick', e => probe.dbl.push({t:e.isTrusted}));
btn.addEventListener('contextmenu', e => { probe.ctx.push({t:e.isTrusted}); e.preventDefault(); });
document.addEventListener('mousemove', e => { probe.moves++; if (e.isTrusted) probe.movesTrusted++; probe.lastMove=[e.clientX,e.clientY]; });
const f = document.getElementById('field');
for (const ty of ['keydown','keypress','keyup']) f.addEventListener(ty, e => probe.keys.push({ty, k:e.key, t:e.isTrusted}));
for (const ty of ['beforeinput','input']) f.addEventListener(ty, e => probe.inputs.push({ty, it:e.inputType ?? null, d:e.data ?? null, t:e.isTrusted}));
f.addEventListener('focus', e => probe.focus.push({t:e.isTrusted}));
document.getElementById('form').addEventListener('submit', e => probe.submits.push({t:e.isTrusted}));
</script></body></html>"#;

pub struct Server {
    pub base: String,
}

pub fn start() -> std::io::Result<Server> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let base = format!("http://{}", listener.local_addr()?);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = handle(stream);
            });
        }
    });
    Ok(Server { base })
}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {
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
    let body = match path.split('?').next().unwrap_or("/") {
        "/user" => USER,
        "/probe" => PROBE,
        _ => {
            return write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body.as_bytes())
}
