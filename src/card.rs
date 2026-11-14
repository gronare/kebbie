use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sh::Result;

pub const BUILT: &str = "Built";
pub const DECISIONS: &str = "Decisions";
pub const NOTICED: &str = "Also noticed";
pub const MAX_BUILT: usize = kebbie_hub_protocol::MAX_BUILT_LINES;
const MAX_LINE: usize = kebbie_hub_protocol::MAX_CARD_LINE_CHARS;
const MAX_DECISION: usize = kebbie_hub_protocol::MAX_DECISION_CHARS;
const MAX_DECISIONS: usize = kebbie_hub_protocol::MAX_DECISIONS;
const MAX_NOTICED: usize = kebbie_hub_protocol::MAX_NOTICED;
const CONFIRMS: [&str; 6] = ["confirm", "bekräfta", "verify", "verifiera", "please check", "att kontrollera"];
const SHAPE: &str = "the card is a bold headline, then Built:, Decisions: and Also noticed: (the skill kebbie:playbook, page CARD.md)";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum By {
    You,
    Lead,
    Investigator,
    Task,
}

impl By {
    pub fn word(self) -> &'static str {
        match self {
            By::You => "you",
            By::Lead => "lead",
            By::Investigator => "investigator",
            By::Task => "task",
        }
    }

    fn named(word: &str) -> Option<By> {
        [By::You, By::Lead, By::Investigator, By::Task].into_iter().find(|by| by.word() == word.trim().to_lowercase())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Question,
    Method,
    Obstacle,
    Plan,
}

impl Source {
    fn word(self) -> &'static str {
        match self {
            Source::Question => "question",
            Source::Method => "question of method",
            Source::Obstacle => "obstacle",
            Source::Plan => "approved with the plan",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub text: String,
    pub by: By,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Noticed {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<String>,
}

// The card as kebbie keeps it: the task's headline, Built and Also noticed in its words, the decisions with who took each from kebbie's own records, and the version before it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Data {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_at: Option<u64>,
    pub headline: String,
    pub built: Vec<String>,
    pub decisions: Vec<Decision>,
    pub noticed: Vec<Noticed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Box<Data>>,
}

impl Data {
    fn same_lines(&self, other: &Data) -> bool {
        (&self.headline, &self.built, &self.decisions, &self.noticed) == (&other.headline, &other.built, &other.decisions, &other.noticed)
    }
}

// A decision line as the task wrote it: the choice, who it says took it, and the line that replaced it.
#[derive(Clone, Debug, PartialEq)]
pub struct Claimed {
    pub text: String,
    pub by: Option<By>,
    pub replaced_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub headline: String,
    pub built: Vec<String>,
    pub decisions: Vec<Claimed>,
    pub noticed: Vec<String>,
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn bare(text: &str) -> &str {
    text.trim().trim_matches(['*', '_']).trim()
}

// A paragraph's first line opens a part when it is a short label: "Built:", "**Also noticed**", "Confirm: the example…".
fn label_of(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("![") {
        return None;
    }
    let (head, rest) = match trimmed.split_once(':') {
        Some((head, rest)) => (head, rest),
        None if trimmed.starts_with("**") && trimmed.ends_with("**") => (trimmed, ""),
        None => return None,
    };
    let rest = rest.trim_start_matches(['*', '_']).trim();
    let head = bare(head);
    let words = head.split_whitespace().count();
    let lettered = head.chars().next().is_some_and(char::is_alphabetic) && head.chars().all(|character| character.is_alphanumeric() || " ()'’,-/".contains(character));
    let shaped = lettered && (1..=4).contains(&words) && !rest.starts_with("//") && (trimmed.starts_with("**") || head.chars().next().is_some_and(char::is_uppercase));
    shaped.then(|| (head.to_string(), rest.to_string()))
}

fn part_of(label: &str) -> Option<&'static str> {
    [BUILT, DECISIONS, NOTICED].into_iter().find(|part| part.eq_ignore_ascii_case(label))
}

fn asks_to_confirm(text: &str) -> bool {
    let lower = text.to_lowercase();
    CONFIRMS.iter().any(|word| lower.contains(word))
}

// A bold lead-in at a line's start, as in "- **Please confirm:** …".
fn lead_in(line: &str) -> Option<&str> {
    let rest = line.trim().trim_start_matches("- ").trim_start();
    let inner = rest.strip_prefix("**")?;
    let end = inner.find("**")?;
    Some(inner[..end].trim_end_matches(':'))
}

fn refused(what: &str) -> String {
    format!("kebbie takes the card only in its shape: {what}. {SHAPE}.")
}

// The items under a part: each "- " line with the lines indented under it, or each line of prose.
fn items(lines: &[&str]) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            Some(item) => items.push(item.trim().to_string()),
            None if line.starts_with([' ', '\t']) && !items.is_empty() => {
                let last = items.last_mut().expect("there is an item to continue");
                last.push(' ');
                last.push_str(trimmed);
            }
            None => items.push(trimmed.to_string()),
        }
    }
    items.into_iter().map(|item| one_line(&item)).collect()
}

