//! What can go wrong talking to BotPass, in words a person (or an agent
//! relaying them) can act on. No variant ever carries a token or a secret.

/// A refusal from BotPass, or a failure to reach it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// 423: the BotPass server is sealed.
    #[error("BotPass is locked — run bp unlock")]
    Locked,
    /// 401 on a device call: no pairing here, or BotPass removed the device.
    #[error(
        "Copper isn't paired with BotPass (or BotPass removed this device) — run bp connect copper"
    )]
    Unpaired,
    /// 401 on pairing: the code is wrong, used or expired.
    #[error("BotPass refused the pairing code: {0} — make a new one with bp connect copper")]
    BadCode(String),
    /// 403: BotPass's grants don't allow this account on this site.
    #[error("BotPass doesn't allow that here: {0}")]
    Denied(String),
    /// 404.
    #[error("BotPass has no such account ({0})")]
    NotFound(String),
    /// The certificate didn't match the pinned fingerprint.
    #[error("BotPass's certificate doesn't match the pinned fingerprint — refused")]
    Fingerprint {
        /// What the server showed (SHA-256 of its leaf DER, hex).
        seen: Option<String>,
    },
    /// No pin, and the public authorities don't vouch for the certificate
    /// (BotPass's own certificate is self-signed: pass its fingerprint).
    #[error(
        "BotPass's certificate isn't trusted — pass its fingerprint (GET /v1/status tls_fingerprint, or bp status)"
    )]
    Untrusted { seen: Option<String> },
    /// No answer (refused, timeout, DNS).
    #[error("BotPass didn't answer: {0}")]
    Unreachable(String),
    /// Any other answer that isn't a success.
    #[error("BotPass answered {status}: {message}")]
    Server { status: u16, message: String },
    /// A bad URL, fingerprint or argument.
    #[error("{0}")]
    Usage(String),
    /// `botpass.json` couldn't be read or written.
    #[error("botpass.json: {0}")]
    File(String),
    /// An answer that didn't read as what was asked for.
    #[error("BotPass's answer didn't read: {0}")]
    Decode(String),
}

impl Error {
    /// A short machine code (`--json` `"error"`).
    pub fn code(&self) -> &'static str {
        match self {
            Error::Locked => "locked",
            Error::Unpaired => "unpaired",
            Error::BadCode(_) => "bad_code",
            Error::Denied(_) => "denied",
            Error::NotFound(_) => "not_found",
            Error::Fingerprint { .. } => "fingerprint",
            Error::Untrusted { .. } => "untrusted",
            Error::Unreachable(_) => "network",
            Error::Server { .. } => "server",
            Error::Usage(_) => "usage",
            Error::File(_) => "file",
            Error::Decode(_) => "decode",
        }
    }

    /// The certificate fingerprint the server showed, when that is what the
    /// failure is about.
    pub fn seen(&self) -> Option<&str> {
        match self {
            Error::Fingerprint { seen } | Error::Untrusted { seen } => seen.as_deref(),
            _ => None,
        }
    }
}
