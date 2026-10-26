use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use toml::Table;

use crate::routines::{self, Mode, Routine, Schedule};
use crate::sh::Result;

#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub id: String,
    pub title: String,
    pub repo: String,
    pub schedule: Schedule,
    pub prompt: String,
    pub mode: Mode,
    pub push: bool,
    pub enabled: bool,
    pub profile: Option<String>,
}

pub fn parse_when(text: &str) -> Result<Schedule> {
    let text = text.trim();
    let every = text.strip_prefix("every ").map(str::trim).or_else(|| text.ends_with(['m', 'h', 'd']).then_some(text));
    match every {
        Some(interval) => routines::parse_every(interval).map(Schedule::Every),
        None => routines::parse_at(text),
    }
}

pub fn when_text(schedule: Schedule) -> String {
    match schedule {
        Schedule::Every(_) => schedule.text(),
        _ => schedule.text().replace("daily at ", "").replace(" at ", " "),
    }
}

// The id names the state file and the tasks, so it comes from the title once and stays when the title changes.
pub fn slug(title: &str) -> String {
    let mut id = String::new();
    for character in title.to_lowercase().chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            id.push(character);
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    let id: String = id.trim_start_matches(|character: char| character.is_ascii_digit() || character == '-').chars().take(40).collect();
    id.trim_end_matches('-').to_string()
}

pub fn free_id(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|id| id == base) {
        return base.to_string();
    }
    (2..).map(|number| {
        let suffix = format!("-{number}");
        let head: String = base.chars().take(40 - suffix.len()).collect();
        format!("{}{suffix}", head.trim_end_matches('-'))
    }).find(|id| !taken.contains(id)).unwrap_or_default()
}

pub fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            character if character.is_control() => out.push_str(&format!("\\u{:04X}", character as u32)),
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

fn multiline(text: &str) -> String {
    let mut out = String::from("\"\"\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\t' => out.push(character),
            character if character.is_control() => out.push_str(&format!("\\u{:04X}", character as u32)),
            character => out.push(character),
        }
    }
    out.push_str("\"\"\"");
    out
}

// A repo config's routine runs in that repo, so its table leaves the repo out when it is that one.
pub fn block(draft: &Draft, default_repo: Option<&str>) -> String {
    let mut lines = vec!["[[routines]]".to_string(), format!("id = {}", quoted(&draft.id)), format!("title = {}", quoted(&draft.title))];
    if default_repo != Some(draft.repo.as_str()) {
        lines.push(format!("repo = {}", quoted(&draft.repo)));
    }
    lines.push(match draft.schedule {
        Schedule::Every(_) => format!("every = {}", quoted(draft.schedule.text().trim_start_matches("every "))),
        schedule => format!("at = {}", quoted(&when_text(schedule))),
    });
    lines.push(format!("mode = {}", quoted(draft.mode.key())));
    if draft.push {
        lines.push("push = true".to_string());
    }
    if !draft.enabled {
        lines.push("enabled = false".to_string());
    }
    if let Some(profile) = &draft.profile {
        lines.push(format!("profile = {}", quoted(profile)));
    }
    lines.push(format!("prompt = {}", multiline(draft.prompt.trim())));
    lines.join("\n") + "\n"
}

// Which lines start outside a string, so a header-looking line inside a prompt is never taken for a table.
fn table_lines(text: &str) -> Vec<bool> {
    #[derive(PartialEq)]
    enum In { Code, Comment, Basic, Literal, LongBasic, LongLiteral }
    let characters: Vec<char> = text.chars().collect();
    let mut state = In::Code;
    let mut starts = vec![true];
    let mut at = 0;
    while at < characters.len() {
        let ahead = |count: usize| characters[at..].iter().take(count).collect::<String>();
        match (&state, characters[at]) {
            (_, '\n') => {
                if matches!(state, In::Comment | In::Basic | In::Literal) {
                    state = In::Code;
                }
                starts.push(state == In::Code);
            }
            (In::Code, '#') => state = In::Comment,
            (In::Code, '"') if ahead(3) == "\"\"\"" => { state = In::LongBasic; at += 2; }
            (In::Code, '"') => state = In::Basic,
            (In::Code, '\'') if ahead(3) == "'''" => { state = In::LongLiteral; at += 2; }
            (In::Code, '\'') => state = In::Literal,
            (In::Basic | In::LongBasic, '\\') => at += 1,
            (In::Basic, '"') | (In::Literal, '\'') => state = In::Code,
            (In::LongBasic, '"') if ahead(3) == "\"\"\"" => { state = In::Code; at += 2; }
            (In::LongLiteral, '\'') if ahead(3) == "'''" => { state = In::Code; at += 2; }
            _ => {}
        }
        at += 1;
    }
    starts
}