fn claimed(line: &str) -> Claimed {
    let lower = line.to_lowercase();
    let (line, replaced_by) = match lower.find("replaced by:") {
        Some(at) => (line[..at].trim_end_matches([' ', ';', ',', '—', '-', '·']).to_string(), Some(one_line(&line[at + "replaced by:".len()..])).filter(|text| !text.is_empty())),
        None => (line.to_string(), None),
    };
    let line = line.trim().trim_end_matches('.');
    let (text, by) = match line.rsplit_once('(').filter(|(_, rest)| rest.ends_with(')')) {
        Some((text, who)) => match By::named(who.trim_end_matches(')').split(',').next().unwrap_or_default()) {
            Some(by) => (text.trim().to_string(), Some(by)),
            None => (line.to_string(), None),
        },
        None => (line.to_string(), None),
    };
    Claimed { text: text.trim_end_matches('.').to_string(), by, replaced_by }
}

fn confirm_refused(what: &str) -> String {
    refused(&format!("{what} asks the user to confirm; a choice that is theirs is asked with kebbie ask before you build it, a check only they can make with kebbie ask --verify, and a choice you took is a line under Decisions:"))
}

fn headline_of(paragraph: &[&str]) -> std::result::Result<String, String> {
    let joined = one_line(&paragraph.join(" "));
    match joined.strip_prefix("**").and_then(|rest| rest.find("**").map(|end| (rest[..end].trim().to_string(), rest[end + 2..].trim().to_string()))) {
        Some((bold, rest)) if rest.is_empty() => Ok(bold),
        Some(_) => Err(refused("the headline is the bold line alone; what follows it goes under Built:")),
        None => Ok(joined),
    }
}

// The card file read into its parts, or why kebbie refuses it: a part asking the user to confirm, or a label outside the three.
pub fn parse(text: &str) -> std::result::Result<Parsed, String> {
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            open = false;
            continue;
        }
        if !open {
            paragraphs.push(Vec::new());
            open = true;
        }
        paragraphs.last_mut().expect("a paragraph is open").push(line);
        if let Some(lead) = lead_in(line).filter(|lead| asks_to_confirm(lead)) {
            return Err(confirm_refused(&format!("\"{lead}\"")));
        }
    }
    let mut headline = String::new();
    let mut current: Option<usize> = None;
    let mut parts: [Vec<String>; 3] = Default::default();
    for (index, paragraph) in paragraphs.iter().enumerate() {
        let label = label_of(paragraph[0]);
        let part = label.as_ref().and_then(|(label, _)| part_of(label));
        if index == 0 && part.is_none() {
            headline = headline_of(paragraph)?;
            continue;
        }
        let mut body: Vec<String> = paragraph.iter().map(|line| line.to_string()).collect();
        match (label, part) {
            (Some((_, rest)), Some(part)) => {
                current = [BUILT, DECISIONS, NOTICED].iter().position(|named| *named == part);
                body.remove(0);
                if !rest.is_empty() {
                    body.insert(0, rest);
                }
            }
            (Some((label, _)), None) if asks_to_confirm(&label) => return Err(confirm_refused(&format!("\"{label}:\""))),
            (Some((label, _)), None) => return Err(refused(&format!("\"{label}:\" is not one of its parts; what the user can do now goes under Built:, a choice taken under Decisions:, and what lies outside the task under Also noticed:"))),
            (None, _) if current.is_none() => return Err(refused("text between the headline and Built: belongs under Built:")),
            (None, _) => {}
        }
        let lines: Vec<&str> = body.iter().map(String::as_str).collect();
        parts[current.expect("a part is open")].extend(items(&lines));
    }
    let [built, decisions, noticed] = parts;
    if headline.is_empty() {
        return Err(refused("it opens with a part, not with the headline"));
    }
    if built.is_empty() {
        return Err(refused("Built: is missing or empty; it says what the user can do now, and for work with no commits it is the whole card"));
    }
    if built.len() > MAX_BUILT {
        return Err(refused(&format!("Built: has {} lines, and holds at most {MAX_BUILT}; say what the user can do now, and leave the how to the commits", built.len())));
    }
    if let Some(long) = built.iter().chain(&noticed).find(|line| line.chars().count() > MAX_LINE) {
        return Err(refused(&format!("a line is over {MAX_LINE} characters: {}…", long.chars().take(60).collect::<String>())));
    }
    if let Some(long) = decisions.iter().find(|line| line.chars().count() > MAX_DECISION) {
        return Err(refused(&format!("a decision is over {MAX_DECISION} characters: {}…", long.chars().take(60).collect::<String>())));
    }
    if decisions.len() > MAX_DECISIONS || noticed.len() > MAX_NOTICED {
        return Err(refused(&format!("it lists at most {MAX_DECISIONS} decisions and {MAX_NOTICED} things noticed")));
    }
    Ok(Parsed { headline, built, decisions: decisions.iter().map(|line| claimed(line)).collect(), noticed })
}

