//! A canvas document without a page: one `yrs::Doc` (lib0 v1, the same
//! roots the page and the server use — `shapes`, `agents`, `meta`, `chat`),
//! and [`LiveCanvas`], a room client that keeps one in sync over y-websocket
//! with presence (awareness) — what the page does in a tab, for agents, tests
//! and a canvas that isn't open anywhere.
//!
//! Shapes are best changed through the server's ops
//! ([`super::CanvasApi::apply_ops`]): they arrive here as ordinary updates.
//! Chat is written here, as the page writes it.

use super::room::{CanvasRoom, RoomEvent};
use crate::client::CloudClient;
use crate::error::Failure;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use yrs::encoding::read::Cursor;
use yrs::sync::{Awareness, Message, MessageReader, SyncMessage};
use yrs::types::ToJson;
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{
    Any, Array, Doc, Map as _, Number, Out, ReadTxn, StateVector, Transact, Update, WriteTxn,
};

/// The origin of updates that came from the room: not sent back.
const REMOTE: &str = "copper-cloud-remote";
/// Chat messages kept at most, the oldest dropped first.
pub const CHAT_CAP: u32 = 1000;
/// A chat message's text at most.
pub const CHAT_TEXT_MAX: usize = 4000;

/// JSON → a Yjs value (integral numbers as integers, as the server writes).
pub fn json_to_any(v: &Value) -> Any {
    match v {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::Number(n) => n
            .as_i64()
            .map(|i| Any::Number(Number::Int(i)))
            .or_else(|| n.as_f64().map(|f| Any::Number(Number::try_i64(f))))
            .unwrap_or(Any::Null),
        Value::String(s) => Any::String(s.as_str().into()),
        Value::Array(items) => Any::from(items.iter().map(json_to_any).collect::<Vec<_>>()),
        Value::Object(map) => Any::from(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_any(v)))
                .collect::<HashMap<_, _>>(),
        ),
    }
}

fn out_to_json<T: ReadTxn>(txn: &T, out: &Out) -> Value {
    let any = match out {
        Out::Any(a) => a.clone(),
        other => other.to_json(txn),
    };
    serde_json::to_value(&any).unwrap_or(Value::Null)
}

/// One canvas document. Cheap to clone; clones share the document.
#[derive(Clone)]
pub struct CanvasDoc {
    doc: Doc,
}

impl std::fmt::Debug for CanvasDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanvasDoc")
            .field("client", &self.doc.client_id())
            .finish()
    }
}

impl Default for CanvasDoc {
    fn default() -> Self {
        CanvasDoc::new()
    }
}

impl CanvasDoc {
    pub fn new() -> Self {
        CanvasDoc { doc: Doc::new() }
    }

    /// From a whole state (`GET /v1/canvases/{id}/state`, a snapshot).
    pub fn from_state(update: &[u8]) -> Result<Self, Failure> {
        let d = CanvasDoc::new();
        d.apply(update, false)?;
        Ok(d)
    }

    /// The underlying `yrs` document.
    pub fn yrs(&self) -> &Doc {
        &self.doc
    }

    /// Apply a v1 update. `remote`: it came from the room (not sent back).
    pub fn apply(&self, update: &[u8], remote: bool) -> Result<(), Failure> {
        let bad = |e: String| {
            Failure::new(
                0,
                "canvas_update",
                format!("A canvas update didn't read: {e}"),
            )
        };
        let decoded = std::panic::catch_unwind(|| Update::decode_v1(update))
            .map_err(|_| bad("panic".into()))?
            .map_err(|e| bad(e.to_string()))?;
        let mut txn = if remote {
            self.doc.transact_mut_with(REMOTE)
        } else {
            self.doc.transact_mut()
        };
        txn.apply_update(decoded).map_err(|e| bad(e.to_string()))
    }

    /// The state vector, lib0 v1.
    pub fn state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    /// Everything a peer with state vector `sv` lacks (all of it for `None`).
    pub fn diff(&self, sv: Option<&[u8]>) -> Vec<u8> {
        let sv = sv
            .and_then(|b| StateVector::decode_v1(b).ok())
            .unwrap_or_default();
        self.doc.transact().encode_state_as_update_v1(&sv)
    }

    /// `shapes` as JSON: shape id → its props (`Y.Text` as a string).
    pub fn shapes(&self) -> Map<String, Value> {
        let txn = self.doc.transact();
        let Some(shapes) = txn.get_map("shapes") else {
            return Map::new();
        };
        shapes
            .iter(&txn)
            .map(|(k, v)| (k.to_string(), out_to_json(&txn, &v)))
            .collect()
    }

