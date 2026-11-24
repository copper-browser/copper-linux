//! `copper bitwarden save -|note -` and the `browser_vault_save` /
//! `browser_vault_note` tools (`BitwardenSave.swift`): an agent keeps a login
//! it was given (a password from a text message, a two-step seed) or a secure
//! note (backup codes) in the Bitwarden vault Copper has open. The values
//! come in as JSON — stdin for the CLI, never argv — and never go back out:
//! the answer is `{ok, id, created, updated, name, folder}`, or
//! `{ok:false, error}` with every value the caller sent cut out of the line.
//!
//! With agent access by folder (share-all off), a new item lands in the
//! Agents folder, so `copper signin` can use it at once; with share-all,
//! where it was asked to (or no folder). A saved item changes only when it
//! is shared with agents — an agent can add to the vault, never overwrite
//! what the user kept back.

use serde_json::{Map, Value, json};

use super::{Bitwarden, BwFolder, BwItem, Result, fail};
use crate::credential::CredentialId;
use crate::site::matches;
use crate::totp;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginDraft {
    pub name: String,
    pub host: String,
    pub uri: String,
    pub username: String,
    pub password: String,
    pub totp: Option<String>,
    pub notes: Option<String>,
    pub folder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteDraft {
    pub name: String,
    pub text: String,
    pub folder: Option<String>,
}

/// Bitwarden refuses a note longer than this.
pub const NOTE_LIMIT: usize = 10_000;

impl Bitwarden {
    /// `op` save | note. `share_all`, `allowed`: agent access as Settings
    /// has it (`agent.credentials.shareAll`, `.allowed`). Never fails and
    /// never returns a value it was given.
    pub async fn vault_write(
        &mut self,
        op: &str,
        p: &Map<String, Value>,
        share_all: bool,
        allowed: &[String],
    ) -> Value {
        let mut secrets: Vec<String> = ["password", "totp", "notes", "text"]
            .iter()
            .filter_map(|k| p.get(*k).and_then(Value::as_str).map(str::to_string))
            .collect();
        secrets.extend(
            p.get("totp")
                .and_then(Value::as_str)
                .and_then(normalized_totp),
        );
        // A bw message could quote one line of a note: every line counts.
        for key in ["text", "notes"] {
            let body = p.get(key).and_then(Value::as_str).unwrap_or("");
            secrets.extend(
                body.lines()
                    .map(str::trim)
                    .filter(|l| l.chars().count() >= 4)
                    .map(str::to_string),
            );
        }
        secrets.retain(|s| !s.is_empty());
        let saved = if op == "note" {
            self.save_note(p, share_all, allowed).await
        } else {
            self.save_login(p, share_all, allowed).await
        };
        saved.unwrap_or_else(|e| json!({ "ok": false, "error": sanitize(&e.0, &secrets) }))
    }

    async fn save_login(
        &mut self,
        p: &Map<String, Value>,
        share_all: bool,
        allowed: &[String],
    ) -> Result<Value> {
        let draft = login_draft(p)?;
        let items = self.items().await?;
        self.folders().await?;
        if let Some(existing) = existing_login(&items, &draft.host, &draft.username) {
            let what = format!("a login for {} on {}", draft.username, draft.host);
            self.require_shared(existing, &what, share_all, allowed)?;
            let mut object = self.raw_item(&existing.id).await?;
            let mut login = object
                .get("login")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            login.insert("password".into(), draft.password.into());
            if let Some(totp) = draft.totp {
                login.insert("totp".into(), totp.into());
            }
            object.insert("login".into(), Value::Object(login));
            if let Some(notes) = draft.notes {
                object.insert("notes".into(), notes.into());
            }
            let id = self.write(&object, Some(&existing.id), "item").await?;
            self.refresh_cache_if_possible().await;
            return Ok(self.answer(&id, false, &existing.name, existing.folder_id.as_deref()));
        }
        let folder = self.target(draft.folder.as_deref(), share_all).await?;
        let mut object = self.item_template().await?;
        object.insert("type".into(), 1.into());
        object.insert("name".into(), draft.name.clone().into());
        object.insert("folderId".into(), json!(folder.as_ref().map(|f| &f.id)));
        object.insert("notes".into(), json!(draft.notes));
        object.insert(
            "login".into(),
            json!({
                "username": draft.username,
                "password": draft.password,
                "totp": draft.totp,
                "uris": [{"uri": draft.uri, "match": null}],
            }),
        );
        let id = self.write(&object, None, "item").await?;
        self.refresh_cache_if_possible().await;
        Ok(self.answer(
            &id,
            true,
            &draft.name,
            folder.as_ref().map(|f| f.id.as_str()),
        ))
    }

    async fn save_note(
        &mut self,
        p: &Map<String, Value>,
        share_all: bool,
        allowed: &[String],
    ) -> Result<Value> {
        let draft = note_draft(p)?;
        let items = self.items().await?;
        self.folders().await?;
        if let Some(existing) = items.iter().find(|i| i.kind == 2 && i.name == draft.name) {
            let what = format!("a note named {}", draft.name);
            self.require_shared(existing, &what, share_all, allowed)?;
            let mut object = self.raw_item(&existing.id).await?;
            object.insert("notes".into(), draft.text.into());
            let id = self.write(&object, Some(&existing.id), "item").await?;
            self.refresh_cache_if_possible().await;
            return Ok(self.answer(&id, false, &existing.name, existing.folder_id.as_deref()));
        }
        let folder = self.target(draft.folder.as_deref(), share_all).await?;
        let mut object = self.item_template().await?;
        object.insert("type".into(), 2.into());
        object.insert("name".into(), draft.name.clone().into());
        object.insert("folderId".into(), json!(folder.as_ref().map(|f| &f.id)));
        object.insert("notes".into(), draft.text.into());
        object.insert("secureNote".into(), json!({ "type": 0 }));
        let id = self.write(&object, None, "item").await?;
        self.refresh_cache_if_possible().await;
        Ok(self.answer(
            &id,
            true,
            &draft.name,
            folder.as_ref().map(|f| f.id.as_str()),
        ))
    }

    /// The folder a new item goes to: the one named, else the Agents folder
    /// while agents see only that one.
    async fn target(&mut self, named: Option<&str>, share_all: bool) -> Result<Option<BwFolder>> {
        match named {
            Some(name) => self.folder_named(name).await.map(Some),
            None if share_all => Ok(None),
            None => self.folder_named("Agents").await.map(Some),
        }
    }

    /// The folder called `name` (any case), made when there is none.
    pub async fn folder_named(&mut self, name: &str) -> Result<BwFolder> {
        let wanted = name.to_lowercase();
        if let Some(found) = self
            .folders()
            .await?
            .into_iter()
            .find(|f| f.name.to_lowercase() == wanted)
        {
            return Ok(found);
        }
        let mut object = Map::new();
        object.insert("name".into(), name.into());
        let id = self.write(&object, None, "folder").await?;
        Ok(BwFolder {
            id,
            name: name.into(),
        })
    }

    fn folder_name(&self, id: Option<&str>) -> Option<String> {
        let id = id?;
        self.folders
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.name.clone())
    }

    fn require_shared(
        &self,
        item: &BwItem,
        what: &str,
        share_all: bool,
        allowed: &[String],
    ) -> Result<()> {
        let folder = self.folder_name(item.folder_id.as_deref());
        if !agent_may_change(item, folder.as_deref(), share_all, allowed) {
            return Err(fail(format!(
                "{what} is already saved and not shared with agents — share it in Settings › Passwords › Agent access, or ask the user to change it"
            )));
        }
        Ok(())
    }

    fn answer(&self, id: &str, created: bool, name: &str, folder_id: Option<&str>) -> Value {
        json!({
            "ok": true,
            "id": id,
            "created": created,
            "updated": !created,
            "name": name,
            "folder": self.folder_name(folder_id),
        })
    }
}

