use std::fs;
use std::path::{Path, PathBuf};

use crate::lead::state_dir;
use crate::messages::{self, Message};
use crate::sh::Result;

// The user's word that the lead may push: one task, or every task in a repo whose reviewer passed its current code.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Permit {
    Task,
    Repo,
}

fn dir(state: &Path) -> PathBuf {
    state.join("push-permits")
}

fn task_file(state: &Path, address: &str) -> PathBuf {
    dir(state).join(address)
}

// Addresses carry an @, so a repo's file never shares a name with a task's.
fn repo_file(state: &Path, repo: &str) -> PathBuf {
    dir(state).join("repos").join(repo)
}

fn switch(file: &Path, on: bool) -> Result<()> {
    if !on {
        return match fs::remove_file(file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        };
    }
    crate::state::write(file, messages::now().to_string())
}

pub fn in_state(state: &Path, address: &str, repo: &str) -> Option<Permit> {
    if task_file(state, address).exists() {
        Some(Permit::Task)
    } else if repo_file(state, repo).exists() {
        Some(Permit::Repo)
    } else {
        None
    }
}

pub fn of(address: &str, repo: &str) -> Option<Permit> {
    in_state(&crate::lead::state_location().ok()?, address, repo)
}

pub fn repo_permitted(repo: &str) -> bool {
    crate::lead::state_location().is_ok_and(|state| repo_file(&state, repo).exists())
}

pub fn set_task_in(state: &Path, address: &str, on: bool) -> Result<()> {
    switch(&task_file(state, address), on)
}

pub fn set_repo_in(state: &Path, repo: &str, on: bool) -> Result<()> {
    switch(&repo_file(state, repo), on)
}

pub fn forget(address: &str) {
    if let Ok(state) = state_dir() {
        let _ = set_task_in(&state, address, false);
    }
}

// The board's key: the user gives or takes back the lead's push permission for one task, and the lead hears of it.
pub fn toggle_task(address: &str) -> Result<String> {
    let state = state_dir()?;
    let on = !task_file(&state, address).exists();
    set_task_in(&state, address, on)?;
    let (told, shown) = if on {
        (format!("The user gave you push permission for {address}: you may push it with kelpie push once it is landed, rules-approved and through any review it is in."), format!("The lead may push {address} now; done, drop and reset take the permission back."))
    } else {
        (format!("The user took back push permission for {address}: route it to them again."), format!("Only you push {address} now."))
    };
    tell_lead(&told);
    Ok(shown)
}

// The Areas page's key: the lead may push any task in the repo whose reviewer passed its current code.
pub fn toggle_repo(repo: &str) -> Result<String> {
    let state = state_dir()?;
    let on = !repo_file(&state, repo).exists();
    set_repo_in(&state, repo, on)?;
    let (told, shown) = if on {
        (format!("The user gave you push permission for the repo {repo}: you may push any task in it whose reviewer passed its current code (kelpie review, verdict ok)."), format!("The lead may push reviewed tasks in {repo} now."))
    } else {
        (format!("The user took back push permission for the repo {repo}; tasks with their own permission keep it."), format!("Only you push {repo}'s tasks now, but those you gave their own permission."))
    };
    tell_lead(&told);
    Ok(shown)
}

fn tell_lead(text: &str) {
    if let Err(error) = messages::tell_lead(text, None) {
        eprintln!("Could not tell the lead: {error}");
    }
}

// A task's permission covers it outright; the repo's covers only code its reviewer passed as it is now.
pub fn lead_may_push(permit: Option<Permit>, reviewed: bool) -> std::result::Result<(), String> {
    match permit {
        Some(Permit::Task) => Ok(()),
        Some(Permit::Repo) if reviewed => Ok(()),
        Some(Permit::Repo) => Err("the repo's push permission covers a task only once its reviewer passed its current code: send it to review (kelpie review) and push after a verdict ok".to_string()),
        None => Err("the user has given no push permission for it or its repo: it waits for the user under Ready to push".to_string()),
    }
}

