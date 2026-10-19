//! Passwords in and out as CSV — the file Chrome, Edge, Brave, Firefox,
//! Safari, Apple Passwords and the password managers write (macOS:
//! `PasswordCSV.swift`, whose header spellings and sentences this keeps).
//!
//! This is the way passwords come over on every system where Copper doesn't
//! read another browser's store itself — Windows always — and the way in
//! from any password manager everywhere. The person exports the file from
//! the browser's own password page ([`export_hint`]) and picks it; nothing
//! here opens a browser's profile.
//!
//! Every outcome is one sentence that says what happened and, when nothing
//! did, the one thing to try next ([`Outcome::sentence`]).

use copper_creds::site::host_of;

use crate::{Browser, BrowserKind, ImportError, ImportedLogin};

/// The header spellings, lowercased, of every export Copper knows: Chrome's
/// `name,url,username,password,note`; Safari's and Apple Passwords'
/// `Title,URL,Username,Password,Notes,OTPAuth`; Firefox's
/// `url,username,password,httpRealm,…,timeLastUsed,…`; the password
/// managers' `login_uri,login_username,login_password,login_totp` and their
/// `Url`, `Website`, `Email`, `TOTP` and `Extra` variants.
const URL_NAMES: &[&str] = &[
    "url",
    "website",
    "web site",
    "login_uri",
    "site",
    "address",
    "login url",
    "web address",
];
const USER_NAMES: &[&str] = &[
    "username",
    "user name",
    "login_username",
    "user",
    "email",
    "email address",
    "login",
    "account",
];
const PASSWORD_NAMES: &[&str] = &["password", "login_password"];
const OTP_NAMES: &[&str] = &[
    "otpauth",
    "otp",
    "totp",
    "login_totp",
    "otpsecret",
    "one-time code",
    "verification code",
];
const NOTES_NAMES: &[&str] = &["notes", "note", "comments", "extra"];
/// Firefox: milliseconds since 1970.
const LAST_USED_NAMES: &[&str] = &["timelastused"];

/// Why a file isn't a passwords export, when nothing more precise applies.
pub const NOT_PASSWORDS: &str =
    "That isn't a passwords CSV — export one again from your browser or password manager";

/// Where a header's columns are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    pub url: usize,
    pub user: usize,
    pub password: usize,
    pub otp: Option<usize>,
    pub notes: Option<usize>,
    pub last_used: Option<usize>,
}

fn normal(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Where a header's site, username and password are — `None` when one of
/// the three is missing.
pub fn columns(header: &[String]) -> Option<Columns> {
    let names: Vec<String> = header.iter().map(|n| normal(n)).collect();
    let find = |set: &[&str]| names.iter().position(|n| set.contains(&n.as_str()));
    Some(Columns {
        url: find(URL_NAMES)?,
        user: find(USER_NAMES)?,
        password: find(PASSWORD_NAMES)?,
        otp: find(OTP_NAMES),
        notes: find(NOTES_NAMES),
        last_used: find(LAST_USED_NAMES),
    })
}

/// Every record: quoted fields, doubled quotes, newlines inside quotes, CR,
/// LF or CRLF between records, a leading byte-order mark; blank lines (and
/// lines of empty fields) dropped.
pub fn parse(text: &str) -> Vec<Vec<String>> {
    records(text, usize::MAX)
}

fn records(text: &str, limit: usize) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();

    fn end_row(rows: &mut Vec<Vec<String>>, row: &mut Vec<String>, field: &mut String) {
        row.push(std::mem::take(field));
        let row = std::mem::take(row);
        if row.iter().any(|f| !f.is_empty()) {
            rows.push(row);
        }
    }

    while let Some(c) = chars.next() {
        if rows.len() >= limit {
            return rows;
        }
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else {
            match c {
                '"' => quoted = true,
                ',' => row.push(std::mem::take(&mut field)),
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    end_row(&mut rows, &mut row, &mut field);
                }
                '\n' => end_row(&mut rows, &mut row, &mut field),
                _ => field.push(c),
            }
        }
    }
    if rows.len() < limit && !(row.is_empty() && field.is_empty()) {
        end_row(&mut rows, &mut row, &mut field);
    }
    rows
}

