//! Copper Cloud client: link, auth, sync, canvas socket.
//!
//! GTK-free and async (tokio). The GTK app and the MCP server talk to one
//! [`CloudClient`] (link codes, pairing, sign in, the pinned HTTP client, the
//! event stream) and one [`SyncEngine`] (the sync domains, driven through a
//! [`sync::BrowserState`] adapter over the browser's own state).
//!
//! Formats and behaviour follow macOS Copper (`Sources/Search/Fork/Cloud/`)
//! and the copper-cloud server's HTTP API.
//!
//! ```no_run
//! # async fn demo(store: copper_core::Store, browser: std::sync::Arc<dyn copper_cloud::BrowserState>) -> copper_cloud::Result<()> {
//! use copper_cloud::{CloudClient, Domain, SyncEngine};
//! let cloud = CloudClient::open(store).expect("cloud.json");
//! cloud.start();
//! cloud.connect_code("copper-cloud://host:8443/#k=…&fp=…", None).await?;
//! cloud.sign_in("ann@example.com", "a long password").await?;
//! let sync = SyncEngine::new(cloud.clone(), browser);
//! sync.start();
//! sync.turn_on(&[Domain::Spaces, Domain::Bookmarks, Domain::History]).await;
//! let mut status = sync.watch(); // a tokio watch of SyncStatus
//! # let _ = status.changed().await;
//! # Ok(()) }
//! ```
//!
//! ## A local server to develop against
//!
//! Native PostgreSQL only (never Docker). On Ubuntu:
//!
//! ```sh
//! sudo apt install postgresql
//! sudo -u postgres psql -c "CREATE ROLE copper_cloud_linux LOGIN PASSWORD 'copper_cloud_linux_test_pw'"
//! sudo -u postgres createdb -O copper_cloud_linux copper_cloud_linux_test
//! # in a copper-cloud checkout (github.com/copper-browser/copper-cloud):
//! cargo build -p copper-cloud
//! B=target/debug/copper-cloud; export COPPER_CLOUD_CONFIG=$PWD/dev.toml COPPER_CLOUD_METRICS_BIND=off
//! $B init-config --write dev.toml --public-url 127.0.0.1:8443 --listen 127.0.0.1:8443 \
//!   --database-url postgres://copper_cloud_linux:copper_cloud_linux_test_pw@127.0.0.1:5432/copper_cloud_linux_test \
//!   --tls-mode self-signed --tls-dir ./tls
//! $B tls-init && $B link-code     # paste this into Copper
//! $B serve
//! ```
//!
//! `tests/live_server.rs` does exactly this on a free port (it needs
//! `COPPER_CLOUD_BIN` or `~/src/copper-cloud-ref/target/debug/copper-cloud`,
//! and `COPPER_CLOUD_TEST_DATABASE_URL`, whose `public` schema it resets).

pub mod canvas;
pub mod client;
pub mod error;
pub mod intelligence;
pub mod link;
pub mod offline;
pub mod sso;
pub mod sync;
pub mod tls;
pub mod ws;

pub use canvas::CanvasApi;
pub use client::{Account, CloudClient, CloudEvent, CloudStatus, Feature, ServerInfo};
pub use error::{Failure, Result};
pub use intelligence::CloudIntelligence;
pub use link::{
    Code, Endpoint, JoinCode, Link, PairingCode, SsoLink, deep_link_code, parse_code,
    parse_join_code, parse_link_code, parse_sso_link,
};
pub use sso::{SsoCancel, SsoInfo, SsoOutcome, SsoProvider};
pub use sync::{BrowserState, Domain, SyncEngine, SyncStatus};