fn away_file(state: &Path) -> PathBuf {
    state.join("away")
}

pub fn away_since_in(state: &Path) -> Option<u64> {
    fs::read_to_string(away_file(state)).ok().and_then(|since| since.trim().parse().ok())
}

pub fn away_since() -> Option<u64> {
    away_since_in(&crate::lead::state_location().ok()?)
}

// Returns when the user went away, for the summary on the way back; None when nothing changed.
pub fn set_away_in(state: &Path, on: bool, now: u64) -> Result<Option<u64>> {
    let since = away_since_in(state);
    match (on, since) {
        (true, None) => crate::state::write(&away_file(state), now.to_string()).map(|_| Some(now)),
        (false, Some(since)) => fs::remove_file(away_file(state)).map(|_| Some(since)).map_err(|error| error.to_string()),
        (false, None) => {
            let _ = fs::remove_file(away_file(state));
            Ok(None)
        }
        _ => Ok(None),
    }
}

// `kelpie away on|off` and the board's key: the user's word only, and the lead is told either way.
pub fn away(word: Option<&str>) -> Result<String> {
    let on = match word {
        Some("on") => true,
        Some("off") => false,
        None => return Ok(if away_since().is_some() { "Away mode is on: kelpie away off when you are back.".to_string() } else { "Away mode is off.".to_string() }),
        Some(other) => return Err(format!("kelpie away takes on or off, not \"{other}\"")),
    };
    user_only("switching away mode")?;
    let Some(since) = set_away_in(&state_dir()?, on, messages::now())? else {
        return Ok(format!("Away mode is already {}.", if on { "on" } else { "off" }));
    };
    if on {
        let log = messages::load();
        let open: Vec<String> = crate::questions::open_now(&log).into_iter().filter(|question| question.question.as_ref().is_some_and(|question| !question.waits))
            .map(|question| format!("\n- {}", crate::questions::question_copy(question, true))).collect();
        let open = if open.is_empty() { String::new() } else { format!("\nQuestions waiting on the board now:{}", open.concat()) };
        tell_lead(&format!("The user is away from now on. Work by the away rules in your role: answer how-questions yourself with kelpie decide, keep what-questions for the user with kelpie defer, push only what their permissions allow, and keep what you decide for the summary they get back.{open}"));
        return Ok("Away mode is on: the lead decides how-questions itself, keeps what-questions for you, and pushes only what you permitted.".to_string());
    }
    let log = messages::load();
    let waiting = crate::questions::open_now(&log).into_iter().filter(|question| question.question.as_ref().is_some_and(|question| question.waits)).cloned().collect::<Vec<_>>();
    tell_lead(&return_brief(&log, since, &waiting));
    Ok("Away mode is off: the lead writes you what landed, what it decided and what waits for you.".to_string())
}

// What the lead writes the user's summary from: the pushes, its own calls and the questions it kept, since the user left.
pub fn return_brief(log: &[Message], since: u64, waiting: &[Message]) -> String {
    let since_left = |kind: &str| -> Vec<String> {
        log.iter().filter(|message| message.at >= since && message.kind.as_deref() == Some(kind))
            .map(|message| if kind == "decision" { format!("- to {}: {}", message.to, message.text) } else { format!("- {}", message.text) }).collect()
    };
    let section = |title: &str, lines: Vec<String>| if lines.is_empty() { format!("{title}: none.") } else { format!("{title}:\n{}", lines.join("\n")) };
    let kept: Vec<String> = waiting.iter().map(|question| format!("- {} asks: {}", question.from, question.text)).collect();
    format!(
        "The user is back. Write them the summary now, a screen at most: what landed, what you decided for them and why, and what waits for them, the kept questions first.\n{}\n{}\n{}",
        section("Pushed while they were away", since_left("push")), section("Your calls", since_left("decision")), section("Questions kept for them", kept),
    )
}

// Who runs this kelpie, read from its ancestors: a Claude that kelpie started carries its role in its --settings file.
#[derive(Clone, Debug, PartialEq)]
pub enum Caller {
    User,
    Lead,
    Task(String),
    Agent,
}

