//! A small blocking HTTP/1.1 client for the loopback server — what the
//! bridge and the CLI use. No runtime: safe to call from anywhere, including
//! inside an async test.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// A response, whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// One request to `127.0.0.1:port`. `timeout` bounds connecting and each read.
pub fn request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: &[u8],
    timeout: Duration,
) -> io::Result<Response> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    for (name, value) in headers {
        head += &format!("{name}: {value}\r\n");
    }
    head += &format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    parse(&raw)
}

fn parse(raw: &[u8]) -> io::Result<Response> {
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| bad("no HTTP head"))?;
    let head = String::from_utf8_lossy(&raw[..end]);
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| bad("no HTTP status"))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            let colon = l.find(':')?;
            Some((
                l[..colon].trim().to_lowercase(),
                l[colon + 1..].trim().to_string(),
            ))
        })
        .collect();
    let mut body = raw[end + 4..].to_vec();
    if let Some(length) = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
    {
        body.truncate(length);
    }
    Ok(Response {
        status,
        headers,
        body,
    })
}

/// Whether Copper answers `/health` on `port`.
pub fn reachable(port: u16) -> bool {
    request(port, "GET", "/health", &[], &[], Duration::from_secs(1))
        .map(|r| r.status == 200)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let r = parse(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nMcp-Session-Id: x\r\n\r\n")
            .unwrap();
        assert_eq!(r.status, 202);
        assert_eq!(r.header("mcp-session-id"), Some("x"));
        assert!(r.body.is_empty());
    }
}
