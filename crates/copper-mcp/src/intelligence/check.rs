//! Settings › Intelligence › Check (`IntelligenceCheck.swift`): one small
//! question to Jev and one to the gateway, timed, and what came back put in
//! words a person can act on (the gateway's or Jev's failure, the network's,
//! a refused key). A verdict is about the setup it was asked with: change a
//! key, the address or the model and it no longer says anything.
//!
//! Only the gateway lane is asked: the Claude account lane is Mac-only for
//! now. Toolkit-free, so every platform's page keeps a [`Check`] and draws it.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use serde_json::json;

use super::{Effective, Keys, Net, jev_ask, model, router_ask};

/// Where a Jev key is made (`Jev.keyPage`).
pub const JEV_KEY_PAGE: &str = "https://console.typesafe.ai";

/// One lane's verdict.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    /// `jev` or `router`.
    pub lane: &'static str,
    pub ok: bool,
    pub text: String,
    /// The key itself was turned away (not the network, not the model).
    pub refused: bool,
}

impl Verdict {
    fn bad(lane: &'static str, text: impl Into<String>, refused: bool) -> Verdict {
        Verdict {
            lane,
            ok: false,
            text: text.into(),
            refused,
        }
    }
}

/// The last check, kept for as long as the setup it asked with holds.
#[derive(Clone, Debug, Default)]
pub struct Check {
    pub running: bool,
    pub verdicts: Vec<Verdict>,
    /// [`whole`] of the setup the verdicts were asked with.
    asked: Option<u64>,
    /// [`lane_key`] of each lane that was asked.
    asked_with: Vec<(&'static str, u64)>,
}

impl Check {
    /// One sentence per lane, or `None` while there is none for `keys`.
    pub fn line(&self, keys: &Keys) -> Option<String> {
        (!self.verdicts.is_empty() && self.asked == Some(whole(keys))).then(|| {
            self.verdicts
                .iter()
                .map(|v| v.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// Every lane answered.
    pub fn all_ok(&self) -> bool {
        self.verdicts.iter().all(|v| v.ok)
    }

    /// The last check saw this lane's key, as it is now, turned away.
    pub fn refused(&self, keys: &Keys, lane: &str) -> bool {
        let now = lane_key(keys, lane);
        self.verdicts.iter().any(|v| v.lane == lane && v.refused)
            && self.asked_with.iter().any(|(l, k)| *l == lane && *k == now)
    }
}

/// Ask Jev and the gateway one small question each (`IntelligenceCheck.run`).
/// Await it from any loop: [`Net`] does the requests on its own runtime.
pub async fn run(net: &Net, eff: &Effective) -> Check {
    let keys = &eff.keys;
    let asked_model = model(keys, None);
    let mut asked_with = Vec::with_capacity(2);
    let mut verdicts = Vec::with_capacity(2);
    verdicts.push(if eff.jev_ready() {
        asked_with.push(("jev", lane_key(keys, "jev")));
        let state = json!({ "word": "apple" });
        let questions = json!({ "kind": {
            "type": "choice",
            "instructions": "What is `word`?",
            "criteria": { "fruit": "a fruit", "tool": "a tool" },
        }});
        match jev_ask(net, keys, &state, &questions, Duration::from_secs(4)).await {
            Ok(a) => Verdict {
                lane: "jev",
                ok: true,
                text: format!("Jev answered in {}.", duration(a.latency_ms)),
                refused: false,
            },
            Err(f) => jev_failure(&f.kind, &f.detail),
        }
    } else {
        Verdict::bad(
            "jev",
            format!("Jev has no key yet — get one at {JEV_KEY_PAGE} and paste it below."),
            false,
        )
    });
    verdicts.push(if eff.router_ready() {
        asked_with.push(("router", lane_key(keys, "router")));
        let reply = router_ask(
            net,
            keys,
            &asked_model,
            "Reply with JSON only.",
            "{\"ping\": true} → reply {\"pong\": true}",
            Duration::from_secs(15),
            20,
        )
        .await;
        match reply {
            Ok(r) => Verdict {
                lane: "router",
                ok: true,
                text: format!(
                    "The gateway answered as {} in {}.",
                    r.model,
                    duration(r.latency_ms)
                ),
                refused: false,
            },
            Err(e) => gateway_failure(&e, &asked_model),
        }
    } else if keys.router_key.trim().is_empty() {
        Verdict::bad(
            "router",
            "The gateway has no key yet — paste one above.",
            false,
        )
    } else {
        Verdict::bad(
            "router",
            "The gateway address isn't a web address — it should start with https://",
            false,
        )
    });
    Check {
        running: false,
        verdicts,
        asked: Some(whole(keys)),
        asked_with,
    }
}

/// One lane's key and address, hashed so no second copy of a key is kept.
pub fn lane_key(keys: &Keys, lane: &str) -> u64 {
    let mut h = DefaultHasher::new();
    lane.hash(&mut h);
    match lane {
        "jev" => (&keys.jev_key, &keys.jev_endpoint).hash(&mut h),
        _ => (&keys.router_key, &keys.router_url).hash(&mut h),
    }
    h.finish()
}

/// Everything a verdict depends on: both lanes' keys and the model asked.
pub fn whole(keys: &Keys) -> u64 {
    let mut h = DefaultHasher::new();
    lane_key(keys, "jev").hash(&mut h);
    lane_key(keys, "router").hash(&mut h);
    model(keys, None).hash(&mut h);
    h.finish()
}

/// Whether `raw` can be a gateway's address (`IntelligencePage.isWebAddress`).
pub fn is_web_address(raw: &str) -> bool {
    url::Url::parse(raw.trim()).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty())
    })
}

/// `340 ms` / `1.2 s`.
pub fn duration(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{} ms", ms.round() as i64)
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

/// A transport failure in a person's words.
fn network(detail: &str, who: &str, waited: u64) -> String {
    let d = detail.to_lowercase();
    if d.contains("timed out") || d.contains("timeout") {
        format!(
            "{who} didn't answer in {waited} seconds. Try again; if it keeps happening, check the address."
        )
    } else if d.contains("dns") || d.contains("lookup") || d.contains("resolve") {
        format!("{who} couldn't be found at that address — check it, or your network.")
    } else if d.contains("refused") {
        format!(
            "Nothing answered at {}'s address — check it.",
            who.to_lowercase()
        )
    } else if d.contains("certificate") || d.contains("tls") {
        format!("{who}'s address has a certificate this computer doesn't trust.")
    } else {
        format!("{who} couldn't be reached. Try again.")
    }
}

/// Jev's failure (`IntelligenceCheck.jevFailure`): `kind` as [`super::JevFailure`].
pub fn jev_failure(kind: &str, detail: &str) -> Verdict {
    let (text, refused) = match kind {
        "transport" => (network(detail, "Jev", 4), false),
        "http_401" | "http_403" => (
            "Jev turned the key away — paste a new one below.".into(),
            true,
        ),
        "http_402" => ("Jev says this key's account is out of credit.".into(), true),
        "http_429" => (
            "Jev is busy with this key just now — try again in a minute.".into(),
            false,
        ),
        "bad_endpoint" => ("Jev's address isn't a web address.".into(), false),
        k if k.starts_with("http_5") => (
            format!(
                "Jev is having trouble on its side ({}). Try again later.",
                &k[5..]
            ),
            false,
        ),
        k if k.starts_with("http_") => {
            (format!("Jev answered with an error ({}).", &k[5..]), false)
        }
        _ => ("Jev's answer didn't make sense. Try again.".into(), false),
    };
    Verdict::bad("jev", text, refused)
}

/// The gateway's failure (`IntelligenceCheck.gatewayFailure`); `error` is
/// [`router_ask`]'s: `router 401: body` for an answer, anything else for the
/// network.
pub fn gateway_failure(error: &str, model: &str) -> Verdict {
    let status = error
        .strip_prefix("router ")
        .and_then(|r| r.split(':').next())
        .and_then(|s| s.parse::<u16>().ok());
    let body = error.to_lowercase();
    let (text, refused) = match status {
        Some(401 | 403) => (
            "The gateway turned the key away — paste a new one above.".into(),
            true,
        ),
        Some(400 | 404) if body.contains("model") => (
            format!("The gateway has no model called “{model}” — check Model names."),
            false,
        ),
        Some(404) => (
            "Nothing at that address answers like a gateway — check the address.".into(),
            false,
        ),
        Some(429) => (
            "The gateway is limiting this key just now — try again in a minute.".into(),
            false,
        ),
        Some(s) if s >= 500 => (
            format!("The gateway is having trouble on its side ({s}). Try again later."),
            false,
        ),
        Some(s) => (format!("The gateway answered with an error ({s})."), false),
        None if error.contains("no choices") => (
            "The gateway's answer didn't make sense — is that the right address?".into(),
            false,
        ),
        None => (network(error, "The gateway", 15), false),
    };
    Verdict::bad("router", text, refused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::IntelligenceSource;

    #[test]
    fn failures_in_plain_words() {
        let v = gateway_failure("router 401: nope", "sonnet");
        assert!(v.refused && v.lane == "router");
        let v = gateway_failure("router 404: no such model", "big");
        assert!(v.text.contains("“big”") && !v.refused);
        let v = gateway_failure("error sending request: operation timed out", "x");
        assert!(v.text.contains("15 seconds"));
        assert!(jev_failure("http_402", "").refused);
        assert!(jev_failure("http_503", "").text.contains("(503)"));
        assert!(
            jev_failure("transport", "dns error")
                .text
                .contains("couldn't be found")
        );
        assert_eq!(duration(212.4), "212 ms");
        assert_eq!(duration(1530.0), "1.5 s");
        assert!(is_web_address(" https://llm.example.com "));
        assert!(!is_web_address("llm.example.com") && !is_web_address("ftp://x"));
    }

    #[test]
    fn a_verdict_holds_only_for_its_setup() {
        let mut keys = Keys {
            router_key: "sk-a".into(),
            ..Keys::default()
        };
        let c = Check {
            running: false,
            verdicts: vec![
                jev_failure("http_401", ""),
                gateway_failure("router 401: no", "sonnet"),
            ],
            asked: Some(whole(&keys)),
            asked_with: vec![("router", lane_key(&keys, "router"))],
        };
        assert!(c.line(&keys).is_some_and(|l| l.lines().count() == 2));
        assert!(!c.all_ok());
        assert!(c.refused(&keys, "router"));
        assert!(!c.refused(&keys, "jev"), "jev was not asked");
        keys.router_key = "sk-b".into();
        assert!(c.line(&keys).is_none() && !c.refused(&keys, "router"));
    }

    #[test]
    fn unready_lanes_are_not_asked() {
        // No keys: nothing goes on the network.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let net = Net::new(rt.handle().clone(), "test");
        let eff = Effective::with_router(&IntelligenceSource::default(), false, None);
        let c = rt.block_on(run(&net, &eff));
        assert!(c.verdicts[0].text.contains(JEV_KEY_PAGE));
        assert!(c.verdicts[1].text.contains("no key yet"));
        assert!(c.line(&eff.keys).is_some());
        assert!(c.asked_with.is_empty());
    }
}