// MARK: the pure parts (BitwardenSaveTests)

fn text(v: Option<&Value>) -> Option<String> {
    let t = v?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// `{name?, url | host, username, password, totp?, notes?, folder?}`. The
/// password and notes are kept as typed; everything else trimmed.
pub fn login_draft(p: &Map<String, Value>) -> Result<LoginDraft> {
    let raw = text(p.get("url"))
        .or_else(|| text(p.get("host")))
        .ok_or_else(|| fail("url (or host) is required: the site the login is for"))?;
    let (host, uri) =
        site(&raw).ok_or_else(|| fail("url must be a web address (https://example.com)"))?;
    let username = text(p.get("username")).ok_or_else(|| fail("username is required"))?;
    let password = p
        .get("password")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fail("password is required"))?
        .to_string();
    let totp = match text(p.get("totp")) {
        Some(seed) => Some(
            normalized_totp(&seed)
                .ok_or_else(|| fail("totp must be an otpauth://totp/ URI or a base32 secret"))?,
        ),
        None => None,
    };
    let notes = p
        .get("notes")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if notes
        .as_ref()
        .is_some_and(|n| n.chars().count() > NOTE_LIMIT)
    {
        return Err(fail(format!(
            "notes are longer than {NOTE_LIMIT} characters"
        )));
    }
    let www = host.strip_prefix("www.").unwrap_or(&host).to_string();
    Ok(LoginDraft {
        name: text(p.get("name")).unwrap_or(www),
        host,
        uri,
        username,
        password,
        totp,
        notes,
        folder: text(p.get("folder")),
    })
}

