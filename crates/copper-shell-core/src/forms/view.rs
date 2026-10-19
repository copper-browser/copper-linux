//! What the forms lane asks of the app's widgets: the save offer (a quiet
//! capsule over the page) and the account list under a sign-in box. The
//! lane decides when each goes up and comes down; the app draws them (GTK:
//! `copper-app`'s `forms::{offer, picker}`).

use super::Forms;
use crate::Platform;
use crate::shell::Shell;
use crate::shell::pages::PageSlot;
use copper_core::TabId;
use copper_creds::credential::{Credential, Source};
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
}

/// One account in the list under a box.
#[derive(Clone, Debug)]
pub struct Row {
    pub credential: Credential,
    pub kind: RowKind,
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
