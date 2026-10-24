use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::sh::Result;

const OLD: &str = "kelpie";
const OLD_PREFIX: &str = "KELPIE_";
const NEW_PREFIX: &str = "KEBBIE_";
// kebbie sets these for its own children, so an old session carrying one is no reason to tell the user.
const SET_BY_KEBBIE: [&str; 9] = ["ADDRESS", "PUSH_TOKEN", "UPDATE_PROGRESS", "PROGRESS_DIR", "BOARD_STATE", "EVENT", "TASK", "REPO", "TEXT"];
const NOTES: &str = "old-name-notes";

#[derive(Debug, PartialEq)]
pub struct Adopted {
    pub set: Vec<(String, String)>,
    pub named: Vec<String>,
}

pub fn adopted(vars: &[(String, String)], present: impl Fn(&str) -> bool) -> Adopted {
    let mut found = Adopted { set: Vec::new(), named: Vec::new() };
    for (name, value) in vars {
        let Some(rest) = name.strip_prefix(OLD_PREFIX) else { continue };
        let new = format!("{NEW_PREFIX}{rest}");
        if !present(&new) {
            found.set.push((new, value.clone()));
        }
        if !SET_BY_KEBBIE.contains(&rest) {
            found.named.push(name.clone());
        }
    }
    found.named.sort();
    found
}

#[derive(Debug, PartialEq)]
pub enum Moved {
    Nothing,
    Done,
    Both,
    Failed(String),
}

pub fn move_dir(old: &Path, new: &Path) -> Moved {
    if std::fs::symlink_metadata(old).is_err() {
        return Moved::Nothing;
    }
    if std::fs::symlink_metadata(new).is_ok() {
        return Moved::Both;
    }
    match std::fs::rename(old, new) {
        Ok(()) => Moved::Done,
        Err(error) => Moved::Failed(error.to_string()),
    }
}

pub fn dir_pairs(var: impl Fn(&str) -> Option<String>) -> Vec<(PathBuf, PathBuf)> {
    let var = |name: &str| var(name).filter(|value| !value.is_empty());
    let home = var("HOME").map(PathBuf::from);
    let mut pairs = Vec::new();
    let base = |xdg: &str, under_home: &str| var(xdg).map(PathBuf::from).or_else(|| home.as_ref().map(|home| home.join(under_home)));
    if var(crate::lead::STATE_DIR_VAR).is_none() {
        pairs.extend(base("XDG_STATE_HOME", ".local/state").map(|base| (base.join(OLD), base.join("kebbie"))));
    }
    pairs.extend(base("XDG_CONFIG_HOME", ".config").map(|base| (base.join(OLD), base.join("kebbie"))));
    pairs
}

// Runs first in main, before anything reads an environment variable or the state and config folders.
pub fn on_start() {
    let vars: Vec<(String, String)> = std::env::vars_os().filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?))).collect();
    let found = adopted(&vars, |name| std::env::var_os(name).is_some());
    for (name, value) in &found.set {
        // SAFETY: main has started no thread yet.
        unsafe { std::env::set_var(name, value) };
    }
    let var = |name: &str| std::env::var(name).ok();
    let mut once = Vec::new();
    for (old, new) in dir_pairs(var) {
        match move_dir(&old, &new) {
            Moved::Nothing => {}
            Moved::Done => eprintln!("kebbie: moved {} to {}", old.display(), new.display()),
            Moved::Both => once.push(format!("kebbie: both {} and {} exist; kebbie uses {} and leaves the old one for you to merge or remove", old.display(), new.display(), new.display())),
            Moved::Failed(error) => eprintln!("kebbie: could not move {} to {}: {error}; kebbie uses {}", old.display(), new.display(), new.display()),
        }
    }
    if !found.named.is_empty() {
        let renamed = found.named.iter().map(|name| name.replacen(OLD_PREFIX, NEW_PREFIX, 1)).collect::<Vec<_>>().join(", ");
        once.push(format!("kebbie: {} still work for now; rename them where you set them: {renamed}", found.named.join(", ")));
    }
    if !once.is_empty() {
        tell_once(&once);
    }
}

