//! A WebSocket to the instance through the same pin (Mac: `Cloud.socket`,
//! `CloudSocket`): `wss://host/path` (`ws://` for a loopback development
//! link), with the key and token in headers and the token in `?token=` too.
//! Reconnecting is the caller's business (see `canvas::room`).

use crate::client::{CloudClient, GATE_HEADER};
use crate::error::{Failure, Result};
use crate::tls::USER_AGENT;
use rustls::pki_types::ServerName;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// The largest message accepted: the server's own cap is 8 MiB, and the
/// first `SyncStep2` carries a whole board (pictures included).
pub const MAX_MESSAGE_BYTES: usize = 64 << 20;

/// Plain TCP (loopback `ws://`) or pinned TLS.
pub enum MaybeTls {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for MaybeTls {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTls {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// An open socket to the instance.
pub type CloudSocket = WebSocketStream<MaybeTls>;

impl CloudClient {
    /// Open a WebSocket to `path` on the linked instance, signed in. The
    /// handshake answer's refusal (401, 404, …) is a [`Failure`] with the
    /// server's status.
    pub async fn socket(&self, path: &str, query: &[(&str, &str)]) -> Result<CloudSocket> {
        let link = self.link().ok_or_else(Failure::not_linked)?;
        let token = self.token().ok_or_else(Failure::not_signed_in)?;
        let trust = self.trust();
        let mut q: Vec<(&str, &str)> = query.to_vec();
        q.push(("token", &token));
        let mut url = Self::url_for(&link.endpoint, path, &q)?;
        url.set_scheme(if link.endpoint.https { "wss" } else { "ws" })
            .map_err(|_| Failure::new(0, "bad_path", "Not a socket address"))?;
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| Failure::new(0, "bad_path", e.to_string()))?;
        let headers = request.headers_mut();
        let value = |v: &str| {
            tungstenite::http::HeaderValue::from_str(v)
                .map_err(|_| Failure::new(0, "bad_header", "header"))
        };
        headers.insert(GATE_HEADER, value(&link.key)?);
        headers.insert("Authorization", value(&format!("Bearer {token}"))?);
        headers.insert("User-Agent", value(USER_AGENT)?);

        let host = link.endpoint.host.clone();
        let port = link.endpoint.effective_port();
        let tcp = tokio::time::timeout(
            Duration::from_secs(30),
            TcpStream::connect((host.as_str(), port)),
        )
        .await
        .map_err(|_| Failure::new(0, "network", "The server didn't answer in time"))?
        .map_err(|e| trust.explain(&e))?;
        let _ = tcp.set_nodelay(true);
        let stream = if link.endpoint.https {
            let name = ServerName::try_from(host.clone())
                .map_err(|_| Failure::new(0, "bad_host", format!("Not a host name: {host}")))?;
            let connector = tokio_rustls::TlsConnector::from(trust.client_config());
            let tls = tokio::time::timeout(Duration::from_secs(30), connector.connect(name, tcp))
                .await
                .map_err(|_| Failure::new(0, "network", "The server didn't answer in time"))?
                .map_err(|e| trust.explain(&e))?;
            MaybeTls::Tls(Box::new(tls))
        } else {
            MaybeTls::Plain(tcp)
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        match tokio::time::timeout(
            Duration::from_secs(30),
            tokio_tungstenite::client_async_with_config(request, stream, Some(config)),
        )
        .await
        {
            Err(_) => Err(Failure::new(
                0,
                "network",
                "The server didn't answer in time",
            )),
            Ok(Ok((socket, _))) => Ok(socket),
            Ok(Err(tungstenite::Error::Http(resp))) => {
                let status = resp.status().as_u16();
                let body = resp.body().clone().unwrap_or_default();
                let failure = Failure::from_body(&body, status);
                if status == 401 && failure.code == "session" {
                    self.forget_account();
                }
                Err(failure)
            }
            Ok(Err(e)) => Err(trust.explain(&e)),
        }
    }
}
