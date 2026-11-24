//! `browser_handoff`'s bar (Mac `HandoffSlot`/`HandoffBar`): a capsule at
//! the bottom centre of the stage, "<agent> needs you on <tab>: <message>",
//! with Show (only when the tab is elsewhere: the person's click goes to it,
//! never the agent's), Done and Not now. One ask at a time, oldest first.
//! The model is copper-shell-core's [`copper_shell_core::agents::handoff`].
//!
//! Copper in the background does not ask for attention here: GTK 4 has no
//! urgency hint, and presenting the window would take focus.

use crate::shell::{GtkShell, Shell};
use copper_core::TabId;
use copper_mcp::HandoffAnswer;
use copper_shell_core::agents::handoff;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;

const STYLE: &str = r#"
.copper-handoff {
  margin-bottom: 16px; padding: 8px 10px 8px 16px; border-radius: 22px;
  background-color: @copper_ground; color: @copper_ink;
  box-shadow: 0 0 0 1px alpha(@copper_ink, .10), 0 6px 20px alpha(black, .12);
}
.copper-handoff .hand { color: @copper_accent; }
.copper-handoff .says { font-size: 12.5px; }
.copper-handoff button {
  min-height: 24px; padding: 0 10px; border-radius: 12px; font-size: 12px;
  background: transparent; box-shadow: none; color: @copper_ink;
}
.copper-handoff button:hover { background-color: alpha(@copper_ink, .07); }
.copper-handoff button.show { font-weight: 500; }
.copper-handoff button.done { background-color: @copper_accent; color: white; }
.copper-handoff button.done:hover { background-color: mix(@copper_accent, black, .1); }
.copper-handoff button.later { color: alpha(@copper_ink, .62); }
"#;

/// The bar's widgets and the ask it shows.
struct Widgets {
    root: gtk::Box,
    says: gtk::Label,
    show: gtk::Button,
    done: gtk::Button,
    shown: RefCell<Option<(u64, TabId)>>,
}

thread_local! {
    static BAR: RefCell<Option<std::rc::Rc<Widgets>>> = const { RefCell::new(None) };
}

/// Build the bar (hidden) over the stage and follow the asks.
pub fn install(shell: &Shell, agents: &super::Server) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(STYLE);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }
    let root: gtk::Box = glib::Object::builder()
        .property("orientation", gtk::Orientation::Horizontal)
        .property("spacing", 12)
        .property("accessible-role", gtk::AccessibleRole::Group)
        .build();
    root.add_css_class("copper-handoff");
    root.set_halign(gtk::Align::Center);
    root.set_valign(gtk::Align::End);
    root.set_visible(false);
    // No hand in the icon theme: the person the step is for.
    let hand = gtk::Image::from_icon_name("avatar-default-symbolic");
    hand.add_css_class("hand");
    hand.set_pixel_size(14);
    hand.set_valign(gtk::Align::Center);
    root.append(&hand);
    let says = gtk::Label::new(None);
    says.add_css_class("says");
    says.set_wrap(true);
    says.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    says.set_lines(2);
    says.set_ellipsize(gtk::pango::EllipsizeMode::End);
    // About 520 px of 12.5 px text.
    says.set_max_width_chars(66);
    says.set_xalign(0.0);
    root.append(&says);
    let show = button("Show", "show", "Go to the tab the agent handed you");
    let done = button("Done", "done", "");
    let later = button("Not now", "later", "");
    root.append(&show);
    root.append(&done);
    root.append(&later);
    shell.ui().stage.overlay.add_overlay(&root);

    let bar = std::rc::Rc::new(Widgets {
        root,
        says,
        show: show.clone(),
        done: done.clone(),
        shown: RefCell::default(),
    });
    BAR.with(|b| *b.borrow_mut() = Some(bar));
    let weak = shell.downgrade();
    show.connect_clicked(move |_| {
        if let (Some(s), Some((_, tab))) = (weak.upgrade(), current()) {
            s.select(&tab);
        }
    });
    done.connect_clicked(|_| answer(HandoffAnswer::Done));
    later.connect_clicked(|_| answer(HandoffAnswer::NotNow));
    handoff::follow(shell, agents, draw);
}

fn button(label: &str, class: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class(class);
    b.set_valign(gtk::Align::Center);
    if !tip.is_empty() {
        b.set_tooltip_text(Some(tip));
    }
    b
}

/// The ask on show: its id and tab.
fn current() -> Option<(u64, TabId)> {
    BAR.with(|b| b.borrow().as_ref()?.shown.borrow().clone())
}

/// Done or Not now on the ask on show.
fn answer(answer: HandoffAnswer) {
    if let (Some((id, _)), Some(a)) = (current(), super::agents()) {
        a.handoff().answer(id, answer);
    }
}

/// What the bar says right now, when it shows (QA).
pub fn shown_text() -> Option<String> {
    BAR.with(|b| {
        let b = b.borrow();
        let w = b.as_ref()?;
        w.root.is_visible().then(|| {
            let show = if w.show.is_visible() { " [Show]" } else { "" };
            format!("{}{show} [Done] [Not now]", w.says.text())
        })
    })
}

fn draw(shell: &Shell) {
    let Some(w) = BAR.with(|b| b.borrow().clone()) else {
        return;
    };
    let Some(bar) = handoff::bar(shell) else {
        w.root.set_visible(false);
        w.shown.replace(None);
        return;
    };
    w.says.set_markup(&format!(
        "<span weight=\"600\">{}</span>{}",
        glib::markup_escape_text(&bar.lead()),
        glib::markup_escape_text(&bar.rest())
    ));
    w.show.set_visible(bar.elsewhere);
    w.done
        .set_tooltip_text(Some(&format!("Tell {} the step is done", bar.ask.agent)));
    w.root
        .update_property(&[gtk::accessible::Property::Label(&bar.label())]);
    w.shown.replace(Some((bar.ask.id, bar.ask.tab.clone())));
    w.root.set_visible(true);
}
