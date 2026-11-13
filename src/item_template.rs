use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::sh::Result;
use crate::vault_cli::Cli;

pub const KIND: &str = "task";
const FILE: &str = "item-template.json";
const VAULT_TTL: u64 = 6 * 60 * 60;
const RETRY: u64 = 10 * 60;
const RECHECK: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub key: String,
    pub heading: String,
    #[serde(default)]
    pub required: bool,
}

// An item's sections in order, which are required, and the one its open questions sit under: the vault's task template, a file config.toml names, or kebbie's own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub sections: Vec<Section>,
    #[serde(default)]
    pub questions: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub text: String,
    pub blocks: bool,
}

fn section(key: &str, heading: &str, required: bool) -> Section {
    Section { key: key.to_string(), heading: heading.to_string(), required }
}

fn heading_of(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|byte| *byte == b'#').count();
    let rest = line.get(level..)?;
    ((1..=6).contains(&level) && rest.starts_with([' ', '\t'])).then(|| (level, rest.trim().trim_end_matches('#').trim()))
}

fn same(one: &str, other: &str) -> bool {
    one.to_lowercase() == other.to_lowercase()
}

// `- [ ] ` blocks the start until the user answers it; a plain bullet is settled during the work; `- [x] ` is answered.
fn question_of(line: &str) -> Option<Question> {
    let line = line.trim();
    let (blocks, text) = match line.strip_prefix("- [ ] ").or_else(|| line.strip_prefix("* [ ] ")) {
        Some(text) => (true, text),
        None => (false, line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))?),
    };
    let text = text.trim();
    (!text.is_empty() && !text.starts_with("[x]") && !text.starts_with("[X]")).then(|| Question { text: text.to_string(), blocks })
}

// The byte range of the text under the section's heading at any level, as the vault finds it, up to the next heading of its level or above, headings in code fences aside.
fn range(body: &str, heading: &str) -> Option<(usize, usize)> {
    let mut at = 0;
    let mut fenced = false;
    let mut start: Option<(usize, usize)> = None;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.trim_start().starts_with("```") || trimmed.trim_start().starts_with("~~~") {
            fenced = !fenced;
        } else if !fenced && let Some((level, title)) = heading_of(trimmed) {
            if let Some((_, open)) = start.filter(|(own, _)| level <= *own) {
                return Some((open, at));
            }
            if start.is_none() && same(title, heading) {
                start = Some((level, at + line.len()));
            }
        }
        at += line.len();
    }
    start.map(|(_, open)| (open, body.len()))
}

impl Template {
    // kebbie's own, the same six sections as the vault's built-in task template.
    pub fn builtin() -> Template {
        Template {
            sections: vec![
                section("background", "Background", true),
                section("goal", "Goal", true),
                section("known", "What we know", false),
                section("questions", "Open questions", false),
                section("scope", "Out of scope", false),
                section("source", "Source", true),
            ],
            questions: Some("questions".to_string()),
        }
    }