fn role_of(args: &str) -> Option<Caller> {
    let words: Vec<&str> = args.split_whitespace().collect();
    let program = words.first().map(|word| Path::new(word).file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default())?;
    if program != "claude" && !args.contains("@anthropic-ai/claude-code") {
        return None;
    }
    let at = words.iter().position(|word| *word == "--settings");
    let settings = at.and_then(|index| words.get(index + 1));
    if settings.is_some_and(|word| word.starts_with('{')) {
        return Some(inline_role(&words));
    }
    let name = settings.and_then(|file| Path::new(file).file_name()).map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    Some(match name.strip_suffix(".settings.json") {
        Some(crate::status::LEAD_LABEL) => Caller::Lead,
        Some(address) if address.contains('@') && !address.ends_with(".review") => Caller::Task(address.to_string()),
        _ => Caller::Agent,
    })
}

// A Claude kelpie started before it wrote settings files carries the JSON inline: the lead's has the messages --unread prompt hook, a task's the kelpie hook command, and a task's role file names it.
fn inline_role(words: &[&str]) -> Caller {
    let args = words.join(" ");
    if args.contains("' hook\"") {
        let role = words.iter().position(|word| *word == "--append-system-prompt-file").and_then(|index| words.get(index + 1));
        let address = role.and_then(|file| Path::new(file).file_stem()).map(|stem| stem.to_string_lossy().to_string()).filter(|stem| stem.contains('@'));
        return Caller::Task(address.unwrap_or_else(|| "started before kelpie wrote settings files".to_string()));
    }
    if args.contains("' messages --unread\"") { Caller::Lead } else { Caller::Agent }
}

// The strictest Claude in the chain decides, so a Claude a task starts with the lead's settings is still the task; a chain kelpie cannot read, or one cut loose from any terminal while a kelpie-started Claude is alive, is an agent.
pub fn caller_in(processes: &[(u32, u32, String)], pid: u32) -> Caller {
    let mut roles = Vec::new();
    let mut hosted = false;
    let mut current = pid;
    let mut rooted = false;
    for _ in 0..64 {
        let Some((_, parent, args)) = processes.iter().find(|(id, _, _)| *id == current) else { break };
        if current != pid {
            roles.extend(role_of(args));
            hosted |= is_session_host(args);
        }
        if *parent <= 1 {
            rooted = true;
            break;
        }
        if *parent == current {
            break;
        }
        current = *parent;
    }
    if let Some(task) = roles.iter().find(|role| matches!(role, Caller::Task(_))) {
        return task.clone();
    }
    if roles.contains(&Caller::Agent) || !rooted {
        return Caller::Agent;
    }
    if roles.contains(&Caller::Lead) {
        return Caller::Lead;
    }
    if !hosted && processes.iter().any(|(_, _, args)| matches!(role_of(args), Some(Caller::Lead | Caller::Task(_)))) {
        return Caller::Agent;
    }
    Caller::User
}