    /// `meta` as JSON (`name`, `createdBy`).
    pub fn meta(&self) -> Map<String, Value> {
        let txn = self.doc.transact();
        let Some(meta) = txn.get_map("meta") else {
            return Map::new();
        };
        meta.iter(&txn)
            .map(|(k, v)| (k.to_string(), out_to_json(&txn, &v)))
            .collect()
    }

    /// The chat, oldest first, as stored.
    pub fn chat(&self) -> Vec<Value> {
        let txn = self.doc.transact();
        let Some(chat) = txn.get_array("chat") else {
            return vec![];
        };
        chat.iter(&txn).map(|o| out_to_json(&txn, &o)).collect()
    }

    /// Add a chat message as the page does: `{id, authorId, authorName, text,
    /// mentions, at}`; the oldest are dropped past [`CHAT_CAP`]. Returns the
    /// message id.
    pub fn push_chat(
        &self,
        author_id: &str,
        author_name: &str,
        text: &str,
        mentions: &[String],
    ) -> Result<String, Failure> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Failure::new(0, "chat", "Say something first"));
        }
        let text: String = text.chars().take(CHAT_TEXT_MAX).collect();
        let id = uuid::Uuid::new_v4().to_string();
        let at = (copper_core::date::now_unix() * 1000.0).floor() as i64;
        let message = json!({
            "id": id, "authorId": author_id, "authorName": author_name, "text": text,
            "mentions": mentions, "at": at,
        });
        let mut txn = self.doc.transact_mut();
        let chat = txn.get_or_insert_array("chat");
        chat.push_back(&mut txn, json_to_any(&message));
        let len = chat.len(&txn);
        if len > CHAT_CAP {
            chat.remove_range(&mut txn, 0, len - CHAT_CAP);
        }
        Ok(id)
    }

    /// Set (or, with `None`, clear) an agent's entry in `agents`.
    pub fn set_agent(&self, id: &str, value: Option<&Value>) {
        let mut txn = self.doc.transact_mut();
        let agents = txn.get_or_insert_map("agents");
        match value {
            Some(v) => {
                agents.insert(&mut txn, id, json_to_any(v));
            }
            None => {
                agents.remove(&mut txn, id);
            }
        }
    }
}

/// Who this client is on the board (the awareness `user`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub color: String,
}

/// Another client in the room.
#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    /// Its awareness (Yjs) client id.
    pub client: u64,
    /// `user.id`, or `id`, or `client:<n>`.
    pub user: String,
    pub state: Value,
}

/// What a live canvas is doing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LiveStatus {
    pub open: bool,
    /// The first SyncStep2 since the socket opened has arrived.
    pub synced: bool,
    /// The room stopped for good (4403 removed, 4404 deleted).
    pub gone: Option<u16>,
    /// Remote updates applied so far.
    pub remote_updates: u64,
    pub peers: usize,
}

struct LiveState {
    awareness: Awareness,
    status: LiveStatus,
}

/// A canvas kept in sync with its room, with presence.
pub struct LiveCanvas {
    doc: CanvasDoc,
    me: Me,
    state: Arc<Mutex<LiveState>>,
    status: watch::Receiver<LiveStatus>,
    out: mpsc::UnboundedSender<Vec<u8>>,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for LiveCanvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveCanvas")
            .field("me", &self.me.id)
            .field("status", &*self.status.borrow())
            .finish()
    }
}

