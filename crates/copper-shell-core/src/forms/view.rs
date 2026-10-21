//! What the forms lane asks of the app's widgets: the save offer (a quiet
//! capsule over the page) and the account list under a sign-in box. The
//! lane decides when each goes up and comes down; the app draws them (GTK:
//! `copper-app`'s `forms::{offer, picker}`).

use super::Forms;
use crate::Platform;
use crate::shell::Shell;
use crate::shell::pages::PageSlot;
use copper_core::TabId;
use copper_creds::credential::{
    AgentHint, AutofillCard, AutofillIdentity, Credential, CredentialId, Source,
};
use copper_creds::offer::Sent;
use copper_pagejs::messages::FieldRect;
use std::rc::Rc;

/// What is offered for saving.
pub struct OfferData {
    pub sent: Sent,
    pub target: Source,
    /// The account is kept already, with another password: offer to update.
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// Fill the name and password.
    Login,
    /// Fill the one-time code.
    Code,
    /// Fill an address form from a vault identity (`autofill.everything`).
    Identity,
    /// Fill a checkout's card boxes from a vault card (`autofill.everything`).
    Card,
}

/// One row in the list under a box: an account, or (under an address or
/// card box) a vault identity or card.
///
/// An identity or card row carries no values — only what the row shows:
/// its `credential` stands in, with the vault item's id and source, the
/// title in `user` (the name, "Visa •••• 1234") and the line under it in
/// `host` (the address, the cardholder). The values are read from the
/// unlocked vault when the row is picked ([`Forms::pick`]), and only then
/// enter the page.
#[derive(Clone, Debug)]
pub struct Row {
    pub credential: Credential,
    pub kind: RowKind,
}

impl Row {
    /// An identity's row (`Accounts.swift` `.identity`).
    pub fn identity(i: &AutofillIdentity) -> Row {
        let name = i.full_name();
        let title = if name.is_empty() {
            i.name.clone()
        } else {
            name
        };
        Row::stand_in(&i.id, &i.name, title, i.summary(), RowKind::Identity)
    }

    /// A card's row (`Accounts.swift` `.card`).
    pub fn card(c: &AutofillCard) -> Row {
        let label = c.label();
        let title = if label.is_empty() {
            c.name.clone()
        } else {
            label
        };
        let under = if c.cardholder_name.is_empty() {
            c.name.clone()
        } else {
            c.cardholder_name.clone()
        };
        Row::stand_in(&c.id, &c.name, title, under, RowKind::Card)
    }

    fn stand_in(id: &str, name: &str, title: String, under: String, kind: RowKind) -> Row {
        let (id, source) = match id.strip_prefix("op:") {
            Some(op) => (CredentialId::OnePassword(op.into()), Source::OnePassword),
            None => (CredentialId::Bitwarden(id.into()), Source::Bitwarden),
        };
        Row {
            credential: Credential {
                id,
                source,
                host: under,
                user: title,
                sites: Vec::new(),
                name: name.into(),
                has_totp: false,
                used: None,
                folder: None,
                agent_hint: AgentHint::None,
            },
            kind,
        }
    }

    /// An identity's or card's id in the vault's autofill cache (1Password's
    /// carry `op:`); `None` for an account.
    pub fn autofill_id(&self) -> Option<String> {
        if !matches!(self.kind, RowKind::Identity | RowKind::Card) {
            return None;
        }
        match &self.credential.id {
            CredentialId::Bitwarden(id) => Some(id.clone()),
            CredentialId::OnePassword(id) => Some(format!("op:{id}")),
            CredentialId::Keychain { .. } | CredentialId::BotPass(_) => None,
        }
    }
}

/// A save offer on a page. Its buttons answer through [`Forms`]
/// (`drop_offer`, the vault, `set_never`).
pub trait OfferView: 'static {
    /// Take it off the page.
    fn remove(&self);
}

/// The account list on a page. A click on a row calls [`Forms::pick`].
pub trait PickerView: 'static {
    /// Take it off the page.
    fn remove(&self);
    /// Follow the box to `rect` (page CSS pixels at `zoom`): it scrolled or
    /// the page was resized.
    fn place(&self, rect: FieldRect, zoom: f64);
    /// A row was clicked and its fill is under way: the list stays up when
    /// the caret leaves the box meanwhile.
    fn busy(&self) -> bool;
    /// The rows' user names, in order (tests, agents).
    fn users(&self) -> Vec<String>;
}

/// The app's widgets for the forms lane.
pub trait FormsUi: Sized + 'static {
    type Platform: Platform;
    type Offer: OfferView;
    type Picker: PickerView;

    /// Put a save offer over `slot`'s page.
    fn show_offer(
        shell: &Shell<Self::Platform>,
        forms: &Rc<Forms<Self>>,
        tab: &TabId,
        slot: &Rc<PageSlot<Self::Platform>>,
        data: OfferData,
    ) -> Self::Offer;

    /// Put the account list under the box at `rect` on `slot`'s page.
    fn show_picker(
        shell: &Shell<Self::Platform>,
        forms: &Rc<Forms<Self>>,
        tab: &TabId,
        slot: &Rc<PageSlot<Self::Platform>>,
        rect: FieldRect,
        rows: Vec<Row>,
    ) -> Self::Picker;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_card_rows_carry_no_values() {
        let who = AutofillIdentity {
            id: "op:abc".into(),
            name: "Home".into(),
            first_name: "Ana".into(),
            last_name: "Lima".into(),
            city: "Springfield".into(),
            ssn: "123-45-6789".into(),
            ..Default::default()
        };
        let r = Row::identity(&who);
        assert_eq!(r.kind, RowKind::Identity);
        assert_eq!(r.credential.source, Source::OnePassword);
        assert_eq!(r.credential.user, "Ana Lima");
        assert_eq!(r.credential.host, "Springfield");
        assert_eq!(r.autofill_id().as_deref(), Some("op:abc"));
        assert!(!format!("{r:?}").contains("123-45"));

        let card = AutofillCard {
            id: "bw-1".into(),
            name: "Travel".into(),
            brand: "Visa".into(),
            number: "4111111111111234".into(),
            code: "987".into(),
            ..Default::default()
        };
        let r = Row::card(&card);
        assert_eq!(r.kind, RowKind::Card);
        assert_eq!(r.credential.source, Source::Bitwarden);
        assert_eq!(r.credential.user, "Visa •••• 1234");
        assert_eq!(r.credential.host, "Travel");
        assert_eq!(r.autofill_id().as_deref(), Some("bw-1"));
        let shown = format!("{r:?}");
        assert!(!shown.contains("4111") && !shown.contains("987"));
    }
}
