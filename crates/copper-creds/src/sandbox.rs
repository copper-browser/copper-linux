//! A private Secret Service for tests: its own session D-Bus and its own
//! `gnome-keyring-daemon --unlock --components=secrets` (empty password on
//! stdin, keyring files under a temporary HOME), so a test never touches the
//! user's keyring. Needs `dbus-daemon` and `gnome-keyring-daemon`.
//!
//! The bus address must be in `DBUS_SESSION_BUS_ADDRESS` before anything
//! connects; set it at the top of a test's `main` (before threads exist) with
//! [`PrivateSecretService::export`].

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct PrivateSecretService {
    dir: PathBuf,
    address: String,
    dbus: Child,
    keyring: Child,
}

impl PrivateSecretService {
    /// Start both daemons and wait until `org.freedesktop.secrets` is owned.
    pub fn start() -> std::io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("copper-secrets-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let runtime = dir.join("run");
        let home = dir.join("home");
        for d in [&runtime, &home] {
            std::fs::create_dir_all(d)?;
        }
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))?;
        }
        let mut dbus = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!(
                "--address=unix:path={}",
                runtime.join("bus").display()
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut line = String::new();
        BufReader::new(dbus.stdout.take().expect("piped")).read_line(&mut line)?;
        let address = line.trim().to_string();
        if address.is_empty() {
            let _ = dbus.kill();
            return Err(std::io::Error::other("dbus-daemon printed no address"));
        }
        let mut keyring = Command::new("gnome-keyring-daemon")
            .args(["--foreground", "--unlock", "--components=secrets"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // The empty password (one empty line; nothing at all creates no
        // login keyring), then EOF.
        if let Some(mut stdin) = keyring.stdin.take() {
            let _ = stdin.write_all(b"\n");
        }
        let me = PrivateSecretService {
            dir,
            address,
            dbus,
            keyring,
        };
        me.wait_for_name(Duration::from_secs(20))?;
        Ok(me)
    }

    fn wait_for_name(&self, limit: Duration) -> std::io::Result<()> {
        let started = Instant::now();
        while started.elapsed() < limit {
            let out = Command::new("dbus-send")
                .args([
                    "--session",
                    "--print-reply",
                    "--dest=org.freedesktop.DBus",
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus.NameHasOwner",
                    "string:org.freedesktop.secrets",
                ])
                .env("DBUS_SESSION_BUS_ADDRESS", &self.address)
                .output()?;
            if String::from_utf8_lossy(&out.stdout).contains("boolean true") {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(std::io::Error::other(
            "gnome-keyring-daemon never took org.freedesktop.secrets",
        ))
    }

    /// `unix:path=…` of the private bus.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The temporary HOME the keyring keeps its files in.
    pub fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Point this process at the private bus.
    ///
    /// # Safety
    /// Sets an environment variable: call before any other thread exists
    /// (first thing in a `harness = false` test's `main`).
    pub unsafe fn export(&self) {
        // SAFETY: the caller guarantees no other thread reads the
        // environment concurrently.
        unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &self.address) };
    }
}

impl Drop for PrivateSecretService {
    fn drop(&mut self) {
        let _ = self.keyring.kill();
        let _ = self.keyring.wait();
        let _ = self.dbus.kill();
        let _ = self.dbus.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