// A decision kebbie itself recorded: an obstacle's decision, the user's answer to a question, the lead's call on a question of method.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub text: String,
    pub by: By,
    pub source: Source,
    pub at: u64,
    pub key: Option<String>,
}

fn significant(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !character.is_alphanumeric()).map(str::to_lowercase).filter(|word| word.chars().count() > 3 && word != "because").collect()
}

fn same_decision(line: &str, record: &Record) -> bool {
    if record.key.as_ref().is_some_and(|key| line.split(|character: char| !character.is_alphanumeric()).any(|word| word == key)) {
        return true;
    }
    let (ours, theirs) = (significant(line), significant(&record.text));
    let common = ours.intersection(&theirs).count();
    common >= 2 && common * 2 >= ours.len().min(theirs.len())
}

fn clip(text: &str, max: usize) -> String {
    let text = one_line(text);
    if text.chars().count() <= max { text } else { format!("{}…", text.chars().take(max - 1).collect::<String>()) }
}

pub fn records(state: &Path, address: &str) -> Vec<Record> {
    let mut records: Vec<Record> = crate::obstacle::load(state, address).into_iter().filter_map(|obstacle| {
        let decision = obstacle.decision.clone().filter(|decision| decision.kind != crate::obstacle::Kind::Ask)?;
        let by = match decision.by {
            crate::obstacle::By::You => By::You,
            crate::obstacle::By::Lead => By::Lead,
            crate::obstacle::By::Investigator => By::Investigator,
        };
        let line = decision.line();
        let what = line.strip_suffix(&format!(" ({})", decision.by.word())).unwrap_or(&line);
        Some(Record { text: clip(&format!("{}: {what}", obstacle.short()), MAX_DECISION), by, source: Source::Obstacle, at: decision.at, key: Some(obstacle.id) })
    }).collect();
    records.extend(answered(&crate::messages::load(), address));
    records.sort_by_key(|record| record.at);
    records
}

// Each question the task asked and how it was settled: the user's answer, or the lead's call when it was a question of method.
pub fn answered(messages: &[crate::messages::Message], address: &str) -> Vec<Record> {
    const LEADS: &str = "Your question is about how to build it, so this is the lead's call, not the user's: ";
    messages.iter().filter(|asked| asked.from == address && asked.question.is_some()).filter_map(|asked| {
        let answer = messages.iter().find(|answer| answer.to == address && answer.about.as_deref() == Some(asked.id.as_str()) && matches!(answer.kind.as_deref(), Some("answer" | "decision")))?;
        let plan = asked.question.as_ref().is_some_and(|question| question.plan.is_some());
        let (by, source, said) = match answer.from.as_str() {
            crate::messages::USER_LABEL => (By::You, if plan { Source::Plan } else { Source::Question }, answer.text.as_str()),
            crate::status::LEAD_LABEL => (By::Lead, Source::Method, answer.text.strip_prefix(LEADS).unwrap_or(&answer.text)),
            _ => return None,
        };
        Some(Record { text: clip(&format!("{} → {}", one_line(&asked.text), one_line(said)), MAX_DECISION), by, source, at: answer.at, key: None })
    }).collect()
}