    // JSON in the shape `mindin template --json` answers, or the same as TOML.
    pub fn parse(text: &str) -> Result<Template> {
        let value: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(json_error) => toml::from_str::<toml::Table>(text).map_err(|_| format!("neither JSON nor TOML ({json_error})")).and_then(|table| serde_json::to_value(table).map_err(|error| error.to_string()))?,
        };
        Template::from_value(&value)
    }

    pub fn from_value(value: &Value) -> Result<Template> {
        let template: Template = serde_json::from_value(value.clone()).map_err(|error| format!("not a template: {error}"))?;
        template.check()?;
        Ok(template)
    }

    fn check(&self) -> Result<()> {
        if self.sections.is_empty() {
            return Err("a template has at least one section".to_string());
        }
        let mut keys = std::collections::BTreeSet::new();
        for section in &self.sections {
            if !kebbie_hub_protocol::is_section_key(&section.key) {
                return Err(format!("\"{}\" is not a section key: a short word in lowercase letters, digits, - or _", section.key));
            }
            if !keys.insert(section.key.as_str()) {
                return Err(format!("the section {} is listed twice", section.key));
            }
            let heading = section.heading.trim();
            if heading.is_empty() || heading.chars().any(char::is_control) || heading.chars().count() > kebbie_hub_protocol::MAX_HEADING_CHARS {
                return Err(format!("the section {} has no heading on one line", section.key));
            }
        }
        match self.questions.as_deref() {
            Some(key) if !keys.contains(key) => Err(format!("the questions sit under {key}, which is not one of its sections")),
            _ => Ok(()),
        }
    }

    pub fn heading(&self, key: &str) -> Option<&str> {
        self.sections.iter().find(|section| section.key == key).map(|section| section.heading.as_str())
    }

    pub fn required(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(|section| section.required)
    }

    // The text under a section's heading, trimmed; empty when the body lacks the heading.
    pub fn text(&self, body: &str, key: &str) -> String {
        self.heading(key).and_then(|heading| range(body, heading)).map_or(String::new(), |(start, end)| body[start..end].trim().to_string())
    }

    pub fn has(&self, body: &str, key: &str) -> bool {
        self.heading(key).is_some_and(|heading| range(body, heading).is_some())
    }

    // Each heading the body lacks, and each required section it leaves empty, as `## Heading`.
    pub fn missing(&self, body: &str) -> Vec<String> {
        self.sections.iter().filter(|section| !self.has(body, &section.key) || (section.required && self.text(body, &section.key).is_empty())).map(|section| format!("## {}", section.heading)).collect()
    }

    pub fn questions_in(&self, body: &str) -> Vec<Question> {
        self.questions.as_deref().map_or_else(Vec::new, |key| self.text(body, key).lines().filter_map(question_of).collect())
    }

    // A section's text as the phone shows and edits it; the questions section without its questions, which go apart.
    pub fn shown(&self, body: &str, key: &str) -> String {
        let text = self.text(body, key);
        if self.questions.as_deref() != Some(key) {
            return text;
        }
        text.lines().filter(|line| question_of(line).is_none()).collect::<Vec<_>>().join("\n").trim().to_string()
    }

    // The body with one section's text replaced, its open questions kept; None when the body lacks the section.
    pub fn replace(&self, body: &str, key: &str, text: &str) -> Option<String> {
        let (start, end) = range(body, self.heading(key)?)?;
        let kept: Vec<&str> = if self.questions.as_deref() == Some(key) { body[start..end].lines().filter(|line| question_of(line).is_some()).collect() } else { Vec::new() };
        let mut written = text.trim().to_string();
        if !kept.is_empty() {
            written = if written.is_empty() { kept.join("\n") } else { format!("{written}\n\n{}", kept.join("\n")) };
        }
        let head = &body[..start];
        let newline = if head.ends_with('\n') { "" } else { "\n" };
        let after = &body[end..];
        let middle = match (written.is_empty(), after.is_empty()) {
            (true, true) => String::new(),
            (true, false) => "\n".to_string(),
            (false, true) => format!("\n{written}\n"),
            (false, false) => format!("\n{written}\n\n"),
        };
        Some(format!("{head}{newline}{middle}{after}"))
    }

    // The section that says what an item is after: one keyed goal, else the first required one.
    pub fn goal_key(&self) -> Option<&str> {
        self.heading("goal").map(|_| "goal").or_else(|| self.required().next().map(|section| section.key.as_str()))
    }

    // The line the board shows for a ready item: the goal's first, else the background's, else the first section's.
    pub fn first_line(&self, body: &str) -> String {
        let keys = self.goal_key().into_iter().chain(["background"]).map(str::to_string).chain(self.sections.first().map(|section| section.key.clone()));
        keys.filter_map(|key| self.shown(body, &key).lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)).next().unwrap_or_default()
    }

    pub fn skeleton(&self) -> String {
        self.sections.iter().map(|section| format!("## {}\n", section.heading)).collect::<Vec<_>>().join("\n")
    }

    // A body with every heading in order, the given texts under theirs.
    pub fn body_with(&self, texts: &[(&str, String)]) -> String {
        let mut body = String::new();
        for section in &self.sections {
            let text = texts.iter().find(|(key, _)| *key == section.key).map(|(_, text)| text.trim()).unwrap_or_default();
            body.push_str(&format!("## {}\n\n{text}{}", section.heading, if text.is_empty() { "" } else { "\n\n" }));
        }
        body
    }

    pub fn names(headings: &[String]) -> String {
        let bare: Vec<&str> = headings.iter().map(|heading| heading.trim_start_matches("## ")).collect();
        match bare.as_slice() {
            [] => String::new(),
            [one] => one.to_string(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        }
    }

    pub fn required_names(&self) -> String {
        Template::names(&self.required().map(|section| section.heading.clone()).collect::<Vec<_>>())
    }

    pub fn shown_lines(&self) -> String {
        let questions = self.questions.as_deref().and_then(|key| self.heading(key)).map_or(String::new(), |heading| format!("\nOpen questions go under {heading}: `- [ ] ` before one that blocks the start, `- ` before one the work settles."));
        let lines: Vec<String> = self.sections.iter().map(|section| format!("## {}{}", section.heading, if section.required { "   (required)" } else { "" })).collect();
        format!("{}{questions}", lines.join("\n"))
    }
}

