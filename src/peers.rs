use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::AlsoNoticed;
use crate::herdr;
use crate::lead::install_root;
use crate::repo::{main_checkout, name_of, setup_note};
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use crate::tasks::slots;

// A Claude that can be told things: a task in a repo (task@repo) or the lead.
pub struct Session {
    pub address: String,
    pub task: Option<String>,
    pub pane: Option<String>,
    pub status: Option<String>,
}

impl Session {
    pub fn state(&self) -> &str {
        match (&self.pane, self.status.as_deref()) {
            (None, _) => "no Claude",
            (Some(_), Some("blocked")) => "waiting for an answer",
            (Some(_), Some(status)) => status,
            (Some(_), None) => "unknown",
        }
    }
}

pub fn address(task: &str, repo: &str) -> String {
    format!("{task}@{repo}")
}

pub fn sessions() -> Result<Vec<Session>> {
    let workspaces = herdr::workspaces()?;
    let agents: Vec<Value> = herdr::agents()?.into_iter().filter(|agent| crate::agent::Agent::of(agent).is_some()).collect();
    let agent_in = |workspace: Option<&str>| workspace.and_then(|id| agents.iter().find(|agent| agent["workspace_id"].as_str() == Some(id)));
    let text = |value: &Value| value.as_str().map(str::to_string);
    let mut list = Vec::new();
    for workspace in workspaces.iter().filter(|workspace| workspace["label"] == LEAD_LABEL) {
        let agent = agent_in(workspace["workspace_id"].as_str());
        list.push(Session { address: LEAD_LABEL.to_string(), task: None, pane: agent.and_then(|agent| text(&agent["pane_id"])), status: agent.and_then(|agent| text(&agent["agent_status"])) });
    }
    let repos: BTreeSet<PathBuf> = workspaces.iter().filter_map(|workspace| workspace["worktree"]["repo_root"].as_str()).map(PathBuf::from).collect();
    for repo in &repos {
        for slot in slots(repo)? {
            let Some(task) = slot.task else { continue };
            let workspace = workspaces.iter().find(|workspace| workspace["worktree"]["checkout_path"].as_str() == slot.path.to_str())
                .and_then(|workspace| workspace["workspace_id"].as_str());
            let agent = agent_in(workspace);
            list.push(Session {
                address: address(&task, &name_of(repo)),
                task: Some(task),
                pane: agent.and_then(|agent| text(&agent["pane_id"])),
                status: agent.and_then(|agent| text(&agent["agent_status"])),
            });
        }
    }
    Ok(list)
}

// Who runs this: the task whose worktree holds cwd, or the lead by its herdr Space.
pub fn me(cwd: &Path) -> Option<String> {
    if let Ok(repo) = main_checkout(cwd)
        && let Some(task) = slots(&repo).ok()?.into_iter().find(|slot| slot.task.is_some() && cwd.starts_with(&slot.path)).and_then(|slot| slot.task)
    {
        return Some(address(&task, &name_of(&repo)));
    }
    let workspace = std::env::var("HERDR_WORKSPACE_ID").ok()?;
    herdr::workspaces().ok()?.iter()
        .any(|space| space["workspace_id"] == workspace.as_str() && space["label"] == LEAD_LABEL)
        .then(|| LEAD_LABEL.to_string())
}

// `task` reaches every half of that task but the sender's own; `task@repo` and `lead` reach one session.
pub fn resolve<'a>(to: &str, from: &str, sessions: &'a [Session]) -> Result<Vec<&'a Session>> {
    let found: Vec<&Session> = if to == LEAD_LABEL || to.contains('@') {
        sessions.iter().filter(|session| session.address == to).collect()
    } else {
        sessions.iter().filter(|session| session.task.as_deref() == Some(to)).collect()
    };
    let found: Vec<&Session> = found.into_iter().filter(|session| session.address != from).collect();
    if found.is_empty() {
        return Err(format!("No session \"{to}\" to tell (other than you); kebbie peers lists them"));
    }
    Ok(found)
}

pub fn list(me: Option<&str>, sessions: &[Session]) -> String {
    let task = me.and_then(|me| me.split_once('@')).map(|(task, _)| task);
    let line = |session: &Session| format!("  {:<36} {}", session.address, session.state());
    let others = |same: bool| -> Vec<String> {
        sessions.iter()
            .filter(|session| session.task.is_some() && Some(session.address.as_str()) != me && (session.task.as_deref() == task) == same)
            .map(line).collect()
    };
    let mut lines = vec![match me {
        Some(me) => format!("You are {me}."),
        None => "You are not in a task's worktree or the lead.".to_string(),
    }];
    let (same, other) = (others(true), others(false));
    if !same.is_empty() {
        lines.push("Same task, other repos:".to_string());
        lines.extend(same);
    }
    lines.push("Other tasks:".to_string());
    lines.extend(if other.is_empty() { vec!["  none".to_string()] } else { other });
    if me != Some(LEAD_LABEL) {
        lines.extend(sessions.iter().filter(|session| session.address == LEAD_LABEL).map(line));
    }
    lines.join("\n")
}