/// `{name, text, folder?}`; the text as typed.
pub fn note_draft(p: &Map<String, Value>) -> Result<NoteDraft> {
    let name = text(p.get("name")).ok_or_else(|| fail("name is required"))?;
    let body = p
        .get("text")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| fail("text is required"))?;
    if body.chars().count() > NOTE_LIMIT {
        return Err(fail(format!("text is longer than {NOTE_LIMIT} characters")));
    }
    Ok(NoteDraft {
        name,
        text: body.to_string(),
        folder: text(p.get("folder")),
    })
}

/// The site from a URL or a bare host: lowercased host, and
/// `scheme://host[:port]` for the item — no path, no query (a token in a
/// link never lands in the vault). `None` for anything but http(s).
pub fn site(raw: &str) -> Option<(String, String)> {
    let trimmed = raw.trim();
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let url = url::Url::parse(&candidate).ok()?;
    let scheme = url.scheme().to_lowercase();
    if !matches!(scheme.as_str(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let host = url.host_str()?.to_lowercase();
    let host = host.trim_end_matches('.').to_string();
    if !(host.contains('.') || host == "localhost") {
        return None;
    }
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let uri = format!("{scheme}://{host}{port}");
    Some((host, uri))
}

/// An otpauth URI as given, or a base32 secret without its spaces and
/// dashes, uppercased — only when a code can be made from it.
pub fn normalized_totp(raw: &str) -> Option<String> {
    let seed = raw.trim();
    if seed.to_lowercase().starts_with("otpauth://") {
        return totp::code(seed, 0.0).map(|_| seed.to_string());
    }
    let secret: String = seed
        .to_uppercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    totp::code(&secret, 0.0).map(|_| secret)
}

/// The saved login a save replaces: same site as `copper signin` matches
/// sites, same username in any case.
pub fn existing_login<'a>(items: &'a [BwItem], host: &str, username: &str) -> Option<&'a BwItem> {
    let user = username.to_lowercase();
    items.iter().find(|i| {
        i.kind == 1 && i.username.to_lowercase() == user && i.uris.iter().any(|u| matches(u, host))
    })
}

