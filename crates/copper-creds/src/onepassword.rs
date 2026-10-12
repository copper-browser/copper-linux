//! 1Password through its official CLI, `op` 2.x (`OnePassword.swift`,
//! `OnePasswordCLI.swift`, `OnePasswordItems.swift`).
//!
//! Three ways in: the 1Password app (`op signin` asks the app; nothing secret
//! passes through Copper), the account password (`op signin --raw`, password
//! on stdin, the session token handed to later calls in `OP_SESSION_<user>`),
//! or a service account (`OP_SERVICE_ACCOUNT_TOKEN=ops_…`). Secrets go to `op`
//! through the environment or stdin, never argv; items are read into memory
//! only and wiped on lock. Tokens kept across launches sit in 0600 files.
//!
//! Where `op` is: `COPPER_OP_PATH` (`none` = absent), `~/.local/bin/op`,
//! then `$PATH`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::credential::{
    AgentHint, AutofillCard, AutofillIdentity, Credential, CredentialId, Source, VaultField,
};
use crate::process::{self, RunError};
use crate::site::{Uri, matches, scheme_for, uri_host};
use crate::totp;

/// What went wrong, in the terms the card speaks (`OnePasswordCLI.Problem`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    NotInstalled,
    NotSignedIn,
    SessionExpired,
    IntegrationOff,
    UnknownAccount,
    Network,
    Dismissed,
    WrongPassword,
    BadToken,
    Timeout,
    Passkey,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct OpError {
    pub problem: Problem,
    pub message: String,
}

impl OpError {
    fn new(problem: Problem, message: impl Into<String>) -> Self {
        OpError {
            problem,
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, OpError>;

/// How Copper is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    App,
    Password,
    Service,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::App => "app",
            Mode::Password => "password",
            Mode::Service => "service",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "app" => Some(Mode::App),
            "password" => Some(Mode::Password),
            "service" => Some(Mode::Service),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpState {
    Missing,
    SignedOut,
    Locked,
    Unlocked,
}

/// An account `op` knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub url: String,
    pub email: String,
    pub user_id: String,
    pub account_id: String,
    pub shorthand: String,
}

impl Account {
    /// The sign-in address without its scheme.
    pub fn host(&self) -> String {
        address(&self.url)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OpVault {
    pub id: String,
    pub name: String,
}

/// A stripped item; secret values never live here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpItem {
    pub id: String,
    pub vault_id: String,
    pub vault_name: String,
    /// LOGIN, PASSWORD, CREDIT_CARD, IDENTITY…
    pub category: String,
    pub title: String,
    pub username: String,
    pub urls: Vec<String>,
    pub tags: Vec<String>,
    pub version: i64,
    pub has_totp: bool,
    pub fields: Vec<VaultField>,
    /// The full item has been read, not only the list row.
    pub detailed: bool,
}

impl OpItem {
    /// What the sign-in picker lists: logins, and password items with a site.
    pub fn is_login(&self) -> bool {
        self.category == "LOGIN" || (self.category == "PASSWORD" && !self.urls.is_empty())
    }
}

#[derive(Debug, Default, Clone)]
struct Secret {
    password: Option<String>,
    totp: Option<String>,
    hidden: HashMap<String, String>,
}

/// The categories Copper reads; everything else stays in 1Password.
pub const CATEGORIES: &str = "Login,Password,Credit Card,Identity";
pub const DEFAULT_ADDRESS: &str = "my.1password.com";
const BATCH: usize = 25;

/// How to reach `op`.
#[derive(Debug, Clone)]
pub struct OpConfig {
    /// `op` itself; `None` = [`executable`].
    pub path: Option<PathBuf>,
    /// Copper's 1Password folder: the 0600 `session` and `service` files.
    pub folder: PathBuf,
    /// `OP_CONFIG_DIR` for the child (`None` = op's own default).
    pub config_dir: Option<PathBuf>,
    /// Extra environment for every child (tests: `OP_MOCK_STATE`).
    pub env: Vec<(String, String)>,
}

impl OpConfig {
    pub fn in_data_dir(data_dir: &Path) -> Self {
        OpConfig {
            path: None,
            folder: data_dir.join("1password"),
            config_dir: None,
            env: Vec::new(),
        }
    }
}

/// Where `op` lives.
pub fn locate(
    override_path: Option<&str>,
    home: Option<&Path>,
    path_var: Option<&str>,
) -> Option<PathBuf> {
    if let Some(raw) = override_path.map(str::trim).filter(|r| !r.is_empty()) {
        if raw == "none" {
            return None;
        }
        let path = process::expand_tilde(raw, home);
        if process::is_executable(&path) {
            return Some(path);
        }
    }
    if let Some(home) = home {
        let local = home.join(".local/bin/op");
        if process::is_executable(&local) {
            return Some(local);
        }
    }
    process::in_path("op", path_var)
}

/// `locate` with this process's environment.
pub fn executable() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    locate(
        std::env::var("COPPER_OP_PATH").ok().as_deref(),
        home.as_deref(),
        std::env::var("PATH").ok().as_deref(),
    )
}

const DROPPED: [&str; 10] = [
    "OP_SERVICE_ACCOUNT_TOKEN",
    "OP_ACCOUNT",
    "OP_SECRET_KEY",
    "OP_FORMAT",
    "OP_CONNECT_HOST",
    "OP_CONNECT_TOKEN",
    "OP_CONFIG_DIR",
    "OP_DEBUG",
    "OP_INCLUDE_ARCHIVE",
    "OP_BIOMETRIC_UNLOCK_ENABLED",
];

/// The variable `op` reads a session token from for `user_id`.
pub fn session_variable(user_id: &str) -> String {
    let cleaned: String = user_id
        .to_uppercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    format!("OP_SESSION_{cleaned}")
}

/// The line meant for people out of `op`'s stderr, stamp removed.
pub fn message(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Using configuration at"))
        .collect();
    let line = lines
        .iter()
        .rev()
        .find(|l| l.contains("[ERROR]"))
        .or(lines.last())
        .copied()
        .unwrap_or("");
    strip(line)
}

fn strip(line: &str) -> String {
    let mut value = line.to_string();
    if let Some(at) = value.find("[ERROR]") {
        value = value[at + "[ERROR]".len()..].trim().to_string();
        let parts: Vec<&str> = value.splitn(3, ' ').filter(|p| !p.is_empty()).collect();
        if parts.len() == 3 && parts[0].contains('/') && parts[1].contains(':') {
            value = parts[2].to_string();
        }
    }
    value.chars().take(240).collect()
}

/// What kind of trouble a line of `op` output is.
pub fn classify(raw: &str, app_path: bool) -> Problem {
    let text = raw.to_lowercase();
    let has = |n: &str| text.contains(n);
    let integration = [
        "integrate with 1password cli",
        "connecting to desktop app",
        "app integration",
        "cannot connect to 1password app",
        "1password app is not running",
        "desktop app",
    ];
    if integration.iter().any(|n| has(n)) {
        return Problem::IntegrationOff;
    }
    if app_path
        && (has("enter the password for")
            || has("no accounts configured")
            || has("operation not supported by device"))
    {
        return Problem::IntegrationOff;
    }
    if has("dismissed") || has("authorization denied") || has("authorization timeout") {
        return Problem::Dismissed;
    }
    if has("session expired") || has("invalid session") || has("session token") {
        return Problem::SessionExpired;
    }
    if has("decodesacredentials")
        || has("service account token")
        || has("invalid token")
        || has("token is invalid")
    {
        return Problem::BadToken;
    }
    if has("no account found")
        || has("found no account")
        || has("isn't known")
        || has("no accounts configured")
    {
        return Problem::UnknownAccount;
    }
    if has("not currently signed in")
        || has("not signed in")
        || has("no active session")
        || has("authentication required")
    {
        return Problem::NotSignedIn;
    }
    if has("couldn't sign in")
        || has("unauthorized")
        || has("incorrect")
        || has("check your sign-in details")
        || has("wrong password")
    {
        return Problem::WrongPassword;
    }
    let network = [
        "dial tcp",
        "no such host",
        "network",
        "connection refused",
        "i/o timeout",
        "couldn't connect",
        "could not connect",
        "tls handshake",
        "offline",
    ];
    if network.iter().any(|n| has(n)) {
        return Problem::Network;
    }
    if has("operation not supported by device") || has("enter the password for") {
        return Problem::NotSignedIn;
    }
    Problem::Other
}

/// The sentence the card shows.
pub fn explain(problem: Problem, raw: &str) -> String {
    match problem {
        Problem::NotInstalled => "The 1Password CLI is not installed".into(),
        Problem::NotSignedIn => "Not signed in to 1Password".into(),
        Problem::SessionExpired => "The 1Password session ended — unlock again".into(),
        Problem::IntegrationOff => {
            "The 1Password app didn't answer — turn on Settings › Developer › Integrate with 1Password CLI".into()
        }
        Problem::UnknownAccount => "1Password doesn't know that account on this computer".into(),
        Problem::Network => "Couldn't reach 1Password — check the connection".into(),
        Problem::Dismissed => "The 1Password approval was dismissed".into(),
        Problem::WrongPassword => "1Password didn't accept those sign-in details".into(),
        Problem::BadToken => "That service account token wasn't accepted".into(),
        Problem::Timeout => "1Password took too long to answer — try again".into(),
        Problem::Passkey => raw.into(),
        Problem::Other => {
            if raw.is_empty() {
                "1Password command failed".into()
            } else {
                raw.into()
            }
        }
    }
}

/// A whole JSON array, or nothing at all.
pub fn is_array(data: &[u8]) -> bool {
    if String::from_utf8_lossy(data).trim().is_empty() {
        return true;
    }
    matches!(serde_json::from_slice::<Value>(data), Ok(Value::Array(_)))
}

/// `op item get -` writes objects back to back; accept that, an array, or
/// one object, splitting by depth (a stray `}` never hides later objects).
pub fn objects(data: &[u8]) -> Vec<Map<String, Value>> {
    match serde_json::from_slice::<Value>(data) {
        Ok(Value::Array(list)) => {
            return list
                .into_iter()
                .filter_map(|v| match v {
                    Value::Object(o) => Some(o),
                    _ => None,
                })
                .collect();
        }
        Ok(Value::Object(o)) => return vec![o],
        _ => {}
    }
    let mut out = Vec::new();
    let (mut depth, mut start, mut in_string, mut escaped) = (0usize, None, false, false);
    for (i, &b) in data.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0
                    && let Some(from) = start.take()
                    && let Ok(Value::Object(o)) = serde_json::from_slice(&data[from..=i])
                {
                    out.push(o);
                }
            }
            _ => {}
        }
    }
    out
}

