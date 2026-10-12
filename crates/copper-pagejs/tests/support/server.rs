//! Serves `tests/pages` on 127.0.0.1 (one thread per connection, `Connection: close`).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

pub struct Server {
    pub base: String,
}

pub fn start() -> std::io::Result<Server> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let base = format!("http://{}", listener.local_addr()?);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = serve(stream);
            });
        }
    });
    Ok(Server { base })
}

fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    // Drain the headers.
    let mut line = String::new();
    while reader.read_line(&mut line)? > 2 {
        line.clear();
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let name = path.trim_start_matches('/');
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/pages");
    let file = root.join(name);
    let ok = !name.is_empty() && !name.contains("..") && file.is_file();
    let (status, kind, body) = if ok {
        let kind = if name.ends_with(".html") {
            "text/html; charset=utf-8"
        } else {
            "application/octet-stream"
        };
        ("200 OK", kind, std::fs::read(&file)?)
    } else {
        ("404 Not Found", "text/plain", b"not found".to_vec())
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    stream.flush()
}
