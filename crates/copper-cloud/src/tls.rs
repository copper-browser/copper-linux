//! Certificate pinning (Mac: `CloudTrust`).
//!
//! A pinned instance is trusted if and only if the SHA-256 of its leaf
//! certificate's DER is the link's fingerprint. The public roots are never
//! consulted for it — not as a fallback — which is what lets a self-signed
//! instance work and what stops anything else from standing in for it. The
//! handshake signature is still verified against the pinned certificate's
//! key, so the server must hold that key.
//!
//! A link with no pin (an instance on a public certificate, such as an ACME
//! one: the sign-in links and agent pairing codes such an instance makes
//! carry no `fp=`) is checked the ordinary way, against the public
//! certificate authorities (Mozilla's list, built in, so it is the same on
//! every system), for the host it names — what the Mac's default handling
//! does. A self-signed certificate fails that, and the verifier remembers the
//! fingerprint it was shown ([`Trust::seen`]), so a person can compare it
//! (`copper-cloud doctor` on the server) and pin it on first use.

use crate::error::Failure;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use std::error::Error as StdError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `User-Agent` on every request: `Copper/<app version> (<system>)`
/// ([`copper_core::version::user_agent`]).
pub fn user_agent() -> String {
    copper_core::version::user_agent()
}

/// SHA-256 of a certificate's DER, lowercase hex.
pub fn fingerprint(der: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The `ring` provider, the only one this crate uses.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

#[derive(Debug, Default)]
struct TrustState {
    seen: Option<String>,
    mismatch_at: Option<Instant>,
}

/// One pin and what the servers it met showed (Mac: `CloudTrust`). Cheap to
/// clone; clones share what was seen.
#[derive(Clone)]
pub struct Trust {
    pin: Option<String>,
    state: Arc<Mutex<TrustState>>,
    config: Arc<ClientConfig>,
}

impl std::fmt::Debug for Trust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trust").field("pin", &self.pin).finish()
    }
}

impl Trust {
    /// A trust that accepts exactly `pin` (lowercase hex SHA-256), or — with
    /// none — a certificate the public authorities vouch for.
    pub fn new(pin: Option<&str>) -> Self {
        let pin = pin.map(str::to_ascii_lowercase);
        let state = Arc::new(Mutex::new(TrustState::default()));
        let provider = provider();
        let verifier = Arc::new(PinVerifier {
            pin: pin.clone(),
            state: state.clone(),
            provider: provider.clone(),
            public: public_verifier(&provider),
        });
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring supports the default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Trust {
            pin,
            state,
            config: Arc::new(config),
        }
    }

    /// A trust with the same pin and a clean slate.
    pub fn fresh(&self) -> Self {
        Trust::new(self.pin.as_deref())
    }

    /// The pinned fingerprint.
    pub fn pin(&self) -> Option<&str> {
        self.pin.as_deref()
    }

    /// The fingerprint of the last leaf certificate a server showed, trusted
    /// or not.
    pub fn seen(&self) -> Option<String> {
        self.state.lock().ok().and_then(|s| s.seen.clone())
    }

    /// A certificate was refused for not matching the pin, just now — what a
    /// failure that follows is really about.
    pub fn mismatched(&self) -> bool {
        self.state
            .lock()
            .ok()
            .and_then(|s| s.mismatch_at)
            .is_some_and(|at| at.elapsed() < Duration::from_secs(30))
    }

    /// The rustls configuration (ALPN `http/1.1`), for sockets.
    pub fn client_config(&self) -> Arc<ClientConfig> {
        self.config.clone()
    }