pub fn peers(cwd: &Path) -> Result<String> {
    Ok(list(me(cwd).as_deref(), &sessions()?))
}

// The stable link when it leads to this binary, so a task's settings survive reinstalls.
pub fn kebbie_command() -> Result<String> {
    let exe = own_exe()?;
    let exe = exe.canonicalize().map_err(|error| format!("{}: {error}", exe.display()))?;
    let home = std::env::var("HOME").unwrap_or_default();
    let stable = PathBuf::from(home).join(".local/share/kebbie/current/target/release/kebbie");
    Ok(if stable.canonicalize().ok() == Some(exe.clone()) { stable } else { exe }.to_string_lossy().to_string())
}

pub fn own_exe() -> Result<PathBuf> {
    std::env::current_exe().map(|exe| replaced_binary(&exe)).map_err(|error| error.to_string())
}

// Linux names a running binary that a rebuild replaced "<path> (deleted)"; the path itself holds the new one.
fn replaced_binary(exe: &Path) -> PathBuf {
    PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"))
}

pub struct Place<'a> {
    pub agent: crate::agent::Agent,
    pub repo: &'a Path,
    pub worktree: &'a Path,
    pub setup_skill: &'a str,
    pub setup_command: Option<&'a str>,
    pub rules_skill: &'a str,
    pub note: Option<&'a str>,
    pub profile: &'a str,
    pub sandboxed: bool,
}

// A sandboxed task's file tools reach its temp folder but not kebbie's state; without the sandbox the plan stays in kebbie's state.
pub fn plan_file(sandboxed: bool, me: &str, tmp: impl FnOnce() -> Result<PathBuf>, state: impl FnOnce() -> Result<PathBuf>) -> Result<PathBuf> {
    if sandboxed { Ok(tmp()?.join("plan.md")) } else { Ok(state()?.join("plans").join(format!("{me}.md"))) }
}

pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values.iter().fold(template.to_string(), |text, (name, value)| text.replace(&format!("{{{name}}}"), value))
}

pub fn worktree_values(place: &Place) -> Result<Vec<(&'static str, String)>> {
    let note = setup_note(place.worktree)?.display().to_string();
    Ok(vec![
        ("path", place.worktree.display().to_string()),
        ("slot", name_of(place.worktree)),
        ("main", place.repo.display().to_string()),
        ("agent", place.agent.name().to_string()),
        ("prepare", prepare_line(place.agent, place.setup_skill, place.setup_command, &note)),
        ("rules", if place.agent == crate::agent::Agent::Claude { rules_line(place.rules_skill) } else { String::new() }),
    ])
}

// Claude runs the setup skill; Codex has no kebbie skill, so it runs the repo's own setup command itself.
fn prepare_line(agent: crate::agent::Agent, setup_skill: &str, command: Option<&str>, note: &str) -> String {
    if agent == crate::agent::Agent::Claude {
        let setup = command.map_or(String::new(), |command| format!(" It prepares the worktree with the repo's own command `{command}` (worktree.setup_command)."));
        if let Some(path) = crate::config::skill_path(setup_skill) {
            return match crate::config::quoted_file(&path) {
                Ok(text) => format!("Before anything else, make this worktree runnable by the steps in `{}` (worktree.setup_skill), quoted below, without touching the main checkout, and leave a note in `{note}` on what you did.{setup}\n\n{text}\n\n", path.display()),
                Err(error) => format!("Before anything else, make this worktree runnable without touching the main checkout: kebbie could not read worktree.setup_skill's file ({error}), so install its dependencies and create what it needs with the repo's own tools, and leave a note in `{note}` on what you did.{setup}"),
            };
        }
        return format!("Before anything else, run the skill `{setup_skill}` and follow it: it makes this worktree runnable without touching the main checkout, and leaves a note in `{note}` on what it did.{setup}");
    }
    let how = command.map_or("install its dependencies and create what it needs with the repo's own tools, never touching the main checkout".to_string(), |command| format!("run the repo's own setup command `{command}` from the worktree (worktree.setup_command)"));
    format!("Before anything else, make this worktree runnable: {how}. Then write a short note in `{note}` on what you did.")
}

fn rules_line(rules_skill: &str) -> String {
    if let Some(path) = crate::config::skill_path(rules_skill) {
        return match crate::config::quoted_file(&path) {
            Ok(text) => format!("\n\nThen follow the rules in `{}` (task.rules_skill), quoted below, for the rest of this task: they hold the user's rules for how work is planned, built and committed here, and when you report ready a read-only reviewer checks your commits against them.\n\n{text}", path.display()),
            Err(error) => format!("\n\nThe user's rules for this task are in task.rules_skill's file, which kebbie could not read ({error}): give each commit a subject, a blank line and a body, and tell the user the rules file is missing."),
        };
    }
    format!("\n\nThen run the skill `{rules_skill}` and follow it for the rest of this task: it holds the user's rules for how work is planned, built and committed here. When you report ready, kebbie refuses a commit without a subject, a blank line and a body, and a read-only reviewer checks your commits against it.")
}

