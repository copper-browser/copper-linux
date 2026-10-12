//! A refusal from the server, or a failure to reach it (Mac: `Cloud.Failure`).

use std::fmt;

/// A refusal from the server (`{"error": code, "message": …}`), or a failure
/// to reach it (status 0, code `network`, `tls`, `fingerprint`, `not_linked`,
/// `not_signed_in`, `untrusted`, `decode`, `health`).
#[derive(Clone, PartialEq, Eq)]
pub struct Failure {
    /// HTTP status, 0 when there was no answer.
    pub status: u16,
    /// The server's `error` code, or one of ours.
    pub code: String,
    /// Words for a person.
    pub message: String,
    /// The answer's body, for a 409 that carries the server's copy. Never
    /// logged (see the `Debug` impl).
    pub body: Option<Vec<u8>>,
    /// `untrusted`: the fingerprint of the certificate the server showed, for
    /// a person to compare and pin on first use.
    pub seen: Option<String>,
}

/// `Result` with a [`Failure`].
pub type Result<T, E = Failure> = std::result::Result<T, E>;

impl Failure {
    /// A failure with no body.
    pub fn new(status: u16, code: impl Into<String>, message: impl Into<String>) -> Self {
        Failure {
            status,
            code: code.into(),
            message: message.into(),
            body: None,
            seen: None,
        }
    }

    /// Not connected to an instance.
    pub fn not_linked() -> Self {
        Failure::new(0, "not_linked", "Copper isn't connected to a cloud")
    }

    /// Connected but not signed in.
    pub fn not_signed_in() -> Self {
        Failure::new(0, "not_signed_in", "Sign in to Copper Cloud first")
    }

    /// The answer didn't read as what was asked for.
    pub fn decode(status: u16, what: &str, err: impl fmt::Display) -> Self {
        Failure::new(
            status,
            "decode",
            format!("The server's answer to {what} didn't read: {err}"),
        )
    }

    /// No answer at all (DNS, refused, reset, timeout).
    pub fn is_network(&self) -> bool {
        self.status == 0 && self.code == "network"
    }

    /// `{"error": code, "message": text}`, or the status line when the body
    /// is not that (Mac: `Cloud.failure(from:status:)`).
    pub fn from_body(data: &[u8], status: u16) -> Self {
        let object = serde_json::from_slice::<serde_json::Value>(data).ok();
        let code = object
            .as_ref()
            .and_then(|o| o.get("error"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("http_{status}"));
        let mut message = object
            .as_ref()
            .and_then(|o| o.get("message"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| status_text(status).to_string());
        if message.is_empty() || message == "unauthorized" || message == "conflict" {
            message = explain(&code, status, &message);
        }
        Failure::new(status, code, message)
    }
}

/// The server's short codes, in words (Mac: `Cloud.explain`).
pub fn explain(code: &str, status: u16, fallback: &str) -> String {
    match code {
        "instance_key" => "The instance refused this key".into(),
        "session" => "Signed out — the session ended".into(),
        "credentials" => "Wrong email or password".into(),
        "rate_limited" => "Too many tries — wait a minute".into(),
        "conflict" => "Changed elsewhere first".into(),
        _ if fallback.is_empty() => format!("HTTP {status}"),
        _ => fallback.to_string(),
    }
}

fn status_text(status: u16) -> &'static str {
    reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("")
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Failure")
            .field("status", &self.status)
            .field("code", &self.code)
            .field("message", &self.message)
            .field("body", &self.body.as_ref().map(|b| b.len()))
            .field("seen", &self.seen)
            .finish()
    }
}

impl std::error::Error for Failure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_server_errors() {
        let f = Failure::from_body(br#"{"error":"instance_key","message":"unauthorized"}"#, 401);
        assert_eq!(f.code, "instance_key");
        assert_eq!(f.message, "The instance refused this key");
        let f = Failure::from_body(b"nope", 502);
        assert_eq!(f.code, "http_502");
        assert_eq!(f.message, "Bad Gateway");
        let f = Failure::from_body(br#"{"error":"bad_request","message":"too many"}"#, 400);
        assert_eq!(f.message, "too many");
    }
}
