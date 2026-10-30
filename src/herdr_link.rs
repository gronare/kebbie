use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::sh::Result;

pub const KIND: &str = "herdr-link";
pub const EVERY: u64 = 3600;
const MANIFEST: &str = "herdr-plugin.toml";

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Unregistered,
    Installed,
    Linked(PathBuf),
    Gone { linked: PathBuf, target: Option<PathBuf> },
}

pub fn is_checkout(path: &Path) -> bool {
    path.join(MANIFEST).is_file()
}

// The checkout herdr should link: the current link's, else the running binary's, as either outlives a renamed folder herdr still names.
pub fn target(home: &Path, exe: Option<&Path>) -> Option<PathBuf> {
    let current = home.join(".local/share/kebbie/current").canonicalize().ok();
    let running = exe.and_then(|exe| exe.canonicalize().ok()).and_then(|exe| exe.ancestors().nth(3).map(Path::to_path_buf));
    current.into_iter().chain(running).find(|root| is_checkout(root))
}

pub fn read(list: &Value, target: Option<&Path>) -> Link {
    let plugins = list["result"]["plugins"].as_array().or_else(|| list["plugins"].as_array());
    let Some(plugin) = plugins.into_iter().flatten().find(|plugin| plugin["plugin_id"] == "kebbie") else { return Link::Unregistered };
    if plugin["source"]["kind"] != "local" {
        return Link::Installed;
    }
    let linked = PathBuf::from(plugin["plugin_root"].as_str().unwrap_or_default());
    if is_checkout(&linked) {
        return Link::Linked(linked);
    }
    Link::Gone { linked, target: target.map(Path::to_path_buf) }
}

pub fn healed_line(home: &Path, linked: &Path, target: &Path) -> String {
    let tilde = |path| crate::setup::tilde(home, path);
    format!("herdr linked kebbie from {}, which is gone; linked it again from {}", tilde(linked), tilde(target))
}

// The server must answer before the unlink, so a herdr that is down or starting never ends up with kebbie unlinked and not linked again.
pub fn relink(herdr: &mut dyn FnMut(&[&str]) -> Result<String>, state: &Path, home: &Path, target: Option<&Path>) -> Result<Option<String>> {
    let list: Value = serde_json::from_str(&herdr(&["plugin", "list", "--json"])?).map_err(|error| format!("herdr plugin list: {error}"))?;
    let Link::Gone { linked, target: Some(target) } = read(&list, target) else { return Ok(None) };
    herdr(&["workspace", "list"]).map_err(|error| format!("herdr links kebbie from {}, which is gone, but its server does not answer ({error}); the next round links it again", linked.display()))?;
    herdr(&["plugin", "unlink", "kebbie"])?;
    herdr(&["plugin", "link", &target.to_string_lossy()])?;
    let line = healed_line(home, &linked, &target);
    crate::messages::log_for_user_in(state, &line, KIND)?;
    Ok(Some(line))
}