// Codex has no kebbie skill, so its line names the docs and not the skill.
pub fn docs_line(agent: crate::agent::Agent, required: bool) -> String {
    match (required, agent) {
        (false, _) => String::new(),
        (true, crate::agent::Agent::Claude) => "\n\nThis repo keeps its docs in step with its code (`docs.required`): when your change alters what a user sees or does, run the skill `kebbie:keep-docs` and update the docs in this task. The reviewer fails a branch that changes such behaviour and documents nothing.".to_string(),
        (true, _) => "\n\nThis repo keeps its docs in step with its code (`docs.required`): when your change alters what a user sees or does, update the page that describes it in this task, found from the index in docs/README.md, or the README when there is none. The reviewer fails a branch that changes such behaviour and documents nothing.".to_string(),
    }
}

// The plan step of roles/task.md: the task's note when kebbie keeps one, else with the vault as a backlog source a note there, else a plan file outside the worktree (plan_file); never a file in the worktree, where it would land in the repo.
pub fn plan_step(sources: &[crate::config::Source], note: Option<&str>, kebbie: &str, task: &str, plans: &Path) -> String {
    if let Some(note) = note {
        return format!("Write your plan into your note `{note}` with the vault tools, and put it to the user with `{kebbie} ask --plan vault:{note} \"<one line on what the plan does>\"`; the board shows them the note.");
    }
    if sources.contains(&crate::config::Source::Vault) {
        return format!(
            "Write your plan into this task's note in the vault with the vault tools: the note you were started from, or else a new task note under `Projects/` with status `active`, tied to this task with `{kebbie} link {task} Projects/<note>.md`. \
             Put it to the user with `{kebbie} ask --plan vault:Projects/<note>.md \"<one line on what the plan does>\"`; the board shows them the note.",
        );
    }
    let plans = plans.display();
    format!("Write your plan as markdown to `{plans}`, kebbie's place for it outside your worktree, and put it to the user with `{kebbie} ask --plan {plans} \"<one line on what the plan does>\"`; the board shows them the file.")
}

pub fn vault_section(note: Option<&str>, kebbie: &str) -> String {
    let Some(note) = note else { return String::new() };
    format!(
        "\n\n## Your note in the vault\n\n\
         This task's note is `{note}` in the vault: kebbie opened it when the task started and keeps it tied to this task. Read and write it with the vault MCP tools.\n\n\
         1. Before your first change to a file, write the note's Problem section: what is wrong or missing, for whom, and what done looks like, in your own clear words, from the user's prompt and what you read. Rewrite the prompt rather than pasting it: the user writes rough notes, the vault keeps clear ones. Edits, and shell commands that write a file in the worktree, wait until Problem is written.\n\
         2. Keep its solution section (what changes, from the user's side) and its decisions section (each choice made, with its reason) current as the work goes, in the note's own headings.\n\
         3. kebbie writes the repo's log line after each push from your commit subjects, so write subjects that read well on their own.\n\n\
         `{kebbie} done` waits until the solution and decisions are written, then closes the note; a reset puts it back in the backlog."
    )
}

// The {noticed} section of roles/task.md; vault is whether the task's Claude has the vault plugin's tools.
pub fn noticed_section(choice: AlsoNoticed, vault: bool, kebbie: &str) -> String {
    let intro = "As you work you will notice things outside this task: a bug nearby, a stale doc, a refactor worth doing.";
    let file = if vault {
        "file it in the vault's backlog with the vault's `backlog_add` tool: its area, one line, a priority and this task as the source".to_string()
    } else {
        format!("file it with `{kebbie} backlog add --priority <priority> \"<Title> — <summary> (<area>)\"`")
    };
    let failed = "`not filed` when filing fails, so the lead files it";
    match choice {
        AlsoNoticed::Backlog => format!(
            "{intro} Keep your commits to this task's change, and {file}, the moment you notice it. \
             List each under **Also noticed** on your sign-off card, one line each: what it is and where you filed it, or {failed}. \
             This is done when everything you noticed is filed and listed."
        ),
        AlsoNoticed::Fix => format!(
            "{intro} When one is small and sits in the code you are changing anyway (a few lines, nothing for the user to decide), fix it in this session as a commit of its own, apart from the task's commits. \
             Anything bigger, {file}. \
             List each under **Also noticed** on your sign-off card, one line each: what it is, and its commit or where you filed it, or {failed}. \
             This is done when everything you noticed is fixed in its own commit or filed, and listed."
        ),
        AlsoNoticed::Ask => format!(
            "{intro} Put each to the user before you change anything for it, one question each: \
             `{kebbie} ask --option \"fix: fix it in this task, as a commit of its own\" --option \"file: file it for later\" --option \"leave: leave it as it is\" --recommend <n> \"<what you noticed, and where>\"`. \
             Until the answer comes, your commits hold this task's change alone. On fix, commit it apart from the task's commits; on file, {file}. \
             List each under **Also noticed** on your sign-off card, one line each: what it is and what the user chose, or {failed}. \
             This is done when everything you noticed has the user's answer, is handled by it, and is listed."
        ),
    }
}