type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    std::fs::metadata(path).ok().and_then(|meta| Some((meta.modified().ok()?, meta.len())))
}

// One answer per set of file stamps, and the stamps looked at no more than every RECHECK, so a pass over every item reads nothing twice.
struct Held<T> {
    checked: Instant,
    stamps: Vec<Stamp>,
    paths: Vec<PathBuf>,
    value: T,
}

fn held<T: Clone>(cache: &Mutex<Option<Held<T>>>, paths: impl Fn() -> Vec<PathBuf>, read: impl FnOnce(&[PathBuf]) -> T) -> T {
    let mut cache = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(kept) = cache.as_mut() {
        if kept.checked.elapsed() < RECHECK {
            return kept.value.clone();
        }
        let paths = paths();
        let stamps: Vec<Stamp> = paths.iter().map(|path| stamp(path)).collect();
        if paths == kept.paths && stamps == kept.stamps {
            kept.checked = Instant::now();
            return kept.value.clone();
        }
    }
    let paths = paths();
    let stamps = paths.iter().map(|path| stamp(path)).collect();
    let value = read(&paths);
    *cache = Some(Held { checked: Instant::now(), stamps, paths, value: value.clone() });
    value
}

// The file [board.backlog] template names, read against the config's own folder when relative.
pub fn configured_file(config: &Path) -> Option<PathBuf> {
    let named = crate::config::load_machine(config).ok()?.template?;
    let named = crate::vault::expand(&named);
    Some(if named.is_absolute() { named } else { config.parent().map_or(named.clone(), |dir| dir.join(&named)) })
}

pub fn read_file(path: &Path) -> Result<Template> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Template::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
}

// The template local items are written to: the file config.toml names, else kebbie's own; a file that does not read is the built-in one, and doctor says why.
pub fn local() -> Template {
    static LOCAL: Mutex<Option<Held<Template>>> = Mutex::new(None);
    if cfg!(test) {
        return Template::builtin();
    }
    let config = crate::config::path();
    held(&LOCAL, || [config.clone()].into_iter().chain(configured_file(&config)).collect(), |paths| paths.get(1).and_then(|file| read_file(file).ok()).unwrap_or_else(Template::builtin))
}

pub fn local_checked() -> Result<(Template, Option<PathBuf>)> {
    match configured_file(&crate::config::path()) {
        Some(file) => read_file(&file).map(|template| (template, Some(file))),
        None => Ok((Template::builtin(), None)),
    }
}

fn file(state: &Path) -> PathBuf {
    state.join(FILE)
}