struct Found {
    start: usize,
    end: usize,
    id: Option<String>,
}

fn quiet(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || line.starts_with('#')
}

// Each [[routines]] table: from its header to the next table, leaving the blank and comment lines before that table to it.
fn routine_tables(lines: &[&str]) -> Vec<Found> {
    let starts = table_lines(&lines.join("\n"));
    let header = |index: usize| starts.get(index).copied().unwrap_or(false) && lines[index].trim_start().starts_with('[');
    let mut found = Vec::new();
    for start in (0..lines.len()).filter(|index| header(*index)) {
        let name = lines[start].split('#').next().unwrap_or_default().trim();
        if name != "[[routines]]" {
            continue;
        }
        let mut end = (start + 1..lines.len()).find(|index| header(*index)).unwrap_or(lines.len());
        while end > start + 1 && quiet(lines[end - 1]) && starts.get(end - 1).copied().unwrap_or(false) {
            end -= 1;
        }
        let id = lines[start..end].join("\n").parse::<Table>().ok()
            .and_then(|table| table.get("routines")?.as_array()?.first()?.get("id")?.as_str().map(str::to_string));
        found.push(Found { start, end, id });
    }
    found
}

fn stem(file: &Path) -> Option<String> {
    file.file_stem().map(|stem| stem.to_string_lossy().to_string())
}

// A config file is often a link into a dotfiles repo, so the write goes to what it points at and keeps its mode.
fn write_config(file: &Path, text: &str) -> Result<()> {
    let target = fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let mode = fs::metadata(&target).ok().map(|metadata| metadata.permissions().mode() & 0o777);
    crate::state::write(&target, text)?;
    if let Some(mode) = mode {
        fs::set_permissions(&target, fs::Permissions::from_mode(mode)).map_err(|error| format!("{}: {error}", target.display()))?;
    }
    Ok(())
}

pub(crate) fn check(file: &Path, machine: bool, text: &str) -> Result<()> {
    let source = file.display().to_string();
    if machine { crate::config::parse_machine(text, &source).map(|_| ()) } else { crate::config::parse_repo(text, &source).map(|_| ()) }
}

fn read(file: &Path) -> Result<String> {
    match fs::read_to_string(file) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("{}: {error}", file.display())),
    }
}

// Every change is checked as the whole file before it is written, so a file that parsed before still parses after.
pub(crate) fn rewrite(file: &Path, machine: bool, change: impl FnOnce(&str) -> Result<String>) -> Result<()> {
    let text = read(file)?;
    check(file, machine, &text).map_err(|error| format!("{error}\nFix that first; kebbie changes a config file only when it parses."))?;
    let changed = change(&text)?;
    check(file, machine, &changed)?;
    write_config(file, &changed)
}

fn line_ending(line: &str) -> &'static str {
    if line.ends_with("\r\n") { "\r\n" } else if line.ends_with('\n') { "\n" } else { "" }
}

// A comment after a one-line basic string value stays with the line; anything else after the value is dropped with it.
fn trailing_comment(line: &str) -> &str {
    let body = line.trim_end_matches(['\r', '\n']);
    let Some(value) = body.split_once('=').map(|(_, value)| value.trim_start()).filter(|value| value.starts_with('"') && !value.starts_with("\"\"\"")) else { return "" };
    let mut escaped = false;
    for (at, character) in value.char_indices().skip(1) {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => {
                let rest = &value[at + 1..];
                return if rest.trim_start().starts_with('#') { rest } else { "" };
            }
            _ => {}
        }
    }
    ""
}

