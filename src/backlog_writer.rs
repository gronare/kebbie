use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::permit::Caller;
use crate::sh::Result;
use crate::trusted_spawn::{self, Queue};

pub const WRITER: &str = "writer";
pub const EVERY: u64 = crate::rules_check::EVERY;
const TIMEOUT: Duration = Duration::from_secs(15 * 60);
const QUEUE: Queue = Queue { dir, args: &["backlog", "--write-run"], is_ours: is_writer };

fn dir(state: &Path) -> PathBuf {
    state.join("write-out")
}

pub fn is_writer(args: &str) -> bool {
    args.contains(" backlog --write-run") && args.split_whitespace().any(|word| Path::new(word).file_name().is_some_and(|name| name.to_string_lossy().starts_with("kebbie")))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Ask {
    request: String,
    cwd: PathBuf,
}

// One request is written once at a time, whoever asks for it again meanwhile.
fn key_of(request: &str) -> String {
    trusted_spawn::digest(request)[..16].to_string()
}

// writer.profile names any profile, whose agent, model and effort the writer takes; the built-in writer is Claude at medium effort.
pub fn profile(dir: &Path, name: &str) -> Result<crate::profiles::Profile> {
    if name == WRITER && !dir.join(format!("{WRITER}.toml")).is_file() {
        let base = crate::profiles::built_in(crate::profiles::TASK).ok_or("the built-in task profile is missing")?;
        return Ok(crate::profiles::Profile { name: WRITER.to_string(), about: "kebbie's writer".to_string(), fits: "writing out backlog items for the lead".to_string(), effort: Some("medium".to_string()), ..base });
    }
    crate::profiles::load_in(dir, name).map_err(|error| format!("writer.profile: {error}"))
}

pub fn review_text(request: &str, draft: &str, profile: &str) -> String {
    format!("kebbie's writer ({profile}) wrote out the backlog item of this request for you to review, below. Do not research it again: read the draft, add what you know from your conversation with the user, take out an open question the code or the notes settle and keep each one only the user can answer, then save it as the request says and show the user the written item.\n\nThe request:\n\n{request}\n\nThe draft:\n\n{draft}")
}

pub fn fallback_text(request: &str, why: &str) -> String {
    format!("kebbie's writer could not write out this backlog item ({why}), so write it out yourself.\n\n{request}")
}

pub fn brief(request: &str, kebbie: &str) -> String {
    format!(
        "The lead was asked to write out a backlog item, and you write it in its place by the instructions in your system prompt, as kebbie's writer. The request the lead got:\n\n{request}\n\n\
         kebbie is {kebbie}: `{kebbie} backlog template` prints the style, `{kebbie} backlog` lists the backlog and `{kebbie} areas` the repos with their paths. \
         Read the item where it is filed, the code, the vault and the web as it needs. Save nothing and mark nothing: answer with the written item alone, `# <title>` on its first line and its body after it."
    )
}

// The written item from the writer's answer: from its title line on, without a code fence around it.
pub fn draft_of(answer: &str) -> Option<String> {
    let lines: Vec<&str> = answer.lines().collect();
    let start = lines.iter().position(|line| line.trim_start().starts_with("# "))?;
    let mut kept: Vec<&str> = lines[start..].to_vec();
    while kept.last().is_some_and(|line| line.trim().is_empty() || line.trim().starts_with("```")) {
        kept.pop();
    }
    Some(kept.join("\n").trim().to_string())
}

fn cwd() -> PathBuf {
    crate::lead::lead_dir().unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".to_string())))
}

// A request to write out an item goes to the writer, or to the lead at once when writer.profile is none or the writer cannot start; your own or the lead's request starts it now, anyone else's waits for the keeper.
pub fn hand(state: &Path, request: &str, caller: &Caller, tell: &dyn Fn(&str) -> Result<()>) -> Result<()> {
    let now = matches!(caller, Caller::User | Caller::Lead);
    let start = |key: &str, ask: &Ask| -> Result<()> {
        let payload = serde_json::to_string(ask).map_err(|error| error.to_string())?;
        QUEUE.start_now(state, key, &ask.cwd, &payload, &crate::peers::own_exe()?, &trusted_spawn::reviewer_env(std::env::vars()), &log(state, key))
    };
    hand_with(state, request, &crate::config::writer_profile(), now, &cwd(), &start, tell)
}

fn hand_with(state: &Path, request: &str, profile: &str, now: bool, cwd: &Path, start: &dyn Fn(&str, &Ask) -> Result<()>, tell: &dyn Fn(&str) -> Result<()>) -> Result<()> {
    if profile == crate::config::NONE {
        return tell(request);
    }
    let key = key_of(request);
    if QUEUE.asked(state, &key).is_some() {
        return Ok(());
    }
    let ask = Ask { request: request.to_string(), cwd: cwd.to_path_buf() };
    let started = if now { start(&key, &ask) } else { serde_json::to_string(&ask).map_err(|error| error.to_string()).and_then(|payload| QUEUE.want(state, &key, cwd, &payload)) };
    started.or_else(|error| tell(&fallback_text(request, &format!("it could not start: {error}"))))
}

fn log(state: &Path, key: &str) -> PathBuf {
    QUEUE.file(state, key, "log")
}

fn keys(state: &Path, kind: &str) -> Vec<String> {
    let suffix = format!(".{kind}");
    fs::read_dir(dir(state)).into_iter().flatten().flatten().filter_map(|entry| entry.file_name().to_string_lossy().strip_suffix(&suffix).map(str::to_string)).collect()
}

fn tell_lead(text: &str) -> Result<()> {
    crate::messages::to_lead(crate::messages::ToLead::BacklogItem, text)
}

// What the lead could not take yet waits beside the queue, and the keeper hands it over again each round.
fn hand_over(state: &Path, key: &str, text: &str, tell: &dyn Fn(&str) -> Result<()>) -> Result<()> {
    let waiting = QUEUE.file(state, key, "tell");
    match tell(text) {
        Ok(()) => {
            let _ = fs::remove_file(&waiting);
            Ok(())
        }
        Err(error) => crate::state::write(&waiting, text).map_err(|kept| format!("{error}, and it could not be kept to try again ({kept})")).and(Err(error)),
    }
}

fn hand_over_waiting(state: &Path, tell: &dyn Fn(&str) -> Result<()>) -> Vec<String> {
    keys(state, "tell").into_iter().filter_map(|key| {
        let text = fs::read_to_string(QUEUE.file(state, &key, "tell")).ok()?;
        tell(&text).ok()?;
        let _ = fs::remove_file(QUEUE.file(state, &key, "tell"));
        Some(format!("writer: {key} handed to the lead"))
    }).collect()
}

fn request_of(payload: &str) -> String {
    serde_json::from_str::<Ask>(payload).map(|ask| ask.request).unwrap_or_default()
}

pub fn tick(state: &Path) -> Vec<String> {
    let mut notes = hand_over_waiting(state, &tell_lead);
    notes.extend(QUEUE.dead(state).into_iter().map(|(key, ask)| {
        let _ = hand_over(state, &key, &fallback_text(&request_of(&ask.payload), "it stopped without a draft"), &tell_lead);
        QUEUE.done(state, &key);
        format!("writer: {key} stopped without a draft; the lead writes it out")
    }));
    let env = trusted_spawn::reviewer_env(std::env::vars());
    for key in keys(state, "wanted") {
        if QUEUE.pid(state, &key).is_some() {
            continue;
        }
        let Some(ask) = QUEUE.wanted(state, &key) else { continue };
        QUEUE.unwant(state, &key);
        let started = crate::peers::own_exe().and_then(|kebbie| QUEUE.start_now(state, &key, &ask.worktree, &ask.payload, &kebbie, &env, &log(state, &key)));
        notes.push(match started {
            Ok(()) => format!("writer: writing out {key}"),
            Err(error) => {
                let _ = hand_over(state, &key, &fallback_text(&request_of(&ask.payload), &format!("it could not start: {error}")), &tell_lead);
                format!("writer: {key} could not start ({error})")
            }
        });
    }
    notes
}

// The background half, kebbie backlog --write-run: only the keeper's own start with its nonce writes anything.
pub fn run() -> Result<String> {
    let presented = trusted_spawn::presented();
    let state = crate::lead::state_dir()?;
    let digest = trusted_spawn::digest(&presented);
    let key = keys(&state, "run").into_iter().find(|key| fs::read_to_string(QUEUE.file(&state, key, "run")).ok().and_then(|record| record.lines().next().map(|hash| hash.trim() == digest)).unwrap_or(false));
    let Some(payload) = key.as_deref().and_then(|key| QUEUE.take(&state, key, &presented, &crate::permit::caller())) else {
        return Err("This writer was not started by kebbie's keeper, so it writes nothing.".to_string());
    };
    let key = key.unwrap_or_default();
    let ask: Ask = serde_json::from_str(&payload).map_err(|error| format!("the writer's request does not read: {error}"))?;
    let name = crate::config::writer_profile();
    let written = write(&key, &ask, &name).and_then(|answer| draft_of(&answer).ok_or_else(|| "its answer has no `# <title>` line".to_string()));
    let settled = settle(&state, &key, &ask, &name, written, &tell_lead);
    QUEUE.done(&state, &key);
    settled
}

// The draft goes to the lead in its message, or the request goes back to the lead to write itself; either waits to be handed over again when the lead cannot take it now.
fn settle(state: &Path, key: &str, ask: &Ask, name: &str, written: Result<String>, tell: &dyn Fn(&str) -> Result<()>) -> Result<String> {
    match written {
        Ok(draft) => {
            hand_over(state, key, &review_text(&ask.request, &draft, name), tell).map_err(|error| format!("the lead gets the draft once it can ({error})"))?;
            let _ = fs::remove_file(log(state, key));
            Ok("The lead reviews the draft".to_string())
        }
        Err(why) => {
            let _ = hand_over(state, key, &fallback_text(&ask.request, &why), tell);
            Err(why)
        }
    }
}

const WRITER_ROLE: &str = "You are kebbie's writer. You write nothing: no vault note, no file, no backlog item, no status, no `kebbie backlog add`, `filed`, `write` or `save`. The skill's steps that mark an item drafting or save it are the lead's, never yours. Search, read and research as the skill says, then return the written item as your answer, `# <title>` on its first line and its body after it; the lead saves it.";

fn system_prompt(skill: &str) -> String {
    format!("{WRITER_ROLE}\n\n{}", skill.trim_end())
}

fn write(key: &str, ask: &Ask, name: &str) -> Result<String> {
    let profile = profile(&crate::profiles::dir(), name)?;
    let kebbie = crate::peers::kebbie_command()?;
    let skill = crate::lead::install_root()?.join("skills/write-backlog-item/SKILL.md");
    let roles = crate::lead::roles_dir()?;
    let prompt = roles.join(format!("backlog-{key}.write.md"));
    let text = fs::read_to_string(&skill).map_err(|error| format!("{}: {error}", skill.display()))?;
    let answer = roles.join(format!("backlog-{key}.write.answer"));
    crate::state::write(&prompt, system_prompt(&text) + &crate::profiles::role_section(&profile)?)?;
    let asked = trusted_spawn::ask_helper(crate::agent::Helper::Writer, &profile, &prompt, &answer, &ask.cwd, &brief(&ask.request, &kebbie), &kebbie, TIMEOUT);
    let _ = fs::remove_file(&prompt);
    let _ = fs::remove_file(&answer);
    asked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn a_request_goes_to_the_writer_once_and_to_the_lead_when_the_writer_is_off_or_cannot_start() {
        let state = crate::sh::test_dir("writer-hand");
        let (told, started) = (RefCell::new(Vec::new()), RefCell::new(Vec::new()));
        let tell = |text: &str| -> Result<()> { told.borrow_mut().push(text.to_string()); Ok(()) };
        let start = |key: &str, ask: &Ask| -> Result<()> { started.borrow_mut().push((key.to_string(), ask.request.clone())); Ok(()) };
        hand_with(&state, "Write out \"Retry\"", crate::config::NONE, true, &state, &start, &tell).unwrap();
        assert_eq!((told.borrow().as_slice(), started.borrow().len()), (["Write out \"Retry\"".to_string()].as_slice(), 0), "none has the lead write it");
        hand_with(&state, "Write out \"Retry\"", WRITER, false, &state, &start, &tell).unwrap();
        assert!(QUEUE.wanted(&state, &key_of("Write out \"Retry\"")).is_some(), "a phone's request waits for the keeper");
        hand_with(&state, "Write out \"Retry\"", WRITER, true, &state, &start, &tell).unwrap();
        assert_eq!((told.borrow().len(), started.borrow().len()), (1, 0), "a request already with the writer is not started twice");
        hand_with(&state, "Write out \"Faster board\"", WRITER, true, &state, &start, &tell).unwrap();
        assert_eq!(started.borrow()[0].1, "Write out \"Faster board\"");
        let refuse = |_: &str, _: &Ask| -> Result<()> { Err("no claude".to_string()) };
        hand_with(&state, "Write out \"Docs\"", WRITER, true, &state, &refuse, &tell).unwrap();
        assert_eq!(told.borrow()[1], fallback_text("Write out \"Docs\"", "it could not start: no claude"), "the lead writes it when the writer cannot start");
    }

    #[test]
    fn what_the_lead_cannot_take_now_is_handed_over_on_a_later_round_and_leaves_no_files() {
        let state = crate::sh::test_dir("writer-settle");
        let told = RefCell::new(Vec::new());
        let away = |_: &str| -> Result<()> { Err("no lead session".to_string()) };
        let here = |text: &str| -> Result<()> { told.borrow_mut().push(text.to_string()); Ok(()) };
        let ask = Ask { request: "Write out \"Retry\"".to_string(), cwd: state.clone() };
        assert!(settle(&state, "k1", &ask, WRITER, Ok("# Retry".to_string()), &away).is_err());
        assert!(settle(&state, "k2", &ask, WRITER, Err("it timed out".to_string()), &away).is_err());
        assert!(told.borrow().is_empty());
        let mut handed = hand_over_waiting(&state, &here);
        handed.sort();
        assert_eq!(handed, ["writer: k1 handed to the lead", "writer: k2 handed to the lead"]);
        let mut texts = told.borrow().clone();
        texts.sort();
        assert_eq!(texts, [review_text(&ask.request, "# Retry", WRITER), fallback_text(&ask.request, "it timed out")]);
        assert!(hand_over_waiting(&state, &here).is_empty(), "handed over once");
        assert!(fs::read_dir(dir(&state)).map_or(true, |mut files| files.next().is_none()), "no draft and nothing waiting is left behind");
    }

    #[test]
    fn the_draft_is_the_written_item_from_its_title_on() {
        assert_eq!(draft_of("I read the code.\n\n```markdown\n# Retry flaky runs\n\n## Goal\n\nRerun once.\n```\n").as_deref(), Some("# Retry flaky runs\n\n## Goal\n\nRerun once."));
        assert_eq!(draft_of("No item here."), None);
    }

    #[test]
    fn the_lead_reviews_the_draft_and_saves_it_as_the_request_says() {
        let text = review_text("The user asks you to write out \"Retry\".", "# Retry\n\nRerun once.", WRITER);
        assert!(text.starts_with("kebbie's writer (writer) wrote out the backlog item of this request for you to review, below.") && text.contains("keep each one only the user can answer") && text.ends_with("The request:\n\nThe user asks you to write out \"Retry\".\n\nThe draft:\n\n# Retry\n\nRerun once."), "{text}");
        assert!(brief("Write out \"Retry\"", "/k").contains("Save nothing and mark nothing"));
    }

    #[test]
    fn the_writer_can_only_read_and_is_told_it_writes_nothing() {
        let prompt = Path::new("/state/roles/backlog-ab.write.md");
        let reader = crate::agent::Reviewer { model: None, effort: None, settings: prompt, prompt, rules: None, docs: None, answer: Path::new("/a") };
        let (_, args) = crate::agent::Agent::Claude.investigator_command(&reader, "/bin/kebbie", crate::agent::Helper::Writer);
        let split = args.iter().position(|arg| arg == "--disallowedTools").expect("a deny list");
        let (allowed, denied) = (&args[..split], &args[split..]);
        for wanted in ["mcp__plugin_vault_vault__search", "mcp__plugin_vault_vault__read_file", "WebFetch", "WebSearch", "Bash(git log:*)", "Bash(/bin/kebbie backlog)", "Bash(/bin/kebbie backlog template)", "Bash(/bin/kebbie areas)"] {
            assert!(allowed.iter().any(|arg| arg == wanted), "{wanted}: {args:?}");
        }
        assert!(!allowed.iter().any(|arg| arg == "mcp__plugin_vault_vault" || arg.contains("write_file") || arg.contains("set_status") || arg.ends_with(":*)") && arg.contains("kebbie")), "{allowed:?}");
        for refused in ["mcp__plugin_vault_vault__write_file", "mcp__plugin_vault_vault__close", "mcp__plugin_vault_vault__set_status", "Bash(/bin/kebbie backlog add:*)", "Bash(/bin/kebbie backlog filed:*)", "Write", "Edit"] {
            assert!(denied.iter().any(|arg| arg == refused), "{refused}: {denied:?}");
        }
        let text = system_prompt("# Write a backlog item\n");
        assert!(text.starts_with("You are kebbie's writer. You write nothing") && text.contains("return the written item") && text.ends_with("# Write a backlog item"), "{text}");
    }

    #[test]
    fn the_built_in_writer_is_claude_at_medium_effort_and_another_profile_is_read_from_its_file() {
        let dir = crate::sh::test_dir("writer-profile");
        let built = profile(&dir, WRITER).unwrap();
        assert_eq!((built.agent, built.effort.as_deref()), (crate::agent::Agent::Claude, Some("medium")));
        assert!(profile(&dir, "gone").unwrap_err().starts_with("writer.profile: "));
        assert!(is_writer("/home/u/.local/bin/kebbie -C /x backlog --write-run") && !is_writer("kebbie backlog write x"));
    }
}