// The {steps} part of roles/task.md, only with task.steps on.
pub fn steps_section(on: bool, kebbie: &str) -> String {
    if !on {
        return String::new();
    }
    format!(
        "## Steps\n\nBefore your first line of code, list your steps with `{kebbie} steps \"<step>\" \"<step>\" …`: this task's steps in the order they must go out, preparatory ones first and a migration as a step of its own; put `--` before the steps when one starts with `-`. \
         A step is one commit, finished and pushable on its own. \
         The user accepts the commits one at a time and pushes the accepted ones while you build the next step, so keep building without waiting for them. \
         When the user says the split is wrong, run `{kebbie} steps` again with the new split before your next commit: every step not pushed yet is yours to reshape. \
         Once the user accepts a step it goes out, so they only ever hold the next one in review: make each one finished and pushable on its own.\n"
    )
}

// roles/task.md filled in for this task, its profile's section after it, written to the state dir for --append-system-prompt-file; returns the file and the role without its peer list, which changes on its own.
pub fn role_file(task: &str, place: &Place) -> Result<(PathBuf, String)> {
    let template = fs::read_to_string(install_root()?.join("roles/task.md")).map_err(|error| format!("roles/task.md: {error}"))?;
    let me = address(task, &name_of(place.repo));
    let listed = list(Some(&me), &sessions()?);
    let values = role_values(task, place, listed.split_once('\n').map(|(_, rest)| rest).unwrap_or_default())?;
    let pairs: Vec<(&str, &str)> = values.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let file = crate::lead::roles_dir()?.join(format!("{me}.md"));
    write_role(&file, &template, &pairs, place.profile)?;
    let quiet = role_values(task, place, "")?;
    let quiet: Vec<(&str, &str)> = quiet.iter().map(|(name, value)| (*name, value.as_str())).collect();
    Ok((file, fill(&template, &quiet) + place.profile))
}

fn role_values(task: &str, place: &Place, peers: &str) -> Result<Vec<(&'static str, String)>> {
    let repo = name_of(place.repo);
    let me = address(task, &repo);
    let peers = peers.to_string();
    let kebbie = kebbie_command()?;
    let plans = plan_file(place.sandboxed, &me, || crate::tasks::task_tmp(place.worktree), crate::lead::state_dir)?;
    let plan = plan_step(&crate::config::sources().unwrap_or_default(), place.note, &kebbie, task, &plans);
    let vault = vault_section(place.note, &kebbie);
    let settings = crate::config::resolve(Some(place.repo))?.settings;
    let noticed = noticed_section(settings.also_noticed.value, crate::vault_cli::detect().is_some(), &kebbie);
    let docs = docs_line(place.agent, settings.docs_required.value);
    let steps = steps_section(settings.steps.value, &kebbie);
    let mut values = vec![("address", me.clone()), ("task", task.to_string()), ("repo", repo), ("noticed", noticed), ("steps", steps), ("kebbie", kebbie), ("peers", peers), ("plan", plan), ("vault", vault), ("docs", docs)];
    values.extend(worktree_values(place)?);
    Ok(values)
}

