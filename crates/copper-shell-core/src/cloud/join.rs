//! `copper://cloud/join?code=<percent-encoded copper-cloud://…>` (Mac:
//! `CloudJoin`) — the instance's `/start` page's **Open in Copper**, a link
//! clicked anywhere, `copper copper://cloud/join?…` on the command line (and
//! so a second `copper` handing it to the running one), or the address typed
//! into the field.
//!
//! It opens Settings › Cloud with the code; a join link (`#sso=`) starts the
//! sign-in at once, so the person is one click from the provider's page. It
//! never switches instance or account without a press: a code for another
//! instance than the one this Copper is linked to waits on the page ("This
//! link is for HOST" + Dismiss), and one for the instance it is signed in to
//! already just says so.
//!
//! [`decide`] is the pure decision; [`handle`] runs it against the app's
//! cloud and [`super::sso::sign_in`] and returns the [`JoinAction`] for the
//! shell to carry out (open Settings › Cloud, show [`JoinAction::message`]).
//! A shell checks [`is_cloud_link`] *before* its usual address handling;
//! the address field offers [`palette_offers`] for one instead of a search.

use super::sso::{JoinOffer, Opener, SsoTarget, code_endpoint, same_instance, sign_in};
use crate::palette::complete::{Action, Kind, Offers, Row, Section};
use copper_cloud::{JoinCode, Link, deep_link_code, parse_join_code};

/// What a `copper://cloud/…` link comes to, for the shell to carry out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinAction {
    /// Not a `copper://cloud/…` link: an address like any other.
    NotJoin,
    /// Copper Cloud is off this run (`cloud.json` couldn't be read).
    Off,
    /// A `copper://cloud/…` link with no code that reads.
    Incomplete,
    /// Signed in to that instance already.
    AlreadySignedIn { host: String, email: String },
    /// Connected to that instance already (not signed in), and the code
    /// isn't a sign-in link: signing in is on the page.
    AlreadyConnected { host: String },
    /// Not linked: Settings › Cloud's Connect field takes the code.
    Fill(JoinOffer),
    /// A sign-in link for this Copper (not linked, or linked to that
    /// instance): the field takes it and the sign-in starts at once.
    StartSso {
        /// `None` from [`decide`]; [`handle`] fills it in.
        offer: Option<JoinOffer>,
        target: SsoTarget,
    },
    /// Linked to another instance: the page says "This link is for HOST"
    /// until it is dismissed, and nothing changes without a press.
    OtherInstance {
        /// `None` from [`decide`]; [`handle`] fills it in.
        offer: Option<JoinOffer>,
        host: String,
        linked: String,
    },
}

impl JoinAction {
    /// Its name, for the log.
    pub fn kind(&self) -> &'static str {
        match self {
            JoinAction::NotJoin => "not-join",
            JoinAction::Off => "off",
            JoinAction::Incomplete => "incomplete",
            JoinAction::AlreadySignedIn { .. } => "already-signed-in",
            JoinAction::AlreadyConnected { .. } => "already-connected",
            JoinAction::Fill(_) => "fill",
            JoinAction::StartSso { .. } => "start-sso",
            JoinAction::OtherInstance { .. } => "other-instance",
        }
    }

    /// Whether the shell opens Settings › Cloud for it.
    pub fn opens_settings(&self) -> bool {
        !matches!(self, JoinAction::NotJoin | JoinAction::Off)
    }

    /// What the shell says about it, if anything (a toast, a status line).
    pub fn message(&self) -> Option<String> {
        match self {
            JoinAction::NotJoin | JoinAction::Fill(_) | JoinAction::StartSso { .. } => None,
            JoinAction::Off => {
                Some("Copper Cloud is off this run — cloud.json couldn't be read".into())
            }
            JoinAction::Incomplete => Some(
                "That Copper Cloud link is incomplete — open it again from the page that gave it"
                    .into(),
            ),
            JoinAction::AlreadySignedIn { host, email } => {
                Some(format!("Already signed in to {host} as {email}"))
            }
            JoinAction::AlreadyConnected { host } => Some(format!(
                "Already connected to {host} — sign in on the Cloud page"
            )),
            JoinAction::OtherInstance { host, .. } => Some(format!("This link is for {host}")),
        }
    }
}