impl Drop for LiveCanvas {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn decode_messages(bytes: &[u8]) -> Vec<Message> {
    std::panic::catch_unwind(|| {
        let mut decoder = DecoderV1::new(Cursor::new(bytes));
        MessageReader::new(&mut decoder)
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default()
    })
    .unwrap_or_default()
}

fn peers_of(awareness: &Awareness) -> Vec<Peer> {
    let me = awareness.client_id();
    let mut out: Vec<Peer> = awareness
        .iter()
        .filter(|(id, _)| *id != me)
        .filter_map(|(client, state)| {
            let data = state.data?;
            let value: Value = serde_json::from_str(&data).ok()?;
            if value.is_null() {
                return None;
            }
            let user = value
                .get("user")
                .and_then(|u| u.get("id"))
                .and_then(Value::as_str)
                .or_else(|| value.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .unwrap_or_else(|| format!("client:{}", client.get()));
            Some(Peer {
                client: client.get(),
                user,
                state: value,
            })
        })
        .collect();
    out.sort_by_key(|p| p.client);
    out
}

impl LiveCanvas {
    /// Join the room of the server's canvas `remote_id` as `me`, with `doc`
    /// (a fresh one, or what is kept here).
    pub fn connect(client: CloudClient, remote_id: &str, me: Me, doc: CanvasDoc) -> LiveCanvas {
        let (room, mut events) = CanvasRoom::start(client, remote_id);
        let awareness = Awareness::new(doc.doc.clone());
        let state = Arc::new(Mutex::new(LiveState {
            awareness,
            status: LiveStatus::default(),
        }));
        let (status_tx, status_rx) = watch::channel(LiveStatus::default());
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        // Local changes go to the room as updates.
        let local = out_tx.clone();
        let _ = doc
            .doc
            .observe_update_v1("copper-cloud-live", move |txn, e| {
                let from_room = txn
                    .origin()
                    .is_some_and(|o| o.as_ref() == REMOTE.as_bytes());
                if !from_room {
                    let _ = local
                        .send(Message::Sync(SyncMessage::Update(e.update.clone())).encode_v1());
                }
            });
        let task_doc = doc.clone();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            let publish = |s: &LiveState| {
                status_tx.send_replace(s.status.clone());
            };
            loop {
                tokio::select! {
                    frame = out_rx.recv() => {
                        let Some(frame) = frame else { return };
                        room.send(frame);
                    }
                    event = events.recv() => {
                        let Some(event) = event else { return };
                        match event {
                            RoomEvent::Open => {
                                let hello = {
                                    let mut s = task_state.lock().unwrap_or_else(|e| e.into_inner());
                                    s.status.open = true;
                                    s.status.synced = false;
                                    publish(&s);
                                    let mut hello = Message::Sync(SyncMessage::SyncStep1(
                                        task_doc.doc.transact().state_vector(),
                                    ))
                                    .encode_v1();
                                    let me = s.awareness.client_id();
                                    if s.awareness.local_state_raw().is_some()
                                        && let Ok(u) = s.awareness.update_with_clients([me])
                                    {
                                        hello.extend(Message::Awareness(u).encode_v1());
                                    }
                                    hello
                                };
                                room.send(hello);
                            }
                            RoomEvent::Data(frame) => {
                                let mut replies: Vec<u8> = vec![];
                                for message in decode_messages(&frame) {
                                    match message {
                                        Message::Sync(SyncMessage::SyncStep1(sv)) => {
                                            let update = task_doc.doc.transact().encode_state_as_update_v1(&sv);
                                            replies.extend(Message::Sync(SyncMessage::SyncStep2(update)).encode_v1());
                                        }
                                        Message::Sync(SyncMessage::SyncStep2(u)) => {
                                            let ok = task_doc.apply(&u, true).is_ok();
                                            let mut s = task_state.lock().unwrap_or_else(|e| e.into_inner());
                                            s.status.synced = true;
                                            if ok { s.status.remote_updates += 1; }
                                            publish(&s);
                                        }
                                        Message::Sync(SyncMessage::Update(u)) => {
                                            if task_doc.apply(&u, true).is_ok() {
                                                let mut s = task_state.lock().unwrap_or_else(|e| e.into_inner());
                                                s.status.remote_updates += 1;
                                                publish(&s);
                                            }
                                        }
                                        Message::Awareness(update) => {
                                            let mut s = task_state.lock().unwrap_or_else(|e| e.into_inner());
                                            let _ = s.awareness.apply_update(update);
                                            s.status.peers = peers_of(&s.awareness).len();
                                            publish(&s);
                                        }
                                        Message::AwarenessQuery => {
                                            let s = task_state.lock().unwrap_or_else(|e| e.into_inner());
                                            if let Ok(u) = s.awareness.update() {
                                                replies.extend(Message::Awareness(u).encode_v1());
                                            }
                                        }
                                        Message::Auth(_) | Message::Custom(..) => {}
                                    }
                                }
                                if !replies.is_empty() {
                                    room.send(replies);
                                }
                            }
                            RoomEvent::Closed => {
                                let mut s = task_state.lock().unwrap_or_else(|e| e.into_inner());
                                s.status.open = false;
                                s.status.synced = false;
                                // Everyone else's presence went with the connection.
                                let me = s.awareness.client_id();
                                let others: Vec<yrs::ClientID> = s.awareness.iter().map(|(id, _)| id).filter(|id| *id != me).collect();
                                for id in others { s.awareness.remove_state(id); }
                                s.status.peers = 0;
                                publish(&s);
                            }
                            RoomEvent::Gone(code) => {
                                let mut s = task_state.lock().unwrap_or_else(|e| e.into_inner());
                                s.status.open = false;
                                s.status.gone = Some(code);
                                publish(&s);
                                return;
                            }
                            RoomEvent::RefreshList => {}
                        }
                    }
                }
            }
        });
        LiveCanvas {
            doc,
            me,
            state,
            status: status_rx,
            out: out_tx,
            task,
        }
    }