// What the vault's template answer was kept as: the command that answered, when, the template, and whether that ask failed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    pub command: String,
    pub at: u64,
    pub template: Option<Template>,
    pub failed: bool,
}

impl Kept {
    // A failed ask is tried again after RETRY, an answer, with a template or without, after VAULT_TTL.
    fn fresh(&self, command: &str, now: u64) -> bool {
        self.command == command && now.saturating_sub(self.at) < if self.failed { RETRY } else { VAULT_TTL }
    }
}

fn read_kept(path: &Path) -> Option<Kept> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some(Kept { command: value["command"].as_str().unwrap_or_default().to_string(), at: value["at"].as_u64().unwrap_or_default(), template: Template::from_value(&value["template"]).ok(), failed: value["failed"] == true })
}

static KEPT: Mutex<Option<Held<Option<Kept>>>> = Mutex::new(None);

pub fn kept_in(state: &Path) -> Option<Kept> {
    if cfg!(test) {
        return read_kept(&file(state));
    }
    held(&KEPT, || vec![file(state)], |paths| read_kept(&paths[0]))
}

// The vault's task template as kebbie last read it, or None while the vault has given none.
pub fn vault() -> Option<Template> {
    kept_in(&crate::lead::state_location().ok()?)?.template
}

pub fn for_vault() -> Template {
    vault().unwrap_or_else(Template::builtin)
}

pub fn for_source(source: &str) -> Template {
    if source == "vault" { for_vault() } else { local() }
}

// Whether the vault command said it gives templates; a command not yet probed has not.
pub fn templated(cli: &Cli) -> bool {
    cli.capabilities.as_ref().is_some_and(|stated| stated.iter().any(|one| one == crate::vault_cli::TEMPLATES))
}

// Asks the vault for its task template once the answer kept from this command is stale; a vault without templates leaves none kept. Runs from the vault routine, never per item.
pub fn refresh(cli: &Cli, state: &Path, now: u64) -> Result<bool> {
    let path = file(state);
    if !templated(cli) {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
            forget();
        }
        return Ok(false);
    }
    let kept = kept_in(state);
    if kept.as_ref().is_some_and(|kept| kept.fresh(&cli.cache_key(), now)) {
        return Ok(false);
    }
    let fetched = cli.template(KIND);
    let template = match &fetched {
        Ok(template) => template.clone(),
        Err(_) => kept.and_then(|kept| kept.template),
    };
    crate::state::write(&path, json!({ "command": cli.cache_key(), "at": now, "template": template, "failed": fetched.is_err() }).to_string())?;
    forget();
    fetched.map(|_| true)
}