/// A sign-in address as `op account add --address` takes it.
pub fn address(raw: &str) -> String {
    let mut value = raw.trim().to_lowercase();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = value.strip_prefix(scheme) {
            value = rest.to_string();
        }
    }
    value.trim_end_matches('/').to_string()
}

fn string(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn token_from(data: &[u8]) -> String {
    let text = String::from_utf8_lossy(data).trim().to_string();
    if let (Some(open), Some(close)) = (text.find('"'), text.rfind('"'))
        && open < close
    {
        return text[open + 1..close].to_string();
    }
    text
}

// ------------------------------------------------------------- items

/// A list row (`op item list`).
pub fn summary(object: &Map<String, Value>) -> Option<OpItem> {
    let id = object.get("id")?.as_str().filter(|s| !s.is_empty())?;
    let vault = object.get("vault").and_then(Value::as_object);
    let category = string(object.get("category")).to_uppercase();
    let username = if category == "LOGIN" {
        string(object.get("additional_information"))
    } else {
        String::new()
    };
    let mut raw: Vec<&Map<String, Value>> = object
        .get("urls")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();
    // The primary website first.
    raw.sort_by_key(|u| !u.get("primary").and_then(Value::as_bool).unwrap_or(false));
    let urls = raw
        .iter()
        .filter_map(|u| u.get("href").and_then(Value::as_str))
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .collect();
    Some(OpItem {
        id: id.into(),
        vault_id: vault.map(|v| string(v.get("id"))).unwrap_or_default(),
        vault_name: vault.map(|v| string(v.get("name"))).unwrap_or_default(),
        category,
        title: string(object.get("title")),
        username,
        urls,
        tags: object
            .get("tags")
            .and_then(Value::as_array)
            .map(|t| {
                t.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        version: object.get("version").and_then(Value::as_i64).unwrap_or(0),
        has_totp: false,
        fields: Vec::new(),
        detailed: false,
    })
}

const LOGIN_BUILT_INS: &[&str] = &["username", "password", "notesplain"];
const CARD_BUILT_INS: &[&str] = &[
    "cardholder",
    "type",
    "ccnum",
    "cvv",
    "expiry",
    "validfrom",
    "bank",
    "phonelocal",
    "phonetollfree",
    "phoneintl",
    "website",
    "pin",
    "creditlimit",
    "cashlimit",
    "interest",
    "issuenumber",
    "notesplain",
];
const IDENTITY_BUILT_INS: &[&str] = &[
    "firstname",
    "initial",
    "lastname",
    "gender",
    "birthdate",
    "occupation",
    "company",
    "department",
    "jobtitle",
    "address",
    "defphone",
    "homephone",
    "cellphone",
    "busphone",
    "username",
    "reminderq",
    "remindera",
    "email",
    "website",
    "icq",
    "skype",
    "jabber",
    "aim",
    "yahoo",
    "msn",
    "notesplain",
];

struct Detail {
    item: OpItem,
    secret: Secret,
    identity: Option<AutofillIdentity>,
    card: Option<AutofillCard>,
}

fn detail(object: &Map<String, Value>) -> Option<Detail> {
    let mut item = summary(object)?;
    let raw_fields: Vec<&Map<String, Value>> = object
        .get("fields")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();
    let mut secret = Secret::default();
    let mut fields = Vec::new();
    let mut username = item.username.clone();
    let mut has_totp = false;
    let built_ins = match item.category.as_str() {
        "CREDIT_CARD" => CARD_BUILT_INS,
        "IDENTITY" => IDENTITY_BUILT_INS,
        _ => LOGIN_BUILT_INS,
    };
    for raw in &raw_fields {
        let lower_id = string(raw.get("id")).to_lowercase();
        let kind = string(raw.get("type")).to_uppercase();
        let purpose = string(raw.get("purpose")).to_uppercase();
        let label = string(raw.get("label")).trim().to_string();
        let value = string(raw.get("value"));
        if kind == "OTP" {
            has_totp = true;
            if secret.totp.is_none() && !value.is_empty() {
                secret.totp = Some(value);
            }
            continue;
        }
        if purpose == "USERNAME" {
            if !value.is_empty() {
                username = value;
            }
            continue;
        }
        if purpose == "PASSWORD" || (item.category == "PASSWORD" && lower_id == "password") {
            if !value.is_empty() {
                secret.password = Some(value);
            }
            continue;
        }
        if purpose == "NOTES" || built_ins.contains(&lower_id.as_str()) || label.is_empty() {
            continue;
        }
        if kind == "CONCEALED" {
            secret.hidden.insert(label.clone(), value);
            fields.push(VaultField {
                name: label,
                value: String::new(),
                hidden: true,
            });
        } else if [
            "STRING",
            "EMAIL",
            "URL",
            "PHONE",
            "MENU",
            "DATE",
            "MONTH_YEAR",
            "",
        ]
        .contains(&kind.as_str())
        {
            fields.push(VaultField {
                name: label,
                value,
                hidden: false,
            });
        }
    }
    item.username = username;
    item.has_totp = has_totp;
    item.fields = fields;
    item.detailed = true;
    let identity = (item.category == "IDENTITY").then(|| make_identity(&item, &raw_fields));
    let card = (item.category == "CREDIT_CARD").then(|| make_card(&item, &raw_fields));
    Some(Detail {
        item,
        secret,
        identity,
        card,
    })
}

struct Lookup(HashMap<String, String>);

impl Lookup {
    fn new(fields: &[&Map<String, Value>]) -> Self {
        let mut by = HashMap::new();
        for raw in fields {
            let value = string(raw.get("value"));
            if value.is_empty() {
                continue;
            }
            for key in ["id", "label"] {
                if let Some(k) = raw.get(key).and_then(Value::as_str) {
                    by.entry(k.to_lowercase()).or_insert_with(|| value.clone());
                }
            }
        }
        Lookup(by)
    }

    fn get(&self, keys: &[&str]) -> String {
        keys.iter()
            .find_map(|k| self.0.get(*k).cloned())
            .unwrap_or_default()
    }
}

fn make_identity(item: &OpItem, fields: &[&Map<String, Value>]) -> AutofillIdentity {
    let get = Lookup::new(fields);
    let (mut street, mut city, mut region, mut zip, mut country) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    if let Some(raw) = fields.iter().find(|f| {
        string(f.get("type")).eq_ignore_ascii_case("ADDRESS") || string(f.get("id")) == "address"
    }) {
        let parts = raw
            .get("address")
            .and_then(Value::as_object)
            .or_else(|| raw.get("value").and_then(Value::as_object));
        if let Some(p) = parts {
            street = string(p.get("street"));
            city = string(p.get("city"));
            region = string(p.get("state"));
            zip = string(p.get("zip"));
            country = string(p.get("country"));
        } else {
            let text = string(raw.get("value"));
            let pieces: Vec<String> = text.split(',').map(|s| s.trim().to_string()).collect();
            let n = pieces.len();
            if n >= 5 {
                country = pieces[n - 1].clone();
                zip = pieces[n - 2].clone();
                region = pieces[n - 3].clone();
                city = pieces[n - 4].clone();
                street = pieces[..n - 4].join(", ");
            } else if !text.is_empty() {
                street = pieces.join(", ");
            }
        }
    }
    let phone = [
        get.get(&["defphone", "phone"]),
        get.get(&["cellphone", "mobile"]),
        get.get(&["homephone"]),
        get.get(&["busphone"]),
    ]
    .into_iter()
    .find(|p| !p.is_empty())
    .unwrap_or_default();
    let two = |s: String| {
        if s.chars().count() == 2 {
            s.to_uppercase()
        } else {
            s
        }
    };
    AutofillIdentity {
        id: format!("op:{}", item.id),
        name: item.title.clone(),
        first_name: get.get(&["firstname", "first name"]),
        middle_name: get.get(&["initial", "middle name"]),
        last_name: get.get(&["lastname", "last name"]),
        username: get.get(&["username"]),
        company: get.get(&["company"]),
        email: get.get(&["email"]),
        phone,
        address1: street,
        city,
        state: two(region),
        postal_code: zip,
        country: two(country),
        ..Default::default()
    }
}

fn make_card(item: &OpItem, fields: &[&Map<String, Value>]) -> AutofillCard {
    let get = Lookup::new(fields);
    let (month, year) = expiry(&get.get(&["expiry", "expiry date"]));
    AutofillCard {
        id: format!("op:{}", item.id),
        name: item.title.clone(),
        cardholder_name: get.get(&["cardholder", "cardholder name"]),
        brand: brand(&get.get(&["type"])),
        exp_month: month,
        exp_year: year,
        number: get.get(&["ccnum", "number"]),
        code: get.get(&["cvv", "verification number"]),
    }
}

/// `YYYYMM` (op) or `MM/YYYY`, `MM/YY`, `YYYY/MM` (people) → (month, year).
pub fn expiry(raw: &str) -> (String, String) {
    let value = raw.trim();
    let digits: String = value.chars().filter(char::is_ascii_digit).collect();
    let num = |s: &str| s.parse::<i64>().unwrap_or(0).to_string();
    if !value.contains('/') && digits.len() == 6 {
        return (num(&digits[4..]), digits[..4].to_string());
    }
    let parts: Vec<&str> = value.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() != 2 {
        return (String::new(), String::new());
    }
    if parts[0].len() == 4 {
        return (num(parts[1]), parts[0].into());
    }
    let year = if parts[1].len() == 2 {
        format!("20{}", parts[1])
    } else {
        parts[1].into()
    };
    (num(parts[0]), year)
}

pub fn brand(raw: &str) -> String {
    match raw.to_lowercase().as_str() {
        "visa" => "Visa".into(),
        "mc" | "mastercard" => "Mastercard".into(),
        "amex" | "american express" => "American Express".into(),
        "discover" => "Discover".into(),
        "jcb" => "JCB".into(),
        "diners" | "dinersclub" | "diners club" => "Diners Club".into(),
        "unionpay" => "UnionPay".into(),
        _ => {
            let mut c = raw.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    }
}

/// An item holding a passkey is left alone: op's templates would drop it.
pub fn holds_passkey(object: &Map<String, Value>) -> bool {
    if object.contains_key("passkey") || object.contains_key("passkeys") {
        return true;
    }
    object
        .get("fields")
        .and_then(Value::as_array)
        .is_some_and(|f| {
            f.iter()
                .any(|x| string(x.get("type")).to_uppercase().contains("PASSKEY"))
        })
}

fn caseless_sort<T>(v: &mut [T], key: impl Fn(&T) -> String) {
    v.sort_by_key(|a| key(a).to_lowercase());
}

// ------------------------------------------------------------- the client

/// 1Password's connection, state and in-memory cache.
pub struct OnePassword {
    cfg: OpConfig,
    pub state: OpState,
    mode: Option<Mode>,
    account: Option<Account>,
    accounts: Vec<Account>,
    session_token: Option<String>,
    service_token: Option<String>,
    stay_unlocked: bool,
    vaults: Vec<OpVault>,
    writable: Vec<OpVault>,
    save_vault_id: Option<String>,
    items: Vec<OpItem>,
    secrets: HashMap<String, Secret>,
    identities: HashMap<String, AutofillIdentity>,
    cards: HashMap<String, AutofillCard>,
    problem: Option<OpError>,
    cache_version: u64,
}

impl std::fmt::Debug for OnePassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnePassword")
            .field("state", &self.state)
            .field("mode", &self.mode)
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

/// What the app remembers between launches (never a token).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remembered {
    pub mode: Option<Mode>,
    pub account: Option<String>,
    pub email: Option<String>,
    pub url: Option<String>,
}

impl OnePassword {
    pub fn new(cfg: OpConfig, stay_unlocked: bool) -> Self {
        let installed = cfg.path.clone().or_else(executable).is_some();
        OnePassword {
            cfg,
            state: if installed {
                OpState::SignedOut
            } else {
                OpState::Missing
            },
            mode: None,
            account: None,
            accounts: Vec::new(),
            session_token: None,
            service_token: None,
            stay_unlocked,
            vaults: Vec::new(),
            writable: Vec::new(),
            save_vault_id: None,
            items: Vec::new(),
            secrets: HashMap::new(),
            identities: HashMap::new(),
            cards: HashMap::new(),
            problem: None,
            cache_version: 0,
        }
    }

    fn exe(&self) -> Option<PathBuf> {
        self.cfg.path.clone().or_else(executable)
    }

    pub fn installed(&self) -> bool {
        self.exe().is_some()
    }

    pub fn is_unlocked(&self) -> bool {
        self.state == OpState::Unlocked
    }

    pub fn mode(&self) -> Option<Mode> {
        self.mode
    }

    pub fn account(&self) -> Option<&Account> {
        self.account.as_ref()
    }

    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    pub fn problem(&self) -> Option<&OpError> {
        self.problem.as_ref()
    }

    pub fn cache_version(&self) -> u64 {
        self.cache_version
    }

    pub fn remembered(&self) -> Remembered {
        Remembered {
            mode: self.mode,
            account: self.account.as_ref().map(|a| a.user_id.clone()),
            email: self.account.as_ref().map(|a| a.email.clone()),
            url: self.account.as_ref().map(|a| a.url.clone()),
        }
    }

    fn session_file(&self) -> PathBuf {
        self.cfg.folder.join("session")
    }

    fn service_file(&self) -> PathBuf {
        self.cfg.folder.join("service")
    }

    /// The vaults read, and those new items may go in.
    pub fn vaults(&self) -> (&[OpVault], &[OpVault]) {
        (&self.vaults, &self.writable)
    }

    /// The vault new logins are created in: the chosen one, else Private,
    /// Personal or Employee, else the first writable.
    pub fn save_vault(&self) -> Option<&OpVault> {
        if let Some(id) = &self.save_vault_id
            && let Some(v) = self.writable.iter().find(|v| &v.id == id)
        {
            return Some(v);
        }
        for name in ["Private", "Personal", "Employee"] {
            if let Some(v) = self
                .writable
                .iter()
                .find(|v| v.name.eq_ignore_ascii_case(name))
            {
                return Some(v);
            }
        }
        self.writable.first()
    }

    pub fn set_save_vault(&mut self, id: Option<String>) {
        self.save_vault_id = id;
    }

    /// Start from what last launch remembered; a kept session or service
    /// token is re-read from its 0600 file and confirmed.
    pub async fn start(&mut self, remembered: Remembered) {
        if !self.installed() {
            self.state = OpState::Missing;
            return;
        }
        self.refresh_accounts().await;
        self.mode = remembered.mode;
        if let Some(id) = remembered.account {
            self.account = self
                .accounts
                .iter()
                .find(|a| a.user_id == id)
                .cloned()
                .or(Some(Account {
                    url: remembered.url.unwrap_or_default(),
                    email: remembered.email.unwrap_or_default(),
                    user_id: id,
                    account_id: String::new(),
                    shorthand: String::new(),
                }));
        }
        let Some(mode) = self.mode else {
            self.state = OpState::SignedOut;
            return;
        };
        self.state = OpState::Locked;
        match mode {
            Mode::Password if self.stay_unlocked => {
                self.session_token = process::read_private(&self.session_file());
                if self.session_token.is_none() {
                    return;
                }
            }
            Mode::Password => return,
            Mode::Service => {
                self.service_token = process::read_private(&self.service_file());
                if self.service_token.is_none() {
                    return;
                }
            }
            // The app's authorization may still be live; a whoami says.
            Mode::App => {}
        }
        if self.confirm().await.is_ok() {
            self.opened();
            let _ = self.reload(false).await;
        } else {
            self.session_token = None;
            self.state = OpState::Locked;
        }
    }

    async fn run(
        &mut self,
        args: &[&str],
        stdin: Option<&[u8]>,
        extra: &[(String, String)],
        auth: bool,
        app_path: bool,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let Some(exe) = self.exe() else {
            return Err(OpError::new(
                Problem::NotInstalled,
                explain(Problem::NotInstalled, ""),
            ));
        };
        let mut arguments: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let mut env: BTreeMap<String, String> = process::scrubbed_env(&DROPPED, &["OP_SESSION"]);
        if let Some(dir) = &self.cfg.config_dir {
            env.insert("OP_CONFIG_DIR".into(), dir.to_string_lossy().into_owned());
        }
        env.insert("OP_FORMAT".into(), "json".into());
        env.insert("NO_COLOR".into(), "1".into());
        for (k, v) in &self.cfg.env {
            env.insert(k.clone(), v.clone());
        }
        if auth {
            match self.mode {
                Some(Mode::Password) => {
                    if let (Some(token), Some(acct)) = (&self.session_token, &self.account) {
                        env.insert(session_variable(&acct.user_id), token.clone());
                    }
                    if let Some(acct) = &self.account {
                        arguments.extend(["--account".into(), acct.user_id.clone()]);
                    }
                }
                Some(Mode::App) => {
                    if let Some(acct) = &self.account {
                        arguments.extend(["--account".into(), acct.user_id.clone()]);
                    }
                }
                Some(Mode::Service) => {
                    if let Some(token) = &self.service_token {
                        env.insert("OP_SERVICE_ACCOUNT_TOKEN".into(), token.clone());
                    }
                }
                None => {}
            }
        }
        for (k, v) in extra {
            env.insert(k.clone(), v.clone());
        }
        tracing::debug!(
            command = args.iter().take(2).copied().collect::<Vec<_>>().join(" "),
            "op"
        );
        let out = match process::execute(&exe, &arguments, &env, stdin, timeout).await {
            Ok(o) => o,
            Err(RunError::TimedOut) => {
                return Err(OpError::new(
                    Problem::Timeout,
                    explain(Problem::Timeout, ""),
                ));
            }
            Err(RunError::Spawn(e)) => {
                return Err(OpError::new(
                    Problem::Other,
                    format!("Couldn't start op: {e}"),
                ));
            }
        };
        if out.status != 0 {
            let raw = message(&out.stderr);
            let problem = classify(&raw, app_path);
            let failure = OpError::new(problem, explain(problem, &raw));
            if auth
                && self.is_unlocked()
                && matches!(problem, Problem::NotSignedIn | Problem::SessionExpired)
            {
                self.expire();
            }
            return Err(failure);
        }
        Ok(out.stdout)
    }

    /// The session ended on 1Password's side: locked, cache gone.
    fn expire(&mut self) {
        self.wipe();
        self.session_token = None;
        let _ = std::fs::remove_file(self.session_file());
        self.state = OpState::Locked;
        self.problem = Some(OpError::new(
            Problem::SessionExpired,
            explain(Problem::SessionExpired, ""),
        ));
    }

    fn wipe(&mut self) {
        self.items.clear();
        self.secrets.clear();
        self.identities.clear();
        self.cards.clear();
        self.vaults.clear();
        self.writable.clear();
        self.cache_version += 1;
    }

    /// `op --version`.
    pub async fn version(&mut self) -> Option<String> {
        let data = self
            .run(
                &["--version"],
                None,
                &[],
                false,
                false,
                Duration::from_secs(10),
            )
            .await
            .ok()?;
        let text = process::trim_newlines(&data).trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    /// `op account list`.
    pub async fn refresh_accounts(&mut self) {
        let Ok(data) = self
            .run(
                &["account", "list", "--format", "json"],
                None,
                &[],
                false,
                false,
                Duration::from_secs(15),
            )
            .await
        else {
            return;
        };
        self.accounts = objects(&data)
            .iter()
            .filter_map(|o| {
                let id = o.get("user_uuid")?.as_str().filter(|s| !s.is_empty())?;
                Some(Account {
                    url: string(o.get("url")),
                    email: string(o.get("email")),
                    user_id: id.into(),
                    account_id: string(o.get("account_uuid")),
                    shorthand: string(o.get("shorthand")),
                })
            })
            .collect();
    }

    async fn confirm(&mut self) -> Result<Account> {
        let data = self
            .run(
                &["whoami", "--format", "json"],
                None,
                &[],
                true,
                false,
                Duration::from_secs(20),
            )
            .await?;
        let object = objects(&data).into_iter().next().ok_or_else(|| {
            OpError::new(Problem::Other, "1Password returned an unreadable account")
        })?;
        let previous = self.account.clone();
        let who = Account {
            url: object
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| previous.as_ref().map(|a| a.url.clone()))
                .unwrap_or_default(),
            email: string(object.get("email")),
            user_id: object
                .get("user_uuid")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| previous.as_ref().map(|a| a.user_id.clone()))
                .unwrap_or_default(),
            account_id: string(object.get("account_uuid")),
            shorthand: previous.map(|a| a.shorthand).unwrap_or_default(),
        };
        if self.mode == Some(Mode::Service) {
            let kind = string(object.get("user_type")).to_uppercase();
            if !kind.is_empty() && !kind.contains("SERVICE") {
                return Err(OpError::new(
                    Problem::BadToken,
                    explain(Problem::BadToken, ""),
                ));
            }
        }
        self.account = Some(who.clone());
        Ok(who)
    }

    fn opened(&mut self) {
        self.state = OpState::Unlocked;
        self.problem = None;
    }

    fn find_account(&self, filter: &str) -> Option<Account> {
        self.accounts
            .iter()
            .find(|a| a.user_id == filter || a.shorthand == filter || a.email == filter)
            .cloned()
    }

    fn default_filter(&self, chosen: Option<&str>) -> Option<String> {
        chosen
            .map(str::to_string)
            .or_else(|| self.account.as_ref().map(|a| a.user_id.clone()))
            .or_else(|| (self.accounts.len() == 1).then(|| self.accounts[0].user_id.clone()))
    }

    fn remember_problem<T>(&mut self, r: Result<T>) -> Result<T> {
        match &r {
            Ok(_) => self.problem = None,
            Err(e) => self.problem = Some(e.clone()),
        }
        r
    }

    /// Through the 1Password app: its own approval, nothing secret here.
    pub async fn unlock_with_app(&mut self, chosen: Option<&str>) -> Result<()> {
        let r = self.unlock_with_app_inner(chosen).await;
        self.remember_problem(r)
    }

    async fn unlock_with_app_inner(&mut self, chosen: Option<&str>) -> Result<()> {
        let filter = self.default_filter(chosen);
        let mut args = vec!["signin".to_string()];
        if let Some(f) = &filter {
            args.extend(["--account".into(), f.clone()]);
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs, None, &[], false, true, Duration::from_secs(120))
            .await?;
        let previous = (self.mode, self.account.clone());
        self.mode = Some(Mode::App);
        self.session_token = None;
        self.service_token = None;
        if let Some(f) = &filter
            && let Some(a) = self.find_account(f)
        {
            self.account = Some(a);
        }
        if let Err(e) = self.confirm().await {
            (self.mode, self.account) = previous;
            return Err(e);
        }
        self.opened();
        self.reload(false).await
    }

    /// An account `op` knows, password on stdin. With the app integration
    /// on, `op` asks the app and returns no token: the app path after all.
    pub async fn sign_in(&mut self, password: &str, chosen: Option<&str>) -> Result<()> {
        let r = self.sign_in_inner(password, chosen).await;
        self.remember_problem(r)
    }

    async fn sign_in_inner(&mut self, password: &str, chosen: Option<&str>) -> Result<()> {
        if password.is_empty() {
            return Err(OpError::new(
                Problem::WrongPassword,
                "Enter the account password",
            ));
        }
        let filter = self.default_filter(chosen).ok_or_else(|| {
            OpError::new(Problem::UnknownAccount, "Choose a 1Password account first")
        })?;
        let input = format!("{password}\n");
        let data = self
            .run(
                &["signin", "--raw", "--account", &filter],
                Some(input.as_bytes()),
                &[],
                false,
                false,
                Duration::from_secs(60),
            )
            .await?;
        self.adopt(token_from(&data), &filter).await
    }

    /// A new account here: address, email, Secret Key (through
    /// `OP_SECRET_KEY`) and password (stdin).
    pub async fn add_account(
        &mut self,
        address_raw: &str,
        email: &str,
        secret_key: &str,
        password: &str,
    ) -> Result<()> {
        let r = self
            .add_account_inner(address_raw, email, secret_key, password)
            .await;
        self.remember_problem(r)
    }

    async fn add_account_inner(
        &mut self,
        address_raw: &str,
        email: &str,
        secret_key: &str,
        password: &str,
    ) -> Result<()> {
        let host = address(if address_raw.is_empty() {
            DEFAULT_ADDRESS
        } else {
            address_raw
        });
        let (who, key) = (email.trim(), secret_key.trim());
        if who.is_empty() || key.is_empty() || password.is_empty() {
            return Err(OpError::new(
                Problem::WrongPassword,
                "Email, Secret Key and password are all needed",
            ));
        }
        let input = format!("{password}\n");
        let data = self
            .run(
                &[
                    "account",
                    "add",
                    "--address",
                    &host,
                    "--email",
                    who,
                    "--signin",
                    "--raw",
                ],
                Some(input.as_bytes()),
                &[("OP_SECRET_KEY".into(), key.into())],
                false,
                false,
                Duration::from_secs(90),
            )
            .await?;
        self.refresh_accounts().await;
        let added = self
            .accounts
            .iter()
            .find(|a| a.email.eq_ignore_ascii_case(who) && a.host() == host)
            .or_else(|| {
                self.accounts
                    .iter()
                    .find(|a| a.email.eq_ignore_ascii_case(who))
            })
            .cloned()
            .ok_or_else(|| {
                OpError::new(
                    Problem::UnknownAccount,
                    "1Password added the account but didn't list it",
                )
            })?;
        self.adopt(token_from(&data), &added.user_id).await
    }

    /// A service account token (`ops_…`), kept in a 0600 file.
    pub async fn connect(&mut self, raw_token: &str) -> Result<()> {
        let r = self.connect_inner(raw_token).await;
        self.remember_problem(r)
    }

    async fn connect_inner(&mut self, raw_token: &str) -> Result<()> {
        let token = raw_token.trim();
        if !token.starts_with("ops_") || token.chars().count() <= 10 {
            return Err(OpError::new(
                Problem::BadToken,
                "A service account token starts with ops_",
            ));
        }
        let previous = (self.mode, self.account.clone(), self.service_token.clone());
        self.mode = Some(Mode::Service);
        self.service_token = Some(token.into());
        self.session_token = None;
        self.account = None;
        if let Err(e) = self.confirm().await {
            (self.mode, self.account, self.service_token) = previous;
            return Err(e);
        }
        if let Err(e) = process::write_private(&self.service_file(), token) {
            tracing::warn!("1password: could not keep the service token file: {e}");
        }
        self.opened();
        self.reload(false).await
    }

    /// Unlock the way this machine was connected.
    pub async fn unlock(&mut self, password: Option<&str>) -> Result<()> {
        match self.mode {
            None | Some(Mode::App) => self.unlock_with_app(None).await,
            Some(Mode::Password) => {
                let acct = self.account.as_ref().map(|a| a.user_id.clone());
                self.sign_in(password.unwrap_or(""), acct.as_deref()).await
            }
            Some(Mode::Service) => {
                let token = self
                    .service_token
                    .clone()
                    .or_else(|| process::read_private(&self.service_file()))
                    .ok_or_else(|| {
                        OpError::new(
                            Problem::BadToken,
                            "The service account token is gone — connect it again",
                        )
                    })?;
                self.connect(&token).await
            }
        }
    }

    async fn adopt(&mut self, token: String, filter: &str) -> Result<()> {
        let previous = (self.mode, self.account.clone(), self.session_token.clone());
        self.account = Some(self.find_account(filter).unwrap_or(Account {
            url: String::new(),
            email: String::new(),
            user_id: filter.into(),
            account_id: String::new(),
            shorthand: String::new(),
        }));
        self.service_token = None;
        if token.is_empty() {
            self.mode = Some(Mode::App);
            self.session_token = None;
        } else {
            self.mode = Some(Mode::Password);
            self.session_token = Some(token);
        }
        if let Err(e) = self.confirm().await {
            (self.mode, self.account, self.session_token) = previous;
            return Err(e);
        }
        if self.stay_unlocked
            && self.mode == Some(Mode::Password)
            && let Some(t) = &self.session_token
            && let Err(e) = process::write_private(&self.session_file(), t)
        {
            tracing::warn!("1password: could not keep the session file: {e}");
        }
        self.opened();
        self.reload(false).await
    }

    /// Close the vault; `op signout` ends the session on 1Password's side.
    pub async fn lock(&mut self) {
        let ending = (self.mode, self.session_token.clone(), self.account.clone());
        self.wipe();
        self.session_token = None;
        self.service_token = None;
        let _ = std::fs::remove_file(self.session_file());
        self.state = if self.mode.is_none() {
            if self.installed() {
                OpState::SignedOut
            } else {
                OpState::Missing
            }
        } else {
            OpState::Locked
        };
        if !matches!(ending.0, Some(Mode::Password) | Some(Mode::App)) {
            return;
        }
        let mut args = vec!["signout".to_string()];
        let mut env = Vec::new();
        if let Some(acct) = &ending.2 {
            args.extend(["--account".into(), acct.user_id.clone()]);
            if ending.0 == Some(Mode::Password)
                && let Some(t) = ending.1
            {
                env.push((session_variable(&acct.user_id), t));
            }
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let _ = self
            .run(&refs, None, &env, false, false, Duration::from_secs(10))
            .await;
    }

    /// Forget Copper's connection (the account stays in op's own config).
    pub async fn sign_out(&mut self) {
        self.lock().await;
        self.mode = None;
        self.account = None;
        let _ = std::fs::remove_file(self.service_file());
        self.state = if self.installed() {
            OpState::SignedOut
        } else {
            OpState::Missing
        };
    }

    /// One `op whoami`; a refusal locks (in `run`), network trouble doesn't.
    pub async fn keep_alive(&mut self) {
        if self.is_unlocked() {
            let _ = self.confirm().await;
        }
    }

    /// Sync now.
    pub async fn sync(&mut self) -> Result<()> {
        if !self.is_unlocked() {
            return Err(OpError::new(Problem::NotSignedIn, "1Password is locked"));
        }
        let r = self.reload(true).await;
        if self.is_unlocked()
            && let Err(e) = &r
        {
            self.problem = Some(e.clone());
        }
        r
    }

    /// Vaults, then every login/card/identity's list row, then details for
    /// rows new or changed since the last read (`item get -` in batches).
    pub async fn reload(&mut self, force: bool) -> Result<()> {
        if !self.is_unlocked() {
            return Ok(());
        }
        let t = Duration::from_secs(60);
        let vault_data = self
            .run(
                &["vault", "list", "--format", "json"],
                None,
                &[],
                true,
                false,
                t,
            )
            .await?;
        let list_data = self
            .run(
                &[
                    "item",
                    "list",
                    "--categories",
                    CATEGORIES,
                    "--format",
                    "json",
                ],
                None,
                &[],
                true,
                false,
                t,
            )
            .await?;
        let writable_ids: Option<HashSet<String>> = self
            .run(
                &[
                    "vault",
                    "list",
                    "--permission",
                    "create_items",
                    "--format",
                    "json",
                ],
                None,
                &[],
                true,
                false,
                Duration::from_secs(20),
            )
            .await
            .ok()
            .map(|d| {
                objects(&d)
                    .iter()
                    .filter_map(|o| o.get("id").and_then(Value::as_str).map(str::to_string))
                    .collect()
            });
        if !is_array(&vault_data) || !is_array(&list_data) {
            let failure = OpError::new(
                Problem::Other,
                "1Password sent a list Copper couldn't read — nothing changed; try Sync now again",
            );
            self.problem = Some(failure.clone());
            return Err(failure);
        }
        let readable: Vec<OpVault> = objects(&vault_data)
            .iter()
            .filter_map(|o| {
                Some(OpVault {
                    id: o.get("id")?.as_str()?.into(),
                    name: string(o.get("name")),
                })
            })
            .collect();
        self.writable = match &writable_ids {
            Some(ids) => readable
                .iter()
                .filter(|v| ids.contains(&v.id))
                .cloned()
                .collect(),
            None => readable.clone(),
        };
        self.vaults = readable;
        let names: HashMap<String, String> = self
            .vaults
            .iter()
            .map(|v| (v.id.clone(), v.name.clone()))
            .collect();
        let previous: HashMap<String, OpItem> = self
            .items
            .iter()
            .map(|i| (i.id.clone(), i.clone()))
            .collect();
        let mut fresh = Vec::new();
        let mut stale = Vec::new();
        for object in objects(&list_data) {
            let Some(mut item) = summary(&object) else {
                continue;
            };
            if item.vault_name.is_empty()
                && let Some(n) = names.get(&item.vault_id)
            {
                item.vault_name = n.clone();
            }
            match previous.get(&item.id) {
                Some(kept) if !force && kept.detailed && kept.version == item.version => {
                    fresh.push(kept.clone())
                }
                _ => {
                    stale.push(item.clone());
                    fresh.push(item);
                }
            }
        }
        let alive: HashSet<String> = fresh.iter().map(|i| i.id.clone()).collect();
        self.secrets.retain(|k, _| alive.contains(k));
        self.identities.retain(|k, _| alive.contains(k));
        self.cards.retain(|k, _| alive.contains(k));
        caseless_sort(&mut fresh, |i| i.title.clone());
        self.items = fresh;
        self.problem = None;
        self.cache_version += 1;
        for batch in stale.chunks(BATCH) {
            let rows: Vec<Value> = batch
                .iter()
                .map(|i| json!({"id": i.id, "vault": {"id": i.vault_id}}))
                .collect();
            let input = serde_json::to_vec(&rows).unwrap_or_default();
            if let Ok(data) = self
                .run(
                    &["item", "get", "-", "--reveal", "--format", "json"],
                    Some(&input),
                    &[],
                    true,
                    false,
                    Duration::from_secs(90),
                )
                .await
            {
                self.ingest(objects(&data));
            }
        }
        Ok(())
    }

    fn ingest(&mut self, objects: Vec<Map<String, Value>>) {
        if objects.is_empty() {
            return;
        }
        let mut by: HashMap<String, OpItem> =
            self.items.drain(..).map(|i| (i.id.clone(), i)).collect();
        for object in &objects {
            let Some(d) = detail(object) else { continue };
            let mut item = d.item;
            if item.vault_name.is_empty()
                && let Some(known) = by.get(&item.id)
            {
                if item.vault_id.is_empty() {
                    item.vault_id = known.vault_id.clone();
                }
                item.vault_name = known.vault_name.clone();
                if item.urls.is_empty() {
                    item.urls = known.urls.clone();
                }
                if item.tags.is_empty() {
                    item.tags = known.tags.clone();
                }
            }
            let id = item.id.clone();
            by.insert(id.clone(), item);
            self.secrets.insert(id.clone(), d.secret);
            if let Some(i) = d.identity {
                self.identities.insert(id.clone(), i);
            }
            if let Some(c) = d.card {
                self.cards.insert(id, c);
            }
        }
        let mut items: Vec<OpItem> = by.into_values().collect();
        caseless_sort(&mut items, |i| i.title.clone());
        self.items = items;
        self.cache_version += 1;
    }

    async fn ensure_detail(&mut self, id: &str, force: bool) -> Result<OpItem> {
        if !self.is_unlocked() {
            return Err(OpError::new(Problem::NotSignedIn, "1Password is locked"));
        }
        let item = self
            .items
            .iter()
            .find(|i| i.id == id)
            .cloned()
            .ok_or_else(|| OpError::new(Problem::Other, "That item isn't in 1Password any more"))?;
        if item.detailed && !force {
            return Ok(item);
        }
        let mut args = vec!["item", "get", id, "--reveal", "--format", "json"];
        if !item.vault_id.is_empty() {
            args.extend(["--vault", item.vault_id.as_str()]);
        }
        let data = self
            .run(&args, None, &[], true, false, Duration::from_secs(30))
            .await?;
        self.ingest(objects(&data));
        Ok(self
            .items
            .iter()
            .find(|i| i.id == id)
            .cloned()
            .unwrap_or(item))
    }

    pub fn items(&self) -> &[OpItem] {
        &self.items
    }

    pub fn identities(&self) -> Vec<AutofillIdentity> {
        let mut v: Vec<_> = self.identities.values().cloned().collect();
        caseless_sort(&mut v, |i| i.name.clone());
        v
    }

    pub fn cards(&self) -> Vec<AutofillCard> {
        let mut v: Vec<_> = self.cards.values().cloned().collect();
        caseless_sort(&mut v, |c| c.name.clone());
        v
    }

    pub async fn password(&mut self, id: &str) -> Result<String> {
        if self
            .secrets
            .get(id)
            .and_then(|s| s.password.as_ref())
            .is_none()
        {
            self.ensure_detail(id, false).await?;
        }
        self.secrets
            .get(id)
            .and_then(|s| s.password.clone())
            .filter(|p| !p.is_empty())
            .ok_or_else(|| OpError::new(Problem::Other, "That 1Password item has no password"))
    }

    pub async fn totp(&mut self, id: &str) -> Result<String> {
        let item = self.ensure_detail(id, false).await?;
        if let Some(code) = self
            .secrets
            .get(id)
            .and_then(|s| s.totp.as_deref())
            .and_then(totp::code_now)
        {
            return Ok(code);
        }
        if !item.has_totp {
            return Err(OpError::new(
                Problem::Other,
                "That 1Password item has no one-time password",
            ));
        }
        let mut args = vec!["item", "get", id, "--otp"];
        if !item.vault_id.is_empty() {
            args.extend(["--vault", item.vault_id.as_str()]);
        }
        let data = self
            .run(&args, None, &[], true, false, Duration::from_secs(30))
            .await?;
        let code = String::from_utf8_lossy(&data).trim().to_string();
        if code.is_empty() {
            return Err(OpError::new(
                Problem::Other,
                "1Password returned no one-time password",
            ));
        }
        Ok(code)
    }

    pub fn field_value(&self, item_id: &str, name: &str) -> Option<String> {
        let item = self.items.iter().find(|i| i.id == item_id)?;
        let field = item.fields.iter().find(|f| f.name == name)?;
        if field.hidden {
            return Some(
                self.secrets
                    .get(item_id)
                    .and_then(|s| s.hidden.get(name).cloned())
                    .unwrap_or_default(),
            );
        }
        Some(field.value.clone())
    }

    /// The same account with another password: changed there, the whole item
    /// back through stdin as JSON (never `password=…` arguments).
    pub async fn update(&mut self, id: &str, password: &str) -> Result<()> {
        if !self.is_unlocked() {
            return Err(OpError::new(Problem::NotSignedIn, "1Password is locked"));
        }
        let vault = self
            .items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.vault_id.clone())
            .unwrap_or_default();
        let mut get = vec!["item", "get", id, "--reveal", "--format", "json"];
        if !vault.is_empty() {
            get.extend(["--vault", vault.as_str()]);
        }
        let data = self
            .run(&get, None, &[], true, false, Duration::from_secs(30))
            .await?;
        let mut object = match serde_json::from_slice::<Value>(&data) {
            Ok(Value::Object(o))
                if o.get("id").and_then(Value::as_str) == Some(id)
                    && o.get("fields").is_some_and(Value::is_array) =>
            {
                o
            }
            _ => {
                return Err(OpError::new(
                    Problem::Other,
                    "1Password returned an unreadable item",
                ));
            }
        };
        if holds_passkey(&object) {
            return Err(OpError::new(
                Problem::Passkey,
                "That 1Password item holds a passkey — change its password in 1Password",
            ));
        }
        let mut fields = object
            .get("fields")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match fields
            .iter_mut()
            .find(|f| string(f.get("purpose")).eq_ignore_ascii_case("PASSWORD"))
        {
            Some(f) => {
                f["value"] = Value::String(password.into());
            }
            None => fields.push(
                json!({"id": "password", "type": "CONCEALED", "purpose": "PASSWORD",
                                       "label": "password", "value": password}),
            ),
        }
        object.insert("fields".into(), Value::Array(fields));
        let body = serde_json::to_vec(&object).unwrap_or_default();
        let mut edit = vec!["item", "edit", id, "--format", "json"];
        if !vault.is_empty() {
            edit.extend(["--vault", vault.as_str()]);
        }
        self.run(
            &edit,
            Some(&body),
            &[],
            true,
            false,
            Duration::from_secs(30),
        )
        .await?;
        let _ = self.ensure_detail(id, true).await;
        Ok(())
    }

    /// A new Login in the save vault; returns its id.
    pub async fn create(&mut self, host: &str, user: &str, password: &str) -> Result<String> {
        if !self.is_unlocked() {
            return Err(OpError::new(Problem::NotSignedIn, "1Password is locked"));
        }
        let vault = self
            .save_vault()
            .cloned()
            .ok_or_else(|| OpError::new(Problem::Other, "No 1Password vault Copper can save to"))?;
        let template = json!({
            "title": host,
            "category": "LOGIN",
            "fields": [
                {"id": "username", "type": "STRING", "purpose": "USERNAME", "label": "username", "value": user},
                {"id": "password", "type": "CONCEALED", "purpose": "PASSWORD", "label": "password", "value": password},
            ],
            "urls": [{"label": "website", "primary": true, "href": format!("{}://{host}", scheme_for(host))}],
        });
        let body = serde_json::to_vec(&template).unwrap_or_default();
        let data = self
            .run(
                &[
                    "item", "create", "--vault", &vault.id, "--format", "json", "-",
                ],
                Some(&body),
                &[],
                true,
                false,
                Duration::from_secs(30),
            )
            .await?;
        let id = objects(&data)
            .first()
            .and_then(|o| o.get("id").and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| OpError::new(Problem::Other, "1Password didn't return the new item"))?;
        let _ = self.reload(false).await;
        Ok(id)
    }

    /// Logins whose websites match `host` (empty while locked).
    pub fn items_matching(&self, host: &str) -> Vec<&OpItem> {
        if !self.is_unlocked() {
            return Vec::new();
        }
        self.items
            .iter()
            .filter(|i| i.is_login() && i.urls.iter().any(|u| matches(&Uri::new(u.clone()), host)))
            .collect()
    }

    pub fn credentials_for(&self, host: &str) -> Vec<Credential> {
        self.items_matching(host)
            .into_iter()
            .map(credential)
            .collect()
    }

    pub fn credentials(&self) -> Vec<Credential> {
        if !self.is_unlocked() {
            return Vec::new();
        }
        self.items
            .iter()
            .filter(|i| i.is_login())
            .map(credential)
            .collect()
    }
}

/// A 1Password login as the picker and agents see it: a vault named
/// `Agents` or a `copper-agent` tag shares it; a `copper-agent: deny` field
/// keeps it back.
fn credential(item: &OpItem) -> Credential {
    let denied = item.fields.iter().any(|f| {
        f.name.trim().eq_ignore_ascii_case("copper-agent")
            && f.value.trim().eq_ignore_ascii_case("deny")
    });
    let tagged = item
        .tags
        .iter()
        .any(|t| t.trim().eq_ignore_ascii_case("copper-agent"));
    let agent_hint = if denied {
        AgentHint::Deny
    } else if item.vault_name.trim().eq_ignore_ascii_case("agents") || tagged {
        AgentHint::Allow
    } else {
        AgentHint::None
    };
    Credential {
        id: CredentialId::OnePassword(item.id.clone()),
        source: Source::OnePassword,
        host: item
            .urls
            .first()
            .and_then(|u| uri_host(u))
            .unwrap_or_else(|| item.title.clone()),
        user: item.username.clone(),
        sites: item.urls.clone(),
        name: item.title.clone(),
        has_totp: item.has_totp,
        used: None,
        folder: Some(item.vault_name.clone()),
        agent_hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_op_errors() {
        let raw = message(b"[ERROR] 2026/10/06 11:39:59 You are not currently signed in. Please run `op signin --help`\n");
        assert_eq!(
            raw,
            "You are not currently signed in. Please run `op signin --help`"
        );
        assert_eq!(classify(&raw, false), Problem::NotSignedIn);
        assert_eq!(
            classify("session expired, sign in", false),
            Problem::SessionExpired
        );
        assert_eq!(
            classify("authorization prompt dismissed", false),
            Problem::Dismissed
        );
        assert_eq!(
            classify("Enter the password for x", true),
            Problem::IntegrationOff
        );
        assert_eq!(
            classify("(401) Unauthorized", false),
            Problem::WrongPassword
        );
        assert_eq!(
            classify("failed to DecodeSACredentials", false),
            Problem::BadToken
        );
        assert_eq!(classify("dial tcp: no such host", false), Problem::Network);
    }

    #[test]
    fn splits_concatenated_objects() {
        let data = br#"{"id":"a","x":"}"} {"id":"b"} } {"id":"c"}"#;
        let ids: Vec<String> = objects(data).iter().map(|o| string(o.get("id"))).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert!(is_array(b"[]"));
        assert!(is_array(b"  "));
        assert!(!is_array(b"[{"));
    }

    #[test]
    fn cards_and_names() {
        assert_eq!(expiry("202812"), ("12".into(), "2028".into()));
        assert_eq!(expiry("03/27"), ("3".into(), "2027".into()));
        assert_eq!(expiry("2029/07"), ("7".into(), "2029".into()));
        assert_eq!(brand("mc"), "Mastercard");
        assert_eq!(brand("elo"), "Elo");
        assert_eq!(session_variable("ab-c1"), "OP_SESSION_ABC1");
        assert_eq!(address("https://My.1Password.com/"), "my.1password.com");
        assert_eq!(token_from(b"export OP_SESSION_X=\"tok\"\n# x"), "tok");
        assert_eq!(token_from(b"tok\n"), "tok");
    }
}
