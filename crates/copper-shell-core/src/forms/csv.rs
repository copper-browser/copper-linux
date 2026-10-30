//! Passwords in and out as CSV (`PasswordCSV.swift`), without the dialogs:
//! the app asks for the file (and, before an export, for a yes to
//! [`EXPORT_TITLE`]) and hands the path here.
//!
//! In: the file is read in place, never changed, checked before anything is
//! saved ([`passwords_csv::read_file`]), and its logins go to the platform's
//! store keeping when each was last used ([`super::vault::Vault::save_imported`]).
//! Out: every login with its password, read one at a time, as Chrome's
//! columns, to a file readable by this user only. Neither direction logs a
//! row, a username or a password.
//!
//! `store` is how the person reads the platform's store, mid-sentence
//! ("the keyring", "Credential Manager").

use super::rt;
use copper_creds::credential::CredentialId;
use copper_import::ImportedLogin;
use copper_import::passwords_csv::{self, Outcome};
use std::path::PathBuf;

/// What came of an import or an export, in one sentence, and whether it
/// worked.
pub type Said = (String, bool);

/// Words for a vault that isn't there (the forms lane never started).
pub const NO_VAULT: &str =
    "Copper's passwords aren't available right now — restart Copper and try again";

/// The yes asked for before an export.
pub const EXPORT_TITLE: &str = "Export every password in plain text?";
pub const EXPORT_DETAIL: &str = "The file holds every password Copper keeps, readable by anyone who opens it. Keep it somewhere safe and delete it when you're done.";
pub const EXPORT_ALLOW: &str = "Export…";
/// The export's suggested file name (`PasswordCSV.swift`).
pub const EXPORT_NAME: &str = "Copper Passwords.csv";

/// Read the CSV at `path` and keep what it holds in the platform's store.
pub async fn import_file(path: PathBuf, store: &str) -> Said {
    let read = rt::blocking(move || passwords_csv::read_file(&path))
        .await
        .unwrap_or_else(|| Err(copper_import::ImportError::Unreadable("the file".into())));
    let read = match read {
        Ok(r) => r,
        Err(e) => {
            let refused = Outcome {
                refused: Some(e.to_string()),
                ..Outcome::default()
            };
            return (refused.sentence(), false);
        }
    };
    let total = read.logins.len();
    let kept = if total == 0 {
        Ok(0)
    } else {
        let Some(v) = super::vault() else {
            return (NO_VAULT.into(), false);
        };
        let logins: Vec<(String, String, String, Option<f64>)> = read
            .logins
            .into_iter()
            .map(|l| (l.host, l.user, l.password, l.used))
            .collect();
        rt::run(async move { v.save_imported(logins).await })
            .await
            .unwrap_or_else(|| Err(NO_VAULT.into()))
    };
    if let Err(e) = &kept {
        tracing::warn!("importing passwords: {e}");
    }
    import_said(total, kept, read.skipped, read.codes, store)
}

/// What an import came to: `total` logins read, `kept` of them saved.
fn import_said(
    total: usize,
    kept: Result<usize, String>,
    skipped: usize,
    codes: usize,
    store: &str,
) -> Said {
    match kept {
        Ok(n) => {
            let outcome = Outcome {
                kept: n,
                skipped,
                codes,
                refused: None,
            };
            let mut line = outcome.sentence();
            if n < total {
                line += &format!(" — {} couldn't be kept in {store}", total - n);
            }
            (line, n > 0 || total == 0)
        }
        Err(e) => (format!("{} refused them: {e}", capital(store)), false),
    }
}

/// Every login with its password, read one at a time, as CSV at `path`
/// (`name,url,username,password,note`), readable by this user only.
pub async fn export_file(path: PathBuf, store: &str) -> Said {
    let Some(v) = super::vault() else {
        return (NO_VAULT.into(), false);
    };
    let collected = rt::run(async move {
        let logins = v.logins().await?;
        let mut out = Vec::with_capacity(logins.len());
        let mut missed = 0usize;
        for l in logins {
            let id = CredentialId::Keychain {
                host: l.host.clone(),
                user: l.user.clone(),
            };
            match v.secret(&id).await {
                Ok(password) => out.push(ImportedLogin {
                    host: l.host,
                    user: l.user,
                    password,
                    used: l.used,
                }),
                Err(_) => missed += 1,
            }
        }
        Ok::<_, String>((out, missed))
    })
    .await
    .unwrap_or_else(|| Err(NO_VAULT.into()));
    let (logins, missed) = match collected {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("exporting passwords: {e}");
            return (format!("{} couldn't be read: {e}", capital(store)), false);
        }
    };
    let n = logins.len();
    if n == 0 && missed == 0 {
        return ("No passwords to export yet".into(), false);
    }
    let text = passwords_csv::csv(&logins);
    drop(logins);
    let name = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let wrote = rt::blocking(move || write_private(&path, &text))
        .await
        .unwrap_or_else(|| Err(std::io::Error::other("the write never finished")));
    match wrote {
        Ok(()) => (export_sentence(n, missed, &name, store), true),
        Err(e) => {
            tracing::warn!("exporting passwords: writing the file failed: {e}");
            (
                format!("Couldn't write {name} — choose another folder"),
                false,
            )
        }
    }
}

/// `text` to `path`, readable by this user only where files have modes.
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        o.mode(0o600);
        // A file chosen over an existing one keeps its own mode otherwise.
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    o.open(path)?.write_all(text.as_bytes())
}

/// What an export came to (`PasswordCSV.exportSentence`), with the
/// platform's store named.
pub fn export_sentence(written: usize, refused: usize, file: &str, store: &str) -> String {
    let head = format!(
        "{written} {} exported to {file}",
        if written == 1 {
            "password"
        } else {
            "passwords"
        }
    );
    if refused == 0 {
        return head;
    }
    format!("{head} — {refused} {store} wouldn't give up")
}

/// `store` at the start of a sentence.
fn capital(store: &str) -> String {
    let mut c = store.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{export_sentence, import_said};

    #[test]
    fn export_says_what_was_written_and_refused() {
        assert_eq!(
            export_sentence(1, 0, "a.csv", "the keyring"),
            "1 password exported to a.csv"
        );
        assert_eq!(
            export_sentence(3, 2, "a.csv", "the keyring"),
            "3 passwords exported to a.csv — 2 the keyring wouldn't give up"
        );
    }

    #[test]
    fn import_says_what_was_kept() {
        assert_eq!(
            import_said(2, Ok(2), 1, 0, "the keyring"),
            (
                "2 passwords imported, 1 skipped (no site or no password)".into(),
                true
            )
        );
        assert_eq!(
            import_said(3, Ok(1), 0, 0, "the keyring"),
            (
                "1 password imported — 2 couldn't be kept in the keyring".into(),
                true
            )
        );
        assert_eq!(
            import_said(0, Ok(0), 0, 0, "the keyring"),
            ("That file has no passwords in it".into(), true)
        );
        assert_eq!(
            import_said(2, Err("locked".into()), 0, 0, "the keyring"),
            ("The keyring refused them: locked".into(), false)
        );
    }

    #[test]
    fn an_export_is_readable_by_its_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.csv");
        std::fs::write(&path, "old").unwrap();
        super::write_private(&path, "name,url\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "name,url\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