// Only the key's line changes, so the rest of the file keeps every byte, comment and order it had.
fn with_key(file: &Path, text: &str, section: &str, key: &str, value: &str, written: fn(&crate::config::Machine) -> Option<&String>) -> Result<String> {
    let line = format!("{key} = {}", quoted(value));
    let starts = table_lines(text);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let header = |index: usize| starts.get(index).copied().unwrap_or(false) && lines[index].trim_start().starts_with('[');
    let table = format!("[{section}]");
    let found = (0..lines.len()).find(|index| header(*index) && lines[*index].split('#').next().unwrap_or_default().trim() == table);
    let changed = match found {
        Some(start) => {
            let end = (start + 1..lines.len()).find(|index| header(*index)).unwrap_or(lines.len());
            let mut owned: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
            match (start + 1..end).find(|index| starts.get(*index).copied().unwrap_or(false) && key_of(lines[*index]) == Some(key)) {
                Some(at) => owned[at] = line + trailing_comment(lines[at]) + line_ending(lines[at]),
                None => {
                    let ending = match line_ending(lines[start]) { "" => { owned[start].push('\n'); "\n" } ending => ending };
                    owned.insert(start + 1, line + ending);
                }
            }
            owned.concat()
        }
        None => {
            let mut text = text.to_string();
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            if !text.trim().is_empty() {
                text.push('\n');
            }
            text + &table + "\n" + &line + "\n"
        }
    };
    let by_hand = format!("{}: [{section}] {key} is set in a form kebbie does not rewrite; edit it by hand", file.display());
    match crate::config::parse_machine(&changed, &file.display().to_string()) {
        Ok(machine) if written(&machine).map(String::as_str) == Some(value) => Ok(changed),
        _ => Err(by_hand),
    }
}

fn with_machine_name(file: &Path, text: &str, name: &str) -> Result<String> {
    with_key(file, text, "machine", "name", name, |machine| machine.machine_name.as_ref())
}

pub fn set_hub_url_in(file: &Path, url: &str) -> Result<()> {
    let url = crate::hub::hub_url(url)?;
    rewrite(file, true, |text| with_key(file, text, "hub", "url", &url, |machine| machine.hub_url.as_ref()))
}

pub fn check_machine_name_in(file: &Path, name: &str) -> Result<()> {
    let name = crate::config::valid_machine_name(name)?;
    let text = read(file)?;
    check(file, true, &text).map_err(|error| format!("{error}\nFix that first; kebbie changes a config file only when it parses."))?;
    with_machine_name(file, &text, &name).map(|_| ())
}

pub fn set_machine_name_in(file: &Path, name: &str) -> Result<()> {
    let name = crate::config::valid_machine_name(name)?;
    rewrite(file, true, |text| with_machine_name(file, text, &name))
}

fn validate(draft: &Draft) -> Result<()> {
    if !routines::valid_id(&draft.id) {
        return Err(format!("\"{}\" is not an id; use lower-case letters, digits and single dashes, starting with a letter, at most 40", draft.id));
    }
    if draft.push && draft.mode != Mode::Fix {
        return Err("only a fix routine pushes".to_string());
    }
    for (name, value) in [("title", &draft.title), ("repo", &draft.repo), ("prompt", &draft.prompt)] {
        if value.trim().is_empty() {
            return Err(format!("a routine needs a {name}"));
        }
    }
    Ok(())
}

pub fn add_in(machine: &Path, repos: &Path, draft: &Draft) -> Result<String> {
    validate(draft)?;
    let (existing, _) = routines::load_from(machine, repos);
    if let Some(other) = existing.iter().find(|routine| routine.id == draft.id) {
        return Err(format!("the id \"{}\" is taken by a routine in {}", draft.id, other.source));
    }
    rewrite(machine, true, |text| {
        let mut text = text.to_string();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if !text.trim().is_empty() {
            text.push('\n');
        }
        Ok(text + &block(draft, None))
    })?;
    Ok(format!("Added the routine {} to {}.", draft.id, machine.display()))
}