pub fn write_role(file: &Path, template: &str, pairs: &[(&str, &str)], profile: &str) -> Result<()> {
    crate::state::write(file, fill(template, pairs) + profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tasks_role_file_carries_its_profiles_section_after_kebbies_role() {
        let dir = crate::sh::test_dir("role-profile");
        let file = dir.join("billing@app.md");
        let section = "\n\n## Your profile: designer\n\nMeasure in the browser.\n";
        write_role(&file, include_str!("../roles/task.md"), &[("agent", "Claude"), ("task", "billing"), ("repo", "app")], section).unwrap();
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.starts_with("# You are a kebbie task\n\nYou are the Claude for the task **billing** in the repo **app**"), "{}", &written[..200]);
        assert!(written.ends_with(section), "the profile's section closes the role");
        write_role(&file, "role", &[], "").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "role", "the built-in task adds nothing");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn every_use_of_kebbies_own_path_goes_through_own_exe() {
        let folder = |name: &str| std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name)).unwrap().map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap()).collect::<String>();
        let (board, tasks) = (folder("board"), folder("tasks"));
        for (name, source) in [("board/", board.as_str()), ("tasks/", tasks.as_str()), ("review.rs", include_str!("review.rs")), ("notify.rs", include_str!("notify.rs")), ("lead.rs", include_str!("lead.rs")), ("restart.rs", include_str!("restart.rs")), ("main.rs", include_str!("main.rs"))] {
            assert!(!source.contains(&["current", "_exe()"].concat()), "{name} reads its path raw, which Linux names \"(deleted)\" after a rebuild");
        }
        assert_eq!(include_str!("peers.rs").matches(&["current", "_exe()"].concat()).count(), 1);
    }

    #[test]
    fn a_binary_replaced_while_running_is_found_at_its_own_path() {
        assert_eq!(replaced_binary(Path::new("/k/target/release/kebbie (deleted)")), PathBuf::from("/k/target/release/kebbie"));
        assert_eq!(replaced_binary(Path::new("/k/target/release/kebbie")), PathBuf::from("/k/target/release/kebbie"));
    }

    fn session(address: &str, status: Option<&str>) -> Session {
        let task = address.split_once('@').map(|(task, _)| task.to_string());
        Session { address: address.to_string(), task, pane: status.map(|_| "w1:p1".to_string()), status: status.map(str::to_string) }
    }

    fn sessions() -> Vec<Session> {
        vec![session("lead", Some("idle")), session("billing@api", Some("working")), session("billing@web", Some("blocked")), session("search@api", None)]
    }

    fn addresses(found: Result<Vec<&Session>>) -> Vec<String> {
        found.unwrap().iter().map(|session| session.address.clone()).collect()
    }

    #[test]
    fn a_task_name_reaches_its_other_halves() {
        let all = sessions();
        assert_eq!(addresses(resolve("billing", "billing@api", &all)), ["billing@web"]);
        assert_eq!(addresses(resolve("billing", "lead", &all)), ["billing@api", "billing@web"]);
    }

    #[test]
    fn an_address_or_the_lead_reaches_one_session() {
        let all = sessions();
        assert_eq!(addresses(resolve("search@api", "billing@web", &all)), ["search@api"]);
        assert_eq!(addresses(resolve("lead", "billing@web", &all)), ["lead"]);
    }

    #[test]
    fn nobody_tells_themselves_or_an_unknown_session() {
        let all = sessions();
        assert!(resolve("search", "search@api", &all).is_err());
        assert!(resolve("lead", "lead", &all).is_err());
        assert!(resolve("payments", "lead", &all).is_err());
    }

    #[test]
    fn the_setup_skill_reaches_the_role_from_each_layer() {
        use crate::config::{Machine, Worktree, parse_machine, parse_repo, settings};
        let template = include_str!("../roles/task.md");
        let machine = parse_machine("[worktree]\nsetup_skill = \"my-tools:set-up-worktree\"\n", "config.toml").unwrap();
        let own = parse_repo("[worktree]\nsetup_skill = \"app-tools:set-up-worktree\"\n", "app.toml").unwrap();
        for (own, machine, skill) in [
            (own.clone(), machine.clone(), "app-tools:set-up-worktree"),
            (Worktree::default(), machine, "my-tools:set-up-worktree"),
            (Worktree::default(), Machine::default(), "kebbie:set-up-worktree"),
        ] {
            let skill_value = settings(&own, &machine).setup_skill.value;
            let prepare = prepare_line(crate::agent::Agent::Claude, &skill_value, None, "/src/app/.git/worktrees/wt2.app/kebbie-setup.md");
            let role = fill(template, &[("path", "/src/wt2.app"), ("slot", "wt2.app"), ("main", "/src/app"), ("prepare", &prepare)]);
            assert!(role.contains(&format!("run the skill `{skill}` and follow it")), "{role}");
            assert!(role.contains("You are in `/src/wt2.app`, worktree wt2.app of `/src/app`"));
            assert!(role.contains("a note in `/src/app/.git/worktrees/wt2.app/kebbie-setup.md`"));
        }
    }

    #[test]
    fn the_setup_command_reaches_the_role_only_when_set() {
        use crate::config::{Machine, Worktree, parse_repo, settings};
        let template = include_str!("../roles/task.md");
        let own = parse_repo("[worktree]\nsetup_command = \"script/prepare_worktree\"\n", "app.toml").unwrap();
        let role = |own: &Worktree| fill(template, &[("prepare", &prepare_line(crate::agent::Agent::Claude, "kebbie:set-up-worktree", settings(own, &Machine::default()).setup_command.value.as_deref(), "/n.md"))]);
        let set = role(&own);
        assert!(set.contains("on what it did. It prepares the worktree with the repo's own command `script/prepare_worktree` (worktree.setup_command). The main checkout"), "{set}");
        let unset = role(&Worktree::default());
        assert!(unset.contains("on what it did. The main checkout is read-only to you."), "{unset}");
        assert!(!unset.contains("{prepare}") && !unset.contains("setup_command"));
    }

    #[test]
    fn a_codex_tasks_role_names_codex_and_runs_the_repos_setup_command_itself() {
        let template = include_str!("../roles/task.md");
        let codex = |command: Option<&str>| fill(template, &[("agent", "Codex"), ("prepare", &prepare_line(crate::agent::Agent::Codex, "kebbie:set-up-worktree", command, "/n.md")), ("rules", "")]);
        let set = codex(Some("script/prepare_worktree"));
        assert!(set.contains("You are the Codex for the task") && !set.contains("You are the Claude"), "{set}");
        assert!(set.contains("run the repo's own setup command `script/prepare_worktree` from the worktree") && set.contains("a short note in `/n.md`"), "{set}");
        assert!(!set.contains("kebbie:set-up-worktree") && !set.contains("{rules}"), "a skill Codex cannot run is not named: {set}");
        assert!(codex(None).contains("install its dependencies and create what it needs with the repo's own tools"));
        let claude = fill(template, &[("agent", "Claude")]);
        assert!(claude.contains("You are the Claude for the task"));
    }

    #[test]
    fn the_rules_skill_reaches_the_role_from_each_layer() {
        use crate::config::{Machine, Worktree, parse_machine, parse_repo, settings};
        let template = include_str!("../roles/task.md");
        let machine = parse_machine("[task]\nrules_skill = \"my-rules\"\n", "config.toml").unwrap();
        let own = parse_repo("[task]\nrules_skill = \"app-rules\"\n", "app.toml").unwrap();
        for (own, machine, skill) in [
            (own.clone(), machine.clone(), "app-rules"),
            (Worktree::default(), machine, "my-rules"),
            (Worktree::default(), Machine::default(), "kebbie:work-rules"),
        ] {
            let role = fill(template, &[("rules", &rules_line(&settings(&own, &machine).rules_skill.value))]);
            assert!(role.contains(&format!("Then run the skill `{skill}` and follow it for the rest of this task")), "{role}");
            assert!(!role.contains("{rules}"));
        }
    }

    #[test]
    fn a_skill_setting_that_names_a_file_gives_the_task_its_text() {
        let dir = crate::sh::test_dir("peers-skill-file");
        let (rules, setup) = (dir.join("RULES.md"), dir.join("SETUP.md"));
        std::fs::write(&rules, "# House rules\n\n1. One commit per change.\n").unwrap();
        std::fs::write(&setup, "Run `make deps`.\n").unwrap();
        let template = include_str!("../roles/task.md");
        let role = fill(template, &[
            ("prepare", &prepare_line(crate::agent::Agent::Claude, &setup.display().to_string(), None, "/n.md")),
            ("rules", &rules_line(&rules.display().to_string())),
        ]);
        assert!(role.contains(&format!("by the steps in `{}` (worktree.setup_skill), quoted below", setup.display())) && role.contains(&format!("--- {} ---\nRun `make deps`.\n--- end of {} ---", setup.display(), setup.display())), "{role}");
        assert!(role.contains(&format!("Then follow the rules in `{}` (task.rules_skill)", rules.display())) && role.contains("# House rules\n\n1. One commit per change.\n--- end of"), "{role}");
        assert!(!role.contains("run the skill"), "a file is not named as a skill: {role}");
        let gone = rules_line(&dir.join("gone.md").display().to_string());
        assert!(gone.contains("which kebbie could not read") && gone.contains("gone.md"), "{gone}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_docs_line_reaches_the_role_only_in_a_repo_that_requires_docs() {
        use crate::agent::Agent;
        use crate::config::{Machine, Worktree, parse_repo, settings};
        let template = include_str!("../roles/task.md");
        let role = |own: &Worktree, agent: Agent| fill(template, &[("rules", ""), ("vault", ""), ("docs", &docs_line(agent, settings(own, &Machine::default()).docs_required.value))]);
        let on = parse_repo("[docs]\nrequired = true\n", "kebbie.toml").unwrap();
        let claude = role(&on, Agent::Claude);
        assert!(claude.contains("read-only to you.\n\nThis repo keeps its docs in step with its code (`docs.required`)") && claude.contains("run the skill `kebbie:keep-docs`"), "{claude}");
        let codex = role(&on, Agent::Codex);
        assert!(codex.contains("`docs.required`") && !codex.contains("kebbie:keep-docs"), "a skill Codex cannot run is not named: {codex}");
        let off = role(&Worktree::default(), Agent::Claude);
        assert!(!off.contains("docs.required") && !off.contains("keep-docs") && !off.contains("{docs}"), "{off}");
    }

    #[test]
    fn the_role_isolates_a_probe_from_the_users_state_and_packages() {
        let role = fill(include_str!("../roles/task.md"), &[]);
        for setting in ["KEBBIE_STATE_DIR=<temp>/kebbie", "`GEM_HOME` and `BUNDLE_PATH`", "`NPM_CONFIG_PREFIX`", "a virtualenv", "`HERDR_BIN_PATH=false`", "HERDR_SOCKET_PATH=<temp>/herdr.sock"] {
            assert!(role.contains(setting), "{setting}");
        }
    }

    #[test]
    fn the_role_asks_the_user_through_kebbie_one_question_at_a_time() {
        let role = fill(include_str!("../roles/task.md"), &[("kebbie", "/bin/kebbie")]);
        for needed in ["/bin/kebbie ask --option", "--recommend <n>", "one question per", "on the kebbie board", "`[from you] <answer>`", "`[from you]` at the very start of a line is the user's answer", "indents every following line", "End an option with ` +comment`", "A mock or a design to approve goes the same way", "ask with `--plan`"] {
            assert!(role.contains(needed), "{needed}");
        }
    }

    #[test]
    fn the_role_sends_how_to_the_lead_lets_a_small_task_build_and_asks_the_user_in_their_words() {
        let role = fill(include_str!("../roles/task.md"), &[("kebbie", "/bin/kebbie")]);
        for needed in ["`/bin/kebbie tell lead \"<question>\"`", "whether or not the user is present", "builds without a plan approval and without questions", "one question at a time", "One plain sentence of context", "by its title, not its branch", "what the user gets with it", "No flags, commit ids, file names or internal terms"] {
            assert!(role.contains(needed), "{needed}");
        }
        assert!(!role.contains("Ask what you need decided before you plan"), "a task no longer asks before every plan");
    }

    #[test]
    fn the_plan_goes_in_the_vault_note_with_the_vault_and_in_the_tasks_temp_folder_without_it() {
        use crate::config::Source;
        let (tmp, state) = (|| Ok(PathBuf::from("/src/app/.git/worktrees/1/kebbie-tmp")), || Ok(PathBuf::from("/state")));
        assert_eq!(plan_file(true, "billing@web", tmp, state).unwrap(), Path::new("/src/app/.git/worktrees/1/kebbie-tmp/plan.md"), "a sandboxed task's file tools are denied kebbie's state");
        assert_eq!(plan_file(false, "billing@web", tmp, state).unwrap(), Path::new("/state/plans/billing@web.md"), "without the sandbox the plan stays where 0.25.0 kept it");
        let source = include_str!("peers.rs");
        let values = &source[source.find("\nfn role_values(").unwrap()..];
        assert!(values[..values.find("\n}\n").unwrap()].contains("plan_file(place.sandboxed,"), "the role names the plan file plan_file picks");
        let plans = Path::new("/src/app/.git/worktrees/1/kebbie-tmp/plan.md");
        let vault = plan_step(&[Source::Linear, Source::Vault], None, "/bin/kebbie", "billing", plans);
        assert!(vault.contains("into this task's note in the vault") && vault.contains("/bin/kebbie link billing Projects/<note>.md") && vault.contains("/bin/kebbie ask --plan vault:Projects/<note>.md"), "{vault}");
        let file = plan_step(&[Source::Local], None, "/bin/kebbie", "billing", plans);
        let note = plan_step(&[Source::Local], Some("Projects/kebbie-billing.md"), "/bin/kebbie", "billing", plans);
        assert!(note.contains("into your note `Projects/kebbie-billing.md`") && note.contains("--plan vault:Projects/kebbie-billing.md"), "{note}");
        assert!(file.contains("to `/src/app/.git/worktrees/1/kebbie-tmp/plan.md`, kebbie's place for it outside your worktree") && file.contains("/bin/kebbie ask --plan /src/app/.git/worktrees/1/kebbie-tmp/plan.md"), "{file}");
        assert!(!file.contains("vault"), "{file}");
        let role = fill(include_str!("../roles/task.md"), &[("plan", &file)]);
        assert!(role.contains(&format!("2. A plan to approve: {file} The user approves it")) && !role.contains("{plan}"), "{role}");
    }

    #[test]
    fn the_steps_section_shows_only_with_task_steps_on() {
        let off = fill(include_str!("../roles/task.md"), &[("steps", &steps_section(false, "/bin/kebbie"))]);
        assert!(!off.contains("## Steps") && !off.contains("kebbie steps") && !off.contains("{steps}"), "off, a task commits its own way");
        let on = fill(include_str!("../roles/task.md"), &[("steps", &steps_section(true, "/bin/kebbie"))]);
        for needed in ["## Steps\n\nBefore your first line of code, list your steps with `/bin/kebbie steps \"<step>\"", "A step is one commit, finished and pushable on its own.", "every step not pushed yet is yours to reshape", "make each one finished and pushable on its own.\n\n## What you notice outside your task"] {
            assert!(on.contains(needed), "{needed}: {on}");
        }
        let source = include_str!("peers.rs");
        let values = &source[source.find("\nfn role_values(").unwrap()..];
        assert!(values[..values.find("\n}\n").unwrap()].contains("steps_section(settings.steps.value,"), "the role follows the repo's setting");
    }

    #[test]
    fn the_vault_section_shows_only_for_a_task_with_a_note() {
        let off = fill(include_str!("../roles/task.md"), &[("vault", &vault_section(None, "/bin/kebbie"))]);
        assert!(!off.contains("vault") && !off.contains("{vault}"), "without the vault plugin no vault concept shows");
        let on = fill(include_str!("../roles/task.md"), &[("vault", &vault_section(Some("Projects/kebbie-billing.md"), "/bin/kebbie"))]);
        for needed in ["## Your note in the vault", "`Projects/kebbie-billing.md`", "write the note's Problem section", "Rewrite the prompt rather than pasting it", "decisions section", "`/bin/kebbie done` waits until the solution and decisions are written"] {
            assert!(on.contains(needed), "{needed}: {on}");
        }
        assert!(on.find("## Your note in the vault").unwrap() < on.find("## Probes").unwrap());
    }

    #[test]
    fn what_a_task_does_with_what_it_notices_follows_the_setting_and_the_vault() {
        let role = |choice, vault| fill(include_str!("../roles/task.md"), &[("noticed", &noticed_section(choice, vault, "/bin/kebbie"))]);
        for vault in [true, false] {
            for choice in AlsoNoticed::ALL {
                let role = role(choice, vault);
                assert!(role.contains("## What you notice outside your task\n\nAs you work you will notice things outside this task") && !role.contains("{noticed}"), "{role}");
                assert!(role.contains("under **Also noticed** on your sign-off card") && role.contains("the **Also noticed** list when you noticed anything outside the task"), "{role}");
                assert!(role.contains("`not filed` when filing fails, so the lead files it"), "{role}");
                let filed_in_vault = role.contains("the vault's `backlog_add` tool");
                let filed_locally = role.contains("`/bin/kebbie backlog add --priority <priority>");
                assert_eq!((filed_in_vault, filed_locally), (vault, !vault), "{choice:?} with the vault {vault}: {role}");
            }
        }
        let backlog = role(AlsoNoticed::Backlog, true);
        assert!(backlog.contains("Keep your commits to this task's change") && backlog.contains("done when everything you noticed is filed and listed") && !backlog.contains("ask --option \"fix"), "{backlog}");
        let fix = role(AlsoNoticed::Fix, false);
        assert!(fix.contains("fix it in this session as a commit of its own, apart from the task's commits") && fix.contains("Anything bigger, file it with"), "{fix}");
        let ask = role(AlsoNoticed::Ask, false);
        assert!(ask.contains("/bin/kebbie ask --option \"fix: fix it in this task, as a commit of its own\" --option \"file: file it for later\" --option \"leave: leave it as it is\" --recommend <n>"), "{ask}");
        assert!(ask.contains("before you change anything for it") && ask.contains("has the user's answer"), "{ask}");
        let placed = role(AlsoNoticed::Backlog, false);
        assert!(placed.find("## Questions").unwrap() < placed.find("## What you notice").unwrap() && placed.find("## What you notice").unwrap() < placed.find("## Reports").unwrap());
    }

    #[test]
    fn the_list_puts_the_other_halves_first() {
        let listed = list(Some("billing@api"), &sessions());
        assert!(listed.starts_with("You are billing@api.\nSame task, other repos:\n  billing@web"), "{listed}");
        assert!(listed.contains("Other tasks:\n  search@api"));
        assert!(listed.lines().last().unwrap().starts_with("  lead"));
    }

    #[test]
    fn a_sessions_state_reads_from_its_pane_and_status() {
        assert_eq!(session("a@x", None).state(), "no Claude");
        assert_eq!(session("a@x", Some("blocked")).state(), "waiting for an answer");
        assert_eq!(session("a@x", Some("working")).state(), "working");
        let unknown = Session { address: "a@x".to_string(), task: None, pane: Some("p".to_string()), status: None };
        assert_eq!(unknown.state(), "unknown");
        let listed = list(None, &sessions());
        assert!(listed.starts_with("You are not in a task's worktree or the lead."));
        assert!(listed.contains("  none") || listed.contains("Other tasks:"), "{listed}");
        assert!(!list(Some("lead"), &sessions()).lines().any(|line| line.starts_with("  lead")), "the lead is not listed to itself");
    }

    #[test]
    fn the_template_fills_only_the_names_it_is_given_and_leaves_the_rest_visible() {
        assert_eq!(fill("{a} and {b} and {a}", &[("a", "x")]), "x and {b} and x");
        assert_eq!(fill("{{a}}", &[("a", "x")]), "{x}");
        assert_eq!(fill("plain", &[]), "plain");
        assert_eq!(fill("{a}", &[("a", "{b}"), ("b", "y")]), "y", "a value is itself filled by the names after it, so the order of pairs matters");
    }

    #[test]
    fn the_address_is_task_at_repo_and_a_message_to_a_bare_name_spans_the_repos() {
        assert_eq!(address("billing", "app"), "billing@app");
        let all = sessions();
        assert!(resolve("billing@web", "billing@web", &all).is_err(), "a session never resolves to itself");
        assert_eq!(addresses(resolve("billing", "billing@web", &all)), ["billing@api"]);
        assert_eq!(addresses(resolve("billing", "search@api", &all)), ["billing@api", "billing@web"]);
    }
}
