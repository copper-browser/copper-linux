//! The loopback HTTP listener (`MCP.route`, `Connection`, `HTTPRequest`,
//! `HTTPResponse`). Hand-written, like the Mac's, so the bytes on the wire
//! are the same: status line, `Content-Type`, `Content-Length`,
//! `Cache-Control: no-store`, extras (`Mcp-Session-Id`), `Connection:
//! keep-alive`, in that order; bodies serialised the way Foundation's
//! `JSONSerialization` does (slashes escaped).
//!
//! * Origin is checked before anything else: a browser page sends one, an
//!   MCP client does not; only `://127.0.0.1` and `://localhost` pass.
//! * `/` and `/health` answer without a token.
//! * `/mcp` needs `Authorization: Bearer <agent.json token>`.
//! * POST carries JSON-RPC; the reply is always JSON, never SSE. GET is 405
//!   (no event stream), DELETE is 200.

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use copper_pagejs::render::compact_json;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::config::PATH;
use crate::server::{Body, Job, JobSender, RequestCtx};

/// The most a connection may buffer before it is refused (413).
pub const MAX_BUFFER: usize = 8_000_000;

/// What the listener needs.
#[derive(Clone, Debug)]
pub struct TransportConfig {
    /// 0 picks a free port (tests).
    pub port: u16,
    /// The bearer token; the app may rotate it while running.
    pub token: Arc<RwLock<String>>,
    pub version: String,
    pub headless: bool,
}

/// A running listener. Dropping it stops listening.
pub struct Server {
    pub port: u16,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Server {
    /// The runtime the listener runs on; model requests run here too
    /// ([`crate::Mcp::set_runtime`]).
    pub fn runtime(&self) -> tokio::runtime::Handle {
        self.runtime
            .as_ref()
            .expect("the runtime lives as long as the server")
            .handle()
            .clone()
    }

    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}{PATH}", self.port)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

/// Bind `127.0.0.1:port` and serve; every JSON-RPC POST goes to `jobs`.
pub fn start(config: TransportConfig, jobs: JobSender) -> io::Result<Server> {
    let std_listener = std::net::TcpListener::bind(("127.0.0.1", config.port))?;
    std_listener.set_nonblocking(true)?;
    let port = std_listener.local_addr()?.port();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("copper-mcp")
        .enable_all()
        .build()?;
    let listener = {
        let _guard = runtime.enter();
        TcpListener::from_std(std_listener)?
    };
    let shared = Arc::new(Shared { config, jobs, port });
    runtime.spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let shared = shared.clone();
                    tokio::spawn(async move { connection(stream, peer, shared).await });
                }
                Err(e) => {
                    tracing::warn!("mcp accept: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    });
    Ok(Server {
        port,
        runtime: Some(runtime),
    })
}

struct Shared {
    config: TransportConfig,
    jobs: JobSender,
    port: u16,
}

/// One parsed request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// Lowercased names; the last of a repeated header wins.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Take one whole request off the front of `buffer`, if one is there
/// (`HTTPRequest.take`). A malformed request line empties the buffer.
pub fn take(buffer: &mut Vec<u8>) -> Option<Request> {
    let end = buffer.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let first = lines.next()?;
    let words: Vec<&str> = first.split(' ').filter(|w| !w.is_empty()).collect();
    if words.len() < 2 {
        buffer.clear();
        return None;
    }
    let mut headers = Vec::new();
    for line in lines {
        let Some(colon) = line.find(':') else {
            continue;
        };
        let name = line[..colon].trim().to_lowercase();
        let value = line[colon + 1..].trim().to_string();
        headers.push((name, value));
    }
    let length = headers
        .iter()
        .rev()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = end + 4;
    let body_end = body_start + length;
    if buffer.len() < body_end {
        return None;
    }
    let body = buffer[body_start..body_end].to_vec();
    buffer.drain(..body_end);
    Some(Request {
        method: words[0].to_uppercase(),
        path: words[1].to_string(),
        headers,
        body,
    })
}

/// One response, as bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub extra: Vec<(String, String)>,
}

impl Response {
    pub fn empty(status: u16) -> Self {
        Response {
            status,
            body: Vec::new(),
            extra: Vec::new(),
        }
    }

    pub fn json(status: u16, value: &Value) -> Self {
        Response {
            status,
            body: compact_json(value).into_bytes(),
            extra: Vec::new(),
        }
    }

    fn error(status: u16, text: &str) -> Self {
        Response::json(status, &json!({ "error": text }))
    }

    fn rpc_error(status: u16, code: i64, message: &str) -> Self {
        Response::json(
            status,
            &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": code, "message": message } }),
        )
    }

    fn reason(status: u16) -> &'static str {
        match status {
            200 => "OK",
            202 => "Accepted",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Payload Too Large",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "OK",
        }
    }

    /// `HTTPResponse.bytes`.
    pub fn bytes(&self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\n",
            self.status,
            Response::reason(self.status)
        );
        head += "Content-Type: application/json\r\n";
        head += &format!("Content-Length: {}\r\n", self.body.len());
        head += "Cache-Control: no-store\r\n";
        for (name, value) in &self.extra {
            head += &format!("{name}: {value}\r\n");
        }
        head += "Connection: keep-alive\r\n\r\n";
        let mut out = head.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

async fn connection(mut stream: TcpStream, _peer: SocketAddr, shared: Arc<Shared>) {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 1 << 16];
    loop {
        let n = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.len() > MAX_BUFFER {
            let refused = Response::error(413, "request too large");
            let _ = stream.write_all(&refused.bytes()).await;
            return;
        }
        while let Some(request) = take(&mut buffer) {
            let close = request
                .header("connection")
                .is_some_and(|c| c.eq_ignore_ascii_case("close"));
            let response = route(&request, &shared).await;
            if stream.write_all(&response.bytes()).await.is_err() {
                return;
            }
            if close {
                let _ = stream.shutdown().await;
                return;
            }
        }
    }
}

