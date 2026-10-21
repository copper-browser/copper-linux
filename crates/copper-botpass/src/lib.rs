//! BotPass (github.com/pprasha/botpass) as a password source for Copper.
//!
//! Copper pairs once as a *device* of one BotPass bot (`bp connect copper`
//! → `copper botpass pair …`, or Settings › Passwords › BotPass). From then
//! on, a sign-in box or an agent's `browser_sign_in` on a page asks BotPass
//! which accounts this bot may use on the page's origin, and fetches the
//! chosen one's password or one-time code at the moment it is filled, for
//! that origin. BotPass's grants decide (and record) every use; the agent
//! never sees a secret, only that the sign-in happened.
//!
//! * [`pairing`] — `botpass.json` in Copper's data folder (0600, atomic).
//! * [`client`] — the HTTP API, pinned TLS through
//!   [`copper_cloud::tls::Trust`].
//! * [`source`] — [`BotPass`], the source the forms lane's `Vault` holds.
//! * [`cli`] — `copper botpass pair|status|unpair`.
//!
//! Nothing here logs or prints a token, a password or a code.

pub mod cli;
pub mod client;
pub mod error;
pub mod pairing;
pub mod source;

pub use client::{Account, Filled, Link, Server, Status, What, page_origin, parse_link};
pub use error::Error;
pub use pairing::Pairing;
pub use source::{BotPass, BpSnapshot, BpState, default_device_name};