fn tell_once(lines: &[String]) {
    let Ok(state) = crate::lead::state_path(|name| std::env::var(name).ok()) else { return };
    let file = state.join(NOTES);
    let said = std::fs::read_to_string(&file).unwrap_or_default();
    let fresh: Vec<&String> = lines.iter().filter(|line| !said.lines().any(|old| old == line.as_str())).collect();
    if fresh.is_empty() {
        return;
    }
    for line in &fresh {
        eprintln!("{line}");
    }
    let all = said.lines().map(str::to_string).chain(fresh.into_iter().cloned()).collect::<Vec<_>>().join("\n");
    let _ = crate::state::write(&file, format!("{all}\n"));
}

pub struct Places {
    pub home: PathBuf,
    pub data_home: Option<PathBuf>,
    pub claude_dir: PathBuf,
    pub claude: String,
    pub herdr: String,
    pub herdr_config: PathBuf,
    pub codex_home: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    Run(Vec<String>),
    RemoveLink(PathBuf),
    RemoveDir(PathBuf),
    Rewrite { file: PathBuf, text: String, via_link: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Leftover {
    pub what: String,
    pub fix: Fix,
}

impl Fix {
    pub fn describe(&self) -> String {
        match self {
            Fix::Run(command) => command.join(" "),
            Fix::RemoveLink(path) => format!("rm {}", path.display()),
            Fix::RemoveDir(path) => format!("rm -r {}", path.display()),
            Fix::Rewrite { file, .. } => format!("replace kelpie with kebbie in {}", file.display()),
        }
    }

    fn apply(&self) -> Result<()> {
        match self {
            Fix::Run(command) => crate::sh::run(&command[0], &command[1..].iter().map(String::as_str).collect::<Vec<_>>(), None).map(|_| ()),
            Fix::RemoveLink(path) => std::fs::remove_file(path).map_err(|error| format!("{}: {error}", path.display())),
            Fix::RemoveDir(path) => std::fs::remove_dir_all(path).map_err(|error| format!("{}: {error}", path.display())),
            Fix::Rewrite { file, text, .. } => std::fs::write(file, text).map_err(|error| format!("{}: {error}", file.display())),
        }
    }
}

fn read_json(path: &Path) -> Value {
    std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

fn registered(list: &Value, id: &str) -> bool {
    list["result"]["plugins"].as_array().into_iter().flatten().any(|plugin| plugin["plugin_id"] == id)
}

pub fn renamed_text(text: &str) -> String {
    text.replace(OLD, "kebbie").replace("Kelpie", "Kebbie")
}

// herdr names a plugin by its manifest id and refuses an update that changes it, so the old id is uninstalled once kebbie is registered.
pub fn leftovers(places: &Places, herdr_list: Option<&str>) -> Vec<Leftover> {
    let mut found = Vec::new();
    let list: Value = herdr_list.and_then(|list| serde_json::from_str(list).ok()).unwrap_or_default();
    if registered(&list, OLD) && registered(&list, "kebbie") {
        found.push(Leftover { what: "herdr still has the kelpie plugin".to_string(), fix: Fix::Run(vec![places.herdr.clone(), "plugin".to_string(), "uninstall".to_string(), OLD.to_string()]) });
    }
    let installed = read_json(&places.claude_dir.join("plugins/installed_plugins.json"));
    for id in installed["plugins"].as_object().into_iter().flatten().map(|(id, _)| id).filter(|id| id.starts_with("kelpie@")) {
        found.push(Leftover { what: format!("Claude Code has the {id} plugin"), fix: Fix::Run(vec![places.claude.clone(), "plugin".to_string(), "uninstall".to_string(), id.clone()]) });
    }
    if read_json(&places.claude_dir.join("plugins/known_marketplaces.json")).get(OLD).is_some() {
        found.push(Leftover { what: "Claude Code has the kelpie marketplace".to_string(), fix: Fix::Run(vec![places.claude.clone(), "plugin".to_string(), "marketplace".to_string(), "remove".to_string(), OLD.to_string()]) });
    }
    if let Some(cache) = places.codex_home.as_ref().map(|home| home.join("plugins/cache").join(OLD)).filter(|cache| cache.is_dir()) {
        found.push(Leftover { what: format!("Codex keeps kelpie's plugin in {}; kebbie setup codex installs kebbie's", cache.display()), fix: Fix::RemoveDir(cache) });
    }
    let link = places.home.join(".local/bin").join(OLD);
    if std::fs::read_link(&link).is_ok_and(|target| target.to_string_lossy().contains("/kelpie/current/")) {
        found.push(Leftover { what: format!("{} links to the old install", link.display()), fix: Fix::RemoveLink(link) });
    }
    let data = places.data_home.clone().unwrap_or_else(|| places.home.join(".local/share")).join(OLD);
    let only_current = std::fs::read_dir(&data).map(|entries| entries.filter_map(|entry| entry.ok()).map(|entry| entry.file_name()).collect::<Vec<_>>()).is_ok_and(|names| names.iter().all(|name| name == "current"));
    if only_current && std::fs::symlink_metadata(data.join("current")).is_ok_and(|meta| meta.file_type().is_symlink()) {
        found.push(Leftover { what: format!("{} holds only the old install's link", data.display()), fix: Fix::RemoveDir(data) });
    }
    if let Ok(text) = std::fs::read_to_string(&places.herdr_config) {
        let renamed = renamed_text(&text);
        if renamed != text && renamed.parse::<toml::Table>().is_ok() {
            let via_link = std::fs::symlink_metadata(&places.herdr_config).is_ok_and(|meta| meta.file_type().is_symlink());
            found.push(Leftover { what: format!("{} still names kelpie", places.herdr_config.display()), fix: Fix::Rewrite { file: places.herdr_config.clone(), text: renamed, via_link } });
        }
    }
    found
}

fn changed_lines(before: &str, after: &str) -> String {
    before.lines().zip(after.lines()).filter(|(old, new)| old != new).map(|(old, new)| format!("  - {old}\n  + {new}")).collect::<Vec<_>>().join("\n")
}

fn line(verb: &str, detail: impl std::fmt::Display) -> String {
    format!("{verb:<7}  {:<18}  {detail}", "kelpie leftovers")
}

// Each leftover is shown and fixed only on the user's y; --yes fixes them all but never writes through a linked herdr config.
pub fn switch_over(found: &[Leftover], asking: crate::setup::Asking, input: &mut impl BufRead, output: &mut impl Write) -> Result<(Vec<String>, bool)> {
    use crate::setup::Asking;
    let mut lines = Vec::new();
    let mut rewrote = false;
    if found.is_empty() {
        return Ok((Vec::new(), false));
    }
    for leftover in found {
        let shown = match &leftover.fix {
            Fix::Rewrite { file, text, .. } => format!("{}:\n{}", leftover.fix.describe(), changed_lines(&std::fs::read_to_string(file).unwrap_or_default(), text)),
            fix => fix.describe(),
        };
        let go = match (asking, &leftover.fix) {
            (Asking::Closed, _) => false,
            (Asking::Yes, Fix::Rewrite { via_link: true, .. }) => false,
            (Asking::Yes, _) => true,
            (Asking::Terminal, _) => {
                write!(output, "{}. Run: {shown}\n  [y/N] ", leftover.what).map_err(|error| error.to_string())?;
                output.flush().map_err(|error| error.to_string())?;
                let mut answer = String::new();
                input.read_line(&mut answer).map_err(|error| error.to_string())?;
                matches!(answer.trim(), "y" | "Y" | "yes")
            }
        };
        if !go {
            lines.push(line("to do", format!("{}: {}", leftover.what, leftover.fix.describe())));
            continue;
        }
        match leftover.fix.apply() {
            Ok(()) => {
                rewrote |= matches!(leftover.fix, Fix::Rewrite { .. });
                lines.push(line("done", leftover.fix.describe()));
            }
            Err(error) => lines.push(line("failed", error)),
        }
    }
    Ok((lines, rewrote))
}

pub fn places_now(home: &Path, claude_dir: &Path, claude: &str, herdr: &str, herdr_config: &Path) -> Places {
    Places {
        home: home.to_path_buf(),
        data_home: std::env::var("XDG_DATA_HOME").ok().filter(|dir| !dir.is_empty()).map(PathBuf::from),
        claude_dir: claude_dir.to_path_buf(),
        claude: claude.to_string(),
        herdr: herdr.to_string(),
        herdr_config: herdr_config.to_path_buf(),
        codex_home: crate::agent::codex::home(),
    }
}

pub fn herdr_list(herdr: &str) -> Option<String> {
    crate::sh::run(herdr, &["plugin", "list", "--json"], None).ok()
}

pub fn update_line(places: &Places) -> Option<String> {
    let found = leftovers(places, herdr_list(&places.herdr).as_deref());
    (!found.is_empty()).then(|| format!("kelpie leftovers: {}; kebbie setup switches them over, asking before each", found.iter().map(|leftover| leftover.what.as_str()).collect::<Vec<_>>().join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::Asking;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kebbie-old-name-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn places(home: &Path) -> Places {
        Places { home: home.to_path_buf(), data_home: None, claude_dir: home.join(".claude"), claude: "/fake/claude".to_string(), herdr: "/fake/herdr".to_string(), herdr_config: home.join(".config/herdr/config.toml"), codex_home: Some(home.join(".codex")) }
    }

    #[test]
    fn an_old_variable_is_read_under_its_new_name_and_only_the_users_own_are_named() {
        let vars = [("KELPIE_CHANNEL", "main"), ("KELPIE_ADDRESS", "billing@app"), ("KELPIE_STATE_DIR", "/s"), ("PATH", "/bin")].map(|(name, value)| (name.to_string(), value.to_string()));
        let found = adopted(&vars, |name| name == "KEBBIE_STATE_DIR");
        assert_eq!(found.set, [("KEBBIE_CHANNEL".to_string(), "main".to_string()), ("KEBBIE_ADDRESS".to_string(), "billing@app".to_string())], "a new name already set wins");
        assert_eq!(found.named, ["KELPIE_CHANNEL", "KELPIE_STATE_DIR"]);
    }

    #[test]
    fn the_old_folders_move_once_and_both_present_keeps_the_new() {
        let dir = scratch("move");
        let (old, new) = (dir.join("kelpie"), dir.join("kebbie"));
        assert_eq!(move_dir(&old, &new), Moved::Nothing);
        std::fs::create_dir_all(old.join("roles")).unwrap();
        assert_eq!(move_dir(&old, &new), Moved::Done);
        assert!(new.join("roles").is_dir() && !old.exists());
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(move_dir(&old, &new), Moved::Both);
        assert!(old.exists() && new.join("roles").is_dir(), "neither is touched");
    }

    #[test]
    fn the_state_folder_stays_where_kebbie_state_dir_points() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string());
        assert_eq!(dir_pairs(env(&[("HOME", "/h")])), [(PathBuf::from("/h/.local/state/kelpie"), PathBuf::from("/h/.local/state/kebbie")), (PathBuf::from("/h/.config/kelpie"), PathBuf::from("/h/.config/kebbie"))]);
        assert_eq!(dir_pairs(env(&[("HOME", "/h"), ("KEBBIE_STATE_DIR", "/s"), ("XDG_CONFIG_HOME", "/x")])), [(PathBuf::from("/x/kelpie"), PathBuf::from("/x/kebbie"))]);
    }

    #[test]
    fn leftovers_are_found_and_fixed_only_on_yes() {
        let home = scratch("leftovers");
        let p = places(&home);
        std::fs::create_dir_all(home.join(".claude/plugins")).unwrap();
        std::fs::write(home.join(".claude/plugins/installed_plugins.json"), r#"{"plugins":{"kelpie@kelpie":[{}],"vault@gronare":[{}]}}"#).unwrap();
        std::fs::write(home.join(".claude/plugins/known_marketplaces.json"), r#"{"kelpie":{}}"#).unwrap();
        std::fs::create_dir_all(home.join(".codex/plugins/cache/kelpie/kelpie/local")).unwrap();
        std::fs::create_dir_all(home.join(".local/share/kelpie")).unwrap();
        std::os::unix::fs::symlink(home.join("old-root"), home.join(".local/share/kelpie/current")).unwrap();
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::os::unix::fs::symlink(home.join(".local/share/kelpie/current/target/release/kelpie"), home.join(".local/bin/kelpie")).unwrap();
        std::fs::create_dir_all(home.join(".config/herdr")).unwrap();
        let config = "[[keys.command]]\nkey = \"prefix+space\"\ntype = \"plugin_action\"\ncommand = \"kelpie.lead\"\n";
        std::fs::write(&p.herdr_config, config).unwrap();
        let list = r#"{"result":{"plugins":[{"plugin_id":"kelpie"},{"plugin_id":"kebbie"}]}}"#;
        let found = leftovers(&p, Some(list));
        let fixes: Vec<String> = found.iter().map(|leftover| leftover.fix.describe()).collect();
        assert_eq!(fixes[..3], ["/fake/herdr plugin uninstall kelpie", "/fake/claude plugin uninstall kelpie@kelpie", "/fake/claude plugin marketplace remove kelpie"]);
        assert_eq!(found.len(), 7, "{fixes:?}");
        assert!(leftovers(&p, Some(r#"{"result":{"plugins":[{"plugin_id":"kelpie"}]}}"#)).iter().all(|leftover| !leftover.what.contains("herdr still")), "herdr's kelpie stays until kebbie is registered");
        let local: Vec<Leftover> = found.into_iter().filter(|leftover| !matches!(leftover.fix, Fix::Run(_))).collect();
        let (lines, rewrote) = switch_over(&local, Asking::Terminal, &mut "n\n".repeat(4).as_bytes(), &mut Vec::new()).unwrap();
        assert!(!rewrote && lines.iter().all(|line| line.starts_with("to do")) && home.join(".local/bin/kelpie").symlink_metadata().is_ok());
        let (lines, rewrote) = switch_over(&local, Asking::Yes, &mut "".as_bytes(), &mut Vec::new()).unwrap();
        assert!(rewrote && lines.iter().all(|line| line.starts_with("done")), "{lines:?}");
        assert!(!home.join(".codex/plugins/cache/kelpie").exists() && !home.join(".local/share/kelpie").exists() && home.join(".local/bin/kelpie").symlink_metadata().is_err());
        assert_eq!(std::fs::read_to_string(&p.herdr_config).unwrap(), config.replace("kelpie.lead", "kebbie.lead"));
    }

    #[test]
    fn yes_never_rewrites_a_linked_herdr_config() {
        let home = scratch("linked");
        let p = places(&home);
        std::fs::create_dir_all(home.join(".config/herdr")).unwrap();
        std::fs::write(home.join("dotfiles.toml"), "x = \"kelpie.lead\"\n").unwrap();
        std::os::unix::fs::symlink(home.join("dotfiles.toml"), &p.herdr_config).unwrap();
        let found = leftovers(&p, None);
        let (lines, _) = switch_over(&found, Asking::Yes, &mut "".as_bytes(), &mut Vec::new()).unwrap();
        assert!(lines[0].starts_with("to do"), "{lines:?}");
        assert_eq!(std::fs::read_to_string(home.join("dotfiles.toml")).unwrap(), "x = \"kelpie.lead\"\n");
    }
}