/// The first record of a CSV; `None` for a single column (a list of words,
/// not a table).
fn first_record(text: &str) -> Option<Vec<String>> {
    records(text, 1).into_iter().next().filter(|r| r.len() > 1)
}

/// Why a text can't be a passwords export, before anything is saved; `None`
/// when its first line names a site, a username and a password column.
pub fn problem(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some("That file is empty".into());
    }
    let Some(header) = first_record(trimmed) else {
        return Some(NOT_PASSWORDS.into());
    };
    let names: Vec<String> = header.iter().map(|n| normal(n)).collect();
    let has = |set: &[&str]| names.iter().any(|n| set.contains(&n.as_str()));
    let mut missing = Vec::new();
    if !has(URL_NAMES) {
        missing.push("website");
    }
    if !has(USER_NAMES) {
        missing.push("username");
    }
    if !has(PASSWORD_NAMES) {
        missing.push("password");
    }
    if missing.is_empty() {
        return None;
    }
    let list = match missing.as_slice() {
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
        [] => unreachable!(),
    };
    Some(format!(
        "That CSV has no {list} column — export it again as a passwords CSV"
    ))
}

/// What a file holds, before anything is kept — counts only; no value is
/// kept or logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    /// Rows with a site and a password: what will be kept.
    pub logins: usize,
    /// Rows without a site or a password.
    pub skipped: usize,
    /// Rows with a one-time-code secret, which Copper's passwords don't hold.
    pub with_codes: usize,
    /// Rows with notes, which don't move either.
    pub with_notes: usize,
    /// Whether the header was understood at all.
    pub understood: bool,
}

fn filled(row: &[String], at: Option<usize>) -> bool {
    at.and_then(|i| row.get(i))
        .is_some_and(|v| !v.trim().is_empty())
}

/// One row as a login, or `None` when it has no site or no password.
fn login(row: &[String], c: &Columns) -> Option<ImportedLogin> {
    let url = row.get(c.url)?;
    let password = row.get(c.password)?;
    let host = host_of(url);
    if host.is_empty() || password.is_empty() {
        return None;
    }
    let used = c
        .last_used
        .and_then(|i| row.get(i))
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|ms| *ms > 0.0)
        .map(|ms| ms / 1000.0);
    Some(ImportedLogin {
        host,
        user: row.get(c.user).cloned().unwrap_or_default(),
        password: password.clone(),
        used,
    })
}

/// What a file holds — counts only.
pub fn summary(text: &str) -> Summary {
    let mut rows = parse(text).into_iter();
    let mut out = Summary::default();
    let Some(c) = rows.next().as_deref().and_then(columns) else {
        return out;
    };
    out.understood = true;
    for row in rows {
        if login(&row, &c).is_none() {
            out.skipped += 1;
            continue;
        }
        out.logins += 1;
        if filled(&row, c.otp) {
            out.with_codes += 1;
        }
        if filled(&row, c.notes) {
            out.with_notes += 1;
        }
    }
    out
}

/// A file read: the logins to keep and what was left behind.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Read {
    pub logins: Vec<ImportedLogin>,
    /// Rows without a site or a password.
    pub skipped: usize,
    /// Kept logins that had a one-time-code secret (not kept).
    pub codes: usize,
}

