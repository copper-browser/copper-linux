//! Canvas on Copper Cloud — the network half (Mac: `Fork/Canvas/Canvas.swift`
//! REST calls, `CanvasHost.swift` `CanvasRoom`/`CanvasWire`, the mention
//! calls of `CanvasChat.swift`, `CanvasShare.swift`).
//!
//! * [`CanvasApi`] — the REST routes: list, create, rename, delete, leave,
//!   members, invites, share links, people, state, read, ops, mentions.
//! * [`room::CanvasRoom`] — the live room socket, reconnecting, frames passed
//!   through (what the page in a tab speaks y-websocket over).
//! * [`wire`] — reading presence and updates off the wire.
//! * [`doc`] — a canvas document and live client without a page.

pub mod doc;
pub mod room;
pub mod wire;

use crate::client::{CloudClient, Feature};
use crate::error::{Failure, Result};
use serde::Deserialize;
use serde_json::{Value, json};

/// A person as the server shows one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct Person {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// A canvas as `GET /v1/canvases` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CanvasInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// `personal` or `shared`.
    #[serde(default)]
    pub kind: String,
    /// `owner` or `editor`.
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub owner: Option<Person>,
    #[serde(default)]
    pub member_count: Option<i64>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A member of a canvas.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Member {
    pub user_id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub added_at: Option<String>,
}

/// An invite (either list, or the answer to sending one).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Invite {
    pub id: String,
    #[serde(default)]
    pub canvas_id: Option<String>,
    #[serde(default)]
    pub canvas_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub invited_by: Option<Person>,
    #[serde(default)]
    pub invitee: Option<Person>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub nudged_at: Option<String>,
    #[serde(default)]
    pub nudged: Option<bool>,
}

/// A share link just made (the token is shown once).
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct NewShareLink {
    pub id: String,
    pub token: String,
    pub canvas_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl std::fmt::Debug for NewShareLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewShareLink")
            .field("id", &self.id)
            .field("canvas_id", &self.canvas_id)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

/// A share link as the owner's list shows it (no token).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ShareLink {
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub uses: i64,
}

/// What `apply_ops` answers.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OpsResult {
    pub applied: i64,
    #[serde(default)]
    pub ids: Vec<Option<String>>,
    #[serde(default)]
    pub errors: Vec<Value>,
}

/// Who an agent's ops are attributed to (`{"id","name","color"}`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Author {
    pub id: Option<String>,
    pub name: String,
    pub color: Option<String>,
}

