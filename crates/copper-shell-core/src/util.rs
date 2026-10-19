//! Small shared helpers: curated page scripts into engine form, running a
//! curated call, the site key of an address.

use crate::main_loop::MainLoop;
use crate::{PageOf, Platform};
use copper_engine::{Frames, InjectionTime, JsonMap, PageView, UserScript, World};
use copper_pagejs::Call;
use serde::de::DeserializeOwned;

/// The engine form of a curated user script.
pub fn engine_script(s: &copper_pagejs::UserScript) -> UserScript {
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

/// Run a curated call in `page` and decode its answer.
pub async fn run<T: DeserializeOwned, V: PageView>(page: &V, call: &Call<T>) -> Result<T, String> {
    let world = engine_world(call.world());
    let v = page
        .call_async(&call.function_body(), &JsonMap::new(), &world)
        .await
        .map_err(|e| e.to_string())?;
    call.decode_value(v).map_err(|e| e.to_string())
}

/// Fire a curated call and forget its answer (logged when it fails).
pub fn fire<P: Platform, T: DeserializeOwned + 'static>(page: &PageOf<P>, call: Call<T>) {
    let page = page.clone();
    P::Loop::spawn_local(async move {
        if let Err(e) = run(&page, &call).await {
            tracing::debug!(script = ?call.script(), error = %e, "page call");
        }
    });
}

/// The site an address belongs to, as `hidden.json` and `shield.paused`
/// key it: the host, lowercased, without a leading `www.` (Mac
/// `Curtain.host(of:)`). `None` for addresses without a host.
pub fn site_key(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = u.host_str()?.trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    Some(copper_shield::curtain::key_for_host(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_keys_like_the_mac() {
        assert_eq!(
            site_key("https://WWW.Example.com/a?b").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            site_key("http://127.0.0.1:8080/").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            site_key("https://news.example.com/").as_deref(),
            Some("news.example.com")
        );
        assert_eq!(site_key("about:blank"), None);
        assert_eq!(site_key("file:///tmp/a.html"), None);
        assert_eq!(site_key("data:text/html,x"), None);
    }
}