/// Checks, then reads: every login in an export's text. A text that isn't a
/// passwords export is [`ImportError::NotPasswords`], with the sentence that
/// says why.
pub fn read(text: &str) -> Result<Read, ImportError> {
    if let Some(why) = problem(text) {
        return Err(ImportError::NotPasswords(why));
    }
    let mut rows = parse(text).into_iter();
    let c = rows
        .next()
        .as_deref()
        .and_then(columns)
        .ok_or_else(|| ImportError::NotPasswords(NOT_PASSWORDS.into()))?;
    let mut out = Read::default();
    for row in rows {
        match login(&row, &c) {
            Some(l) => {
                if filled(&row, c.otp) {
                    out.codes += 1;
                }
                out.logins.push(l);
            }
            None => out.skipped += 1,
        }
    }
    Ok(out)
}

/// Reads, checks and reads the logins of the file the person picked. The
/// file is read in place, shared with whoever else has it open (an export
/// still open in a spreadsheet).
pub fn read_file(path: &std::path::Path) -> Result<Read, ImportError> {
    let bytes = crate::copy::read_shared(path)
        .map_err(|e| crate::copy::read_error(path.display().to_string(), &e))?;
    let text = text_of(&bytes).ok_or_else(|| {
        ImportError::NotPasswords(
            "Couldn't read that file as text — choose the .csv the export made".into(),
        )
    })?;
    read(&text)
}

/// Text from a file the way exports come: UTF-8 (with or without a BOM),
/// else UTF-16 with a BOM, else Latin-1 — or `None` for something that
/// isn't text.
pub fn text_of(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return Some(String::new());
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| {
                if le {
                    u16::from_le_bytes([p[0], p[1]])
                } else {
                    u16::from_be_bytes([p[0], p[1]])
                }
            })
            .collect();
        String::from_utf16(&units).ok()
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    if bytes.contains(&0) {
        return None;
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Some(s.strip_prefix('\u{FEFF}').unwrap_or(s).to_string());
    }
    Some(bytes.iter().map(|&b| b as char).collect())
}

/// What an import came to, as the person reads it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    pub kept: usize,
    pub skipped: usize,
    /// Kept logins that had a one-time-code secret.
    pub codes: usize,
    /// Set when nothing could be read at all: why, in a sentence.
    pub refused: Option<String>,
}

impl Outcome {
    /// One sentence: what happened, or the one thing to try next.
    pub fn sentence(&self) -> String {
        if let Some(refused) = &self.refused {
            return refused.clone();
        }
        if self.kept == 0 && self.skipped == 0 {
            return "That file has no passwords in it".into();
        }
        if self.kept == 0 {
            return if self.skipped == 1 {
                "Nothing imported — its one row has no site or no password".into()
            } else {
                format!(
                    "Nothing imported — none of its {} rows has both a site and a password",
                    self.skipped
                )
            };
        }
        let mut line = if self.kept == 1 {
            "1 password imported".to_string()
        } else {
            format!("{} passwords imported", self.kept)
        };
        if self.skipped > 0 {
            line += &format!(", {} skipped (no site or no password)", self.skipped);
        }
        if self.codes > 0 {
            line += " — one-time codes aren’t kept";
        }
        line
    }
}

/// One field, quoted when it has to be.
pub fn cell(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Copper's own export: Chrome's header, then one row per login — a file any
/// of the browsers above (and Copper) takes back in.
pub fn csv(logins: &[ImportedLogin]) -> String {
    let mut out = String::from("name,url,username,password,note\n");
    for l in logins {
        let url = format!("https://{}/", l.host);
        let row = [l.host.as_str(), &url, &l.user, &l.password, ""];
        out += &row.iter().map(|v| cell(v)).collect::<Vec<_>>().join(",");
        out.push('\n');
    }
    out
}

/// Where a browser's own password export is, for the card that walks the
/// person through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportHint {
    /// The page to open in that browser (paste into its address bar).
    pub page: &'static str,
    /// What to do there, as one sentence.
    pub steps: &'static str,
}