// The card's decisions: each line the task wrote, with who took it from kebbie's records, then each recorded decision the task left out; a line no record backs is the task's own, whatever it says.
pub fn decisions(claimed: &[Claimed], records: &[Record], red: Option<&str>) -> (Vec<Decision>, Vec<String>) {
    let mut used = vec![false; records.len()];
    let mut taken: Vec<Decision> = Vec::new();
    let mut unbacked = Vec::new();
    let matched = |text: &str, used: &mut Vec<bool>| -> Option<usize> {
        let found = records.iter().enumerate().find(|(index, record)| !used[*index] && same_decision(text, record)).map(|(index, _)| index)?;
        used[found] = true;
        Some(found)
    };
    let mut replacements: Vec<(usize, String)> = Vec::new();
    for line in claimed {
        let decision = match matched(&line.text, &mut used) {
            Some(index) => Decision { text: line.text.clone(), by: records[index].by, source: Some(records[index].source), at: Some(records[index].at), replaced_by: None },
            None => {
                if line.by.is_some_and(|by| by != By::Task) {
                    unbacked.push(line.text.clone());
                }
                Decision { text: line.text.clone(), by: By::Task, source: None, at: None, replaced_by: None }
            }
        };
        if let Some(replacement) = &line.replaced_by {
            replacements.push((taken.len(), replacement.clone()));
        }
        taken.push(decision);
    }
    for (_, record) in records.iter().enumerate().filter(|(index, _)| !used[*index]) {
        taken.push(Decision { text: record.text.clone(), by: record.by, source: Some(record.source), at: Some(record.at), replaced_by: None });
    }
    if let Some(cause) = red {
        taken.push(Decision { text: clip(&format!("The latest test run stays red: {cause}"), MAX_DECISION), by: By::Task, source: None, at: None, replaced_by: None });
    }
    for (index, replacement) in replacements {
        let probe = Record { text: replacement.clone(), by: By::Task, source: Source::Method, at: 0, key: None };
        let later = taken.iter().enumerate().skip(index + 1).find(|(_, decision)| decision.text == replacement || same_decision(&decision.text, &probe)).map(|(at, _)| at);
        let at = later.unwrap_or_else(|| {
            taken.push(Decision { text: clip(&replacement, MAX_DECISION), by: By::Task, source: None, at: None, replaced_by: None });
            taken.len() - 1
        });
        taken[index].replaced_by = Some(at);
    }
    taken.truncate(MAX_DECISIONS);
    let len = taken.len();
    for decision in &mut taken {
        decision.replaced_by = decision.replaced_by.filter(|at| *at < len);
    }
    (taken, unbacked)
}

fn found_mark(line: &str) -> (String, Option<String>) {
    match line.rfind("(found:") {
        Some(at) => {
            let id: String = line[at + "(found:".len()..].chars().take_while(char::is_ascii_alphanumeric).collect();
            (line[..at].trim().to_string(), (!id.is_empty()).then_some(id))
        }
        None => (line.to_string(), None),
    }
}

// The new version of the card: the same lines keep the version they have, changed ones count up and carry the version before them.
pub fn next_version(parsed: &Parsed, decisions: Vec<Decision>, before: Option<Data>, now: u64) -> Data {
    let mut seen = BTreeSet::new();
    let noticed = parsed.noticed.iter().map(|line| {
        let (text, found) = found_mark(line);
        Noticed { text, found: found.filter(|id| seen.insert(id.clone())) }
    }).collect();
    let fresh = Data { version: 1, changed_at: None, headline: parsed.headline.clone(), built: parsed.built.clone(), decisions, noticed, previous: None };
    match before {
        Some(before) if before.same_lines(&fresh) => before,
        Some(before) => Data { version: before.version + 1, changed_at: Some(now), previous: Some(Box::new(Data { previous: None, ..before })), ..fresh },
        None => fresh,
    }
}

fn file(state: &Path, address: &str) -> PathBuf {
    state.join("cards").join(format!("{address}.json"))
}

pub fn load(state: &Path, address: &str) -> Option<Data> {
    serde_json::from_str(&fs::read_to_string(file(state, address)).ok()?).ok()
}

pub fn save(state: &Path, address: &str, data: &Data) -> Result<()> {
    crate::state::write(&file(state, address), serde_json::to_string_pretty(data).map_err(|error| error.to_string())?)
}

pub fn forget(state: &Path, address: &str) {
    let _ = fs::remove_file(file(state, address));
}

// What the user chose for a filed line, read from its item, so the choice lives once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Chosen {
    Subtask,
    Task,
    Kept,
    Dismissed,
}

pub fn chosen(id: &str, items: &[crate::found::Item]) -> Option<(Chosen, String)> {
    let item = items.iter().find(|item| item.id == id)?;
    let outcome = item.outcome.clone().unwrap_or_default();
    match item.state {
        crate::found::State::Open => None,
        crate::found::State::Kept => Some((Chosen::Kept, "kept in the backlog".to_string())),
        crate::found::State::Added => Some((Chosen::Subtask, outcome)),
        crate::found::State::Started => Some((Chosen::Task, outcome)),
        crate::found::State::Dismissed => Some((Chosen::Dismissed, outcome)),
    }
}