// A terminal session: an interactive shell (no -c), a login, or a terminal multiplexer, herdr among them since the board runs straight under it; a shell running a script line is not one.
fn is_session_host(args: &str) -> bool {
    let words: Vec<&str> = args.split_whitespace().collect();
    let Some(first) = words.first() else { return false };
    let login = first.starts_with('-');
    let program = Path::new(first.trim_start_matches('-')).file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    let shell = ["sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "csh", "nu", "elvish", "xonsh"].contains(&program.as_str());
    if shell {
        return login || !words.iter().skip(1).any(|word| *word == "-c" || (word.starts_with('-') && !word.starts_with("--") && word.contains('c')));
    }
    ["herdr", "login", "sshd", "tmux", "screen", "zellij", "mosh-server"].iter().any(|host| program == *host || program.starts_with(&format!("{host}:")))
}

pub fn processes(table: &str) -> Vec<(u32, u32, String)> {
    table.lines().filter_map(|line| {
        let mut words = line.split_whitespace();
        let pid = words.next()?.parse().ok()?;
        let parent = words.next()?.parse().ok()?;
        Some((pid, parent, words.collect::<Vec<_>>().join(" ")))
    }).collect()
}

// A ps that fails or answers nothing reads as an agent: no proof of the user is no permission.
pub fn caller() -> Caller {
    match crate::sh::run("ps", &["-A", "-o", "pid=,ppid=,args="], None) {
        Ok(table) => caller_in(&processes(&table), std::process::id()),
        Err(_) => Caller::Agent,
    }
}

pub fn user_only(what: &str) -> Result<()> {
    user_only_as(&caller(), what)
}

pub fn user_only_as(caller: &Caller, what: &str) -> Result<()> {
    match caller {
        Caller::User => Ok(()),
        other => Err(format!("{what} is the user's call, from the board or their own terminal; {} cannot", describe(other))),
    }
}

pub fn describe(caller: &Caller) -> String {
    match caller {
        Caller::User => "the user".to_string(),
        Caller::Lead => "the lead".to_string(),
        Caller::Task(address) => format!("the task {address}"),
        Caller::Agent => "a Claude session".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_task_permission_wins_over_the_repo_and_both_go_when_switched_off() {
        let state = std::env::temp_dir().join(format!("kelpie-permits-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        assert_eq!(in_state(&state, "billing@app", "app"), None);
        set_repo_in(&state, "app", true).unwrap();
        assert_eq!(in_state(&state, "billing@app", "app"), Some(Permit::Repo));
        assert_eq!(in_state(&state, "billing@web", "web"), None, "a repo's permission is that repo's");
        set_task_in(&state, "billing@app", true).unwrap();
        assert_eq!(in_state(&state, "billing@app", "app"), Some(Permit::Task));
        set_task_in(&state, "billing@app", false).unwrap();
        set_task_in(&state, "billing@app", false).unwrap();
        assert_eq!(in_state(&state, "billing@app", "app"), Some(Permit::Repo));
        set_repo_in(&state, "app", false).unwrap();
        assert_eq!(in_state(&state, "billing@app", "app"), None);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_lead_pushes_a_permitted_task_and_under_the_repos_permission_only_reviewed_code() {
        assert_eq!(lead_may_push(Some(Permit::Task), false), Ok(()));
        assert_eq!(lead_may_push(Some(Permit::Repo), true), Ok(()));
        assert!(lead_may_push(Some(Permit::Repo), false).unwrap_err().contains("reviewer passed its current code"));
        assert!(lead_may_push(None, true).unwrap_err().contains("no push permission"));
    }

    #[test]
    fn away_mode_records_when_the_user_left_and_reports_it_on_the_way_back() {
        let state = std::env::temp_dir().join(format!("kelpie-away-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        assert_eq!(away_since_in(&state), None);
        assert_eq!(set_away_in(&state, true, 100).unwrap(), Some(100));
        assert_eq!(set_away_in(&state, true, 200).unwrap(), None, "away twice keeps when the user left");
        assert_eq!(away_since_in(&state), Some(100));
        assert_eq!(set_away_in(&state, false, 300).unwrap(), Some(100));
        assert_eq!(away_since_in(&state), None);
        assert_eq!(set_away_in(&state, false, 400).unwrap(), None);
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn the_return_brief_lists_the_pushes_the_leads_calls_and_the_kept_questions_since_the_user_left() {
        let line = |at: u64, from: &str, to: &str, text: &str, kind: Option<&str>| json!({ "id": format!("{at}"), "at": at, "from": from, "to": to, "text": text, "state": "delivered", "kind": kind });
        let log = messages::fold(&[
            line(50, "kelpie", "lead", "the user pushed search@app: 1 commit", Some("push")),
            line(150, "kelpie", "lead", "the lead pushed billing@app: 2 commits to main (a1b2c3d Add refunds)", Some("push")),
            line(160, "lead", "billing@app", "Use the existing retry queue.", Some("decision")),
            line(170, "lead", "billing@app", "Carry on.", None),
        ].iter().map(|value| value.to_string()).collect::<Vec<_>>().join("\n"));
        let kept = messages::fold(&json!({ "id": "q", "at": 165, "from": "search@app", "to": "lead", "text": "Drop the old endpoint?", "state": "asked", "question": { "options": [], "recommend": 1 } }).to_string());
        let brief = return_brief(&log, 100, &kept);
        assert!(brief.contains("Pushed while they were away:\n- the lead pushed billing@app"), "{brief}");
        assert!(!brief.contains("search@app: 1 commit"), "a push before the user left is not news");
        assert!(brief.contains("Your calls:\n- to billing@app: Use the existing retry queue."), "{brief}");
        assert!(!brief.contains("Carry on"), "only the lead's recorded calls");
        assert!(brief.contains("Questions kept for them:\n- search@app asks: Drop the old endpoint?"), "{brief}");
        assert!(return_brief(&[], 0, &[]).contains("Your calls: none."));
    }

    #[test]
    fn the_caller_is_read_from_the_claude_kelpie_started_above_it() {
        let lead = "/Users/ada/.local/bin/claude --append-system-prompt-file /state/roles/lead.md --settings /state/roles/lead.settings.json";
        let task = "claude --permission-mode acceptEdits --settings /state/roles/billing@app.settings.json --append-system-prompt-file /state/roles/billing@app.md --session-id 1";
        let reviewer = "claude -p --settings /state/roles/billing@app.review.settings.json";
        let table = |claude: &str| processes(&format!("  1     0 /sbin/launchd\n 40     1 /usr/bin/herdr server\n 50    40 {claude}\n 60    50 /bin/zsh -c kelpie push billing\n 70    60 /bin/kelpie push billing\n"));
        assert_eq!(caller_in(&table(lead), 70), Caller::Lead);
        assert_eq!(caller_in(&table(task), 70), Caller::Task("billing@app".to_string()));
        assert_eq!(caller_in(&table(reviewer), 70), Caller::Agent);
        assert_eq!(caller_in(&table("node /opt/node_modules/@anthropic-ai/claude-code/cli.js"), 70), Caller::Agent, "a Claude kelpie did not start is an agent");
        assert_eq!(caller_in(&table("/bin/zsh -l"), 70), Caller::User, "the user's own shell, also when herdr was started from a Claude and its panes inherit CLAUDECODE");
        let disguised = processes(&format!("  1 0 launchd\n 50 1 {task}\n 55 50 claude -p --settings /state/roles/lead.settings.json\n 70 55 kelpie push billing\n"));
        assert_eq!(caller_in(&disguised, 70), Caller::Task("billing@app".to_string()), "a task cannot pass as the lead by starting a Claude with the lead's settings");
        assert_eq!(caller_in(&processes(" 70 70 /bin/kelpie board\n"), 70), Caller::Agent, "a loop in the table ends the walk unread");
        assert_eq!(caller_in(&processes("garbage\n"), 70), Caller::Agent, "a table without kelpie in it proves no user");
        assert_eq!(caller_in(&[], 70), Caller::Agent, "an empty ps answer proves no user");
    }

    #[test]
    fn a_claude_started_with_inline_settings_json_reads_as_the_lead_or_its_task() {
        let kelpie = "/Users/ada/.local/share/kelpie/current/kelpie";
        let lead_json = serde_json::json!({ "hooks": { "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": format!("'{kelpie}' messages --unread") }] }] }, "permissions": { "allow": [format!("Bash({kelpie} push:*)")] } });
        let hook = serde_json::json!([{ "hooks": [{ "type": "command", "command": format!("'{kelpie}' hook") }] }]);
        let task_json = serde_json::json!({ "permissions": { "deny": ["Bash(git push:*)"] }, "hooks": { "UserPromptSubmit": hook, "Stop": hook } });
        let table = |claude: String| processes(&format!("  1     0 /sbin/launchd\n 50     1 {claude}\n 60    50 /bin/zsh -c kelpie push billing\n 70    60 /bin/kelpie push billing\n"));
        let lead = format!("claude --append-system-prompt-file /state/roles/lead.md --settings {lead_json}");
        let task = format!("claude --permission-mode acceptEdits --settings {task_json} --append-system-prompt-file /state/roles/billing@app.md");
        assert_eq!(caller_in(&table(lead), 70), Caller::Lead, "the user's lead from before settings files still pushes, decides and defers");
        assert_eq!(caller_in(&table(task), 70), Caller::Task("billing@app".to_string()));
        assert_eq!(caller_in(&table(format!("claude --settings {}", serde_json::json!({ "model": "opus" }))), 70), Caller::Agent, "other inline settings are no role");
    }

    #[test]
    fn a_claude_run_through_node_or_with_half_an_argument_list_still_reads_as_its_role() {
        let task = "/state/roles/billing@app.settings.json";
        assert_eq!(role_of(&format!("node /opt/node_modules/@anthropic-ai/claude-code/cli.js --settings {task}")), Some(Caller::Task("billing@app".to_string())), "the npm install runs as node");
        assert_eq!(role_of("claude --settings"), Some(Caller::Agent), "a dangling --settings names no file");
        assert_eq!(role_of("claude --settings /state/roles/other.settings.json"), Some(Caller::Agent), "a settings file without @ is no task");
        assert_eq!(role_of("claude --settings /state/roles/billing@app.review.settings.json"), Some(Caller::Agent), "the reviewer is an agent, never the task it reads");
        assert_eq!(role_of("vim --settings /state/roles/lead.settings.json"), None, "only a Claude carries a role");
        assert_eq!(role_of(""), None);
        assert_eq!(role_of("   "), None);
    }

    #[test]
    fn the_process_table_keeps_arguments_with_spaces_and_skips_lines_it_cannot_read() {
        let table = processes("  1     0 /sbin/launchd\n 50     1 claude --settings /state/roles/a@b.settings.json --append-system-prompt-file x\nbad line here\n 60 50\n");
        assert_eq!(table.len(), 3, "{table:?}");
        assert_eq!(table[1], (50, 1, "claude --settings /state/roles/a@b.settings.json --append-system-prompt-file x".to_string()));
        assert_eq!(table[2], (60, 50, String::new()), "a process whose command is gone still links the chain");
    }

    #[test]
    fn the_strictest_role_in_the_chain_wins_however_the_claudes_nest() {
        let lead = "claude --settings /state/roles/lead.settings.json";
        let agent = "claude --settings /state/roles/x.settings.json";
        let task = "claude --settings /state/roles/billing@app.settings.json";
        let chain = |middle: &str, top: &str| processes(&format!("  1 0 launchd\n 10 1 {top}\n 20 10 {middle}\n 30 20 kelpie push billing\n"));
        assert_eq!(caller_in(&chain(lead, agent), 30), Caller::Agent, "a lead started by an agent is an agent");
        assert_eq!(caller_in(&chain(agent, lead), 30), Caller::Agent, "an agent started by the lead is an agent");
        assert_eq!(caller_in(&chain(lead, task), 30), Caller::Task("billing@app".to_string()));
        assert_eq!(caller_in(&chain(agent, task), 30), Caller::Task("billing@app".to_string()));
        assert_eq!(caller_in(&chain("/bin/zsh", lead), 30), Caller::Lead);
        assert_eq!(caller_in(&chain(task, lead), 30), Caller::Task("billing@app".to_string()), "a task under the lead's shell is still the task");
        assert_eq!(caller_in(&processes(" 30 29 kelpie push\n"), 30), Caller::Agent, "a parent missing from the table is ancestry kelpie cannot read");
        assert_eq!(caller_in(&processes(" 30 30 kelpie push\n"), 30), Caller::Agent);
    }

    #[test]
    fn a_kelpie_detached_from_the_claude_that_started_it_is_not_the_user() {
        let task = "claude --settings /state/roles/billing@app.settings.json";
        let orphaned = processes(&format!("  1 0 /sbin/launchd\n 50 1 {task}\n 70 1 /bin/kelpie approve billing\n"));
        assert_ne!(caller_in(&orphaned, 70), Caller::User, "with a task's Claude alive on the machine, a parentless kelpie must not pass as the user");
        let through_sh = processes(&format!("  1 0 /sbin/launchd\n 50 1 {task}\n 60 1 /bin/zsh -c kelpie approve billing\n 70 60 /bin/kelpie approve billing\n"));
        assert_eq!(caller_in(&through_sh, 70), Caller::Agent, "a detached shell running a command line is no terminal");
        let bundled = processes(&format!("  1 0 /sbin/launchd\n 50 1 {task}\n 60 1 /bin/zsh -lc kelpie\n 70 60 /bin/kelpie approve billing\n"));
        assert_eq!(caller_in(&bundled, 70), Caller::Agent, "-lc is still a command line");
        let lead = "claude --settings /state/roles/lead.settings.json";
        let alone = processes(&format!("  1 0 /sbin/launchd\n 50 1 {lead}\n 70 1 /bin/kelpie away on\n"));
        assert_eq!(caller_in(&alone, 70), Caller::Agent, "the lead alive is enough");
        assert_eq!(caller_in(&processes("  1 0 /sbin/launchd\n 70 1 /bin/kelpie away on\n"), 70), Caller::User, "with no kelpie Claude alive there is no agent to suspect");
        for host in ["-zsh", "/bin/zsh -l", "/bin/bash", "login -pf ada", "tmux: server", "/usr/sbin/sshd: ada@ttys001"] {
            let terminal = processes(&format!("  1 0 /sbin/launchd\n 50 1 {task}\n 40 1 /Applications/Ghostty.app/Contents/MacOS/ghostty\n 60 40 {host}\n 70 60 /bin/kelpie approve billing\n"));
            assert_eq!(caller_in(&terminal, 70), Caller::User, "the user's terminal under {host} stays the user while tasks run");
        }
        let board = processes(&format!("  1 0 /sbin/launchd\n 40 1 /usr/local/bin/herdr server\n 50 40 {task}\n 70 40 /plugin/target/release/kelpie board --overlay\n"));
        assert_eq!(caller_in(&board, 70), Caller::User, "the board herdr opens as a plugin pane is the user's");
    }

    #[test]
    fn the_user_alone_passes_a_user_only_gate_and_the_refusal_names_who_tried() {
        assert_eq!(user_only_as(&Caller::User, "switching away mode"), Ok(()));
        let lead = user_only_as(&Caller::Lead, "switching away mode").unwrap_err();
        assert_eq!(lead, "switching away mode is the user's call, from the board or their own terminal; the lead cannot");
        assert!(user_only_as(&Caller::Task("billing@app".to_string()), "x").unwrap_err().contains("the task billing@app cannot"));
        assert!(user_only_as(&Caller::Agent, "x").unwrap_err().contains("a Claude session cannot"));
        assert_eq!(describe(&Caller::User), "the user");
    }

    #[test]
    fn the_permit_files_never_leave_their_folder_for_an_address_kelpie_made() {
        let state = std::env::temp_dir().join(format!("kelpie-permit-paths-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        set_task_in(&state, "billing@app", true).unwrap();
        set_repo_in(&state, "app", true).unwrap();
        assert!(state.join("push-permits/billing@app").is_file());
        assert!(state.join("push-permits/repos/app").is_file());
        assert_eq!(in_state(&state, "app", "repos"), None, "a task named like the repos folder is not the repo file");
        assert_eq!(in_state(&state, "search@app", "app"), Some(Permit::Repo));
        fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn an_away_file_that_holds_no_time_is_not_away_mode() {
        let state = std::env::temp_dir().join(format!("kelpie-away-garbage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir_all(&state).unwrap();
        fs::write(away_file(&state), "garbage").unwrap();
        assert_eq!(away_since_in(&state), None);
        fs::remove_dir_all(state).unwrap();
    }
}
