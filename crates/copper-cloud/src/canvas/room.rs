//! One canvas room on the linked cloud: a socket to `/v1/canvases/<id>/ws`,
//! reconnected after every drop with a backoff from 1 s to 30 s (±30 %
//! jitter) until stopped (Mac: `CanvasRoom`). Frames are passed through as
//! they are — the page (or a [`super::doc::LiveCanvas`]) speaks y-websocket
//! over it.
//!
//! The server closes a room on a member it revoked or a canvas it deleted
//! (4403 / 4404): that stops the room and says so. Connections that never
//! open (2 in a row, then every 10th) ask the owner to read the canvas list
//! again.

use crate::client::CloudClient;
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// What a room says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomEvent {
    /// Connected: the server's greeting (SyncStep1, awareness) follows.
    Open,
    /// One binary frame from the server.
    Data(Vec<u8>),
    /// The connection dropped; reconnecting.
    Closed,
    /// The server said this account may no longer be here (4403 removed,
    /// 4404 deleted). The room has stopped.
    Gone(u16),
    /// Connections keep failing before they open: read the canvas list again
    /// (a canvas gone from it should stop its room).
    RefreshList,
}

#[derive(Default)]
struct Shared {
    out: Option<mpsc::UnboundedSender<Vec<u8>>>,
    open: bool,
}

/// A running room. Dropping it stops it.
pub struct CanvasRoom {
    remote_id: String,
    shared: Arc<Mutex<Shared>>,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for CanvasRoom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanvasRoom")
            .field("remote_id", &self.remote_id)
            .field("open", &self.is_open())
            .finish()
    }
}

impl Drop for CanvasRoom {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl CanvasRoom {
    /// Start the room for the server's canvas `remote_id` (a UUID or
    /// `personal`). Events come on the returned receiver.
    pub fn start(
        client: CloudClient,
        remote_id: &str,
    ) -> (CanvasRoom, mpsc::UnboundedReceiver<RoomEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let task = tokio::spawn(run(client, remote_id.to_string(), shared.clone(), tx));
        (
            CanvasRoom {
                remote_id: remote_id.to_string(),
                shared,
                task,
            },
            rx,
        )
    }

    /// The server's canvas id.
    pub fn remote_id(&self) -> &str {
        &self.remote_id
    }

    /// Connected right now.
    pub fn is_open(&self) -> bool {
        self.shared.lock().map(|s| s.open).unwrap_or(false)
    }

    /// Send one binary frame; dropped while not open (the handshake after a
    /// reconnect resyncs).
    pub fn send(&self, frame: Vec<u8>) -> bool {
        let Ok(s) = self.shared.lock() else {
            return false;
        };
        match (&s.out, s.open) {
            (Some(out), true) => out.send(frame).is_ok(),
            _ => false,
        }
    }

    /// Stop for good.
    pub fn stop(self) {
        drop(self);
    }
}

async fn run(
    client: CloudClient,
    remote: String,
    shared: Arc<Mutex<Shared>>,
    tx: mpsc::UnboundedSender<RoomEvent>,
) {
    let mut delay = 1.0_f64;
    let mut failures = 0_u32;
    let path = format!("/v1/canvases/{remote}/ws");
    loop {
        if tx.is_closed() {
            return;
        }
        if client.is_signed_in() {
            match client.socket(&path, &[]).await {
                Ok(socket) => {
                    delay = 1.0;
                    failures = 0;
                    let (mut sink, mut stream) = socket.split();
                    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
                    if let Ok(mut s) = shared.lock() {
                        s.out = Some(out_tx);
                        s.open = true;
                    }
                    let _ = tx.send(RoomEvent::Open);
                    let mut gone = None;
                    loop {
                        tokio::select! {
                            frame = out_rx.recv() => {
                                let Some(frame) = frame else { break };
                                if sink.send(Message::binary(frame)).await.is_err() { break; }
                            }
                            // The server pings every 30 s; 90 s of nothing is a dead line.
                            next = tokio::time::timeout(Duration::from_secs(90), stream.next()) => {
                                match next {
                                    Err(_) | Ok(None) | Ok(Some(Err(_))) => break,
                                    Ok(Some(Ok(Message::Binary(b)))) => {
                                        if tx.send(RoomEvent::Data(b.to_vec())).is_err() { return; }
                                    }
                                    Ok(Some(Ok(Message::Close(frame)))) => {
                                        if let Some(f) = frame {
                                            let code = u16::from(f.code);
                                            if code == 4403 || code == 4404 { gone = Some(code); }
                                        }
                                        break;
                                    }
                                    Ok(Some(Ok(_))) => {}
                                }
                            }
                        }
                        if tx.is_closed() {
                            let _ = sink
                                .send(Message::Close(Some(
                                    tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                        code: CloseCode::Normal,
                                        reason: "".into(),
                                    },
                                )))
                                .await;
                            return;
                        }
                    }
                    if let Ok(mut s) = shared.lock() {
                        s.out = None;
                        s.open = false;
                    }
                    if let Some(code) = gone {
                        let _ = tx.send(RoomEvent::Gone(code));
                        return;
                    }
                    let _ = tx.send(RoomEvent::Closed);
                }
                Err(f) => {
                    failures += 1;
                    tracing::debug!(room = %remote, error = %f.message, "room did not open");
                    if f.status == 404 || f.status == 403 {
                        // Not a member (any more) or no such canvas.
                        let _ = tx.send(RoomEvent::RefreshList);
                    } else if failures == 2 || failures.is_multiple_of(10) {
                        let _ = tx.send(RoomEvent::RefreshList);
                    }
                }
            }
        }
        let wait = delay * (0.7 + fastrand::f64() * 0.6);
        delay = (delay * 2.0).min(30.0);
        tokio::time::sleep(Duration::from_secs_f64(wait)).await;
    }
}
