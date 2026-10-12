//! The Secret Service store against a real gnome-keyring on a private bus.
//! `harness = false`: the bus address goes in the environment before any
//! thread exists.

use copper_creds::sandbox::PrivateSecretService;
use copper_creds::store::{self, SecretStore};

type Res = Result<String, String>;

fn ensure(cond: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(why()) }
}

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

async fn crud(s: &SecretStore) -> Res {
    s.save("example.com", "ann", "pw-1", None)
        .await
        .map_err(e)?;
    s.save("example.com", "bob", "pw-b", Some(100.0))
        .await
        .map_err(e)?;
    s.save("accounts.example.com", "cat", "pw-c", Some(200.0))
        .await
        .map_err(e)?;
    s.save("other.org", "ann", "pw-o", None).await.map_err(e)?;
    let all = s.all().await.map_err(e)?;
    ensure(all.len() == 4, || format!("all: {all:?}"))?;
    let here = s.logins_for("example.com").await.map_err(e)?;
    ensure(here.len() == 2, || format!("for: {here:?}"))?;
    let site = s.logins_matching("example.com").await.map_err(e)?;
    let users: Vec<&str> = site.iter().map(|l| l.user.as_str()).collect();
    ensure(users == ["cat", "bob", "ann"], || {
        format!("matching order {users:?}")
    })?;
    ensure(
        s.secret("example.com", "ann").await.map_err(e)?.as_deref() == Some("pw-1"),
        || "secret".into(),
    )?;
    // Update in place: one item, new secret, `used` kept.
    s.save("example.com", "bob", "pw-b2", None)
        .await
        .map_err(e)?;
    let bob = s.logins_for("example.com").await.map_err(e)?;
    let bob = bob.iter().find(|l| l.user == "bob").unwrap();
    ensure(bob.used == Some(100.0), || format!("used lost: {bob:?}"))?;
    ensure(
        s.secret("example.com", "bob").await.map_err(e)?.as_deref() == Some("pw-b2"),
        || "updated secret".into(),
    )?;
    ensure(s.all().await.map_err(e)?.len() == 4, || {
        "update duplicated".into()
    })?;
    s.touch("example.com", "ann", 300.0).await.map_err(e)?;
    let first = s.logins_matching("example.com").await.map_err(e)?;
    ensure(
        first[0].user == "ann" && first[0].used == Some(300.0),
        || format!("touch: {first:?}"),
    )?;
    ensure(
        s.secret("example.com", "ann").await.map_err(e)?.as_deref() == Some("pw-1"),
        || "touch changed the secret".into(),
    )?;
    s.forget("example.com", "ann").await.map_err(e)?;
    ensure(
        s.secret("example.com", "ann").await.map_err(e)?.is_none(),
        || "forget".into(),
    )?;
    let n = s.forget_origin("example.com").await.map_err(e)?;
    ensure(n == 1, || format!("forget_origin {n}"))?;
    ensure(s.all().await.map_err(e)?.len() == 2, || "left".into())?;
    ensure(
        s.save("", "x", "y", None).await.is_err() && s.save("h", "x", "", None).await.is_err(),
        || "empty accepted".into(),
    )?;
    Ok("save/list/update/touch/secret/forget".into())
}

/// Items carry app/origin/username as attributes another Secret Service
/// client can see (`secret-tool search`), and the test tag keeps its items
/// apart from Copper's real ones.
async fn attributes(s: &SecretStore, svc: &PrivateSecretService) -> Res {
    let real = SecretStore::open().await.map_err(e)?;
    real.save("tagged.example", "zed", "real-pw", None)
        .await
        .map_err(e)?;
    ensure(
        s.logins_for("tagged.example").await.map_err(e)?.is_empty(),
        || "test store sees real items".into(),
    )?;
    let out = std::process::Command::new("secret-tool")
        .args([
            "search",
            "--all",
            "app",
            "Copper",
            "origin",
            "tagged.example",
        ])
        .env("DBUS_SESSION_BUS_ADDRESS", svc.address())
        .output()
        .map_err(e)?;
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    ensure(
        text.contains("attribute.username = zed")
            && text.contains("attribute.origin = tagged.example"),
        || format!("secret-tool: {text}"),
    )?;
    let looked = store::lookup(&[("app", "Copper"), ("origin", "tagged.example")])
        .await
        .map_err(e)?;
    ensure(looked.as_deref() == Some(b"real-pw".as_slice()), || {
        "lookup".into()
    })?;
    real.forget_origin("tagged.example").await.map_err(e)?;
    Ok("app=Copper origin username visible to secret-tool; tags isolate".into())
}

/// Another application's key, the way Chrome keeps "Chrome Safe Storage".
async fn raw_items() -> Res {
    store::store_raw(
        "Chrome Safe Storage",
        &[
            ("xdg:schema", "chrome_libsecret_os_crypt_password_v2"),
            ("application", "chrome"),
        ],
        b"c2VjcmV0LWtleQ==",
    )
    .await
    .map_err(e)?;
    let got = store::lookup(&[("application", "chrome")])
        .await
        .map_err(e)?;
    ensure(
        got.as_deref() == Some(b"c2VjcmV0LWtleQ==".as_slice()),
        || "raw".into(),
    )?;
    ensure(
        store::lookup(&[("application", "nope")])
            .await
            .map_err(e)?
            .is_none(),
        || "phantom".into(),
    )?;
    Ok("store_raw / lookup".into())
}

fn main() {
    if std::env::args().any(|a| a == "--list") {
        println!("secret_service: test");
        return;
    }
    let svc = PrivateSecretService::start().expect("private Secret Service");
    // SAFETY: no other thread exists yet.
    unsafe { svc.export() };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let fails = rt.block_on(async {
        let s = SecretStore::open_tagged("Copper (test)")
            .await
            .expect("open");
        let mut fails = 0;
        for (name, r) in [
            ("crud", crud(&s).await),
            ("attributes", attributes(&s, &svc).await),
            ("raw", raw_items().await),
        ] {
            match r {
                Ok(d) => println!("PASS {name}: {d}"),
                Err(why) => {
                    fails += 1;
                    println!("FAIL {name}: {why}");
                }
            }
        }
        fails
    });
    drop(rt);
    drop(svc);
    std::process::exit(if fails == 0 { 0 } else { 1 });
}