pub fn add(draft: &Draft) -> Result<String> {
    add_in(&crate::config::path(), &crate::config::repos_dir(), draft)
}

fn in_table(file: &Path, machine: &Path, id: &str, change: impl FnOnce(&mut Vec<String>, &Found) -> Result<()>) -> Result<()> {
    let is_machine = file == machine;
    rewrite(file, is_machine, |text| {
        let lines: Vec<&str> = text.lines().collect();
        let tables = routine_tables(&lines);
        let found = tables.iter().find(|table| table.id.as_deref() == Some(id)).ok_or(format!("{} has no [[routines]] table with id = \"{id}\"; edit it by hand", file.display()))?;
        let mut owned: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        change(&mut owned, found)?;
        let mut joined = owned.join("\n");
        if !joined.is_empty() {
            joined.push('\n');
        }
        Ok(joined)
    })
}

// An edit keeps the routine in the file it came from, and its id, since its runs and state are named by it.
pub fn replace_in(machine: &Path, file: &Path, id: &str, draft: &Draft) -> Result<String> {
    validate(draft)?;
    if draft.id != id {
        return Err(format!("a routine keeps its id; {id} cannot become {}", draft.id));
    }
    let default_repo = if file == machine { None } else { stem(file) };
    let table = block(draft, default_repo.as_deref());
    in_table(file, machine, id, |lines, found| {
        lines.splice(found.start..found.end, table.trim_end().lines().map(str::to_string));
        Ok(())
    })?;
    Ok(format!("Saved the routine {id} in {}.", file.display()))
}

fn key_of(line: &str) -> Option<&str> {
    line.split_once('=').map(|(key, _)| key.trim())
}

pub fn set_enabled_in(machine: &Path, file: &Path, id: &str, enabled: bool) -> Result<String> {
    in_table(file, machine, id, |lines, found| {
        let starts = table_lines(&lines.join("\n"));
        let at = (found.start + 1..found.end).find(|index| starts.get(*index).copied().unwrap_or(false) && key_of(&lines[*index]) == Some("enabled"));
        match (at, enabled) {
            (Some(index), true) => { lines.remove(index); }
            (Some(index), false) => lines[index] = "enabled = false".to_string(),
            (None, true) => {}
            (None, false) => {
                let last = (found.start..found.end).rev().find(|index| !quiet(&lines[*index])).unwrap_or(found.start);
                lines.insert(last + 1, "enabled = false".to_string());
            }
        }
        Ok(())
    })?;
    Ok(format!("{} the routine {id}.", if enabled { "Resumed" } else { "Paused" }))
}

pub fn delete_in(machine: &Path, file: &Path, id: &str, state: Option<&Path>) -> Result<String> {
    in_table(file, machine, id, |lines, found| {
        let mut start = found.start;
        while start > 0 && lines[start - 1].trim().is_empty() {
            start -= 1;
        }
        lines.drain(start..found.end);
        Ok(())
    })?;
    if let Some(dir) = state {
        let _ = fs::remove_file(routines::state_file(dir, id));
    }
    Ok(format!("Deleted the routine {id} from {}.", file.display()))
}

pub struct Asked<'a> {
    pub title: Option<&'a str>,
    pub id: Option<&'a str>,
    pub repo: Option<&'a str>,
    pub at: Option<&'a str>,
    pub every: Option<&'a str>,
    pub mode: Option<&'a str>,
    pub push: bool,
    pub paused: bool,
    pub prompt: &'a str,
}