// The kept answer is read afresh after this process rewrote it, not up to RECHECK later.
fn forget() {
    *KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

// What a task's note must hold: with the vault's templates, the task template's required sections, which task check reads when no keys are named; else the keys of the notes before templates.
#[derive(Clone, Debug, PartialEq)]
pub struct Need {
    pub keys: Vec<&'static str>,
    pub named: String,
}

impl Need {
    pub fn templated(&self) -> bool {
        self.keys.is_empty()
    }

    // The sections a check answered missing, by their headings where the template names them.
    pub fn shown(&self, missing: &[String]) -> String {
        let template = for_vault();
        Template::names(&missing.iter().map(|key| template.heading(key).map_or_else(|| key.clone(), str::to_string)).collect::<Vec<_>>())
    }
}

pub fn start_need(templated: bool) -> Need {
    if templated { Need { keys: Vec::new(), named: for_vault().required_names() } } else { Need { keys: vec!["problem"], named: "Problem".to_string() } }
}

pub fn done_need(templated: bool) -> Need {
    if templated { Need { keys: Vec::new(), named: for_vault().required_names() } } else { Need { keys: vec!["solution", "decisions"], named: "solution and decisions".to_string() } }
}

// For the task's role, when it is written: whether the vault this machine runs states templates, by the verdict kept from its last probe.
pub fn templated_now() -> bool {
    crate::vault_cli::cli().is_some_and(|cli| templated(&cli))
}

// `kebbie backlog template`: the template where new items go, or the one asked for; the vault's is read afresh when kebbie has none kept.
pub fn shown(local_only: bool, vault_only: bool, as_json: bool) -> Result<String> {
    let filing = crate::config::filing(&crate::config::sources()?);
    let from_vault = vault_only || (!local_only && filing == Some(crate::config::Source::Vault));
    let (template, from) = if from_vault {
        let state = crate::lead::state_dir()?;
        if let Some(cli) = crate::vault_cli::detect() {
            refresh(&cli, &state, crate::messages::now())?;
        }
        match vault() {
            Some(template) => (template, "the vault's task template".to_string()),
            None => (Template::builtin(), "kebbie's own: the vault gives no template".to_string()),
        }
    } else {
        match local_checked()? {
            (template, Some(file)) => (template, file.display().to_string()),
            (template, None) => (template, "kebbie's own".to_string()),
        }
    };
    if as_json {
        return serde_json::to_string_pretty(&json!({ "from": from, "sections": template.sections, "questions": template.questions, "skeleton": template.skeleton() })).map_err(|error| error.to_string());
    }
    Ok(format!("A backlog item's sections, in order ({from}):\n\n{}", template.shown_lines()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "## Background\n\nIt broke.\n\n## Goal\n\nIt works.\nFor everyone.\n\n## What we know\n\n```\n## not a heading\n```\n\n## Open questions\n\nSome prose.\n- [ ] Keep the old links?\n- Which test, settled in the work\n- [x] Which repo: app\n\n## Out of scope\n\n## Source\n\nThe user, 2026-11-12.\n";

    #[test]
    fn a_section_is_read_by_its_heading_up_to_the_next_one_of_its_level() {
        let template = Template::builtin();
        assert_eq!(template.text(BODY, "goal"), "It works.\nFor everyone.");
        assert_eq!(template.text(BODY, "known"), "```\n## not a heading\n```", "a heading inside a code fence is text");
        assert_eq!(template.text(BODY, "scope"), "");
        assert_eq!(template.text("## goal\n\nlower case\n", "goal"), "lower case", "headings match whatever their case");
        assert_eq!(template.text("## Goal\n\nOne\n### Detail\n\nTwo\n## Source\n", "goal"), "One\n### Detail\n\nTwo", "a deeper heading stays inside");
        assert!(template.missing(BODY).is_empty());
        assert_eq!(template.missing("## Background\n\nx\n\n## Goal\n\n## Source\n\ny\n"), ["## Goal", "## What we know", "## Open questions", "## Out of scope"], "an empty required section and each absent heading");
    }

    #[test]
    fn the_questions_go_apart_from_their_sections_prose_and_survive_an_edit_of_it() {
        let template = Template::builtin();
        assert_eq!(template.questions_in(BODY), [Question { text: "Keep the old links?".to_string(), blocks: true }, Question { text: "Which test, settled in the work".to_string(), blocks: false }]);
        assert_eq!(template.shown(BODY, "questions"), "Some prose.\n- [x] Which repo: app");
        let edited = template.replace(BODY, "questions", "Other prose.").unwrap();
        assert_eq!(template.shown(&edited, "questions"), "Other prose.");
        assert_eq!(template.questions_in(&edited).len(), 2, "{edited}");
        let goal = template.replace(BODY, "goal", "  New goal.\n").unwrap();
        assert_eq!(template.text(&goal, "goal"), "New goal.");
        assert_eq!(template.text(&goal, "background"), "It broke.");
        assert_eq!(template.text(&goal, "source"), "The user, 2026-11-12.");
        assert!(goal.contains("## Goal\n\nNew goal.\n\n## What we know"), "{goal}");
        let last = template.replace(BODY, "source", "Someone else.").unwrap();
        assert!(last.ends_with("## Source\n\nSomeone else.\n"), "{last}");
        let emptied = template.replace(BODY, "background", "").unwrap();
        assert!(emptied.starts_with("## Background\n\n## Goal\n\nIt works."), "{emptied}");
        assert!(template.replace("## Goal\n\nx\n", "source", "y").is_none(), "a body without the heading is not edited");
        assert_eq!(template.first_line(BODY), "It works.");
    }

    #[test]
    fn a_template_reads_from_the_vaults_answer_or_a_file_and_refuses_what_the_phone_could_not_take() {
        let answer = r##"{"kind":"task","language":"sv","sections":[{"key":"problem","heading":"Problem","required":true},{"key":"open","heading":"Öppna frågor","required":false}],"questions":"open","skeleton":""}"##;
        let template = Template::parse(answer).unwrap();
        assert_eq!((template.heading("open"), template.questions.as_deref(), template.required_names()), (Some("Öppna frågor"), Some("open"), "Problem".to_string()));
        let toml = "questions = \"risks\"\n[[sections]]\nkey = \"goal\"\nheading = \"Goal\"\nrequired = true\n[[sections]]\nkey = \"risks\"\nheading = \"Risks\"\n";
        assert_eq!(Template::parse(toml).unwrap().sections.len(), 2);
        for wrong in [r#"{"sections":[]}"#, r#"{"sections":[{"key":"Goal","heading":"Goal"}]}"#, r#"{"sections":[{"key":"goal","heading":"Goal"},{"key":"goal","heading":"Again"}]}"#, r#"{"sections":[{"key":"goal","heading":"Goal"}],"questions":"risks"}"#, r#"{"sections":[{"key":"goal","heading":" "}]}"#, "not a template"] {
            assert!(Template::parse(wrong).is_err(), "{wrong}");
        }
        assert_eq!(Template::builtin().skeleton(), "## Background\n\n## Goal\n\n## What we know\n\n## Open questions\n\n## Out of scope\n\n## Source\n");
        assert_eq!(Template::builtin().required_names(), "Background, Goal and Source");
    }

    #[test]
    fn the_vaults_template_is_asked_once_per_command_and_while_fresh_never_again() {
        let fake = crate::vault_cli::fake::Fake::new("template");
        let state = fake.state();
        fake.answer("template", r##"{"kind":"task","language":"en","sections":[{"key":"problem","heading":"Problem","required":true}],"questions":null,"skeleton":""}"##);
        let plain = fake.cli.clone();
        assert!(!refresh(&Cli { capabilities: None, ..plain.clone() }, &state, 100).unwrap(), "a command not probed has stated no templates");
        let cli = Cli { capabilities: Some(vec!["tasks".to_string(), "templates".to_string()]), ..plain.clone() };
        assert!(refresh(&cli, &state, 100).unwrap());
        assert!(!refresh(&cli, &state, 100 + VAULT_TTL - 1).unwrap(), "fresh: not asked again");
        assert_eq!(fake.calls().iter().filter(|call| call.starts_with("template")).count(), 1);
        assert_eq!(kept_in(&state).and_then(|kept| kept.template).map(|template| template.required_names()), Some("Problem".to_string()));
        assert!(refresh(&cli, &state, 100 + VAULT_TTL).unwrap(), "asked again once old");
        fake.fail("template---json", "the vault is away");
        assert!(refresh(&cli, &state, 100 + 3 * VAULT_TTL).is_err());
        assert!(kept_in(&state).and_then(|kept| kept.template).is_some(), "a failed ask keeps the template it had");
        assert!(!refresh(&Cli { capabilities: Some(vec!["tasks".to_string()]), ..plain }, &state, 100).unwrap());
        assert!(kept_in(&state).is_none(), "a vault without templates leaves none kept");
    }

    #[test]
    fn a_vault_with_no_task_template_or_one_that_fails_is_not_asked_on_every_tick() {
        let fake = crate::vault_cli::fake::Fake::new("template-none");
        let state = fake.state();
        let cli = Cli { capabilities: Some(vec!["templates".to_string()]), ..fake.cli.clone() };
        let asked = || fake.calls().iter().filter(|call| call.starts_with("template")).count();
        fake.answer("template", r##"{"kind":"task","language":"en","sections":[],"questions":null,"skeleton":""}"##);
        assert!(refresh(&cli, &state, 100).unwrap());
        assert_eq!(kept_in(&state).map(|kept| (kept.template, kept.failed)), Some((None, false)), "no template is an answer");
        for tick in 1..10 {
            assert!(!refresh(&cli, &state, 100 + tick * 20).unwrap());
        }
        assert_eq!(asked(), 1, "one ask while the answer is fresh");
        assert!(refresh(&cli, &state, 100 + VAULT_TTL).unwrap());
        assert_eq!(asked(), 2);

        let away = crate::vault_cli::fake::Fake::new("template-away");
        let state = away.state();
        let cli = Cli { capabilities: Some(vec!["templates".to_string()]), ..away.cli.clone() };
        away.fail("template---json", "the vault is away");
        assert!(refresh(&cli, &state, 100).is_err());
        assert!(!refresh(&cli, &state, 120).unwrap() && !refresh(&cli, &state, 100 + RETRY - 1).unwrap(), "a failure waits RETRY before the next ask");
        assert_eq!(away.calls().len(), 1);
        assert!(refresh(&cli, &state, 100 + RETRY).is_err());
        assert_eq!(away.calls().len(), 2);
    }

    #[test]
    fn the_kept_answer_is_read_afresh_right_after_this_process_rewrote_it() {
        let fake = crate::vault_cli::fake::Fake::new("template-forget");
        let state = fake.state();
        let cli = Cli { capabilities: Some(vec!["templates".to_string()]), ..fake.cli.clone() };
        fake.answer("template", r##"{"kind":"task","sections":[{"key":"problem","heading":"Problem","required":true}]}"##);
        refresh(&cli, &state, 100).unwrap();
        let read = || held(&KEPT, || vec![file(&state)], |paths| read_kept(&paths[0])).and_then(|kept| kept.template).map(|template| template.required_names());
        assert_eq!(read().as_deref(), Some("Problem"));
        fake.answer("template", r##"{"kind":"task","sections":[{"key":"goal","heading":"Goal","required":true}]}"##);
        refresh(&cli, &state, 100 + VAULT_TTL).unwrap();
        assert_eq!(read().as_deref(), Some("Goal"), "not the answer held from before the rewrite");
    }

    #[test]
    fn a_section_is_found_under_a_heading_of_any_level_as_the_vault_finds_it() {
        let template = Template::builtin();
        assert_eq!(template.text("# Goal\n\nTop.\n\n## Detail\n\nMore.\n\n# Source\n\nx\n", "goal"), "Top.\n\n## Detail\n\nMore.");
        assert_eq!(template.text("### goal\n\nDeep.\n## Source\n", "goal"), "Deep.");
        assert!(template.missing("# Background\n\nb\n\n### Goal\n\ng\n\n## What we know\n\n## Open questions\n\n## Out of scope\n\n#### Source\n\ns\n").is_empty());
    }

    #[test]
    fn a_template_file_config_toml_names_is_read_against_its_folder() {
        let dir = crate::sh::test_dir("item-template-file");
        let config = dir.join("config.toml");
        std::fs::write(&config, "[board.backlog]\nsources = [\"local\"]\ntemplate = \"item.toml\"\n").unwrap();
        std::fs::write(dir.join("item.toml"), "[[sections]]\nkey = \"goal\"\nheading = \"Goal\"\nrequired = true\n").unwrap();
        let file = configured_file(&config).unwrap();
        assert_eq!(file, dir.join("item.toml"));
        assert_eq!(read_file(&file).unwrap().required_names(), "Goal");
        std::fs::write(&config, "[board.backlog]\nsources = [\"local\"]\n").unwrap();
        assert_eq!(configured_file(&config), None, "kebbie's own without the key");
    }
}
