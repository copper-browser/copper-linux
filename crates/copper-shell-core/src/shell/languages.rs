//! The languages pages are told the user reads (`Accept-Language`,
//! `navigator.languages`), from the environment.
//!
//! WebKitGTK's own guess reads `setlocale(LC_CTYPE)` in each web process and
//! cuts it at the dot, so the `C.UTF-8` locale of containers, servers and
//! minimal installs reaches every site as the language `C`. Here the list is
//! read the way GLib reads it — `LANGUAGE` (a list), else `LC_ALL`,
//! `LC_MESSAGES`, `LANG` — turned into BCP 47 tags, with the C / POSIX
//! locale meaning American English, as WebKit itself means it.

/// What an unset, C or POSIX locale reads as.
pub const FALLBACK: &str = "en-US";

/// The page languages for this process's environment, best first.
pub fn from_env() -> Vec<String> {
    from_vars(|name| std::env::var(name).ok())
}

/// [`from_env`] over any variable source (tests).
pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let set = |name: &str| var(name).filter(|v| !v.trim().is_empty());
    let raw = set("LANGUAGE")
        .or_else(|| set("LC_ALL"))
        .or_else(|| set("LC_MESSAGES"))
        .or_else(|| set("LANG"))
        .unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for entry in raw.split(':') {
        if let Some(tag) = tag(entry)
            && !out.contains(&tag)
        {
            out.push(tag);
        }
    }
    if out.is_empty() {
        out.push(FALLBACK.to_string());
    }
    out
}

/// `de_DE.UTF-8@euro` → `de-DE`; `C`, `POSIX` and anything that is not a
/// language tag → `None`.
fn tag(entry: &str) -> Option<String> {
    let base = entry.trim();
    let base = base.split('@').next().unwrap_or(base);
    let base = base.split('.').next().unwrap_or(base);
    if base.is_empty() || base.eq_ignore_ascii_case("c") || base.eq_ignore_ascii_case("posix") {
        return None;
    }
    let mut parts = base.split(['_', '-']);
    let lang = parts.next()?;
    if !(2..=3).contains(&lang.len()) || !lang.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut out = lang.to_ascii_lowercase();
    for p in parts {
        if p.is_empty() || p.len() > 8 || !p.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        out.push('-');
        // Regions are upper case (`en-US`), scripts title case (`sr-Latn`).
        if p.len() == 2 {
            out.push_str(&p.to_ascii_uppercase());
        } else if p.len() == 4 {
            let mut c = p.chars();
            let first = c.next().map(|f| f.to_ascii_uppercase()).unwrap_or_default();
            out.push(first);
            out.push_str(&c.as_str().to_ascii_lowercase());
        } else {
            out.push_str(p);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        from_vars(move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        })
    }

    #[test]
    fn the_c_locale_reads_as_american_english() {
        assert_eq!(env(&[("LANG", "C.UTF-8")]), ["en-US"]);
        assert_eq!(env(&[("LANG", "C")]), ["en-US"]);
        assert_eq!(
            env(&[("LC_ALL", "POSIX"), ("LANG", "de_DE.UTF-8")]),
            ["en-US"]
        );
        assert_eq!(env(&[]), ["en-US"]);
        assert_eq!(env(&[("LANG", "")]), ["en-US"]);
    }

    #[test]
    fn locales_become_language_tags() {
        assert_eq!(env(&[("LANG", "de_DE.UTF-8")]), ["de-DE"]);
        assert_eq!(env(&[("LANG", "en_GB.utf8@euro")]), ["en-GB"]);
        assert_eq!(env(&[("LANG", "sr_RS@latin")]), ["sr-RS"]);
        assert_eq!(env(&[("LANG", "fr")]), ["fr"]);
        assert_eq!(env(&[("LANG", "zh_hant_TW.UTF-8")]), ["zh-Hant-TW"]);
    }

    #[test]
    fn glibs_order_language_then_lc_all_then_lc_messages_then_lang() {
        assert_eq!(
            env(&[
                ("LANGUAGE", "pt_BR:pt:en"),
                ("LC_ALL", "de_DE.UTF-8"),
                ("LANG", "C.UTF-8")
            ]),
            ["pt-BR", "pt", "en"]
        );
        assert_eq!(
            env(&[("LC_MESSAGES", "es_ES.UTF-8"), ("LANG", "it_IT.UTF-8")]),
            ["es-ES"]
        );
        assert_eq!(
            env(&[("LC_ALL", "nl_NL.UTF-8"), ("LC_MESSAGES", "es_ES.UTF-8")]),
            ["nl-NL"]
        );
    }

    #[test]
    fn junk_and_repeats_are_dropped() {
        assert_eq!(
            env(&[("LANGUAGE", "C:en_US:en_US.UTF-8::en")]),
            ["en-US", "en"]
        );
        assert_eq!(
            env(&[("LANGUAGE", "C"), ("LANG", "de_DE.UTF-8")]),
            ["en-US"]
        );
        assert_eq!(env(&[("LANG", "en_US; rm -rf")]), ["en-US"]);
    }
}