/// `copper://cloud/…` (or `copper:cloud/…`, `copper:///cloud/…`): Copper
/// Cloud's own links, which never load as a page. Check this before treating
/// text as an address.
pub fn is_cloud_link(text: &str) -> bool {
    let t = text.trim();
    let Some(rest) = t
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("copper:"))
        .map(|_| &t[7..])
    else {
        return false;
    };
    let rest = rest.trim_start_matches('/');
    let head = rest.split(['/', '?', '#']).next().unwrap_or("");
    head.eq_ignore_ascii_case("cloud")
}

/// The address field's one row for a `copper://cloud/…` link typed or
/// pasted into it: "Join Copper Cloud — HOST", which opens Settings › Cloud
/// ([`Action::Go`] with the link; the shell's Go checks [`is_cloud_link`]
/// first). `None` for anything else.
pub fn palette_row(text: &str) -> Option<Row> {
    if !is_cloud_link(text) {
        return None;
    }
    let host = deep_link_code(text)
        .and_then(|code| parse_join_code(&code, None))
        .map(|code| code_endpoint(&code).host_text());
    Some(Row {
        kind: Kind::Address,
        title: match &host {
            Some(host) => format!("Join Copper Cloud — {host}"),
            None => "Copper Cloud link".into(),
        },
        detail: "Opens Settings › Cloud".into(),
        hint: "Open".into(),
        action: Action::Go(text.trim().to_string()),
        url: None,
        icon: Some("network-server-symbolic".into()),
        section: Some(Section::Search),
    })
}

/// [`palette_row`] as the palette's whole offer.
pub fn palette_offers(text: &str) -> Option<Offers> {
    palette_row(text).map(|row| Offers {
        rows: vec![row],
        default: Some(0),
    })
}

/// What a `copper://cloud/…` link does, given the instance this Copper is
/// linked to and who is signed in there (pure).
pub fn decide(text: &str, linked: Option<&Link>, signed_in_as: Option<&str>) -> JoinAction {
    if !is_cloud_link(text) {
        return JoinAction::NotJoin;
    }
    let Some(code_text) = deep_link_code(text) else {
        return JoinAction::Incomplete;
    };
    let Some(code) = parse_join_code(&code_text, linked) else {
        return JoinAction::Incomplete;
    };
    let host = code_endpoint(&code).host_text();
    let same = linked.is_some_and(|l| same_instance(&l.endpoint, code_endpoint(&code)));
    if same && let Some(email) = signed_in_as {
        return JoinAction::AlreadySignedIn {
            host,
            email: email.to_string(),
        };
    }
    match (code, linked) {
        (JoinCode::Sso(sso), None) => JoinAction::StartSso {
            offer: None,
            target: SsoTarget::Join(sso),
        },
        // Linked there already: its own link (and pin) is the one to keep.
        (JoinCode::Sso(sso), Some(link)) if same => JoinAction::StartSso {
            offer: None,
            target: SsoTarget::Linked {
                endpoint: link.endpoint.clone(),
                provider: sso.provider,
            },
        },
        (_, Some(_)) if same => JoinAction::AlreadyConnected { host },
        (code, None) => JoinAction::Fill(JoinOffer {
            id: 0,
            text: code_text,
            code,
        }),
        (_, Some(link)) => JoinAction::OtherInstance {
            offer: None,
            host,
            linked: link.host(),
        },
    }
}

