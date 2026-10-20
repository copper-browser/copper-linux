//! Every live page gets the forms relay and is listened to (`officeForms`
//! messages and load events), on its own event stream beside the shell's.
//!
//! * A sign-in the page sends is held; it is judged once the page moved on
//!   (a new document, or its boxes went away by themselves and stayed away),
//!   and offered for saving only when the page no longer asks for a
//!   password — the sign-in took ([`copper_creds::offer`]).
//! * The caret landing in a name or password box offers that site's saved
//!   accounts in a list under the box; a code box offers the one-time codes.
//!   Nothing is filled until a row is clicked.

use super::view::{FormsUi, OfferData, OfferView, PickerView, Row, RowKind};
use super::{Forms, rt, site_host};
use crate::control::ShellEvent;
use crate::main_loop::{MainLoop, Task, spawn};
use crate::shell::Shell;
use crate::shell::pages::PageSlot;
use crate::{PageOf, Platform};
use copper_core::TabId;
use copper_creds::credential::{self, Credential, Source};
use copper_creds::offer::{self as judge, Context, Decision, Outcome, Sent, Settle, SignInWatch};
use copper_engine::{JsonMap, LoadState, PageEvent, PageId, PageView, UserScript, World};
use copper_pagejs::Call;
use copper_pagejs::messages::{FORMS, FieldInfo, FieldRect, FormsMessage};
use serde::de::DeserializeOwned;
use std::rc::Rc;
use std::time::Duration;

/// The main loop the forms lane runs on.
type Loop<U> = <<U as FormsUi>::Platform as Platform>::Loop;
/// A tab's page slot.
type Slot<U> = Rc<PageSlot<<U as FormsUi>::Platform>>;

/// One tab's forms state; dropped with its page.
pub(crate) struct TabForms<U: FormsUi> {
    /// The page this state is for: a tab's new page (after a crash or a
    /// sleep) gets new state.
    page: PageId,
    pump: Option<Task>,
    pub watch: SignInWatch,
    pub offer: Option<U::Offer>,
    pub picker: Option<U::Picker>,
    /// A row was picked into this sign-in: the list stays down until the
    /// caret leaves the boxes.
    picked_into: bool,
    /// The account last filled here (its code goes first on the next page).
    pub signed_with: Option<Credential>,
    /// The field the list is up for (`kind|label`).
    anchor: Option<String>,
    /// Bumped on every caret move: an answer for an older one is dropped.
    focus_gen: u64,
    /// The list's lowering a beat after the caret left ([`Forms::lower_soon`]).
    lowering: Option<Task>,
}

impl<U: FormsUi> Drop for TabForms<U> {
    fn drop(&mut self) {
        if let Some(p) = self.pump.take() {
            p.abort();
        }
        if let Some(t) = self.lowering.take() {
            t.abort();
        }
        if let Some(o) = self.offer.take() {
            o.remove();
        }
        if let Some(p) = self.picker.take() {
            p.remove();
        }
    }
}

/// The engine form of a curated user script.
pub fn engine_script(s: &copper_pagejs::UserScript) -> UserScript {
    use copper_engine::{Frames, InjectionTime};
    let mut u = UserScript::new(s.source.clone());
    u.injection_time = match s.injection {
        copper_pagejs::Injection::DocumentStart => InjectionTime::DocumentStart,
        copper_pagejs::Injection::DocumentEnd => InjectionTime::DocumentEnd,
    };
    u.frames = match s.frames {
        copper_pagejs::Frames::MainFrame => Frames::TopOnly,
        copper_pagejs::Frames::AllFrames => Frames::All,
    };
    u.world = engine_world(s.world);
    u
}

pub fn engine_world(w: copper_pagejs::World) -> World {
    match w {
        copper_pagejs::World::Page => World::Page,
        copper_pagejs::World::Isolated(n) => World::Isolated(n.to_string()),
    }
}

/// Run a curated call in a page and decode its answer. The arguments
/// (a password, a code) go into the page and nowhere else.
pub async fn call<T: DeserializeOwned, V: PageView>(page: &V, call: &Call<T>) -> Result<T, String> {
    let world = engine_world(call.world());
    let v = page
        .call_async(&call.function_body(), &JsonMap::new(), &world)
        .await
        .map_err(|e| e.to_string())?;
    call.decode_value(v).map_err(|e| e.to_string())
}