/// The export page for each browser in the family.
pub fn export_hint(kind: BrowserKind) -> ExportHint {
    const CHROMIUM_STEPS: &str =
        "Open this page in the browser, choose Export passwords, then pick the file here";
    match kind {
        BrowserKind::Edge => ExportHint {
            page: "edge://wallet/passwords",
            steps: "Open this page in Edge, choose … › Export passwords, then pick the file here",
        },
        BrowserKind::Brave => ExportHint {
            page: "brave://password-manager/settings",
            steps: CHROMIUM_STEPS,
        },
        BrowserKind::Vivaldi => ExportHint {
            page: "vivaldi://password-manager/settings",
            steps: CHROMIUM_STEPS,
        },
        BrowserKind::Chrome | BrowserKind::Chromium | BrowserKind::Arc => ExportHint {
            page: "chrome://password-manager/settings",
            steps: CHROMIUM_STEPS,
        },
    }
}

/// How a browser's passwords come over on this system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordsVia {
    /// Read from the profile ([`crate::passwords`], Linux).
    Profile,
    /// Through the browser's own CSV export: the person exports, then picks
    /// the file ([`read`]).
    Export(ExportHint),
}

/// Linux reads the profile; every other system goes through the export.
pub fn passwords_via(browser: &Browser) -> PasswordsVia {
    if cfg!(target_os = "linux") {
        PasswordsVia::Profile
    } else {
        PasswordsVia::Export(export_hint(browser.kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Vec<Vec<String>> {
        parse(text)
    }

    #[test]
    fn parses_quotes_newlines_bom_and_line_endings() {
        let text =
            "\u{FEFF}a,b,c\r\n\"x, y\",\"say \"\"hi\"\"\",\"two\nlines\"\r\n\n,,\nlast,row,here";
        assert_eq!(
            rows(text),
            vec![
                vec!["a", "b", "c"],
                vec!["x, y", "say \"hi\"", "two\nlines"],
                vec!["last", "row", "here"],
            ]
        );
        assert_eq!(rows("a,b\rc,d\r"), vec![vec!["a", "b"], vec!["c", "d"]]);
    }

    #[test]
    fn chrome_export() {
        let text = "name,url,username,password,note\n\
            example.com,https://www.Example.com/login,ann,hunter2,\n\
            nosite,,bob,pw,\n\
            nopass,https://a.test/,carl,,\n\
            \"b.test\",https://b.test/,\"d,an\",\"p\"\"w\",hello\n";
        let read = read(text).unwrap();
        assert_eq!(read.skipped, 2);
        assert_eq!(read.codes, 0);
        assert_eq!(read.logins.len(), 2);
        assert_eq!(read.logins[0].host, "example.com");
        assert_eq!(read.logins[0].user, "ann");
        assert_eq!(read.logins[0].password, "hunter2");
        assert_eq!(read.logins[1].user, "d,an");
        assert_eq!(read.logins[1].password, "p\"w");
        let s = summary(text);
        assert_eq!(
            (s.logins, s.skipped, s.with_notes, s.understood),
            (2, 2, 1, true)
        );
    }

    #[test]
    fn firefox_export_keeps_last_used() {
        let text = "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"\n\
            \"https://accounts.test\",\"ann@x.test\",\"s3cret\",,\"https://accounts.test\",\"{1}\",\"1700000000000\",\"1700000500000\",\"1700000000000\"\n";
        let read = read(text).unwrap();
        assert_eq!(read.logins.len(), 1);
        assert_eq!(read.logins[0].host, "accounts.test");
        assert_eq!(read.logins[0].used, Some(1_700_000_500.0));
    }

    #[test]
    fn safari_and_bitwarden_exports() {
        let safari = "Title,URL,Username,Password,Notes,OTPAuth\n\
            Site,https://s.test/,ann,pw,,otpauth://totp/x?secret=ABC\n";
        let r = read(safari).unwrap();
        assert_eq!((r.logins.len(), r.codes), (1, 1));
        let bw = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
            ,,login,Site,,,0,https://bw.test,ann,pw,\n\
            ,,note,Secure note,text,,0,,,,\n";
        let r = read(bw).unwrap();
        assert_eq!((r.logins.len(), r.skipped), (1, 1));
        assert_eq!(r.logins[0].host, "bw.test");
    }

    #[test]
    fn refuses_what_is_not_a_passwords_export() {
        assert_eq!(problem("  \n"), Some("That file is empty".into()));
        assert_eq!(problem("just words\n"), Some(NOT_PASSWORDS.into()));
        assert_eq!(
            problem("name,url,note\n"),
            Some(
                "That CSV has no username or password column — export it again as a passwords CSV"
                    .into()
            )
        );
        assert_eq!(
            problem("a,b\n"),
            Some("That CSV has no website, username or password column — export it again as a passwords CSV".into())
        );
        assert!(matches!(
            read("a,b\n1,2\n"),
            Err(ImportError::NotPasswords(_))
        ));
    }

    #[test]
    fn sentences() {
        let o = |kept, skipped, codes| Outcome {
            kept,
            skipped,
            codes,
            refused: None,
        };
        assert_eq!(o(0, 0, 0).sentence(), "That file has no passwords in it");
        assert_eq!(
            o(0, 1, 0).sentence(),
            "Nothing imported — its one row has no site or no password"
        );
        assert_eq!(o(1, 0, 0).sentence(), "1 password imported");
        assert_eq!(
            o(3, 2, 1).sentence(),
            "3 passwords imported, 2 skipped (no site or no password) — one-time codes aren’t kept"
        );
    }

    #[test]
    fn decodes_the_ways_exports_come() {
        assert_eq!(text_of(b"\xEF\xBB\xBFurl,a").as_deref(), Some("url,a"));
        let le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("url,é".encode_utf16().flat_map(|u| u.to_le_bytes()))
            .collect();
        assert_eq!(text_of(&le).as_deref(), Some("url,é"));
        let be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("x".encode_utf16().flat_map(|u| u.to_be_bytes()))
            .collect();
        assert_eq!(text_of(&be).as_deref(), Some("x"));
        assert_eq!(text_of(b"caf\xE9").as_deref(), Some("café"));
        assert_eq!(text_of(b"\x00\x01binary"), None);
    }

    #[test]
    fn reads_a_picked_file() {
        let dir = std::env::temp_dir().join(format!("pw-csv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Chrome Passwords.csv");
        std::fs::write(
            &file,
            b"\xEF\xBB\xBFname,url,username,password,note\r\nx,https://x.test/,ann,pw,\r\n",
        )
        .unwrap();
        let r = read_file(&file).unwrap();
        assert_eq!(r.logins.len(), 1);
        std::fs::write(&file, b"\x00\x01\x02").unwrap();
        assert!(matches!(
            read_file(&file),
            Err(ImportError::NotPasswords(_))
        ));
        assert!(matches!(
            read_file(&dir.join("missing.csv")),
            Err(ImportError::Unreadable(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_round_trips() {
        let logins = vec![ImportedLogin {
            host: "a.test".into(),
            user: "ann, \"the\" user".into(),
            password: "p,w\nx".into(),
            used: None,
        }];
        let back = read(&csv(&logins)).unwrap();
        assert_eq!(back.logins, logins);
    }

    #[test]
    fn every_browser_has_an_export_page() {
        for kind in [
            BrowserKind::Chrome,
            BrowserKind::Chromium,
            BrowserKind::Brave,
            BrowserKind::Edge,
            BrowserKind::Vivaldi,
            BrowserKind::Arc,
        ] {
            let hint = export_hint(kind);
            assert!(hint.page.contains("://"));
            assert!(!hint.steps.is_empty());
        }
        assert_eq!(
            export_hint(BrowserKind::Edge).page,
            "edge://wallet/passwords"
        );
    }
}