/// Carry out `text` against the app's cloud: keep the code for Settings ›
/// Cloud ([`super::sso::SignIn::offer`]) and start the sign-in a join link
/// asks for (`open` opens the system browser). The shell then opens
/// Settings › Cloud when [`JoinAction::opens_settings`] and shows
/// [`JoinAction::message`].
pub fn handle(text: &str, open: Opener) -> JoinAction {
    if !is_cloud_link(text) {
        return JoinAction::NotJoin;
    }
    let Some(cloud) = super::get() else {
        return JoinAction::Off;
    };
    let linked = cloud.client.link();
    let email = cloud
        .client
        .is_signed_in()
        .then(|| cloud.client.account().map(|a| a.email))
        .flatten();
    let action = decide(text, linked.as_ref(), email.as_deref());
    let code_text = deep_link_code(text).unwrap_or_default();
    let parsed = parse_join_code(&code_text, linked.as_ref());
    tracing::info!(action = action.kind(), "copper://cloud/join");
    match action {
        JoinAction::Fill(offer) => JoinAction::Fill(sign_in().offer(&offer.text, offer.code)),
        JoinAction::StartSso { target, .. } => {
            // Not linked: the Connect field shows the link it came from. On
            // the instance linked already the Account step has it all.
            let offer = parsed
                .filter(|_| matches!(target, SsoTarget::Join(_)))
                .map(|code| sign_in().offer(&code_text, code));
            let center = sign_in();
            // A sign-in to that instance already under way: leave it be.
            if !center.state().running_for(target.endpoint()) {
                center.start(cloud, target.clone(), None, open);
            }
            JoinAction::StartSso { offer, target }
        }
        JoinAction::OtherInstance { host, linked, .. } => {
            let offer = parsed.map(|code| sign_in().offer(&code_text, code));
            JoinAction::OtherInstance {
                offer,
                host,
                linked,
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_cloud::parse_link_code;

    const FP: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

    fn encode(text: &str) -> String {
        url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
    }

    fn join(code: &str) -> String {
        format!("copper://cloud/join?code={}", encode(code))
    }

    fn sso() -> String {
        format!("copper-cloud://100.57.251.123:443/#sso=google&fp={FP}")
    }

    fn linked(host: &str) -> Link {
        parse_link_code(&format!("copper-cloud://{host}/#k=ck_key&fp={FP}")).unwrap()
    }

    #[test]
    fn only_copper_cloud_links_are_taken() {
        assert!(is_cloud_link("copper://cloud/join?code=x"));
        assert!(is_cloud_link("  COPPER://Cloud/join"));
        assert!(is_cloud_link("copper:cloud/join?code=x"));
        assert!(is_cloud_link("copper:///cloud/join?code=x"));
        assert!(!is_cloud_link("copper://canvas/personal"));
        assert!(!is_cloud_link("https://cloud/join"));
        assert!(!is_cloud_link("copper-cloud://h/#sso=google"));
        assert!(!is_cloud_link("cop"));
        assert_eq!(
            decide("https://example.com", None, None),
            JoinAction::NotJoin
        );
        assert!(!JoinAction::NotJoin.opens_settings());
    }

    #[test]
    fn a_join_link_not_linked_starts_the_sign_in() {
        let action = decide(&join(&sso()), None, None);
        let JoinAction::StartSso { target, offer } = &action else {
            panic!("{action:?}")
        };
        assert!(offer.is_none());
        let SsoTarget::Join(link) = target else {
            panic!("{target:?}")
        };
        assert_eq!(link.provider, "google");
        assert_eq!(link.fingerprint.as_deref(), Some(FP));
        assert_eq!(target.host(), "100.57.251.123:443");
        assert!(action.opens_settings());
        assert_eq!(action.message(), None);
        // A `#` left unencoded still reads whole, with its pin.
        let raw = format!("copper://cloud/join?code=copper-cloud://h.test/%23sso=google&fp={FP}");
        let JoinAction::StartSso {
            target: SsoTarget::Join(link),
            ..
        } = decide(&raw, None, None)
        else {
            panic!()
        };
        assert_eq!(link.fingerprint.as_deref(), Some(FP));
        // An ACME domain without a pin.
        let JoinAction::StartSso {
            target: SsoTarget::Join(link),
            ..
        } = decide(
            &join("copper-cloud://copper.dev.exowatt.com/#sso=google"),
            None,
            None,
        )
        else {
            panic!()
        };
        assert_eq!(link.fingerprint, None);
        assert_eq!(link.host(), "copper.dev.exowatt.com");
    }

    #[test]
    fn other_codes_not_linked_fill_the_field() {
        let code = format!("copper-cloud://h.test:8443/#k=ck_key&fp={FP}");
        let JoinAction::Fill(offer) = decide(&join(&code), None, None) else {
            panic!()
        };
        assert_eq!(offer.text, code);
        assert!(matches!(offer.code, JoinCode::Link(_)));
        assert_eq!(offer.host(), "h.test:8443");
        let pairing = format!("copper-cloud://h.test:8443/#p=cp_ABCDEFGHIJKLMNOPQRSTUV&fp={FP}");
        let JoinAction::Fill(offer) = decide(&join(&pairing), None, None) else {
            panic!()
        };
        assert!(matches!(offer.code, JoinCode::Pairing(_)));
    }

    #[test]
    fn linked_elsewhere_nothing_changes_without_a_press() {
        let other = linked("cloud.other.test:8443");
        for code in [
            sso(),
            format!("copper-cloud://100.57.251.123:443/#k=ck_x&fp={FP}"),
        ] {
            let action = decide(&join(&code), Some(&other), Some("me@other.test"));
            assert_eq!(
                action,
                JoinAction::OtherInstance {
                    offer: None,
                    host: "100.57.251.123:443".into(),
                    linked: "cloud.other.test:8443".into(),
                }
            );
            assert_eq!(
                action.message().as_deref(),
                Some("This link is for 100.57.251.123:443")
            );
        }
    }

    #[test]
    fn signed_in_there_already_says_so() {
        let here = linked("100.57.251.123:443");
        let action = decide(&join(&sso()), Some(&here), Some("farce@exowatt.com"));
        assert_eq!(
            action.message().as_deref(),
            Some("Already signed in to 100.57.251.123:443 as farce@exowatt.com")
        );
        // :443 is the same as no port.
        let here = linked("100.57.251.123");
        assert!(matches!(
            decide(&join(&sso()), Some(&here), Some("a@b.c")),
            JoinAction::AlreadySignedIn { .. }
        ));
    }

    #[test]
    fn linked_there_not_signed_in() {
        let here = linked("100.57.251.123:443");
        // A join link: sign in on the instance as linked (its own pin).
        let JoinAction::StartSso { target, .. } = decide(&join(&sso()), Some(&here), None) else {
            panic!()
        };
        assert_eq!(
            target,
            SsoTarget::Linked {
                endpoint: here.endpoint.clone(),
                provider: "google".into()
            }
        );
        // Anything else: signing in is on the page.
        let code = format!("copper-cloud://100.57.251.123:443/#k=ck_other&fp={FP}");
        assert_eq!(
            decide(&join(&code), Some(&here), None).message().as_deref(),
            Some("Already connected to 100.57.251.123:443 — sign in on the Cloud page")
        );
    }

    #[test]
    fn the_address_field_offers_to_join() {
        let text = join(&sso());
        let offers = palette_offers(&format!("  {text} ")).unwrap();
        assert_eq!(offers.default, Some(0));
        let row = &offers.rows[0];
        assert_eq!(row.title, "Join Copper Cloud — 100.57.251.123:443");
        assert_eq!(row.detail, "Opens Settings › Cloud");
        assert_eq!(row.action, Action::Go(text));
        assert_eq!(
            palette_row("copper://cloud/join").unwrap().title,
            "Copper Cloud link"
        );
        assert!(palette_row("example.com").is_none());
    }

    #[test]
    fn a_broken_link_is_called_incomplete() {
        for text in [
            "copper://cloud/join",
            "copper://cloud/join?code=",
            "copper://cloud/join?code=hello",
            "copper://cloud/somewhere?code=x",
        ] {
            let action = decide(text, None, None);
            assert_eq!(action, JoinAction::Incomplete, "{text}");
            assert!(action.opens_settings());
            assert!(action.message().unwrap().contains("incomplete"));
        }
    }
}