/// Listen to the shell: every page that appears gets the relay.
pub fn start<U: FormsUi>(forms: &Rc<Forms<U>>, shell: &Shell<U::Platform>) {
    let mut rx = shell.hub().subscribe();
    let weak = Rc::downgrade(forms);
    Loop::<U>::spawn_local(async move {
        loop {
            let ev = rx.recv().await;
            let Some(f) = weak.upgrade() else { break };
            match ev {
                Ok(ShellEvent::TabClosed(id)) => {
                    f.tabs.borrow_mut().remove(&id);
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => f.sweep(),
                Err(_) => break,
            }
        }
    });
    // Pages also appear without an event (a profile change rebuilds them).
    let weak = Rc::downgrade(forms);
    Loop::<U>::spawn_local(async move {
        loop {
            Loop::<U>::sleep(Duration::from_millis(700)).await;
            let Some(f) = weak.upgrade() else { break };
            f.sweep();
        }
    });
    forms.sweep();
}

impl<U: FormsUi> Forms<U> {
    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// Attach to every page not yet attached; forget pages that went away.
    pub(crate) fn sweep(self: &Rc<Self>) {
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let pages: Vec<(TabId, Slot<U>)> = shell
            .pages_map()
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let fresh: Vec<(TabId, Slot<U>)> = {
            let mut tabs = self.tabs.borrow_mut();
            tabs.retain(|id, _| pages.iter().any(|(p, _)| p == id));
            pages
                .into_iter()
                .filter(|(id, slot)| tabs.get(id).is_none_or(|t| t.page != slot.page.page_id()))
                .collect()
        };
        for (id, slot) in fresh {
            let state = self.attach(&id, &slot);
            self.tabs.borrow_mut().insert(id, state);
        }
    }

    fn attach(self: &Rc<Self>, id: &TabId, slot: &Slot<U>) -> TabForms<U> {
        let page = slot.page.clone();
        page.add_message_handler(FORMS, &World::Page);
        let relay = copper_pagejs::forms::relay();
        page.add_user_script(&engine_script(&relay));
        // The document already there (the relay is idempotent).
        let now = page.eval(&relay.source, &World::Page);
        Loop::<U>::spawn_local(async move {
            let _ = now.await;
        });
        let mut events = page.events();
        let weak = Rc::downgrade(self);
        let tab = id.clone();
        let pump = spawn::<Loop<U>>(async move {
            while let Some(ev) = events.next().await {
                let Some(f) = weak.upgrade() else { break };
                f.on_event(&tab, ev);
            }
        });
        TabForms {
            page: page.page_id(),
            pump: Some(pump),
            watch: SignInWatch::default(),
            offer: None,
            picker: None,
            picked_into: false,
            signed_with: None,
            anchor: None,
            focus_gen: 0,
            lowering: None,
        }
    }

    fn with_tab<R>(&self, id: &TabId, f: impl FnOnce(&mut TabForms<U>) -> R) -> Option<R> {
        self.tabs.borrow_mut().get_mut(id).map(f)
    }

    /// `f` on `tab`'s save offer, while one is up (self-tests press it).
    pub fn with_offer<R>(&self, tab: &TabId, f: impl FnOnce(&U::Offer) -> R) -> Option<R> {
        self.tabs.borrow().get(tab)?.offer.as_ref().map(f)
    }

    /// `f` on `tab`'s account list, while it is up (self-tests press a row).
    pub fn with_picker<R>(&self, tab: &TabId, f: impl FnOnce(&U::Picker) -> R) -> Option<R> {
        self.tabs.borrow().get(tab)?.picker.as_ref().map(f)
    }

    fn on_event(self: &Rc<Self>, tab: &TabId, ev: PageEvent) {
        match ev {
            PageEvent::ScriptMessage { name, body } if name == FORMS => {
                match serde_json::from_value::<FormsMessage>(body) {
                    Ok(FormsMessage::Submit { user, password }) => {
                        self.on_submit(tab, user, password)
                    }
                    Ok(FormsMessage::Settled) => self.settle(tab, false),
                    Ok(FormsMessage::Focus {
                        typing,
                        rect,
                        field,
                        hint,
                    }) => self.on_focus(tab, typing, rect, field, hint),
                    Ok(_) => {}
                    Err(e) => tracing::debug!(error = %e, "officeForms message"),
                }
            }
            PageEvent::Load(LoadState::Started { .. }) => {
                self.lower_now(tab);
                self.with_tab(tab, |t| {
                    t.picked_into = false;
                });
            }
            PageEvent::Load(LoadState::Finished { .. }) => self.settle(tab, true),
            PageEvent::Crashed(_) => {
                self.lower_now(tab);
                self.with_tab(tab, |t| t.watch.clear());
            }
            _ => {}
        }
    }

    fn page_of(&self, tab: &TabId) -> Option<(Shell<U::Platform>, Slot<U>)> {
        let shell = self.shell.upgrade()?;
        let slot = shell.slot(tab)?;
        Some((shell, slot))
    }

    fn is_private(&self, tab: &TabId) -> bool {
        self.shell
            .upgrade()
            .is_some_and(|s| s.browser().tab(tab).is_none_or(|t| t.private))
    }

    // MARK: saving

    fn on_submit(self: &Rc<Self>, tab: &TabId, user: String, password: String) {
        if self.is_private(tab) {
            return;
        }
        let Some((_, slot)) = self.page_of(tab) else {
            return;
        };
        let host = slot.page.url().as_deref().and_then(site_host);
        let now = self.now();
        self.with_tab(tab, |t| {
            t.watch.sent(host.as_deref(), &user, &password, now)
        });
    }

    fn settle(self: &Rc<Self>, tab: &TabId, navigated: bool) {
        let now = self.now();
        let Some(next) = self.with_tab(tab, |t| t.watch.settle(navigated, now)) else {
            return;
        };
        self.follow(tab, next);
    }

    fn follow(self: &Rc<Self>, tab: &TabId, next: Settle) {
        match next {
            Settle::Nothing => {}
            Settle::RecheckAfter { delay, stamp } => {
                let weak = Rc::downgrade(self);
                let tab = tab.clone();
                Loop::<U>::spawn_local(async move {
                    Loop::<U>::sleep(delay).await;
                    let Some(f) = weak.upgrade() else { return };
                    let now = f.now();
                    if let Some(next) = f.with_tab(&tab, |t| t.watch.recheck(stamp, now)) {
                        f.follow(&tab, next);
                    }
                });
            }
            Settle::AskPage => self.ask(tab),
        }
    }

    fn ask(self: &Rc<Self>, tab: &TabId) {
        let Some((_, slot)) = self.page_of(tab) else {
            return;
        };
        let fut = slot.page.eval(judge::HAS_PASSWORD_JS, &World::Page);
        let weak = Rc::downgrade(self);
        let tab = tab.clone();
        Loop::<U>::spawn_local(async move {
            let still = fut.await.ok().and_then(|v| v.as_bool());
            let Some(f) = weak.upgrade() else { return };
            let took = f.with_tab(&tab, |t| t.watch.page_answered(still)).flatten();
            if let Some(sent) = took {
                f.judge(&tab, sent).await;
            }
        });
    }

    /// A sign-in that took: offer to save it, offer an update, or just
    /// mark the kept one used.
    async fn judge(self: &Rc<Self>, tab: &TabId, sent: Sent) {
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let (saves, never) = {
            let s = shell.settings();
            (s.saves_passwords(), super::never_list(&shell))
        };
        let private = self.is_private(tab);
        let snap = self.vault.snapshot();
        let target = judge::save_target(
            super::chosen_backend(&shell),
            snap.bw_unlocked(),
            snap.op_unlocked(),
            snap.op_save_vault.is_some(),
        );
        let v = self.vault.clone();
        let host = sent.host.clone();
        let known = rt::run(async move { v.known_users(target, &host).await })
            .await
            .unwrap_or_default();
        if target == Source::Keychain && !self.vault.snapshot().keyring_ready() {
            tracing::info!("no Secret Service: not offering to save");
            return;
        }
        let ctx = Context {
            saves_passwords: saves,
            private_tab: private,
            never: &never,
            target,
            known_users: &known,
        };
        let changed = match judge::decide(&sent, &ctx) {
            Decision::Ignore => return,
            Decision::OfferSave => false,
            Decision::Compare => {
                let v = self.vault.clone();
                let (host, user) = (sent.host.clone(), sent.user.clone());
                let kept = rt::run(async move { v.kept(target, &host, &user).await })
                    .await
                    .unwrap_or(Err(()));
                let current = match &kept {
                    Ok(p) => Ok(p.as_deref()),
                    Err(()) => Err(()),
                };
                match judge::after_compare(&sent, current) {
                    Outcome::Touch => {
                        let v = self.vault.clone();
                        let c = copper_creds::Login {
                            host: sent.host.clone(),
                            user: sent.user.clone(),
                            used: None,
                        }
                        .credential();
                        rt::run(async move { v.touch(&c).await }).await;
                        return;
                    }
                    Outcome::OfferUpdate => true,
                    Outcome::OfferSave => false,
                }
            }
        };
        self.show_offer(
            tab,
            OfferData {
                sent,
                target,
                changed,
            },
        );
    }

    fn show_offer(self: &Rc<Self>, tab: &TabId, data: OfferData) {
        let Some((shell, slot)) = self.page_of(tab) else {
            return;
        };
        // One offer at a time, as on the Mac.
        for t in self.tabs.borrow_mut().values_mut() {
            if let Some(o) = t.offer.take() {
                o.remove();
            }
        }
        let o = U::show_offer(&shell, self, tab, &slot, data);
        self.with_tab(tab, |t| t.offer = Some(o));
    }

    /// The offer was answered: take it down.
    pub fn drop_offer(&self, tab: &TabId) {
        if let Some(o) = self.with_tab(tab, |t| t.offer.take()).flatten() {
            o.remove();
        }
    }

    /// Whether `tab` shows a save offer (self-test, agents).
    pub fn offering(&self, tab: &TabId) -> bool {
        self.tabs
            .borrow()
            .get(tab)
            .is_some_and(|t| t.offer.is_some())
    }

    // MARK: the list under the box

    fn on_focus(
        self: &Rc<Self>,
        tab: &TabId,
        typing: bool,
        rect: Option<FieldRect>,
        field: Option<FieldInfo>,
        hint: String,
    ) {
        let stamp = self
            .with_tab(tab, |t| {
                t.focus_gen += 1;
                t.focus_gen
            })
            .unwrap_or(0);
        let (Some(rect), Some(field), true) = (rect, field, typing) else {
            self.with_tab(tab, |t| t.picked_into = false);
            self.lower_soon(tab);
            return;
        };
        let kind = field.kind.as_str();
        let Some(want) = Want::of(kind, &field.group) else {
            self.with_tab(tab, |t| t.picked_into = false);
            self.lower_soon(tab);
            return;
        };
        let Some((shell, slot)) = self.page_of(tab) else {
            return;
        };
        let on = {
            let s = shell.settings();
            match want {
                Want::Login | Want::Code => s.fills_passwords(),
                // `Browser.swift` `suggestions`: addresses and cards hang
                // from their boxes while "Fill addresses and cards" is on.
                Want::Identity | Want::Card => s.fills_everything(),
            }
        };
        if !on || self.is_private(tab) {
            return;
        }
        let picked = self.with_tab(tab, |t| t.picked_into).unwrap_or(false);
        if want != Want::Code && picked {
            return;
        }
        let key = format!("{kind}|{}", field.label);
        let same = self
            .with_tab(tab, |t| {
                t.picker.is_some() && t.anchor.as_deref() == Some(&key)
            })
            .unwrap_or(false);
        if same {
            // The box moved (scroll, resize): the list follows it.
            self.cancel_lowering(tab);
            let zoom = slot.page.zoom();
            self.with_tab(tab, |t| {
                if let Some(p) = &t.picker {
                    p.place(rect, zoom);
                }
            });
            return;
        }
        let Some(host) = slot.page.url().as_deref().and_then(site_host) else {
            return;
        };
        let signed = self.with_tab(tab, |t| t.signed_with.clone()).flatten();
        let v = self.vault.clone();
        let weak = Rc::downgrade(self);
        let tab = tab.clone();
        Loop::<U>::spawn_local(async move {
            let vault = v.clone();
            let rows: Option<Vec<Row>> = match want {
                Want::Identity => rt::run(async move { vault.identities().await })
                    .await
                    .map(|all| all.iter().map(Row::identity).collect()),
                Want::Card => rt::run(async move { vault.cards().await })
                    .await
                    .map(|all| all.iter().map(Row::card).collect()),
                Want::Login | Want::Code => None,
            };
            if let Some(rows) = rows {
                let Some(f) = weak.upgrade() else { return };
                if f.with_tab(&tab, |t| t.focus_gen) != Some(stamp) {
                    return;
                }
                if rows.is_empty() {
                    f.lower_now(&tab);
                } else {
                    f.show_picker(&tab, key, rect, rows);
                }
                return;
            }
            let h = host.clone();
            let all = rt::run(async move { v.candidates(&h).await })
                .await
                .unwrap_or_default();
            let Some(f) = weak.upgrade() else { return };
            if f.with_tab(&tab, |t| t.focus_gen) != Some(stamp) {
                return;
            }
            let rows: Vec<Row> = if want == Want::Code {
                let mut coded: Vec<Credential> = all.into_iter().filter(|c| c.has_totp).collect();
                if let Some(s) = signed.filter(|s| s.has_totp) {
                    coded.retain(|c| c.id != s.id);
                    coded.insert(0, s);
                }
                coded
                    .into_iter()
                    .map(|c| Row {
                        credential: c,
                        kind: RowKind::Code,
                    })
                    .collect()
            } else {
                credential::ranked(all, &host, &hint)
                    .into_iter()
                    .map(|c| Row {
                        credential: c,
                        kind: RowKind::Login,
                    })
                    .collect()
            };
            if rows.is_empty() {
                f.lower_now(&tab);
                return;
            }
            f.show_picker(&tab, key, rect, rows);
        });
    }

    fn show_picker(self: &Rc<Self>, tab: &TabId, key: String, rect: FieldRect, rows: Vec<Row>) {
        let Some((shell, slot)) = self.page_of(tab) else {
            return;
        };
        self.cancel_lowering(tab);
        if let Some(old) = self.with_tab(tab, |t| t.picker.take()).flatten() {
            old.remove();
        }
        let p = U::show_picker(&shell, self, tab, &slot, rect, rows);
        self.with_tab(tab, |t| {
            t.picker = Some(p);
            t.anchor = Some(key);
        });
    }

    fn cancel_lowering(&self, tab: &TabId) {
        if let Some(t) = self.with_tab(tab, |t| t.lowering.take()).flatten() {
            t.abort();
        }
    }

    /// The caret left: the list goes a beat later (a click on a row can
    /// take the caret out of the page first).
    fn lower_soon(self: &Rc<Self>, tab: &TabId) {
        let up = self.with_tab(tab, |t| t.picker.is_some()).unwrap_or(false);
        if !up {
            return;
        }
        self.cancel_lowering(tab);
        let weak = Rc::downgrade(self);
        let id = tab.clone();
        let source = spawn::<Loop<U>>(async move {
            Loop::<U>::sleep(Duration::from_millis(220)).await;
            if let Some(f) = weak.upgrade() {
                f.with_tab(&id, |t| t.lowering = None);
                if !f
                    .with_tab(&id, |t| t.picker.as_ref().is_some_and(|p| p.busy()))
                    .unwrap_or(false)
                {
                    f.lower_now(&id);
                }
            }
        });
        self.with_tab(tab, |t| t.lowering = Some(source));
    }

    pub(crate) fn lower_now(&self, tab: &TabId) {
        self.cancel_lowering(tab);
        if let Some(p) = self
            .with_tab(tab, |t| {
                t.anchor = None;
                t.picker.take()
            })
            .flatten()
        {
            p.remove();
        }
    }

    /// Whether `tab` shows the account list, and its rows' users.
    pub fn picker_rows(&self, tab: &TabId) -> Option<Vec<String>> {
        self.tabs
            .borrow()
            .get(tab)
            .and_then(|t| t.picker.as_ref().map(|p| p.users()))
    }

    /// A row was clicked: fetch its secret (only now) and fill.
    pub fn pick(self: &Rc<Self>, tab: &TabId, row: Row) {
        let Some((shell, slot)) = self.page_of(tab) else {
            return;
        };
        let v = self.vault.clone();
        let weak = Rc::downgrade(self);
        let tab = tab.clone();
        let page = slot.page.clone();
        if let Some(id) = row.autofill_id() {
            return self.pick_values(&shell, tab, &page, row.kind, id);
        }
        Loop::<U>::spawn_local(async move {
            let id = row.credential.id.clone();
            let code = row.kind == RowKind::Code;
            let got = rt::run(async move {
                if code {
                    v.totp(&id).await
                } else {
                    v.secret(&id).await
                }
            })
            .await
            .unwrap_or_else(|| Err("Copper couldn't reach the vault".into()));
            let Some(f) = weak.upgrade() else { return };
            let value = match got {
                Ok(v) => v,
                Err(e) => {
                    f.lower_now(&tab);
                    shell.toast(&e);
                    return;
                }
            };
            let filled = if code {
                call(&page, &copper_pagejs::forms::fill_otp(&value)).await
            } else {
                call(
                    &page,
                    &copper_pagejs::forms::fill_login(&row.credential.user, &value),
                )
                .await
            };
            drop(value);
            f.lower_now(&tab);
            if filled.ok().flatten() != Some(true) {
                shell.toast(if code {
                    "Couldn't find the code box anymore"
                } else {
                    "Couldn't find the sign-in fields anymore"
                });
                return;
            }
            let c = row.credential.clone();
            f.with_tab(&tab, |t| {
                t.picked_into = !code;
                t.signed_with = Some(c.clone());
            });
            let v = f.vault.clone();
            rt::run(async move { v.touch(&c).await }).await;
        });
    }

    /// An identity or card row was clicked: read its values from the
    /// unlocked vault (only now) and fill the boxes they fit
    /// (`Browser.swift` `choose(_ suggestion:)`). Nothing is put in a log or
    /// an announcement; the page answers only how many boxes it filled.
    fn pick_values(
        self: &Rc<Self>,
        shell: &Shell<U::Platform>,
        tab: TabId,
        page: &PageOf<U::Platform>,
        kind: RowKind,
        id: String,
    ) {
        let v = self.vault.clone();
        let weak = Rc::downgrade(self);
        let shell = shell.clone();
        let page = page.clone();
        Loop::<U>::spawn_local(async move {
            let values = rt::run(async move {
                match kind {
                    RowKind::Card => v
                        .cards()
                        .await
                        .iter()
                        .find(|c| c.id == id)
                        .map(super::mcp::card_values),
                    _ => v
                        .identities()
                        .await
                        .iter()
                        .find(|i| i.id == id)
                        .map(super::mcp::identity_values),
                }
            })
            .await
            .flatten();
            let Some(f) = weak.upgrade() else { return };
            let Some(values) = values else {
                f.lower_now(&tab);
                shell.toast("Copper couldn't reach the vault");
                return;
            };
            let filled = call(&page, &copper_pagejs::forms::fill_values(&values)).await;
            drop(values);
            f.lower_now(&tab);
            if filled.ok().flatten().unwrap_or(0) == 0 {
                shell.toast("Couldn't find the boxes anymore");
                return;
            }
            f.with_tab(&tab, |t| t.picked_into = true);
        });
    }

    /// Remember the account an agent signed in with (its code comes next).
    pub(crate) fn signed_in_with(&self, tab: &TabId, c: &Credential) {
        self.with_tab(tab, |t| t.signed_with = Some(c.clone()));
    }
}

/// What the box with the caret asks for (`Browser.swift` `suggestions`,
/// the page's `classify`/`groupFor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Want {
    /// A sign-in box: the site's accounts.
    Login,
    /// A one-time-code box: the accounts that keep a code.
    Code,
    /// An address/contact box: the vaults' identities.
    Identity,
    /// A card box: the vaults' cards.
    Card,
}

impl Want {
    fn of(kind: &str, group: &str) -> Option<Want> {
        match (kind, group) {
            ("username" | "password", _) => Some(Want::Login),
            ("otp", _) => Some(Want::Code),
            (_, "card") => Some(Want::Card),
            (_, "identity") => Some(Want::Identity),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Want;

    #[test]
    fn the_box_decides_the_rows() {
        assert_eq!(Want::of("username", "login"), Some(Want::Login));
        assert_eq!(Want::of("password", "login"), Some(Want::Login));
        assert_eq!(Want::of("otp", "login"), Some(Want::Code));
        assert_eq!(Want::of("cardNumber", "card"), Some(Want::Card));
        assert_eq!(Want::of("cardExp", "card"), Some(Want::Card));
        assert_eq!(Want::of("address1", "identity"), Some(Want::Identity));
        assert_eq!(Want::of("email", "identity"), Some(Want::Identity));
        assert_eq!(Want::of("other", "other"), None);
    }
}
