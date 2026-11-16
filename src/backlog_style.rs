use crate::sh::Result;
use crate::vault_cli::{Cli, STYLES, StyleWrote};

pub const KIND: &str = "backlog";
pub const DEFAULT: &str = include_str!("../styles/backlog.md");
const STAMP: (&str, &str) = ("<!-- style-version:", "-->");

#[derive(Debug, PartialEq)]
pub enum Synced {
    Wrote(u32),
    Newer,
    Skipped,
}

#[derive(Debug, PartialEq)]
pub struct Applied {
    pub text: String,
    pub from: String,
}

fn stamp_of(line: &str) -> Option<u32> {
    line.trim().strip_prefix(STAMP.0)?.strip_suffix(STAMP.1)?.trim().parse().ok()
}

pub fn version(text: &str) -> u32 {
    text.lines().next().and_then(stamp_of).unwrap_or(0)
}

// The style as a reader sees it, without its version stamp.
pub fn unstamped(text: &str) -> &str {
    match text.split_once('\n') {
        Some((first, rest)) if stamp_of(first).is_some() => rest.trim_start_matches('\n'),
        _ => text,
    }
}

// A command not yet probed is asked, as `can` would otherwise trust it with a call an older vault does not know.
fn stated(cli: &Cli) -> bool {
    match &cli.capabilities {
        Some(stated) => stated.iter().any(|one| one == STYLES),
        None => cli.probe(false).is_ok_and(|probe| probe.can(STYLES)),
    }
}

pub fn sync(cli: &Cli) -> Result<Synced> {
    if !stated(cli) {
        return Ok(Synced::Skipped);
    }
    let version = version(DEFAULT);
    Ok(match cli.write_style(KIND, version, DEFAULT)? {
        StyleWrote::Saved => Synced::Wrote(version),
        StyleWrote::Newer => Synced::Newer,
    })
}

// The line setup, update and the keeper say: a write or a failure; a skip or a newer style in the vault says nothing.
pub fn said(synced: Result<Synced>) -> Option<String> {
    match synced {
        Ok(Synced::Wrote(version)) => Some(format!("Wrote kebbie's backlog style, version {version}, to the vault")),
        Ok(Synced::Newer | Synced::Skipped) => None,
        Err(error) => Some(format!("kebbie's backlog style was not written to the vault: {error}")),
    }
}

pub fn sync_here() -> Option<String> {
    said(sync(&crate::vault_cli::detect()?))
}

fn source_shown(source: &str) -> String {
    match source {
        "override" => "the user's override in the vault, .vault/styles/backlog.override.md".to_string(),
        "default" => "the vault's default, .vault/styles/backlog.md".to_string(),
        other => format!("the vault ({other})"),
    }
}

fn bundled() -> Applied {
    Applied { text: unstamped(DEFAULT).to_string(), from: "kebbie's bundled default".to_string() }
}

// The vault's style for backlog items, the override when the user has one; kebbie's bundled default when the vault gives none.
pub fn applied(cli: Option<&Cli>) -> Applied {
    let found = cli.filter(|cli| stated(cli)).and_then(|cli| cli.style(KIND).ok().flatten());
    found.map_or_else(bundled, |style| Applied { text: unstamped(&style.text).to_string(), from: source_shown(&style.source) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_cli::fake::Fake;

    fn capable(fake: &Fake, stated: &[&str]) -> Cli {
        Cli { capabilities: Some(stated.iter().map(|one| one.to_string()).collect()), ..fake.cli.clone() }
    }

    fn writes(fake: &Fake) -> Vec<String> {
        fake.calls().into_iter().filter(|call| call.starts_with("style")).collect()
    }

    #[test]
    fn the_bundled_default_is_stamped_short_and_names_what_kebbie_reads() {
        assert!(version(DEFAULT) >= 1, "the first line stamps the version");
        assert!(DEFAULT.lines().count() <= 25, "agents read it on every item");
        assert!(!unstamped(DEFAULT).contains("style-version"));
        for read in ["Title", "priority", "repo", "status", "- [ ] "] {
            assert!(DEFAULT.contains(read), "{read}");
        }
    }

    #[test]
    fn a_vault_stating_styles_gets_the_default_written_through_style_write() {
        let fake = Fake::new("style-sync");
        let cli = capable(&fake, &["tasks", "styles"]);
        assert_eq!(sync(&cli).unwrap(), Synced::Wrote(version(DEFAULT)));
        assert_eq!(writes(&fake), [format!("style write --kind backlog --file - --version {}", version(DEFAULT))]);
    }

    #[test]
    fn a_vault_without_styles_is_left_alone_and_says_nothing() {
        let fake = Fake::new("style-skip");
        assert_eq!(sync(&capable(&fake, &["tasks", "templates"])).unwrap(), Synced::Skipped);
        assert!(fake.calls().is_empty());
        fake.answer("about", r#"{"name":"mindin","version":"0.19.0","contract":1,"capabilities":["tasks"]}"#);
        fake.answer("status", r#"{"configured":true}"#);
        assert_eq!(sync(&fake.cli).unwrap(), Synced::Skipped, "a command not yet probed is asked first");
        assert!(writes(&fake).is_empty());
        assert_eq!(said(Ok(Synced::Skipped)), None);
    }

    #[test]
    fn a_newer_style_in_the_vault_is_kept_without_a_word() {
        let fake = Fake::new("style-newer");
        fake.fail("style-write", "mindin: .vault/styles/backlog.md holds style-version 9, newer than the 1 offered; it was left as it is");
        assert_eq!(sync(&capable(&fake, &["styles"])).unwrap(), Synced::Newer);
        assert_eq!(said(Ok(Synced::Newer)), None);
        fake.fail("style-write", "mindin: a shared vault refuses .vault/ writes");
        assert!(said(sync(&capable(&fake, &["styles"]))).unwrap().contains("a shared vault refuses"), "any other refusal is said");
    }

    #[test]
    fn the_sync_never_names_the_users_override() {
        let fake = Fake::new("style-override");
        sync(&capable(&fake, &["styles"])).unwrap();
        let calls = writes(&fake);
        assert_eq!(calls.len(), 1);
        assert!(calls.iter().all(|call| !call.contains("override") && call.contains("--kind backlog ")), "{calls:?}");
    }

    #[test]
    fn the_template_shows_the_users_override_over_the_default_and_the_bundled_one_without_a_vault_style() {
        let fake = Fake::new("style-applied");
        let cli = capable(&fake, &["templates", "styles"]);
        fake.answer("template", r#"{"kind":"backlog","sections":[],"style":"Mine, short.","style_source":"override","style_note":"The user's override applies in place of the default."}"#);
        let shown = applied(Some(&cli));
        assert_eq!((shown.text.as_str(), shown.from.contains("override")), ("Mine, short.", true));
        fake.answer("template", r#"{"kind":"backlog","sections":[],"style":"<!-- style-version: 1 -->\nThe vault's.","style_source":"default"}"#);
        assert_eq!(applied(Some(&cli)).text, "The vault's.");
        fake.answer("template", r#"{"kind":"backlog","sections":[],"style":null,"style_source":null}"#);
        assert_eq!(applied(Some(&cli)), bundled());
        assert_eq!(applied(Some(&capable(&fake, &["templates"]))), bundled(), "an older vault has no style to give");
        assert_eq!(applied(None), bundled());
    }
}
