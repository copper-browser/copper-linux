//! A minimal client for WebKitGTK's remote-inspector socket, the channel
//! WebKitWebDriver uses to drive a `WebKitAutomationSession`.
//!
//! There is no client-side C API for automation: `WebKitAutomationSession`
//! only has `get_id` / `set_application_info` / `create-web-view`. Commands
//! arrive over the inspector server that WebKit starts when the process is
//! launched with `WEBKIT_INSPECTOR_SERVER=host:port`. This module speaks that
//! protocol from inside the same process (a worker thread with a blocking
//! `TcpStream`; the server side runs on the GTK main loop).
//!
//! Wire format (WTF/wtf/glib/SocketConnection.cpp, WebKit 2.52):
//!   u32 big-endian body size | u8 flags (bit 0 = little-endian payload) |
//!   body = message name, NUL, GVariant in serialized form.
//! Driver → server: `StartAutomationSession (sa{sv})`, `Setup (tt)`,
//!   `SendMessageToBackend (tts)` (JSON `Automation.*` command).
//! Server → driver: `DidStartAutomationSession (ss)`,
//!   `SetTargetList (ta(tsssb))`, `SendMessageToFrontend (tts)`.
//! (RemoteInspectorServer.cpp, WebDriver/glib/SessionHostGlib.cpp.)

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glib::variant::{FromVariant, ToVariant};

