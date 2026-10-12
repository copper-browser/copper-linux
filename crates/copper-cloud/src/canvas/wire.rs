//! Just enough of lib0's encoding to see what passes through a room (Mac:
//! `CanvasWire`). A y-websocket frame holds one or more messages, each a
//! varuint type and its payload: 0 sync (a varuint subtype and a
//! varuint8array), 1 awareness (a varuint8array holding a count and, per
//! client, its id, clock and state as JSON — `null` when the client has
//! gone), 2 auth, 3 awareness query.

use serde_json::{Map, Value};

/// `sync` subtypes.
pub const SYNC_STEP1: u64 = 0;
pub const SYNC_STEP2: u64 = 1;
pub const SYNC_UPDATE: u64 = 2;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn var_uint(&mut self) -> Option<u64> {
        let mut value = 0_u64;
        let mut shift = 0;
        while self.at < self.bytes.len() {
            let byte = self.bytes[self.at];
            self.at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Some(value);
            }
            shift += 7;
            if shift > 56 {
                return None;
            }
        }
        None
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = usize::try_from(self.var_uint()?).ok()?;
        if n > self.left() {
            return None;
        }
        let out = &self.bytes[self.at..self.at + n];
        self.at += n;
        Some(out)
    }
}

/// The document updates in a frame — a sync message's SyncStep2 or Update
/// payload, each a whole Yjs v1 update — and whether a SyncStep2 was among
/// them.
pub fn sync(frame: &[u8]) -> (Vec<Vec<u8>>, bool) {
    let mut r = Reader {
        bytes: frame,
        at: 0,
    };
    let mut updates = vec![];
    let mut step2 = false;
    while r.left() > 0 {
        let Some(kind) = r.var_uint() else { break };
        match kind {
            0 => {
                let Some(sub) = r.var_uint() else {
                    return (updates, step2);
                };
                let Some(payload) = r.bytes() else {
                    return (updates, step2);
                };
                if (sub == SYNC_STEP2 || sub == SYNC_UPDATE) && !payload.is_empty() {
                    updates.push(payload.to_vec());
                }
                if sub == SYNC_STEP2 {
                    step2 = true;
                }
            }
            1 => {
                if r.bytes().is_none() {
                    return (updates, step2);
                }
            }
            3 => continue,
            _ => return (updates, step2),
        }
    }
    (updates, step2)
}

/// A client's awareness state: its id, and its state (`None` when it has gone).
pub type AwarenessState = (u64, Option<Map<String, Value>>);

/// Every awareness state in a frame. `None` when the frame has no awareness
/// message.
pub fn awareness(frame: &[u8]) -> Option<Vec<AwarenessState>> {
    let mut r = Reader {
        bytes: frame,
        at: 0,
    };
    let mut out = vec![];
    let mut saw = false;
    while r.left() > 0 {
        let Some(kind) = r.var_uint() else { break };
        match kind {
            0 => {
                if r.var_uint().is_none() || r.bytes().is_none() {
                    return saw.then_some(out);
                }
            }
            1 => {
                let Some(inner) = r.bytes() else {
                    return saw.then_some(out);
                };
                saw = true;
                let mut i = Reader {
                    bytes: inner,
                    at: 0,
                };
                let Some(count) = i.var_uint().filter(|c| *c < 10_000) else {
                    continue;
                };
                for _ in 0..count {
                    let (Some(client), Some(_clock), Some(state)) =
                        (i.var_uint(), i.var_uint(), i.bytes())
                    else {
                        break;
                    };
                    let text = String::from_utf8_lossy(state);
                    if text == "null" {
                        out.push((client, None));
                    } else {
                        let parsed = serde_json::from_str::<Value>(&text)
                            .ok()
                            .and_then(|v| v.as_object().cloned())
                            .unwrap_or_default();
                        out.push((client, Some(parsed)));
                    }
                }
            }
            3 => continue,
            _ => return saw.then_some(out),
        }
    }
    saw.then_some(out)
}

fn put_var_uint(out: &mut Vec<u8>, mut n: u64) {
    while n >= 0x80 {
        out.push((n as u8 & 0x7f) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_var_uint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// `sync` message with `sub` and its payload.
pub fn sync_message(sub: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0];
    put_var_uint(&mut out, sub);
    put_bytes(&mut out, payload);
    out
}

/// An awareness message from `(client, clock, state JSON or "null")`.
pub fn awareness_message(states: &[(u64, u64, String)]) -> Vec<u8> {
    let mut inner = vec![];
    put_var_uint(&mut inner, states.len() as u64);
    for (client, clock, json) in states {
        put_var_uint(&mut inner, *client);
        put_var_uint(&mut inner, *clock);
        put_bytes(&mut inner, json.as_bytes());
    }
    let mut out = vec![1];
    put_bytes(&mut out, &inner);
    out
}

/// "Tell me everyone who is here."
pub fn awareness_query() -> Vec<u8> {
    vec![3]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut frame = sync_message(SYNC_STEP2, &[1, 2, 3]);
        frame.extend(awareness_message(&[
            (7, 1, r#"{"user":{"id":"u1","name":"Ann"}}"#.into()),
            (9, 4, "null".into()),
        ]));
        frame.extend(sync_message(SYNC_UPDATE, &[4]));
        frame.extend(sync_message(SYNC_STEP1, &[0]));
        let (updates, step2) = sync(&frame);
        assert_eq!(updates, vec![vec![1, 2, 3], vec![4]]);
        assert!(step2);
        let who = awareness(&frame).unwrap();
        assert_eq!(who.len(), 2);
        assert_eq!(who[0].0, 7);
        assert_eq!(who[0].1.as_ref().unwrap()["user"]["name"], "Ann");
        assert_eq!(who[1], (9, None));
        assert!(awareness(&sync_message(SYNC_UPDATE, &[1])).is_none());
        // Big client ids take several bytes.
        let big = awareness_message(&[(3_000_000_000, 300, "{}".into())]);
        assert_eq!(awareness(&big).unwrap()[0].0, 3_000_000_000);
        // Truncated frames read as far as they go.
        let (u, _) = sync(&frame[..4]);
        assert!(u.is_empty());
    }
}
