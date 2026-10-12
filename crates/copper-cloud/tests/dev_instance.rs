//! Read-only checks against the real dev instance (https://100.57.251.123,
//! the one the team's Macs are linked to): `/healthz` answers through its
//! pinned certificate, and a wrong pin is refused. No account is made, no key
//! is sent.
//!
//! Override with `COPPER_CLOUD_DEV_URL` / `COPPER_CLOUD_DEV_FP`. When the
//! instance can't be reached at all (another network), the test says so and
//! passes, unless `COPPER_CLOUD_REQUIRE_DEV=1`.

use copper_cloud::client::CloudClient;
use copper_cloud::link::{endpoint, normalize_fingerprint};

const DEV_URL: &str = "https://100.57.251.123:443";
/// The fingerprint the Macs pin (their `cloud.json` `link.fingerprint`).
const DEV_FP: &str = "837b024d986e9f22632eece8478d77099b89d49c0337974aa84336c6928d82c4";

#[tokio::test]
async fn dev_instance_healthz_through_its_pin() {
    let url = std::env::var("COPPER_CLOUD_DEV_URL").unwrap_or_else(|_| DEV_URL.into());
    let fp = std::env::var("COPPER_CLOUD_DEV_FP").unwrap_or_else(|_| DEV_FP.into());
    let (end, pin) = endpoint(&url, Some(&fp)).expect("the dev address reads");
    let pin = pin.expect("an https address keeps its pin");
    match CloudClient::probe_health(&end, Some(&pin)).await {
        Ok(body) => assert_eq!(body.trim(), "ok"),
        Err(f) if f.is_network() => {
            let required = std::env::var("COPPER_CLOUD_REQUIRE_DEV").is_ok_and(|v| v == "1");
            assert!(!required, "dev instance unreachable: {f:?}");
            eprintln!(
                "SKIPPED: dev instance {url} unreachable from here ({})",
                f.message
            );
            return;
        }
        Err(f) => panic!("dev instance /healthz through its pin failed: {f:?}"),
    }
    // A wrong pin is refused — the system's trust never stands in.
    let flipped = if pin.starts_with('0') { '1' } else { '0' };
    let wrong = normalize_fingerprint(&format!("{flipped}{}", &pin[1..])).unwrap();
    let err = CloudClient::probe_health(&end, Some(&wrong))
        .await
        .unwrap_err();
    assert_eq!(err.code, "fingerprint", "{err:?}");
    // No pin at all: refused, and the certificate it showed is the pinned one.
    let err = CloudClient::probe_health(&end, None).await.unwrap_err();
    assert_eq!(err.code, "untrusted", "{err:?}");
    assert_eq!(err.seen.as_deref(), Some(pin.as_str()));
}