fn path_id(id: &str) -> String {
    // `personal` or a UUID; never a path of its own.
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The canvas REST routes on the linked instance.
#[derive(Debug, Clone)]
pub struct CanvasApi {
    client: CloudClient,
}

impl CanvasApi {
    pub fn new(client: CloudClient) -> Self {
        CanvasApi { client }
    }

    pub fn client(&self) -> &CloudClient {
        &self.client
    }

    fn ready(&self) -> Result<()> {
        if !self.client.is_signed_in() {
            return Err(Failure::not_signed_in());
        }
        Ok(())
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        self.ready()?;
        self.client.request_json("GET", path, None, query).await
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Result<Vec<u8>> {
        self.ready()?;
        Ok(self.client.request(method, path, body, &[]).await?.body)
    }

    /// Mine, Personal first, then most recently updated.
    pub async fn list(&self) -> Result<Vec<CanvasInfo>> {
        self.get("/v1/canvases", &[]).await
    }

    /// A new shared canvas owned by me.
    pub async fn create(&self, name: &str) -> Result<CanvasInfo> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Failure::new(0, "name", "Give the canvas a name"));
        }
        let body = self
            .send("POST", "/v1/canvases", Some(json!({"name": name})))
            .await?;
        serde_json::from_slice(&body).map_err(|e| Failure::decode(201, "POST /v1/canvases", e))
    }

    pub async fn get_canvas(&self, id: &str) -> Result<CanvasInfo> {
        self.get(&format!("/v1/canvases/{}", path_id(id)), &[])
            .await
    }

    /// Owner only.
    pub async fn rename(&self, id: &str, name: &str) -> Result<()> {
        self.send(
            "PATCH",
            &format!("/v1/canvases/{}", path_id(id)),
            Some(json!({"name": name.trim()})),
        )
        .await
        .map(|_| ())
    }

    /// Owner only; connected sockets close with 4404.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.send("DELETE", &format!("/v1/canvases/{}", path_id(id)), None)
            .await
            .map(|_| ())
    }

    /// Leave a canvas someone else owns.
    pub async fn leave(&self, id: &str) -> Result<()> {
        let me = self
            .client
            .account()
            .ok_or_else(Failure::not_signed_in)?
            .user_id
            .to_ascii_lowercase();
        self.remove_member(id, &me).await
    }

    pub async fn members(&self, id: &str) -> Result<Vec<Member>> {
        self.get(&format!("/v1/canvases/{}/members", path_id(id)), &[])
            .await
    }

    pub async fn remove_member(&self, id: &str, user_id: &str) -> Result<()> {
        self.send(
            "DELETE",
            &format!("/v1/canvases/{}/members/{}", path_id(id), path_id(user_id)),
            None,
        )
        .await
        .map(|_| ())
    }

    /// Invite by email (a re-send if one is pending).
    pub async fn invite(&self, id: &str, email: &str) -> Result<Invite> {
        let body = self
            .send(
                "POST",
                &format!("/v1/canvases/{}/invites", path_id(id)),
                Some(json!({"email": email.trim()})),
            )
            .await?;
        serde_json::from_slice(&body).map_err(|e| Failure::decode(201, "an invite", e))
    }

    /// A canvas's pending invites.
    pub async fn invites(&self, id: &str) -> Result<Vec<Invite>> {
        self.get(&format!("/v1/canvases/{}/invites", path_id(id)), &[])
            .await
    }

    /// Withdraw one (0.5.0).
    pub async fn revoke_invite(&self, id: &str, invite_id: &str) -> Result<()> {
        let r = self
            .send(
                "DELETE",
                &format!(
                    "/v1/canvases/{}/invites/{}",
                    path_id(id),
                    path_id(invite_id)
                ),
                None,
            )
            .await;
        self.note_lacking(&r, Feature::InviteReminders);
        r.map(|_| ())
    }

    /// Invites addressed to me.
    pub async fn my_invites(&self) -> Result<Vec<Invite>> {
        self.get("/v1/invites", &[]).await
    }

    pub async fn accept_invite(&self, invite_id: &str) -> Result<CanvasInfo> {
        let body = self
            .send(
                "POST",
                &format!("/v1/invites/{}/accept", path_id(invite_id)),
                None,
            )
            .await?;
        serde_json::from_slice(&body).map_err(|e| Failure::decode(200, "the accepted canvas", e))
    }

    pub async fn decline_invite(&self, invite_id: &str) -> Result<()> {
        self.send(
            "POST",
            &format!("/v1/invites/{}/decline", path_id(invite_id)),
            None,
        )
        .await
        .map(|_| ())
    }

    fn note_lacking<T>(&self, r: &Result<T>, feature: Feature) {
        if let Err(f) = r
            && f.status == 404
            && f.code == "not_found"
            && self.client.supports(feature).is_none()
        {
            self.client.lacks(feature);
        }
    }

    /// A share link (0.3.0), editor role.
    pub async fn create_share_link(&self, id: &str) -> Result<NewShareLink> {
        let r = self
            .send(
                "POST",
                &format!("/v1/canvases/{}/links", path_id(id)),
                Some(json!({"role": "editor"})),
            )
            .await;
        self.note_lacking(&r, Feature::ShareLinks);
        serde_json::from_slice(&r?).map_err(|e| Failure::decode(201, "a share link", e))
    }

    /// The owner's list of share links.
    pub async fn share_links(&self, id: &str) -> Result<Vec<ShareLink>> {
        #[derive(Deserialize)]
        struct Links {
            links: Vec<ShareLink>,
        }
        let l: Links = self
            .get(&format!("/v1/canvases/{}/links", path_id(id)), &[])
            .await?;
        Ok(l.links)
    }

    /// Revoke one link, or every link with `None`.
    pub async fn revoke_share_links(&self, id: &str, link_id: Option<&str>) -> Result<()> {
        let path = match link_id {
            Some(l) => format!("/v1/canvases/{}/links/{}", path_id(id), path_id(l)),
            None => format!("/v1/canvases/{}/links", path_id(id)),
        };
        self.send("DELETE", &path, None).await.map(|_| ())
    }

    /// What a share link opens, without joining.
    pub async fn preview_link(&self, token: &str) -> Result<Value> {
        let token: String = token
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        self.get(&format!("/v1/canvas-links/{token}"), &[]).await
    }

    /// Join through a share link (safe to retry).
    pub async fn join_link(&self, token: &str) -> Result<CanvasInfo> {
        let token: String = token
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let body = self
            .send("POST", &format!("/v1/canvas-links/{token}/join"), None)
            .await?;
        serde_json::from_slice(&body).map_err(|e| Failure::decode(200, "the joined canvas", e))
    }

    /// Active accounts on the instance (0.3.0), filtered by `q`.
    pub async fn people(&self, q: Option<&str>, limit: Option<u32>) -> Result<Vec<Person>> {
        #[derive(Deserialize)]
        struct People {
            people: Vec<Person>,
        }
        let limit_text = limit.map(|l| l.min(50).to_string());
        let mut query = vec![];
        if let Some(q) = q.filter(|q| !q.trim().is_empty()) {
            query.push(("q", q.trim()));
        }
        if let Some(l) = &limit_text {
            query.push(("limit", l.as_str()));
        }
        let r: Result<People> = self.get("/v1/people", &query).await;
        self.note_lacking(&r, Feature::People);
        Ok(r?.people)
    }

    /// The whole document as one v1 update, or what state vector `sv` lacks.
    pub async fn state(&self, id: &str, sv: Option<&[u8]>) -> Result<Vec<u8>> {
        #[derive(Deserialize)]
        struct State {
            state: String,
        }
        let encoded = sv.map(copper_core::cloud::encode_base64);
        let mut query = vec![];
        if let Some(e) = &encoded {
            query.push(("sv", e.as_str()));
        }
        let s: State = self
            .get(&format!("/v1/canvases/{}/state", path_id(id)), &query)
            .await?;
        copper_core::cloud::decode_base64(&s.state)
            .ok_or_else(|| Failure::new(200, "decode", "The canvas state didn't read"))
    }

    /// The agent read format (`canvas_read`), with the chat's tail.
    pub async fn read(&self, id: &str, full: bool, ids: &[&str], types: &[&str]) -> Result<Value> {
        let ids = ids.join(",");
        let types = types.join(",");
        let mut query = vec![];
        if full {
            query.push(("full", "true"));
        }
        if !ids.is_empty() {
            query.push(("ids", ids.as_str()));
        }
        if !types.is_empty() {
            query.push(("types", types.as_str()));
        }
        self.get(&format!("/v1/canvases/{}/read", path_id(id)), &query)
            .await
    }

    /// Run ops on the server (`canvas_apply`): one transaction, in order; a
    /// bad op is skipped and reported.
    pub async fn apply_ops(
        &self,
        id: &str,
        ops: &[Value],
        author: Option<&Author>,
        confirm: bool,
    ) -> Result<OpsResult> {
        let mut body = json!({"ops": ops});
        if let Some(a) = author {
            let mut who = json!({"name": a.name});
            if let Some(i) = &a.id {
                who["id"] = json!(i);
            }
            if let Some(c) = &a.color {
                who["color"] = json!(c);
            }
            body["as"] = who;
        }
        if confirm {
            body["confirm"] = json!(true);
        }
        let bytes = self
            .send(
                "POST",
                &format!("/v1/canvases/{}/ops", path_id(id)),
                Some(body),
            )
            .await?;
        serde_json::from_slice(&bytes).map_err(|e| Failure::decode(200, "the ops result", e))
    }

    /// After a chat message that mentions people (0.6.0).
    pub async fn notify_mentions(
        &self,
        id: &str,
        message_id: &str,
        user_ids: &[String],
        excerpt: &str,
    ) -> Result<()> {
        let excerpt: String = excerpt.chars().take(200).collect();
        let r = self
            .send(
                "POST",
                &format!("/v1/canvases/{}/mentions", path_id(id)),
                Some(json!({"message_id": message_id, "user_ids": user_ids, "excerpt": excerpt})),
            )
            .await;
        self.note_lacking(&r, Feature::Mentions);
        r.map(|_| ())
    }

    /// Mentions of me (unread only with `unread`).
    pub async fn mentions(&self, unread: bool, limit: u32) -> Result<Vec<Value>> {
        let limit = limit.clamp(1, 100).to_string();
        let mut query = vec![("limit", limit.as_str())];
        if unread {
            query.push(("unread", "1"));
        }
        let r: Result<Value> = self.get("/v1/mentions", &query).await;
        self.note_lacking(&r, Feature::Mentions);
        Ok(r?
            .get("mentions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Mark mentions read, by id or for a whole canvas.
    pub async fn mark_mentions_read(&self, ids: &[String], canvas_id: Option<&str>) -> Result<i64> {
        let body = match canvas_id {
            Some(c) => json!({"canvas_id": path_id(c)}),
            None => json!({"ids": ids}),
        };
        let bytes = self.send("POST", "/v1/mentions/read", Some(body)).await?;
        Ok(serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v.get("updated").and_then(Value::as_i64))
            .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_never_escape_the_path() {
        assert_eq!(path_id("../../v1/admin"), "v1admin");
        assert_eq!(path_id("0192ABCD-ef"), "0192abcd-ef");
        assert_eq!(path_id("personal"), "personal");
    }

    #[test]
    fn canvas_rows_read_leniently() {
        let c: CanvasInfo = serde_json::from_str(
            r#"{"id":"x","name":"Personal","kind":"personal","role":"owner","owner":{"id":"u","email":"a@b.c","display_name":"Ann"},"member_count":1,"created_at":"t","updated_at":"t","extra":1}"#,
        )
        .unwrap();
        assert_eq!(c.owner.unwrap().display_name.as_deref(), Some("Ann"));
        let c: CanvasInfo = serde_json::from_str(r#"{"id":"y"}"#).unwrap();
        assert_eq!(c.member_count, None);
    }
}