pub fn draft_from(asked: &Asked, taken: &[String]) -> Result<Draft> {
    let usage = "kebbie routine add --title <name> --repo <folder or path> --at <\"07:00\" or \"mon 07:00\"> | --every <\"6h\"> [--mode report|plan|fix] [--push] [--paused] [--id <id>] <prompt>";
    let title = asked.title.map(str::trim).filter(|title| !title.is_empty()).ok_or(format!("a routine needs --title\n{usage}"))?;
    let repo = asked.repo.map(str::trim).filter(|repo| !repo.is_empty()).ok_or(format!("a routine needs --repo, the folder name or path of the repo it runs in\n{usage}"))?;
    let schedule = match (asked.at, asked.every) {
        (Some(at), None) => routines::parse_at(at)?,
        (None, Some(every)) => Schedule::Every(routines::parse_every(every)?),
        _ => return Err(format!("a routine needs one schedule, --at or --every\n{usage}")),
    };
    let mode = match asked.mode {
        None => Mode::Report,
        Some(name) => Mode::ALL.into_iter().find(|mode| mode.key() == name).ok_or(format!("--mode \"{name}\" is not a mode; use report, plan or fix"))?,
    };
    let prompt = asked.prompt.trim();
    if prompt.is_empty() {
        return Err(format!("a routine needs a prompt, what to check, as the last argument\n{usage}"));
    }
    let id = match asked.id {
        Some(id) => id.to_string(),
        None => free_id(&slug(title), taken),
    };
    Ok(Draft { id, title: title.to_string(), repo: repo.to_string(), schedule, prompt: prompt.to_string(), mode, push: asked.push, enabled: !asked.paused, profile: None })
}

pub fn add_cli(asked: &Asked) -> Result<String> {
    let taken: Vec<String> = routines::load().0.into_iter().map(|routine| routine.id).collect();
    add(&draft_from(asked, &taken)?)
}

