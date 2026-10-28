//! Settings › Tabs › Calendar: the calendars the Calendar pin's card lists
//! meetings from (copper-shell-core `calendar`). Each is a calendar's
//! secret iCal address, kept in `calendar.json` on this computer only.

use super::{button, line};
use crate::shell::Shell;
use copper_shell_core::calendar::{self, Feed};
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

/// The page the card's "Open Calendar Settings" opens.
pub const PAGE: &str = "tabs";

pub fn group(shell: &Shell) -> adw::PreferencesGroup {
    let g = super::group(Some("Calendar"));
    g.set_description(Some(&format!(
        "Rest the pointer on a Google Calendar pin to see your next meetings. {} The address is kept on this computer only.",
        calendar::WHERE
    )));
    for (feed, name) in calendar::list(shell) {
        g.add(&row(shell, &g, &feed, &name));
    }
    let entry = adw::EntryRow::new();
    entry.set_title("Secret address in iCal format");
    entry.set_show_apply_button(true);
    let weak = shell.downgrade();
    let shown = g.downgrade();
    entry.connect_apply(move |e| {
        let (Some(s), Some(g)) = (weak.upgrade(), shown.upgrade()) else {
            return;
        };
        match calendar::add(&s, &e.text()) {
            Ok(()) => {
                e.set_text("");
                if let Some((feed, name)) = calendar::list(&s).pop() {
                    // Above the address box, which stays last.
                    g.remove(e);
                    g.add(&row(&s, &g, &feed, &name));
                    g.add(e);
                }
                s.toast("Calendar added");
            }
            Err(why) => s.toast(&why),
        }
    });
    g.add(&entry);
    g
}

/// A calendar: its name, whose it is (never the address), and Remove.
fn row(shell: &Shell, g: &adw::PreferencesGroup, feed: &Feed, name: &str) -> adw::ActionRow {
    let whose = calendar::owner(&feed.url).unwrap_or_else(|| crate::shell::host_of(&feed.url));
    let r = line(name, if whose == name { "" } else { &whose });
    let remove = button("Remove", false);
    let (weak, url) = (shell.downgrade(), feed.url.clone());
    let (shown, me) = (g.downgrade(), r.downgrade());
    remove.connect_clicked(move |_| {
        let Some(s) = weak.upgrade() else { return };
        match calendar::remove(&s, &url) {
            Ok(()) => {
                if let (Some(g), Some(me)) = (shown.upgrade(), me.upgrade()) {
                    g.remove(&me);
                }
            }
            Err(why) => s.toast(&why),
        }
    });
    r.add_suffix(&remove);
    r
}