    /// An HTTP client that trusts only this pin. No proxy (an instance is
    /// self-hosted, usually on a LAN or a tailnet), no cookies, no cache, a
    /// 15 s connect limit; per-request limits are the caller's.
    pub fn http_client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .use_preconfigured_tls((*self.config).clone())
            .no_proxy()
            .user_agent(user_agent())
            .connect_timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .expect("a reqwest client with a rustls configuration")
    }

    /// A transport error in words (Mac: `Cloud.transportFailure`). A pin
    /// mismatch wins over whatever the error says.
    pub fn explain(&self, error: &(dyn StdError + 'static)) -> Failure {
        if self.mismatched() {
            return Failure::new(
                0,
                "fingerprint",
                "The server's certificate doesn't match the link code's fingerprint — refused",
            );
        }
        let mut text = error.to_string();
        let mut source = Some(error);
        let mut tls = None;
        let mut refused = false;
        let mut timed_out = false;
        while let Some(e) = source {
            if let Some(t) = e.downcast_ref::<rustls::Error>() {
                tls = Some(t.to_string());
            }
            if let Some(io) = e.downcast_ref::<std::io::Error>() {
                match io.kind() {
                    std::io::ErrorKind::ConnectionRefused => refused = true,
                    std::io::ErrorKind::TimedOut => timed_out = true,
                    _ => {}
                }
                if let Some(inner) = io.get_ref()
                    && let Some(t) = inner.downcast_ref::<rustls::Error>()
                {
                    tls = Some(t.to_string());
                }
            }
            if let Some(r) = e.downcast_ref::<reqwest::Error>()
                && r.is_timeout()
            {
                timed_out = true;
            }
            let here = e.to_string();
            if !text.contains(&here) {
                text = format!("{text}: {here}");
            }
            source = e.source();
        }
        if let Some(tls) = tls {
            return Failure::new(
                0,
                "tls",
                format!("The server's certificate isn't trusted ({tls})"),
            );
        }
        if timed_out {
            return Failure::new(0, "network", "The server didn't answer in time");
        }
        if refused {
            return Failure::new(0, "network", "Nothing is listening there");
        }
        let lower = text.to_lowercase();
        if lower.contains("dns error") || lower.contains("failed to lookup address") {
            return Failure::new(0, "network", "No such host");
        }
        Failure::new(0, "network", text)
    }
}

/// The public authorities' verifier, for a link without a pin. `None` only
/// if the built-in roots could not be read, which then trusts nothing.
fn public_verifier(
    provider: &Arc<CryptoProvider>,
) -> Option<Arc<rustls::client::WebPkiServerVerifier>> {
    static ROOTS: std::sync::OnceLock<Arc<rustls::RootCertStore>> = std::sync::OnceLock::new();
    let roots = ROOTS
        .get_or_init(|| {
            Arc::new(rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })
        })
        .clone();
    rustls::client::WebPkiServerVerifier::builder_with_provider(roots, provider.clone())
        .build()
        .map_err(|e| tracing::error!(error = %e, "no public certificate roots"))
        .ok()
}

/// The verifier behind [`Trust`].
#[derive(Debug)]
pub struct PinVerifier {
    pin: Option<String>,
    state: Arc<Mutex<TrustState>>,
    provider: Arc<CryptoProvider>,
    public: Option<Arc<rustls::client::WebPkiServerVerifier>>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let shown = fingerprint(end_entity.as_ref());
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.seen = Some(shown.clone());
        match &self.pin {
            Some(pin) if *pin == shown => Ok(ServerCertVerified::assertion()),
            Some(_) => {
                state.mismatch_at = Some(Instant::now());
                Err(rustls::Error::InvalidCertificate(
                    CertificateError::ApplicationVerificationFailure,
                ))
            }
            None => match &self.public {
                Some(public) => {
                    drop(state);
                    public.verify_server_cert(
                        end_entity,
                        intermediates,
                        server_name,
                        ocsp_response,
                        now,
                    )
                }
                None => Err(rustls::Error::InvalidCertificate(
                    CertificateError::UnknownIssuer,
                )),
            },
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_lowercase_sha256() {
        assert_eq!(
            fingerprint(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn verifier_accepts_only_the_pin() {
        let der = CertificateDer::from(b"not really a certificate".to_vec());
        let fp = fingerprint(der.as_ref());
        let name = ServerName::try_from("example.com").unwrap();
        let now = UnixTime::now();
        let good = Trust::new(Some(&fp.to_uppercase()));
        let v = PinVerifier {
            pin: good.pin.clone(),
            state: good.state.clone(),
            provider: provider(),
            public: public_verifier(&provider()),
        };
        assert!(v.verify_server_cert(&der, &[], &name, &[], now).is_ok());
        assert_eq!(good.seen().as_deref(), Some(fp.as_str()));
        assert!(!good.mismatched());

        let bad = Trust::new(Some(&"0".repeat(64)));
        let v = PinVerifier {
            pin: bad.pin.clone(),
            state: bad.state.clone(),
            provider: provider(),
            public: public_verifier(&provider()),
        };
        assert!(v.verify_server_cert(&der, &[], &name, &[], now).is_err());
        assert!(bad.mismatched());
        assert_eq!(bad.seen().as_deref(), Some(fp.as_str()));

        // No pin: only what the public authorities vouch for (not this),
        // and what was shown is remembered.
        let none = Trust::new(None);
        let v = PinVerifier {
            pin: None,
            state: none.state.clone(),
            provider: provider(),
            public: public_verifier(&provider()),
        };
        assert!(v.public.is_some(), "the built-in roots load");
        assert!(v.verify_server_cert(&der, &[], &name, &[], now).is_err());
        assert!(!none.mismatched());
        assert_eq!(none.seen().as_deref(), Some(fp.as_str()));
    }
}
