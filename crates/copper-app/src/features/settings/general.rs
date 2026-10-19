//! Settings › General (Mac: `Settings.swift` `general`, `downloads`):
//! Copper as the default browser, the search engine, the download folder.

use super::{button, combo, group, line};
use crate::shell::GtkShell;
use crate::shell::Shell;
use copper_core::address::ENGINES;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::path::PathBuf;

/// The desktop entry other apps' links go to.
pub fn desktop_id() -> String {
    format!("{}.desktop", crate::APP_ID)
}

/// Running inside a Flatpak sandbox.
pub fn flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some_and(|v| !v.is_empty())
}

pub fn page(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let here = group(Some("Copper on this computer"));
    let default = line("Open links from other apps", "Checking…");
    let make = button("Make default", true);
    make.set_visible(false);
    let tick = gtk::Image::from_icon_name("object-select-symbolic");
    tick.set_visible(false);
    default.add_suffix(&tick);
    default.add_suffix(&make);
    here.add(&default);
    page.add(&here);
    show_default(&default, &make, &tick);
    let weak = shell.downgrade();
    let (row, tick2) = (default.clone(), tick.clone());
    make.connect_clicked(move |b| {
        let Some(s) = weak.upgrade() else { return };
        b.set_sensitive(false);
        let (row, b, tick) = (row.clone(), b.clone(), tick2.clone());
        let weak = s.downgrade();
        glib::spawn_future_local(async move {
            let worked = become_default().await;
            b.set_sensitive(true);
            show_default(&row, &b, &tick);
            if let Some(s) = weak.upgrade() {
                s.toast(if worked && is_default().await {
                    "Links now open here"
                } else {
                    "The desktop didn’t change it"
                });
            }
        });
    });

    let search = group(Some("Search"));
    let current = shell.settings().search_engine().id;
    let at = ENGINES.iter().position(|e| e.id == current).unwrap_or(0);
    let names: Vec<&str> = ENGINES.iter().map(|e| e.name).collect();
    search.add(&combo(
        shell,
        "Search with",
        "What the field asks when you type words rather than an address",
        &names,
        at,
        |s, i| {
            if let Some(e) = ENGINES.get(i) {
                let id = e.id;
                s.update_settings(|st| st.set_search_engine(id));
            }
        },
    ));
    page.add(&search);

    let downloads = group(Some("Downloads"));
    let folder = shell.downloads().folder(shell);
    let save_to = line("Save to", &pretty_path(&folder));
    let change = button("Change…", false);
    save_to.add_suffix(&change);
    downloads.add(&save_to);
    let weak = shell.downgrade();
    let row = save_to.clone();
    change.connect_clicked(move |_| {
        let Some(s) = weak.upgrade() else { return };
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Save downloads to");
        dialog.set_initial_folder(Some(&gio::File::for_path(s.downloads().folder(&s))));
        let weak = s.downgrade();
        let row = row.clone();
        dialog.select_folder(Some(s.window()), gio::Cancellable::NONE, move |r| {
            let (Some(s), Ok(f)) = (weak.upgrade(), r) else {
                return;
            };
            let Some(path) = f.path() else { return };
            set_folder(&s, &path);
            row.set_subtitle(&pretty_path(&path));
        });
    });
    page.add(&downloads);
    page
}

/// Downloads go to `dir` from now on (setting `downloads`, every profile).
pub fn set_folder(shell: &Shell, dir: &std::path::Path) {
    let p = dir.to_string_lossy().to_string();
    shell.update_settings(|st| st.set_downloads_dir(&p));
    shell.downloads().set_folder(shell, dir);
}

/// `~/Downloads` rather than `/home/me/Downloads`.
pub fn pretty_path(p: &std::path::Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.display())
        };
    }
    p.display().to_string()
}

fn show_default(row: &adw::ActionRow, make: &gtk::Button, tick: &gtk::Image) {
    let (row, make, tick) = (row.clone(), make.clone(), tick.clone());
    glib::spawn_future_local(async move {
        let yes = is_default().await;
        tick.set_visible(yes);
        make.set_visible(!yes && !flatpak());
        row.set_subtitle(if yes {
            "Copper is the default browser on this computer"
        } else if flatpak() {
            "Choose Copper under Default Applications in your system settings"
        } else {
            "Mail, chat and the rest still send links elsewhere"
        });
    });
}

/// Run a command, its standard output when it worked.
async fn output(argv: &[&str]) -> Option<String> {
    let args: Vec<&std::ffi::OsStr> = argv.iter().map(std::ffi::OsStr::new).collect();
    let p = gio::Subprocess::newv(
        &args,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    )
    .ok()?;
    let (out, _) = p.communicate_utf8_future(None).await.ok()?;
    if !p.is_successful() {
        return None;
    }
    Some(out.map(|s| s.to_string()).unwrap_or_default())
}

/// Copper is the desktop's web browser (`xdg-settings`, or GIO's handler
/// for `http` when `xdg-settings` is missing).
pub async fn is_default() -> bool {
    if let Some(out) = output(&["xdg-settings", "get", "default-web-browser"]).await {
        return out.trim() == desktop_id();
    }
    gio::AppInfo::default_for_uri_scheme("http")
        .and_then(|a| a.id())
        .is_some_and(|id| id == desktop_id())
}

/// Ask the desktop to send links here.
async fn become_default() -> bool {
    if output(&["xdg-settings", "set", "default-web-browser", &desktop_id()])
        .await
        .is_some()
    {
        return true;
    }
    // No xdg-utils: tell GIO directly (mimeapps.list).
    let Some(app) = gio::AppInfo::all()
        .into_iter()
        .find(|a| a.id().is_some_and(|id| id == desktop_id()))
    else {
        return false;
    };
    [
        "x-scheme-handler/http",
        "x-scheme-handler/https",
        "text/html",
    ]
    .iter()
    .all(|t| app.set_as_default_for_type(t).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_read_short() {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return;
        }
        assert_eq!(
            pretty_path(&PathBuf::from(&home).join("Downloads")),
            "~/Downloads"
        );
        assert_eq!(pretty_path(std::path::Path::new("/tmp/x")), "/tmp/x");
        assert_eq!(desktop_id(), "com.copperbrowser.Copper.desktop");
    }
}