fn decision_line(decision: &Decision, data: &Data) -> String {
    let mut who = vec![decision.by.word().to_string()];
    who.extend(decision.source.map(|source| source.word().to_string()));
    let replaced = decision.replaced_by.and_then(|at| data.decisions.get(at)).map_or_else(String::new, |by| format!(" · replaced by: {}", by.text));
    format!("- {} ({}){replaced}", decision.text, who.join(" · "))
}

fn changes(data: &Data) -> Vec<String> {
    let Some(before) = &data.previous else { return Vec::new() };
    let parts: [(&str, Vec<&str>, Vec<&str>); 3] = [
        (BUILT, before.built.iter().map(String::as_str).collect(), data.built.iter().map(String::as_str).collect()),
        (DECISIONS, before.decisions.iter().map(|one| one.text.as_str()).collect(), data.decisions.iter().map(|one| one.text.as_str()).collect()),
        (NOTICED, before.noticed.iter().map(|one| one.text.as_str()).collect(), data.noticed.iter().map(|one| one.text.as_str()).collect()),
    ];
    parts.iter().flat_map(|(part, old, new)| {
        let gone = old.iter().filter(|line| !new.contains(line)).map(move |line| format!("- − {part}: {line}"));
        let came = new.iter().filter(|line| !old.contains(line)).map(move |line| format!("- + {part}: {line}"));
        gone.chain(came).collect::<Vec<_>>()
    }).collect()
}

// The card as the board draws it: the headline, its three parts with who decided and what became of each noticed line, and what changed since the version before.
pub fn render(data: &Data, items: &[crate::found::Item]) -> String {
    let mut text = vec![format!("**{}**", data.headline)];
    let changed = changes(data);
    if let Some(before) = &data.previous {
        text.push(String::new());
        text.push(format!("Changed: version {} → {}", before.version, data.version));
        text.extend(if changed.is_empty() { vec!["- its lines read the same; a decision's order or decider moved".to_string()] } else { changed });
    }
    text.push(String::new());
    text.push(format!("{BUILT}:"));
    text.extend(data.built.iter().map(|line| if line.starts_with("![") { line.clone() } else { format!("- {line}") }));
    if !data.decisions.is_empty() {
        text.push(String::new());
        text.push(format!("{DECISIONS}:"));
        text.extend(data.decisions.iter().map(|decision| decision_line(decision, data)));
    }
    if !data.noticed.is_empty() {
        text.push(String::new());
        text.push(format!("{NOTICED}:"));
        text.extend(data.noticed.iter().map(|noticed| match &noticed.found {
            Some(id) => {
                let state = chosen(id, items).map_or_else(|| format!("open under {}: start it, add it to a task or keep it from the backlog column", crate::found::SECTION), |(_, said)| said);
                format!("- {} (found:{id} · {state})", noticed.text)
            }
            None => format!("- {}", noticed.text),
        }));
    }
    text.join("\n")
}

fn wire_by(by: By) -> kebbie_hub_protocol::DecidedBy {
    use kebbie_hub_protocol::DecidedBy;
    match by {
        By::You => DecidedBy::You,
        By::Lead => DecidedBy::Lead,
        By::Investigator => DecidedBy::Investigator,
        By::Task => DecidedBy::Task,
    }
}

fn wire_source(source: Source) -> kebbie_hub_protocol::DecisionSource {
    use kebbie_hub_protocol::DecisionSource;
    match source {
        Source::Question => DecisionSource::Question,
        Source::Method => DecisionSource::Method,
        Source::Obstacle => DecisionSource::Obstacle,
        Source::Plan => DecisionSource::Plan,
    }
}

fn wire_line(text: &str, max: usize) -> String {
    let text: String = text.chars().filter(|character| !character.is_control()).collect();
    clip(&text, max)
}

fn wire_version(data: &Data, items: &[crate::found::Item]) -> kebbie_hub_protocol::Card {
    use kebbie_hub_protocol::{Card, CardDecision, ItemKey, NoticedChoice};
    let decisions: Vec<CardDecision> = data.decisions.iter().take(MAX_DECISIONS).map(|decision| CardDecision {
        text: wire_line(&decision.text, MAX_DECISION),
        by: wire_by(decision.by),
        source: decision.source.map(wire_source),
        at: decision.at,
        subtask: None,
        replaced_by: decision.replaced_by.and_then(|at| u32::try_from(at).ok()),
    }).collect();
    let noticed = data.noticed.iter().take(MAX_NOTICED).map(|noticed| {
        let found = noticed.found.as_ref().and_then(|id| ItemKey::parse(&format!("found:{id}")).ok());
        let chosen = noticed.found.as_ref().and_then(|id| chosen(id, items)).and_then(|(chosen, _)| match chosen {
            Chosen::Subtask => Some(NoticedChoice::Subtask),
            Chosen::Task => Some(NoticedChoice::Task),
            Chosen::Kept => Some(NoticedChoice::Kept),
            Chosen::Dismissed => None,
        });
        kebbie_hub_protocol::Noticed { text: wire_line(&noticed.text, MAX_LINE), chosen: chosen.filter(|_| found.is_some()), found }
    }).collect();
    Card { version: data.version.max(1), changed_at: data.changed_at, built: data.built.iter().take(MAX_BUILT).map(|line| wire_line(line, MAX_LINE)).collect(), decisions, noticed, previous: None }
}