/// Whether a request's Origin may reach the server.
pub fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(o) => {
            let o = o.to_lowercase();
            o.contains("://127.0.0.1") || o.contains("://localhost")
        }
    }
}

/// Whether the request carries the bearer token.
pub fn authorised(header: Option<&str>, token: &str) -> bool {
    let Some(header) = header else { return false };
    let mut parts = header.splitn(2, ' ');
    let (Some(scheme), Some(given)) = (parts.next(), parts.next()) else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() && given == token
}

async fn route(request: &Request, shared: &Shared) -> Response {
    if !origin_allowed(request.header("origin")) {
        return Response::error(403, "forbidden origin");
    }
    let path = request.path.split('?').next().unwrap_or(&request.path);
    if path == "/" || path == "/health" {
        return Response::json(
            200,
            &json!({
                "name": "copper",
                "version": shared.config.version,
                "mcp": PATH,
                "running": !shared.jobs.is_closed(),
                "headless": shared.config.headless,
                "port": shared.port,
            }),
        );
    }
    if path != PATH {
        return Response::error(404, "not found");
    }
    let token = shared
        .config
        .token
        .read()
        .map(|t| t.clone())
        .unwrap_or_default();
    if !authorised(request.header("authorization"), &token) {
        return Response::error(401, "bearer token required — Settings › Agents");
    }
    match request.method.as_str() {
        "POST" => post(request, shared).await,
        "GET" => Response::error(405, "no event stream — POST JSON-RPC messages here"),
        "DELETE" => Response::empty(200),
        _ => Response::error(405, "method not allowed"),
    }
}

async fn post(request: &Request, shared: &Shared) -> Response {
    let Ok(parsed) = serde_json::from_slice::<Value>(&request.body) else {
        return Response::rpc_error(400, -32700, "Parse error");
    };
    let mut ctx = RequestCtx {
        session: request.header("mcp-session-id").map(str::to_string),
        agent: request.header("x-copper-agent").map(str::to_string),
    };
    let (body, minted) = match parsed {
        Value::Object(one) => {
            // A session id for each `initialize` that comes without one.
            let minted = if one.get("method").and_then(Value::as_str) == Some("initialize")
                && ctx.session.is_none()
            {
                let sid = uuid::Uuid::new_v4().hyphenated().to_string().to_lowercase();
                ctx.session = Some(sid.clone());
                Some(sid)
            } else {
                None
            };
            (Body::One(one), minted)
        }
        Value::Array(many) => (Body::Many(many), None),
        _ => return Response::rpc_error(400, -32600, "Invalid request"),
    };
    let (reply, answer) = oneshot::channel();
    if shared.jobs.send(Job { body, ctx, reply }).is_err() {
        return Response::empty(503);
    }
    match answer.await {
        Ok(Some(value)) => {
            let mut response = Response::json(200, &value);
            if let Some(sid) = minted {
                response.extra.push(("Mcp-Session-Id".into(), sid));
            }
            response
        }
        Ok(None) => Response::empty(202),
        Err(_) => Response::empty(503),
    }
}

/// For in-process callers that already have a parsed object.
pub fn body_of(value: Value) -> Option<Body> {
    match value {
        Value::Object(one) => Some(Body::One(one)),
        Value::Array(many) => Some(Body::Many(many)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_requests() {
        let mut buf = b"POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nX-Copper-Agent: abc\r\n\r\n{}GET /health HTTP/1.1\r\n\r\n".to_vec();
        let r = take(&mut buf).unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.body, b"{}");
        assert_eq!(r.header("x-copper-agent"), Some("abc"));
        let r = take(&mut buf).unwrap();
        assert_eq!(r.path, "/health");
        assert!(buf.is_empty());
        let mut partial = b"POST /mcp HTTP/1.1\r\nContent-Length: 5\r\n\r\n{}".to_vec();
        assert!(take(&mut partial).is_none());
    }

    #[test]
    fn response_bytes() {
        let mut r = Response::json(200, &json!({"a": "/x"}));
        r.extra.push(("Mcp-Session-Id".into(), "s".into()));
        assert_eq!(
            String::from_utf8(r.bytes()).unwrap(),
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nCache-Control: no-store\r\nMcp-Session-Id: s\r\nConnection: keep-alive\r\n\r\n{\"a\":\"\\/x\"}"
        );
    }

    #[test]
    fn origins_and_tokens() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("http://localhost:3000")));
        assert!(origin_allowed(Some("HTTP://127.0.0.1")));
        assert!(!origin_allowed(Some("https://evil.test")));
        assert!(authorised(Some("Bearer t0k"), "t0k"));
        assert!(authorised(Some("bearer t0k"), "t0k"));
        assert!(!authorised(Some("Bearer t0k"), ""));
        assert!(!authorised(Some("Basic t0k"), "t0k"));
        assert!(!authorised(None, "t0k"));
    }
}
