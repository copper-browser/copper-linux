//! The save offer: one quiet capsule over the top of the page, under the
//! field, after a sign-in took (Mac `App.swift` `keepAsking`). Offered once,
//! answered once; the password is never shown back.

use super::{Forms, OfferData, rt, source_title};
use crate::shell::Shell;
use crate::shell::pages::PageSlot;
use copper_core::TabId;
use copper_creds::credential::Source;
use copper_creds::offer::Sent;
use copper_shell_core::forms::OfferView;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// The capsule on a page.
pub struct Offer {
    root: gtk::Box,
    frame: glib::WeakRef<gtk::Overlay>,
    save: gtk::Button,
    later: gtk::Button,
}

impl OfferView for Offer {
    fn remove(&self) {
        if let Some(f) = self.frame.upgrade() {
            f.remove_overlay(&self.root);
        }
    }
}

impl Offer {
    /// Press Save (self-test), on the next turn of the loop.
    pub fn press_save(&self) {
        let b = self.save.clone();
        glib::idle_add_local_once(move || b.emit_clicked());
    }

    /// Press Not now (self-test), on the next turn of the loop.
    pub fn press_later(&self) {
        let b = self.later.clone();
        glib::idle_add_local_once(move || b.emit_clicked());
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }
}

/// The question, in the Mac's words.
pub fn question(sent: &Sent, target: Source, changed: bool) -> (String, String) {
    let vault = target != Source::Keychain;
    let name = source_title(target);
    let (user, host) = (&sent.user, &sent.host);
    let text = match (vault, changed) {
        (true, true) => format!("Update the password in {name} for {user} on {host}?"),
        (true, false) => format!("Save to {name} for {user} on {host}?"),
        (false, true) => format!("Update the password for {user} on {host}?"),
        (false, false) if user.is_empty() => format!("Save this password for {host}?"),
        (false, false) => format!("Save the password for {user} on {host}?"),
    };
    let button = match (vault, changed) {
        (true, true) => format!("Update in {name}"),
        (true, false) => format!("Save to {name}"),
        (false, true) => "Update".to_string(),
        (false, false) => "Save".to_string(),
    };
    (text, button)
}

pub fn show(
    shell: &Shell,
    forms: &Rc<Forms>,
    tab: &TabId,
    slot: &PageSlot,
    data: OfferData,
) -> Offer {
    let (text, primary) = question(&data.sent, data.target, data.changed);
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    root.add_css_class("copper-forms-offer");
    root.set_halign(gtk::Align::Center);
    root.set_valign(gtk::Align::Start);
    root.set_margin_top(12);
    root.set_margin_start(16);
    root.set_margin_end(16);
    root.update_property(&[gtk::accessible::Property::Label(&text)]);

    let key = gtk::Image::from_icon_name("dialog-password-symbolic");
    key.set_opacity(0.6);
    root.append(&key);
    let label = gtk::Label::new(Some(&text));
    label.add_css_class("text");
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(64);
    label.set_margin_end(6);
    root.append(&label);

    let save = gtk::Button::with_label(&primary);
    save.add_css_class("suggested-action");
    let later = gtk::Button::with_label("Not now");
    later.add_css_class("flat");
    root.append(&save);
    root.append(&later);
    let never = (!data.changed).then(|| {
        let b = gtk::Button::with_label("Never here");
        b.add_css_class("flat");
        root.append(&b);
        b
    });
    slot.frame.add_overlay(&root);

    let held: Rc<RefCell<Option<OfferData>>> = Rc::new(RefCell::new(Some(data)));
    {
        let held = held.clone();
        let weak = Rc::downgrade(forms);
        let shell = shell.downgrade();
        let tab = tab.clone();
        save.connect_clicked(move |_| {
            let (Some(f), Some(s), Some(d)) = (weak.upgrade(), shell.upgrade(), held.take()) else {
                return;
            };
            f.drop_offer(&tab);
            let v = f.vault.clone();
            glib::spawn_future_local(async move {
                let OfferData {
                    sent,
                    target,
                    changed,
                } = d;
                let host = sent.host.clone();
                let r =
                    rt::run(
                        async move { v.save(target, &sent.host, &sent.user, &sent.password).await },
                    )
                    .await
                    .unwrap_or_else(|| Err("Copper couldn't reach the keyring".into()));
                let vault = target != Source::Keychain;
                let name = source_title(target);
                let said = match r {
                    Ok(()) if changed && vault => format!("Password updated in {name} for {host}"),
                    Ok(()) if changed => format!("Password updated for {host}"),
                    Ok(()) if vault => format!("Password saved to {name}"),
                    Ok(()) => format!("Password saved for {host}"),
                    Err(e) if !vault => {
                        tracing::warn!(error = %e, "saving a password");
                        "The keyring refused it".to_string()
                    }
                    Err(e) => e,
                };
                s.toast(&said);
            });
        });
    }
    {
        let held = held.clone();
        let weak = Rc::downgrade(forms);
        let tab = tab.clone();
        later.connect_clicked(move |_| {
            held.take();
            if let Some(f) = weak.upgrade() {
                f.drop_offer(&tab);
            }
        });
    }
    if let Some(never) = never {
        let weak = Rc::downgrade(forms);
        let shell = shell.downgrade();
        let tab = tab.clone();
        never.connect_clicked(move |_| {
            let (Some(f), Some(s), Some(d)) = (weak.upgrade(), shell.upgrade(), held.take()) else {
                return;
            };
            f.drop_offer(&tab);
            super::set_never(&s, &d.sent.host, true);
            s.toast(&format!("Never for {}", d.sent.host));
        });
    }

    let frame = glib::WeakRef::new();
    frame.set(Some(slot.frame.overlay()));
    Offer {
        root,
        frame,
        save,
        later,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_wording() {
        let sent = Sent {
            host: "example.com".into(),
            user: "ann".into(),
            password: "pw".into(),
            at: 0.0,
        };
        assert_eq!(
            question(&sent, Source::Keychain, false),
            (
                "Save the password for ann on example.com?".into(),
                "Save".into()
            )
        );
        assert_eq!(
            question(&sent, Source::Bitwarden, true).1,
            "Update in Bitwarden"
        );
        let anon = Sent {
            user: String::new(),
            ..sent
        };
        assert_eq!(
            question(&anon, Source::Keychain, false).0,
            "Save this password for example.com?"
        );
    }
}