    /// The document (changes made to it go to the room).
    pub fn doc(&self) -> &CanvasDoc {
        &self.doc
    }

    /// Now.
    pub fn status(&self) -> LiveStatus {
        self.status.borrow().clone()
    }

    /// Every change to the status.
    pub fn watch(&self) -> watch::Receiver<LiveStatus> {
        self.status.clone()
    }

    /// Wait until `test` holds for the status, at most `limit`.
    pub async fn wait_for(&self, limit: Duration, test: impl Fn(&LiveStatus) -> bool) -> bool {
        let mut rx = self.status.clone();
        tokio::time::timeout(limit, async {
            loop {
                if test(&rx.borrow_and_update()) {
                    return true;
                }
                if rx.changed().await.is_err() {
                    return false;
                }
            }
        })
        .await
        .unwrap_or(false)
    }

    /// Wait for the first sync with the room.
    pub async fn synced(&self, limit: Duration) -> bool {
        self.wait_for(limit, |s| s.synced).await
    }

    /// Say where this client is: `{"user": {id, name, color}, …extra}` — the
    /// page's presence shape (cursor, selection, status).
    pub fn set_presence(&self, extra: Map<String, Value>) {
        let mut state = extra;
        state.insert(
            "user".into(),
            json!({"id": self.me.id, "name": self.me.name, "color": self.me.color}),
        );
        let frame = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.awareness
                .set_local_state_raw(Value::Object(state).to_string());
            let me = s.awareness.client_id();
            s.awareness
                .update_with_clients([me])
                .ok()
                .map(|u| Message::Awareness(u).encode_v1())
        };
        if let Some(frame) = frame {
            let _ = self.out.send(frame);
        }
    }

    /// Leave the room's presence (others see this client go).
    pub fn clear_presence(&self) {
        let frame = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.awareness.clean_local_state();
            let me = s.awareness.client_id();
            s.awareness
                .update_with_clients([me])
                .ok()
                .map(|u| Message::Awareness(u).encode_v1())
        };
        if let Some(frame) = frame {
            let _ = self.out.send(frame);
        }
    }

    /// The others in the room now.
    pub fn peers(&self) -> Vec<Peer> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        peers_of(&s.awareness)
    }

    /// Post a chat message as `me`.
    pub fn chat(&self, text: &str, mentions: &[String]) -> Result<String, Failure> {
        self.doc
            .push_chat(&self.me.id, &self.me.name, text, mentions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_docs_converge_through_updates() {
        let a = CanvasDoc::new();
        let b = CanvasDoc::new();
        a.push_chat("u1", "Ann", "  hello  ", &["u2".into()])
            .unwrap();
        a.set_agent(
            "agent:x",
            Some(&json!({"name": "X", "status": "idle", "cursor": null})),
        );
        b.apply(&a.diff(Some(&b.state_vector())), true).unwrap();
        let chat = b.chat();
        assert_eq!(chat.len(), 1);
        assert_eq!(chat[0]["text"], "hello");
        assert_eq!(chat[0]["authorName"], "Ann");
        assert_eq!(chat[0]["mentions"][0], "u2");
        assert!(chat[0]["at"].is_i64());
        assert!(a.push_chat("u1", "Ann", "   ", &[]).is_err());
        assert!(b.apply(b"junk", true).is_err());
        let c = CanvasDoc::from_state(&a.diff(None)).unwrap();
        assert_eq!(c.chat().len(), 1);
    }

    #[test]
    fn chat_is_capped() {
        let a = CanvasDoc::new();
        for i in 0..(CHAT_CAP + 5) {
            a.push_chat("u", "U", &format!("m{i}"), &[]).unwrap();
        }
        let chat = a.chat();
        assert_eq!(chat.len() as u32, CHAT_CAP);
        assert_eq!(chat[0]["text"], "m5");
    }
}