pub fn file_of(routine: &Routine) -> PathBuf {
    PathBuf::from(&routine.source)
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Draft {
        fn of(routine: &Routine) -> Draft {
            Draft {
                id: routine.id.clone(), title: routine.title.clone(), repo: routine.repo.clone(), schedule: routine.schedule, prompt: routine.prompt.clone(),
                mode: routine.mode, push: routine.push, enabled: routine.enabled, profile: routine.profile.clone(),
            }
        }
    }

    fn draft() -> Draft {
        Draft {
            id: "cluster-health".to_string(), title: "Cluster health".to_string(), repo: "~/homelab-gitops".to_string(), schedule: Schedule::Daily(7 * 60),
            prompt: "Check every node, Longhorn volume and certificate.".to_string(), mode: Mode::Report, push: false, enabled: true, profile: None,
        }
    }

    fn files(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = crate::sh::test_dir(name);
        let repos = dir.join("repos");
        fs::create_dir_all(&repos).unwrap();
        (dir.join("config.toml"), repos, dir)
    }

    fn loaded(machine: &Path, repos: &Path) -> Vec<Routine> {
        let (found, errors) = routines::load_from(machine, repos);
        assert!(errors.is_empty(), "{errors:?}");
        found
    }

    #[test]
    fn the_when_box_reads_a_time_a_weekday_and_time_or_an_interval() {
        assert_eq!(parse_when("07:00"), Ok(Schedule::Daily(420)));
        assert_eq!(parse_when("mon 08:00"), Ok(Schedule::Weekly(0, 480)));
        assert_eq!(parse_when("every 6h"), Ok(Schedule::Every(6 * 3600)));
        assert_eq!(parse_when(" 30m "), Ok(Schedule::Every(1800)));
        assert!(parse_when("soon").unwrap_err().contains("is not a time"));
        assert!(parse_when("every day").unwrap_err().contains("is not an interval"));
        for text in ["07:00", "mon 08:00", "every 6h", "every 30m", "every 1d"] {
            assert_eq!(when_text(parse_when(text).unwrap()), text, "the box shows a schedule as it is typed");
        }
    }

    #[test]
    fn the_id_comes_from_the_name_and_steps_aside_for_a_taken_one() {
        assert_eq!(slug("Cluster health"), "cluster-health");
        assert_eq!(slug("  Disk on the devbox!  "), "disk-on-the-devbox");
        assert_eq!(slug("2nd backup"), "nd-backup", "an id starts with a letter");
        assert_eq!(slug("Åtgärd"), "tg-rd");
        assert_eq!(slug("!!"), "");
        assert!(routines::valid_id(&slug(&"word ".repeat(20))));
        assert_eq!(free_id("cluster-health", &["backups".to_string()]), "cluster-health");
        assert_eq!(free_id("cluster-health", &["cluster-health".to_string(), "cluster-health-2".to_string()]), "cluster-health-3");
        assert!(free_id(&"a".repeat(40), &["a".repeat(40)]).len() <= 40);
    }

    #[test]
    fn an_edited_routine_keeps_the_profile_its_table_names() {
        let text = block(&Draft { profile: Some("ops".to_string()), ..draft() }, None);
        assert!(text.contains("mode = \"report\"\nprofile = \"ops\"\n"), "{text}");
        let parsed = routines::parse_list(&text.parse::<Table>().unwrap(), None, "x").unwrap().remove(0);
        assert_eq!(Draft::of(&parsed).profile.as_deref(), Some("ops"), "the board's edit writes the table again with its profile");
    }

    #[test]
    fn the_popups_answers_become_the_table_it_shows_and_writes() {
        assert_eq!(block(&draft(), None), [
            "[[routines]]", "id = \"cluster-health\"", "title = \"Cluster health\"", "repo = \"~/homelab-gitops\"", "at = \"07:00\"", "mode = \"report\"",
            "prompt = \"\"\"Check every node, Longhorn volume and certificate.\"\"\"", "",
        ].join("\n"));
        let fix = Draft { schedule: Schedule::Every(6 * 3600), mode: Mode::Fix, push: true, enabled: false, prompt: "Say \"hi\" \\ then\nstop".to_string(), ..draft() };
        let text = block(&fix, Some("~/homelab-gitops"));
        assert!(text.contains("every = \"6h\"\nmode = \"fix\"\npush = true\nenabled = false\n") && !text.contains("repo ="), "{text}");
        let parsed = routines::parse_list(&text.parse::<Table>().unwrap(), Some("~/homelab-gitops"), "x").unwrap().remove(0);
        assert_eq!(Draft::of(&parsed), fix, "what is written reads back as the same routine, quotes and all");
    }

    #[test]
    fn adding_appends_a_table_to_config_toml_and_keeps_what_was_there() {
        let (machine, repos, dir) = files("routine-add");
        assert!(add_in(&machine, &repos, &draft()).unwrap().starts_with("Added the routine cluster-health to"));
        assert_eq!(loaded(&machine, &repos).len(), 1, "a missing config.toml is made");
        let before = "# my settings\n[keys]\ninbox = \"I\"";
        fs::write(&machine, before).unwrap();
        add_in(&machine, &repos, &draft()).unwrap();
        let text = fs::read_to_string(&machine).unwrap();
        assert!(text.starts_with("# my settings\n[keys]\ninbox = \"I\"\n\n[[routines]]\n"), "{text}");
        assert!(add_in(&machine, &repos, &draft()).unwrap_err().contains("the id \"cluster-health\" is taken by a routine in"));
        fs::write(repos.join("infra.toml"), "[[routines]]\nid = \"deps\"\ntitle = \"Deps\"\nevery = \"1d\"\nprompt = \"p\"\n").unwrap();
        assert!(add_in(&machine, &repos, &Draft { id: "deps".to_string(), ..draft() }).unwrap_err().contains("infra.toml"), "an id is unique across every file");
        assert!(add_in(&machine, &repos, &Draft { id: "x".to_string(), mode: Mode::Plan, push: true, ..draft() }).unwrap_err().contains("only a fix routine pushes"));
        fs::write(&machine, "[keys\n").unwrap();
        assert!(add_in(&machine, &repos, &Draft { id: "y".to_string(), ..draft() }).unwrap_err().contains("Fix that first"), "a file that does not parse is left alone");
        assert_eq!(fs::read_to_string(&machine).unwrap(), "[keys\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_command_lines_flags_make_the_same_draft_as_the_popup() {
        let asked = Asked { title: Some("Cluster health"), id: None, repo: Some("~/homelab-gitops"), at: Some("07:00"), every: None, mode: None, push: false, paused: false, prompt: " Check every node, Longhorn volume and certificate. " };
        assert_eq!(draft_from(&asked, &[]), Ok(draft()));
        assert_eq!(draft_from(&asked, &["cluster-health".to_string()]).unwrap().id, "cluster-health-2");
        let fix = draft_from(&Asked { every: Some("6h"), at: None, mode: Some("fix"), push: true, paused: true, id: Some("ci"), ..asked }, &[]).unwrap();
        assert_eq!((fix.id.as_str(), fix.schedule, fix.mode, fix.push, fix.enabled), ("ci", Schedule::Every(6 * 3600), Mode::Fix, true, false));
        assert!(draft_from(&Asked { title: None, ..asked }, &[]).unwrap_err().starts_with("a routine needs --title"));
        assert!(draft_from(&Asked { every: Some("6h"), ..asked }, &[]).unwrap_err().contains("one schedule, --at or --every"));
        assert!(draft_from(&Asked { mode: Some("auto"), ..asked }, &[]).unwrap_err().contains("\"auto\" is not a mode"));
        assert!(draft_from(&Asked { prompt: "", ..asked }, &[]).unwrap_err().contains("needs a prompt"));
    }

    #[test]
    fn a_config_toml_that_is_a_link_stays_a_link_and_its_target_gets_the_routine() {
        let (machine, repos, dir) = files("routine-link");
        let target = dir.join("dotfiles-config.toml");
        fs::write(&target, "[keys]\ninbox = \"I\"\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&target, &machine).unwrap();
        add_in(&machine, &repos, &draft()).unwrap();
        assert!(fs::symlink_metadata(&machine).unwrap().file_type().is_symlink());
        assert!(fs::read_to_string(&target).unwrap().contains("id = \"cluster-health\""));
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o644);
        fs::remove_dir_all(dir).unwrap();
    }

    const TWO: &str = "[keys]\ninbox = \"I\"\n\n[[routines]]\nid = \"backups\"\ntitle = \"Backups\"\nrepo = \"infra\"\nevery = \"6h\"\nprompt = \"\"\"\n[[routines]] is in this prompt\n\"\"\"\n\n# the cluster\n[[routines]]\nid = \"cluster-health\"\ntitle = \"Cluster health\"\nrepo = \"infra\"\nat = \"07:00\"\nprompt = \"p\"\n\n# notify\n[notify]\ncommand = \"true\"\n";

    #[test]
    fn editing_pausing_and_deleting_change_only_that_routines_table() {
        let (machine, repos, dir) = files("routine-edit");
        fs::write(&machine, TWO).unwrap();
        let routines = loaded(&machine, &repos);
        assert_eq!(routines.len(), 2);
        set_enabled_in(&machine, &machine, "cluster-health", false).unwrap();
        let text = fs::read_to_string(&machine).unwrap();
        assert!(text.contains("prompt = \"p\"\nenabled = false\n\n# notify\n[notify]"), "{text}");
        assert!(!loaded(&machine, &repos)[1].enabled && loaded(&machine, &repos)[0].enabled);
        set_enabled_in(&machine, &machine, "cluster-health", false).unwrap();
        assert_eq!(fs::read_to_string(&machine).unwrap(), text, "pausing twice changes nothing more");
        set_enabled_in(&machine, &machine, "cluster-health", true).unwrap();
        assert_eq!(fs::read_to_string(&machine).unwrap(), TWO, "resuming takes the line out again");
        let edited = Draft { title: "Nodes".to_string(), schedule: Schedule::Weekly(0, 480), ..Draft::of(&routines[0]) };
        assert!(replace_in(&machine, &machine, "backups", &edited).unwrap().starts_with("Saved the routine backups"));
        let after = loaded(&machine, &repos);
        assert_eq!((after[0].title.as_str(), after[0].schedule, after[1].id.as_str()), ("Nodes", Schedule::Weekly(0, 480), "cluster-health"));
        let text = fs::read_to_string(&machine).unwrap();
        assert!(text.starts_with("[keys]\ninbox = \"I\"\n\n[[routines]]\nid = \"backups\"\ntitle = \"Nodes\"") && text.contains("\"\"\"\n\n# the cluster\n[[routines]]\nid = \"cluster-health\""), "{text}");
        assert!(replace_in(&machine, &machine, "backups", &Draft { id: "other".to_string(), ..edited.clone() }).unwrap_err().contains("keeps its id"));
        let state = dir.join("state");
        routines::save_state(&state, "backups", &routines::State::default()).unwrap();
        delete_in(&machine, &machine, "backups", Some(&state)).unwrap();
        assert_eq!(fs::read_to_string(&machine).unwrap(), "[keys]\ninbox = \"I\"\n\n# the cluster\n[[routines]]\nid = \"cluster-health\"\ntitle = \"Cluster health\"\nrepo = \"infra\"\nat = \"07:00\"\nprompt = \"p\"\n\n# notify\n[notify]\ncommand = \"true\"\n");
        assert!(!routines::state_file(&state, "backups").exists(), "its state goes with it");
        assert!(delete_in(&machine, &machine, "backups", None).unwrap_err().contains("has no [[routines]] table with id = \"backups\""));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_repo_configs_routine_is_changed_in_its_own_file_without_a_repo_key() {
        let (machine, repos, dir) = files("routine-repo");
        let file = repos.join("infra.toml");
        fs::write(&file, "[worktree]\ndev_server = \"bin/dev\"\n\n[[routines]]\nid = \"deps\"\ntitle = \"Deps\"\nat = \"mon 07:00\"\nprompt = \"p\"\n").unwrap();
        let routine = loaded(&machine, &repos).remove(0);
        assert_eq!(file_of(&routine), file);
        replace_in(&machine, &file, "deps", &Draft { mode: Mode::Plan, ..Draft::of(&routine) }).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("[worktree]\ndev_server = \"bin/dev\"\n\n[[routines]]\nid = \"deps\"") && text.contains("mode = \"plan\"") && !text.contains("repo ="), "{text}");
        set_enabled_in(&machine, &file, "deps", false).unwrap();
        assert!(!loaded(&machine, &repos)[0].enabled);
        assert!(!machine.exists(), "config.toml is not touched");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_machine_name_goes_under_machine_and_leaves_every_other_byte() {
        let file = Path::new("/home/u/.config/kebbie/config.toml");
        let set = |text: &str| with_machine_name(file, text, "mac");
        assert_eq!(set("[machine] # me\r\n[keys]\r\ninbox = \"I\"\r\n").unwrap(), "[machine] # me\r\nname = \"mac\"\r\n[keys]\r\ninbox = \"I\"\r\n");
        assert_eq!(set("[machine]").unwrap(), "[machine]\nname = \"mac\"\n");
        assert_eq!(set("").unwrap(), "[machine]\nname = \"mac\"\n");
        assert_eq!(set("[machine]\nname = \"o\\\"#ld\"  # first\n").unwrap(), "[machine]\nname = \"mac\"  # first\n");
        assert_eq!(set("[machine]\nname = 'old' # literal\n").unwrap(), "[machine]\nname = \"mac\"\n");
        let prompt = "[[routines]]\nid = \"x\"\ntitle = \"X\"\nrepo = \"r\"\nevery = \"1d\"\nprompt = \"\"\"\n[machine]\nname = \"no\"\n\"\"\"\n";
        assert_eq!(set(prompt).unwrap(), format!("{prompt}\n[machine]\nname = \"mac\"\n"), "a header inside a prompt is no table");
        for other_form in ["machine = { name = \"old\" }\n", "machine.name = \"old\"\n"] {
            assert_eq!(set(other_form).unwrap_err(), "/home/u/.config/kebbie/config.toml: [machine] name is set in a form kebbie does not rewrite; edit it by hand", "{other_form}");
        }
        let dir = crate::sh::test_dir("machine-name-check");
        assert!(check_machine_name_in(&dir.join("config.toml"), &"x".repeat(65)).unwrap_err().contains("at most 64 characters"));
    }
}
