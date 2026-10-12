//! Command-line options. Parsed by hand before GTK sees `argv`, so GTK never
//! complains about flags it does not know.

use std::path::PathBuf;

/// What the binary was asked to do.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    /// Addresses (or words) to open in new tabs after the session is back.
    pub open: Vec<String>,
    /// Debug builds: save a PNG of the whole window here once the active
    /// page has loaded, then quit (`--screenshot <path>` or
    /// `COPPER_SCREENSHOT`).
    pub screenshot: Option<PathBuf>,
    /// Debug builds: run the scripted UI self-test, phase `name`, writing
    /// screenshots into `out` (`--selftest-ui <phase> [--out <dir>]`).
    pub selftest: Option<String>,
    pub selftest_out: Option<PathBuf>,
    /// Print the version and exit.
    pub version: bool,
    /// Print usage and exit.
    pub help: bool,
}

pub const USAGE: &str = "\
usage: copper [options] [ADDRESS…]

  ADDRESS…               open each in a new tab
  --version              print the version
  --help                 this text
debug builds only:
  --screenshot PATH      save a PNG of the window once the page has loaded, then quit
                         (also COPPER_SCREENSHOT=PATH)
  --selftest-ui PHASE    run the scripted UI test (fresh | restore | mac)
  --out DIR              where the self-test writes its screenshots

Set COPPER_WORLD=<name> to keep everything in separate copper-<name> folders.
";

impl Options {
    /// Parse `args` (without the program name). Unknown `--flags` are an error.
    pub fn parse<I, S>(args: I) -> Result<Options, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut o = Options::default();
        let mut it = args.into_iter().map(Into::into);
        while let Some(a) = it.next() {
            let (flag, inline) = match a.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
                _ => (a.clone(), None),
            };
            let mut value = |name: &str| -> Result<String, String> {
                inline
                    .clone()
                    .or_else(|| it.next())
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match flag.as_str() {
                "--version" | "-V" => o.version = true,
                "--help" | "-h" => o.help = true,
                "--screenshot" => o.screenshot = Some(PathBuf::from(value("--screenshot")?)),
                "--selftest-ui" => o.selftest = Some(value("--selftest-ui")?),
                "--out" => o.selftest_out = Some(PathBuf::from(value("--out")?)),
                f if f.starts_with("--") => return Err(format!("unknown option {f}")),
                _ => o.open.push(a),
            }
        }
        if o.screenshot.is_none()
            && let Ok(p) = std::env::var("COPPER_SCREENSHOT")
            && !p.trim().is_empty()
        {
            o.screenshot = Some(PathBuf::from(p));
        }
        Ok(o)
    }

    /// A scripted run (screenshot or self-test): never the user's own
    /// single instance.
    pub fn scripted(&self) -> bool {
        self.screenshot.is_some() || self.selftest.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flags_and_addresses() {
        let o = Options::parse(["--screenshot", "/tmp/a.png", "example.com", "--out=/tmp/x"]).unwrap();
        assert_eq!(o.screenshot, Some(PathBuf::from("/tmp/a.png")));
        assert_eq!(o.selftest_out, Some(PathBuf::from("/tmp/x")));
        assert_eq!(o.open, ["example.com"]);
        assert!(o.scripted());
        let o = Options::parse(["--selftest-ui=fresh"]).unwrap();
        assert_eq!(o.selftest.as_deref(), Some("fresh"));
        assert!(Options::parse(["--nope"]).is_err());
        assert!(Options::parse(["--screenshot"]).is_err());
    }
}
