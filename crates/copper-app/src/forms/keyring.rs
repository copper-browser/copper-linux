//! The Secret Service, looked at without opening it (the forms lane's
//! [`copper_shell_core::forms::vault::Look`] on Linux): plain GIO D-Bus
//! reads that never prompt. Opening the store — which may prompt — is
//! `copper_creds::store::SecretStore::open`, on the first fill or save.

use copper_shell_core::forms::vault::Keyring;
use glib::prelude::*;

const SECRETS: &str = "org.freedesktop.secrets";

/// Look at the session's Secret Service without opening it: no prompt. A
/// service that is not running yet is left alone (`start` = false, launch)
/// or started by D-Bus activation (`start` = true: Settings, the first
/// fill). Blocking (plain synchronous D-Bus calls, any thread); the vault
/// runs it off the main loop.
pub fn look(start: bool) -> Keyring {
    // In a Flatpak oo7 keeps passwords in its own file, keyed by the secret
    // portal; whether that asks anything is the portal's business.
    if std::path::Path::new("/.flatpak-info").exists() {
        return Keyring::Closed {
            has_default: None,
            locked: false,
        };
    }
    look_bus(start).unwrap_or_else(Keyring::Unavailable)
}

fn look_bus(start: bool) -> Result<Keyring, String> {
    use gio::DBusCallFlags as F;
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|e| format!("no session bus: {e}"))?;
    let call = |dest: &str, path: &str, iface: &str, method: &str, args: glib::Variant, flags| {
        bus.call_sync(
            Some(dest),
            path,
            iface,
            method,
            Some(&args),
            None,
            flags,
            3000,
            gio::Cancellable::NONE,
        )
    };
    let dbus = |method: &str, args: glib::Variant| {
        call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            method,
            args,
            F::NONE,
        )
    };
    let running = dbus("NameHasOwner", (SECRETS,).to_variant())
        .map_err(|e| e.to_string())?
        .child_value(0)
        .get::<bool>()
        .unwrap_or(false);
    if !running {
        let startable = dbus("ListActivatableNames", ().to_variant())
            .ok()
            .and_then(|v| v.child_value(0).get::<Vec<String>>())
            .is_some_and(|names| names.iter().any(|n| n == SECRETS));
        if !startable {
            return Err("no Secret Service is running in this session".into());
        }
        if !start {
            return Ok(Keyring::Closed {
                has_default: None,
                locked: false,
            });
        }
        // ReadAlias below starts it.
    }
    let flags = if running { F::NO_AUTO_START } else { F::NONE };
    let alias = call(
        SECRETS,
        "/org/freedesktop/secrets",
        "org.freedesktop.Secret.Service",
        "ReadAlias",
        ("default",).to_variant(),
        flags,
    )
    .map_err(|e| e.to_string())?;
    let path = alias.child_value(0).str().unwrap_or("/").to_string();
    if path == "/" {
        return Ok(Keyring::Closed {
            has_default: Some(false),
            locked: false,
        });
    }
    let locked = call(
        SECRETS,
        &path,
        "org.freedesktop.DBus.Properties",
        "Get",
        ("org.freedesktop.Secret.Collection", "Locked").to_variant(),
        F::NO_AUTO_START,
    )
    .ok()
    .and_then(|v| v.child_value(0).as_variant())
    .and_then(|v| v.get::<bool>())
    .unwrap_or(false);
    Ok(Keyring::Closed {
        has_default: Some(true),
        locked,
    })
}