/// Whether an agent may change a saved item: shared with agents (share
/// all, the Agents folder, its own toggle) and not `copper-agent: deny`.
pub fn agent_may_change(
    item: &BwItem,
    folder: Option<&str>,
    share_all: bool,
    allowed: &[String],
) -> bool {
    let clean = |s: &str| s.trim().to_lowercase();
    if item
        .fields
        .iter()
        .any(|f| clean(&f.name) == "copper-agent" && clean(&f.value) == "deny")
    {
        return false;
    }
    let id = CredentialId::Bitwarden(item.id.clone()).to_string();
    share_all || folder.map(clean).as_deref() == Some("agents") || allowed.contains(&id)
}

/// One line for an error: every secret cut out (longest first), the first
/// line only, no control characters, at most 200 characters.
pub fn sanitize(message: &str, hiding: &[String]) -> String {
    let mut text = message.to_string();
    let mut secrets: Vec<&String> = hiding.iter().filter(|s| !s.is_empty()).collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    for secret in secrets {
        text = text.replace(secret.as_str(), "…");
    }
    let line = text
        .split(['\n', '\r'])
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let clean: String = line.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    let out = if clean.is_empty() {
        "Bitwarden command failed"
    } else {
        clean
    };
    if out.chars().count() > 200 {
        return out.chars().take(199).collect::<String>() + "…";
    }
    out.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::VaultField;
    use crate::site::Uri;

    fn m(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    fn item(id: &str, user: &str, uri: &str, kind: i64, fields: Vec<VaultField>) -> BwItem {
        BwItem {
            id: id.into(),
            kind,
            name: id.into(),
            folder_id: None,
            username: user.into(),
            uris: vec![Uri::new(uri)],
            has_totp: false,
            has_notes: false,
            fields,
        }
    }

    fn fails(r: Result<impl std::fmt::Debug>, words: &str) -> bool {
        r.is_err_and(|e| e.0.contains(words))
    }

    #[test]
    fn save_takes_url_or_host_and_keeps_only_the_site() {
        let draft = login_draft(&m(json!({
            "url": " https://www.PubAdHub.com/login?token=abc ", "username": " me@x.com ", "password": " p@ss ",
        })))
        .unwrap();
        assert_eq!(draft.host, "www.pubadhub.com");
        assert_eq!(draft.uri, "https://www.pubadhub.com");
        assert_eq!(draft.name, "pubadhub.com");
        assert_eq!(draft.username, "me@x.com");
        assert_eq!(draft.password, " p@ss "); // as typed
        assert!(draft.totp.is_none() && draft.notes.is_none() && draft.folder.is_none());

        let bare = login_draft(&m(json!({
            "host": "example.com", "username": "u", "password": "p", "name": "Mine", "folder": "Work",
        })))
        .unwrap();
        assert_eq!(bare.uri, "https://example.com");
        assert_eq!(bare.name, "Mine");
        assert_eq!(bare.folder.as_deref(), Some("Work"));
        assert_eq!(
            site("http://localhost:8080/x").map(|s| s.1).as_deref(),
            Some("http://localhost:8080")
        );
    }

    #[test]
    fn save_refuses_what_is_missing_or_wrong() {
        let d = |v: Value| login_draft(&m(v));
        assert!(fails(
            d(json!({"username": "u", "password": "p"})),
            "url (or host) is required"
        ));
        assert!(fails(
            d(json!({"url": "ftp://x.com", "username": "u", "password": "p"})),
            "web address"
        ));
        assert!(fails(
            d(json!({"url": "nodot", "username": "u", "password": "p"})),
            "web address"
        ));
        assert!(fails(
            d(json!({"url": "https://me:pw@x.com", "username": "u", "password": "p"})),
            "web address"
        ));
        assert!(fails(
            d(json!({"url": "x.com", "password": "p"})),
            "username is required"
        ));
        assert!(fails(
            d(json!({"url": "x.com", "username": "u", "password": ""})),
            "password is required"
        ));
        assert!(fails(
            d(json!({"url": "x.com", "username": "u", "password": 42})),
            "password is required"
        ));
        assert!(fails(
            d(json!({"url": "x.com", "username": "u", "password": "p", "totp": "not base32!"})),
            "totp must be"
        ));
    }

    #[test]
    fn totp_seeds_are_checked_and_cleaned() {
        assert_eq!(
            normalized_totp("jbsw y3dp ehpk 3pxp").as_deref(),
            Some("JBSWY3DPEHPK3PXP")
        );
        let uri = "otpauth://totp/X:me?secret=JBSWY3DPEHPK3PXP&issuer=X";
        assert_eq!(normalized_totp(uri).as_deref(), Some(uri));
        assert_eq!(
            normalized_totp("otpauth://hotp/X?secret=JBSWY3DPEHPK3PXP"),
            None
        );
        assert_eq!(normalized_totp("123456"), None);
        let draft = login_draft(&m(json!({
            "url": "x.com", "username": "u", "password": "p", "totp": "jbsw-y3dp-ehpk-3pxp",
        })))
        .unwrap();
        assert_eq!(draft.totp.as_deref(), Some("JBSWY3DPEHPK3PXP"));
    }

    #[test]
    fn note_needs_a_name_and_text() {
        let note = note_draft(&m(json!({
            "name": " Google backup codes ", "text": "1111 2222\n3333 4444\n",
        })))
        .unwrap();
        assert_eq!(
            note,
            NoteDraft {
                name: "Google backup codes".into(),
                text: "1111 2222\n3333 4444\n".into(),
                folder: None,
            }
        );
        assert!(fails(
            note_draft(&m(json!({"text": "x"}))),
            "name is required"
        ));
        assert!(fails(
            note_draft(&m(json!({"name": "n", "text": " \n"}))),
            "text is required"
        ));
        assert!(fails(
            note_draft(&m(json!({"name": "n", "text": "x".repeat(10_001)}))),
            "longer than"
        ));
    }

    #[test]
    fn save_finds_the_same_site_and_user_not_another() {
        let items = vec![
            item("other-user", "you@x.com", "https://pubadhub.com", 1, vec![]),
            item("note", "me@x.com", "https://pubadhub.com", 2, vec![]),
            item(
                "mine",
                "Me@X.com",
                "https://www.pubadhub.com/login",
                1,
                vec![],
            ),
            item("elsewhere", "me@x.com", "https://example.com", 1, vec![]),
        ];
        let id = |host: &str, user: &str| existing_login(&items, host, user).map(|i| i.id.clone());
        assert_eq!(id("pubadhub.com", "me@x.com").as_deref(), Some("mine"));
        assert_eq!(id("app.pubadhub.com", "me@x.com").as_deref(), Some("mine"));
        assert_eq!(id("pubadhub.com", "nobody"), None);
        assert_eq!(id("other.com", "me@x.com"), None);
    }

    #[test]
    fn agents_change_only_shared_items() {
        let plain = item("a", "u", "x.com", 1, vec![]);
        let denied = item(
            "b",
            "u",
            "x.com",
            1,
            vec![VaultField {
                name: "copper-agent".into(),
                value: " Deny ".into(),
                hidden: false,
            }],
        );
        let none: &[String] = &[];
        assert!(!agent_may_change(&plain, None, false, none));
        assert!(!agent_may_change(&plain, Some("Personal"), false, none));
        assert!(agent_may_change(&plain, Some(" agents "), false, none));
        assert!(agent_may_change(&plain, None, true, none));
        assert!(agent_may_change(&plain, None, false, &["bw:a".into()]));
        assert!(!agent_may_change(
            &denied,
            Some("Agents"),
            true,
            &["bw:b".into()]
        ));
    }

    #[test]
    fn errors_never_carry_the_values_sent() {
        let line = sanitize(
            "bad item p@ssw0rd JBSWY3DPEHPK3PXP\nsecond line",
            &["p@ssw0rd".into(), "JBSWY3DPEHPK3PXP".into()],
        );
        assert_eq!(line, "bad item … …");
        assert!(!line.contains("p@ssw0rd"));
    }
}