// After a relink the board and the keeper are started as herdr's startup would have, since that startup ran in the missing folder.
pub fn heal() -> Result<Vec<String>> {
    let state = crate::lead::state_dir()?;
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let exe = crate::peers::own_exe().ok();
    let bin = crate::herdr::bin();
    let mut herdr = |args: &[&str]| crate::sh::run(&bin, args, None);
    let Some(line) = relink(&mut herdr, &state, &home, target(&home, exe.as_deref()).as_deref())? else { return Ok(Vec::new()) };
    let mut lines = vec![line];
    lines.extend(crate::lead::revive_board_detached().err());
    lines.push(crate::keeper::start().unwrap_or_else(|error| error));
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(MANIFEST), "id = \"kebbie\"\n").unwrap();
        dir.to_path_buf()
    }

    fn list(kind: &str, root: &Path) -> String {
        format!(r#"{{"result":{{"plugins":[{{"plugin_id":"kebbie","enabled":true,"plugin_root":"{}","source":{{"kind":"{kind}"}}}}]}}}}"#, root.display())
    }

    struct Fake {
        list: String,
        server: bool,
        calls: Vec<String>,
    }

    impl Fake {
        fn run(&mut self, args: &[&str]) -> Result<String> {
            self.calls.push(args.join(" "));
            match args {
                ["plugin", "list", "--json"] => Ok(self.list.clone()),
                ["workspace", "list"] if !self.server => Err("herdr workspace list failed".to_string()),
                _ => Ok(String::new()),
            }
        }
    }

    fn relinked(fake: &mut Fake, state: &Path, home: &Path, target: Option<&Path>) -> Result<Option<String>> {
        relink(&mut |args| fake.run(args), state, home, target)
    }

    #[test]
    fn a_local_link_to_a_missing_folder_is_linked_again_from_the_current_checkout_and_said_on_the_board() {
        let home = crate::sh::test_dir("herdr-link-home");
        let state = home.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let current = checkout(&home.join("projects/kebbie"));
        let mut fake = Fake { list: list("local", &home.join("projects/kelpie")), server: true, calls: Vec::new() };
        let line = relinked(&mut fake, &state, &home, Some(&current)).unwrap();
        assert_eq!(line.as_deref(), Some("herdr linked kebbie from ~/projects/kelpie, which is gone; linked it again from ~/projects/kebbie"));
        assert_eq!(fake.calls, ["plugin list --json", "workspace list", "plugin unlink kebbie", &format!("plugin link {}", current.display())]);
        let logged = crate::messages::load_in(&state);
        let said = logged.last().unwrap();
        assert_eq!((said.from.as_str(), said.to.as_str(), said.kind.as_deref(), said.text.as_str()), ("kebbie", crate::messages::USER_LABEL, Some(KIND), line.unwrap().as_str()), "the board's messages show it");
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn a_healthy_link_a_github_install_and_a_link_to_another_checkout_are_left_alone() {
        let home = crate::sh::test_dir("herdr-link-alone");
        let state = home.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let current = checkout(&home.join("kebbie"));
        let other = checkout(&home.join("kebbie-dev"));
        for (kind, root, want) in [("local", &current, Link::Linked(current.clone())), ("local", &other, Link::Linked(other.clone())), ("github", &home.join("gone"), Link::Installed)] {
            let mut fake = Fake { list: list(kind, root), server: true, calls: Vec::new() };
            assert_eq!(read(&serde_json::from_str(&fake.list).unwrap(), Some(&current)), want);
            assert_eq!(relinked(&mut fake, &state, &home, Some(&current)).unwrap(), None);
            assert_eq!(fake.calls, ["plugin list --json"], "{kind} at {}: no link or unlink", root.display());
        }
        let mut fake = Fake { list: r#"{"result":{"plugins":[]}}"#.to_string(), server: true, calls: Vec::new() };
        assert_eq!(relinked(&mut fake, &state, &home, Some(&current)).unwrap(), None, "an unregistered kebbie is the doctor's herdr plugin line");
        assert!(crate::messages::load_in(&state).is_empty());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn a_missing_link_waits_while_herdrs_server_does_not_answer_or_no_checkout_is_found() {
        let home = crate::sh::test_dir("herdr-link-wait");
        let state = home.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let current = checkout(&home.join("kebbie"));
        let mut fake = Fake { list: list("local", &home.join("kelpie")), server: false, calls: Vec::new() };
        let error = relinked(&mut fake, &state, &home, Some(&current)).unwrap_err();
        assert!(error.contains("its server does not answer") && error.contains("the next round links it again"), "{error}");
        assert!(!fake.calls.iter().any(|call| call.contains("link kebbie") || call.starts_with("plugin link")), "never unlinked while the server is down: {:?}", fake.calls);
        let mut fake = Fake { list: list("local", &home.join("kelpie")), server: true, calls: Vec::new() };
        assert_eq!(relinked(&mut fake, &state, &home, None).unwrap(), None, "with no checkout to link, only the doctor names it");
        assert_eq!(fake.calls, ["plugin list --json"]);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn the_target_is_the_current_links_checkout_else_the_running_binarys() {
        let home = crate::sh::test_dir("herdr-link-target");
        let current = checkout(&home.join("projects/kebbie"));
        let running = checkout(&home.join("build"));
        std::fs::create_dir_all(running.join("target/release")).unwrap();
        std::fs::write(running.join("target/release/kebbie"), "").unwrap();
        let exe = running.join("target/release/kebbie");
        assert_eq!(target(&home, Some(&exe)), Some(running.canonicalize().unwrap()), "no current link: the running binary's checkout");
        std::fs::create_dir_all(home.join(".local/share/kebbie")).unwrap();
        std::os::unix::fs::symlink(&current, home.join(".local/share/kebbie/current")).unwrap();
        assert_eq!(target(&home, Some(&exe)), Some(current.canonicalize().unwrap()));
        std::fs::remove_file(current.join(MANIFEST)).unwrap();
        std::fs::remove_file(running.join(MANIFEST)).unwrap();
        assert_eq!(target(&home, Some(&exe)), None, "a folder without herdr-plugin.toml is no checkout");
        let _ = std::fs::remove_dir_all(home);
    }
}
