//! Passwords for Copper on Linux (PLAN T-015 backend).
//!
//! - [`store::SecretStore`] — the Secret Service (GNOME Keyring, KWallet,
//!   KeePassXC…) via `oo7`; items tagged `app=Copper`, `origin`, `username`.
//!   macOS: the keychain (`Vault.swift`).
//! - [`bitwarden::Bitwarden`] — the `bw` CLI (`Bitwarden.swift`).
//! - [`onepassword::OnePassword`] — the `op` CLI (`OnePassword*.swift`).
//! - [`credential`] — ids, rows, merge/rank (`Credential.swift`,
//!   `CredentialStore.swift`).
//! - [`offer`] — when a sign-in took and what to offer (`Tab.swift`,
//!   `Browser.swift`), pure.
//! - [`site`] — registrable domains and vault URI matching; [`totp`] codes.
//!
//! Nothing here logs a secret, puts one on a command line, or writes one to
//! disk outside the Secret Service and 0600 session files.

pub mod bitwarden;
pub mod credential;
pub mod offer;
pub mod onepassword;
mod process;
/// A private Secret Service for tests (its own D-Bus + gnome-keyring).
pub mod sandbox;
pub mod site;
pub mod store;
pub mod totp;

pub use credential::{AgentHint, Credential, CredentialId, Login, Source};
pub use store::{SecretStore, StoreError};