#[derive(Debug)]
pub enum Incoming {
    DidStart {
        browser: String,
        version: String,
    },
    Targets {
        connection: u64,
        targets: Vec<(u64, String, String, bool)>,
    },
    Frontend {
        json: serde_json::Value,
    },
    Closed,
    Other(#[allow(dead_code)] String),
}

pub struct Client {
    writer: Arc<Mutex<TcpStream>>,
    rx: Receiver<Incoming>,
    connection: u64,
    target: u64,
    next_id: u64,
    /// Messages read while waiting for something else.
    backlog: Vec<Incoming>,
}

fn frame(name: &str, params: Option<glib::Variant>) -> Vec<u8> {
    let data: Vec<u8> = params.map(|v| v.data().to_vec()).unwrap_or_default();
    let body = name.len() + 1 + data.len();
    let mut out = Vec::with_capacity(5 + body);
    out.extend_from_slice(&(body as u32).to_be_bytes());
    out.push(if cfg!(target_endian = "little") { 1 } else { 0 });
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(&data);
    out
}

fn parse<T: FromVariant>(ty: &str, data: &[u8]) -> Option<T> {
    let ty = glib::VariantTy::new(ty).ok()?;
    glib::Variant::from_bytes_with_type(&glib::Bytes::from(data), ty).get::<T>()
}

fn reader(mut stream: TcpStream, tx: Sender<Incoming>) {
    loop {
        let mut head = [0u8; 5];
        if stream.read_exact(&mut head).is_err() {
            let _ = tx.send(Incoming::Closed);
            return;
        }
        let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let mut body = vec![0u8; size];
        if stream.read_exact(&mut body).is_err() {
            let _ = tx.send(Incoming::Closed);
            return;
        }
        let Some(nul) = body.iter().position(|b| *b == 0) else {
            let _ = tx.send(Incoming::Other("no name".into()));
            continue;
        };
        let name = String::from_utf8_lossy(&body[..nul]).to_string();
        let data = &body[nul + 1..];
        let msg = match name.as_str() {
            "DidStartAutomationSession" => match parse::<(String, String)>("(ss)", data) {
                Some((browser, version)) => Incoming::DidStart { browser, version },
                None => Incoming::Other(name),
            },
            "SetTargetList" => {
                match parse::<(u64, Vec<(u64, String, String, String, bool)>)>("(ta(tsssb))", data)
                {
                    Some((connection, list)) => Incoming::Targets {
                        connection,
                        targets: list
                            .into_iter()
                            .map(|(id, ty, nm, _, paired)| (id, ty, nm, paired))
                            .collect(),
                    },
                    None => Incoming::Other(name),
                }
            }
            "SendMessageToFrontend" => match parse::<(u64, u64, String)>("(tts)", data) {
                Some((_, _, json)) => Incoming::Frontend {
                    json: serde_json::from_str(&json).unwrap_or(serde_json::Value::String(json)),
                },
                None => Incoming::Other(name),
            },
            _ => Incoming::Other(name),
        };
        if tx.send(msg).is_err() {
            return;
        }
    }
}

/// Poll the channel from the GTK main loop without blocking it.
async fn next(rx: &Receiver<Incoming>, deadline: Instant) -> Option<Incoming> {
    loop {
        if let Ok(m) = rx.try_recv() {
            return Some(m);
        }
        if Instant::now() > deadline {
            return None;
        }
        glib::timeout_future(Duration::from_millis(5)).await;
    }
}

impl Client {
    /// Connect, start a session and attach to its automation target.
    pub async fn connect(addr: &str, timeout: Duration) -> Result<(Self, String), String> {
        let deadline = Instant::now() + timeout;
        let stream = loop {
            match TcpStream::connect(addr) {
                Ok(s) => break s,
                Err(e) if Instant::now() > deadline => return Err(format!("connect {addr}: {e}")),
                Err(_) => glib::timeout_future(Duration::from_millis(50)).await,
            }
        };
        stream.set_nodelay(true).ok();
        let (tx, rx) = channel();
        let read_half = stream.try_clone().map_err(|e| e.to_string())?;
        std::thread::spawn(move || reader(read_half, tx));
        let writer = Arc::new(Mutex::new(stream));
        let caps: HashMap<String, glib::Variant> = HashMap::new();
        let start = ("copper-backstage-spike".to_string(), caps).to_variant();
        writer
            .lock()
            .unwrap()
            .write_all(&frame("StartAutomationSession", Some(start)))
            .map_err(|e| e.to_string())?;

        let mut browser = String::new();
        let mut found = None;
        let mut seen = Vec::new();
        while found.is_none() {
            match next(&rx, deadline).await {
                None => return Err(format!("no automation target before timeout; saw {seen:?}")),
                Some(Incoming::DidStart {
                    browser: b,
                    version,
                }) => browser = format!("{b} {version}"),
                Some(Incoming::Targets {
                    connection,
                    targets,
                }) => {
                    seen.push(format!("{connection}:{targets:?}"));
                    if let Some(t) = targets.iter().find(|t| t.1 == "Automation") {
                        found = Some((connection, t.0));
                    }
                }
                Some(Incoming::Closed) => return Err("server closed the connection".into()),
                Some(other) => seen.push(format!("{other:?}")),
            }
        }
        let (connection, target) = found.unwrap();
        writer
            .lock()
            .unwrap()
            .write_all(&frame("Setup", Some((connection, target).to_variant())))
            .map_err(|e| e.to_string())?;
        Ok((
            Client {
                writer,
                rx,
                connection,
                target,
                next_id: 1,
                backlog: Vec::new(),
            },
            browser,
        ))
    }

    /// Send one `Automation.*` command and wait for its reply.
    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = serde_json::json!({"id": id, "method": method, "params": params}).to_string();
        let v = (self.connection, self.target, msg).to_variant();
        self.writer
            .lock()
            .unwrap()
            .write_all(&frame("SendMessageToBackend", Some(v)))
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        loop {
            match next(&self.rx, deadline).await {
                None => return Err(format!("{method}: no reply in {timeout:?}")),
                Some(Incoming::Frontend { json }) if json["id"] == id => {
                    if let Some(err) = json.get("error") {
                        return Err(format!("{method}: {err}"));
                    }
                    return Ok(json["result"].clone());
                }
                Some(Incoming::Closed) => return Err(format!("{method}: connection closed")),
                Some(other) => self.backlog.push(other),
            }
        }
    }
}