// The card as the phone reads it, with the version before it when that one still reads; None when even the card alone breaks the protocol's rules.
pub fn wire(data: &Data, items: &[crate::found::Item]) -> Option<kebbie_hub_protocol::Card> {
    let mut card = wire_version(data, items);
    card.previous = data.previous.as_deref().map(|before| Box::new(wire_version(before, items))).filter(|before| before.version < card.version && card.changed_at.is_some());
    if card.check().is_ok() {
        return Some(card);
    }
    card.previous = None;
    card.check().is_ok().then_some(card)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "**A long link on the board opens whole, also where it wraps or is cut short.**\n\nBuilt:\n- Cmd-click any part of a wrapped link opens the full address: in a task's CI lines, its messages and the push window.\n\nDecisions:\n- A link wider than its row breaks across rows, because clipping hid the end of the address (task).\n- The real-terminal check went to you before the push, because this task cannot reach a running terminal (investigator).\n\nAlso noticed:\n- The note line under a task could link to the note too, in the board's detail lines.\n";

    #[test]
    fn the_playbooks_example_reads_as_a_headline_and_its_three_parts() {
        let parsed = parse(EXAMPLE).unwrap();
        assert_eq!(parsed.headline, "A long link on the board opens whole, also where it wraps or is cut short.");
        assert_eq!(parsed.built, ["Cmd-click any part of a wrapped link opens the full address: in a task's CI lines, its messages and the push window."]);
        assert_eq!(parsed.decisions[0], Claimed { text: "A link wider than its row breaks across rows, because clipping hid the end of the address".to_string(), by: Some(By::Task), replaced_by: None });
        assert_eq!(parsed.decisions[1].by, Some(By::Investigator));
        assert_eq!(parsed.noticed, ["The note line under a task could link to the note too, in the board's detail lines."]);
    }

    #[test]
    fn a_card_with_a_part_to_confirm_or_a_label_of_its_own_is_refused_with_what_to_change() {
        let cases = [
            ("**Tasks ask you only what you want**\n\n- A small task builds straight away.\n\nConfirm: the example uses your question.\n", "between the headline and Built:"),
            ("**Clippy checks with CI's Rust.**\n\nBuilt:\n- The pin.\n\nConfirm:\n- Upgrading Rust is manual.\n", "\"Confirm:\" asks the user to confirm"),
            ("**Ett vägrat namnserverbyte syns nu.**\n\nBuilt:\n- Banderollen.\n\n**Att bekräfta:** knappen finns kvar.\n", "asks the user to confirm"),
            ("**The app starts without the blink.**\n\nBuilt:\n- **Cause, measured:** the worker reloaded.\n- **Please confirm:** reloading a stale app when hidden.\n", "\"Please confirm\" asks the user to confirm"),
            ("**Ett stoppat byte syns.**\n\nBuilt:\n- Banderollen.\n\n**Orsak (produktion, bara läst):** dina tryck kl. 19:05.\n", "\"Orsak (produktion, bara läst):\" is not one of its parts"),
            ("**Clippy checks with CI's Rust.**\n\nBuilt:\n- The pin.\n\nSide effect: rustup updated itself.\n", "\"Side effect:\" is not one of its parts"),
            ("**Ett vägrat namnserverbyte syns nu på zonsidan.** Vägrar Loopia visar sidan en banderoll.\n\nBuilt:\n- x\n", "the headline is the bold line alone"),
            ("**Refunds show.**\n\nDecisions:\n- Per invoice (you)\n", "Built: is missing"),
            ("**Refunds show.**\n\nBuilt:\n- 1\n- 2\n- 3\n- 4\n- 5\n- 6\n- 7\n", "Built: has 7 lines"),
        ];
        for (card, said) in cases {
            let error = parse(card).unwrap_err();
            assert!(error.contains(said) && error.contains("Built:, Decisions: and Also noticed:"), "{said}: {error}");
        }
        assert_eq!(parse("**Fix: refunds round per invoice**\n\nBuilt:\n- x\n").unwrap().headline, "Fix: refunds round per invoice", "a headline may hold a colon");
        assert!(parse("**Refunds.**\n\n**Built**\n- Each invoice lists its refunds.\n\n**Also noticed**\n- The README names an old flag.\n").is_ok(), "a bold label without its colon is the part");
        assert!(parse("**Refunds.**\n\nBuilt: Each invoice lists its refunds, see it at /billing.\n![the row](/tmp/row.png)\n").unwrap().built.len() == 2, "a label's own line is its first line, and an image a line of its own");
    }

    fn record(text: &str, by: By, source: Source, at: u64, key: Option<&str>) -> Record {
        Record { text: text.to_string(), by, source, at, key: key.map(str::to_string) }
    }

    #[test]
    fn who_decided_comes_from_kebbies_records_and_a_line_without_one_is_the_tasks_whatever_it_claims() {
        let claimed = parse("**Refunds.**\n\nBuilt:\n- x\n\nDecisions:\n- Retries use Solid Queue, because one less service (you)\n- Keep the old endpoint a week (you)\n- Store frontmatter in the index first, obstacle o1 (investigator)\n- A narrower except around the probe (task)\n").unwrap().decisions;
        let records = [
            record("Which queue should retries use Solid Queue: one less service", By::You, Source::Question, 50, None),
            record("the lint needs frontmatter: subtask first: Store frontmatter, because the index stores only titles", By::Investigator, Source::Obstacle, 40, Some("o1")),
            record("Which file holds the flags: config.toml", By::Lead, Source::Method, 60, None),
        ];
        let (decisions, unbacked) = decisions(&claimed, &records, None);
        let who: Vec<(By, Option<Source>)> = decisions.iter().map(|decision| (decision.by, decision.source)).collect();
        assert_eq!(who, [(By::You, Some(Source::Question)), (By::Task, None), (By::Investigator, Some(Source::Obstacle)), (By::Task, None), (By::Lead, Some(Source::Method))]);
        assert_eq!(decisions[1].text, "Keep the old endpoint a week", "the claim of (you) is dropped, its words kept");
        assert_eq!(unbacked, ["Keep the old endpoint a week"], "the task is told which of its lines no record backs");
        assert_eq!(decisions[4].text, "Which file holds the flags: config.toml", "a recorded decision the card left out is added");
    }

    #[test]
    fn a_reversed_decision_keeps_its_place_and_points_at_the_one_that_replaced_it() {
        let claimed = parse("**VAT.**\n\nBuilt:\n- x\n\nDecisions:\n- Round VAT per line (task) replaced by: Round VAT per invoice, as the accountant books it\n- Round VAT per invoice, as the accountant books it (you)\n- Keep the cache (task); replaced by: drop the cache\n").unwrap().decisions;
        let (decisions, _) = decisions(&claimed, &[], Some("the CI-only test needs a device"));
        assert_eq!(decisions[0].replaced_by, Some(1));
        assert_eq!(decisions[2].text, "Keep the cache");
        let last = decisions.last().unwrap();
        assert_eq!((decisions[2].replaced_by, last.text.as_str()), (Some(4), "drop the cache"), "a replacement not on the card is added after it");
        assert_eq!(decisions[3].text, "The latest test run stays red: the CI-only test needs a device");
    }

    fn data(built: &str, noticed: &[&str]) -> Data {
        let parsed = parse(&format!("**Refunds.**\n\nBuilt:\n- {built}\n\nAlso noticed:\n{}\n", noticed.iter().map(|line| format!("- {line}")).collect::<Vec<_>>().join("\n"))).unwrap();
        next_version(&parsed, vec![Decision { text: "Per invoice".to_string(), by: By::You, source: Some(Source::Question), at: Some(5), replaced_by: None }], None, 10)
    }

    #[test]
    fn a_rewritten_card_counts_up_and_keeps_the_version_before_it_and_the_same_card_stays_as_it_was() {
        let first = data("Refunds list under each invoice.", &["The README names an old flag (found:f2)"]);
        assert_eq!((first.version, first.changed_at, first.noticed[0].found.as_deref(), first.noticed[0].text.as_str()), (1, None, Some("f2"), "The README names an old flag"));
        let parsed = parse("**Refunds.**\n\nBuilt:\n- Refunds list under each invoice.\n\nAlso noticed:\n- The README names an old flag (found:f2)\n").unwrap();
        let again = next_version(&parsed, first.decisions.clone(), Some(first.clone()), 20);
        assert_eq!(again, first, "the same lines keep their version");
        let rewritten = next_version(&parse("**Refunds.**\n\nBuilt:\n- Refunds and credits list under each invoice.\n").unwrap(), Vec::new(), Some(first.clone()), 30);
        assert_eq!((rewritten.version, rewritten.changed_at), (2, Some(30)));
        assert_eq!(rewritten.previous.as_deref().map(|before| (before.version, before.built.clone())), Some((1, vec!["Refunds list under each invoice.".to_string()])));
        let third = next_version(&parse("**Refunds.**\n\nBuilt:\n- Only credits.\n").unwrap(), Vec::new(), Some(rewritten.clone()), 40);
        assert!(third.previous.as_ref().is_some_and(|before| before.version == 2 && before.previous.is_none()), "one version back, not the whole history");
        let shown = render(&rewritten, &[]);
        assert!(shown.contains("Changed: version 1 → 2") && shown.contains("- − Built: Refunds list under each invoice.") && shown.contains("- + Built: Refunds and credits list under each invoice.") && shown.contains("- − Decisions: Per invoice"), "{shown}");
        let wired = wire(&rewritten, &[]).unwrap();
        assert_eq!((wired.version, wired.previous.as_ref().map(|before| before.version)), (2, Some(1)));
    }

    #[test]
    fn the_phone_and_the_board_show_who_decided_and_what_became_of_each_noticed_line() {
        let card = data("Refunds list under each invoice.", &["The README names an old flag (found:f2)", "The VAT test is flaky (found:f3)", "A line not filed"]);
        let item = |id: &str, state: crate::found::State, outcome: &str| crate::found::Item { id: id.to_string(), line: String::new(), repo: "app".to_string(), sources: vec!["billing@app".to_string()], count: 1, first_at: 0, last_at: 0, state, outcome: Some(outcome.to_string()).filter(|said| !said.is_empty()), raised: false };
        let items = [item("f2", crate::found::State::Kept, ""), item("f3", crate::found::State::Started, "started as vat-flaky")];
        let wired = wire(&card, &items).unwrap();
        let value = serde_json::to_value(&wired).unwrap();
        assert_eq!(value["decisions"][0], serde_json::json!({ "text": "Per invoice", "by": "you", "source": "question", "at": 5 }));
        assert_eq!(value["noticed"], serde_json::json!([{ "text": "The README names an old flag", "found": "found:f2", "chosen": "kept" }, { "text": "The VAT test is flaky", "found": "found:f3", "chosen": "task" }, { "text": "A line not filed" }]));
        assert!(kebbie_hub_protocol::Card::read(&serde_json::to_vec(&wired).unwrap()).is_ok(), "the phone takes it");
        let shown = render(&card, &items);
        assert!(shown.contains("- Per invoice (you · question)") && shown.contains("(found:f2 · kept in the backlog)") && shown.contains("(found:f3 · started as vat-flaky)") && shown.contains("- A line not filed"), "{shown}");
        let open = render(&card, &[]);
        assert!(open.contains("(found:f2 · open under Found by tasks: start it, add it to a task or keep it"), "{open}");
    }

    #[test]
    fn a_question_the_user_answered_and_one_the_lead_settled_are_records_of_who_decided() {
        let log = [
            r#"{"id":"q1","at":10,"from":"billing@app","to":"you","text":"Which queue should retries use?","state":"asked","question":{"options":[["Sidekiq","reuse"],["Solid Queue","one less service"]],"recommend":2}}"#,
            r#"{"id":"a1","at":20,"from":"you","to":"billing@app","text":"Solid Queue: one less service","state":"delivered","about":"q1","kind":"answer"}"#,
            r#"{"id":"q2","at":30,"from":"billing@app","to":"you","text":"Split the module?","state":"asked","question":{"options":[["Yes","y"],["No","n"]],"recommend":1}}"#,
            r#"{"id":"a2","at":40,"from":"lead","to":"billing@app","text":"Your question is about how to build it, so this is the lead's call, not the user's: keep it in one module","state":"delivered","about":"q2","kind":"decision"}"#,
            r#"{"id":"q3","at":50,"from":"search@app","to":"you","text":"Other task?","state":"asked","question":{"options":[["Yes","y"],["No","n"]],"recommend":1}}"#,
        ].join("\n");
        let records = answered(&crate::messages::fold(&log), "billing@app");
        assert_eq!(records.iter().map(|record| (record.text.as_str(), record.by, record.source, record.at)).collect::<Vec<_>>(), [
            ("Which queue should retries use? → Solid Queue: one less service", By::You, Source::Question, 20),
            ("Split the module? → keep it in one module", By::Lead, Source::Method, 40),
        ]);
    }
}
