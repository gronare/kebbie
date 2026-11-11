use std::path::{Path, PathBuf};

use super::{load_machine, repos_dir, resolve, shown, AgentUpdates, AlsoNoticed, AutoApprove, Channel, HubCommands, HubTrust, Layer, Loaded, OngoingAt, ReportText, ReviewAuto, Second, Settings, PERMISSION_MODES};
use crate::repo::main_checkout;
use crate::sh::Result;
use crate::status::Group;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scope {
    Either,
    Machine,
    Repo,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Bool,
    Choice(Vec<&'static str>),
    Profile,
    ReviewProfile,
    SecondOpinion,
    Text,
    Number,
    List,
    Skill,
    Secret,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: &'static str,
    pub value: String,
    pub layer: Layer,
    pub kind: Kind,
    pub scope: Scope,
    pub about: String,
}

fn or_none(value: Option<&String>) -> String {
    value.map_or("none".to_string(), |value| format!("{value:?}"))
}

fn quoted(items: &[&str]) -> String {
    format!("[{}]", items.iter().map(|item| format!("{item:?}")).collect::<Vec<_>>().join(", "))
}

// The one list of keys: kebbie config prints it and the Settings page edits it.
pub fn entries(loaded: &Loaded) -> Vec<Entry> {
    entries_with(loaded, &board_keys(loaded).0)
}

fn board_keys(loaded: &Loaded) -> (crate::keys::Keys, Vec<String>) {
    crate::keys::Keys::with(&load_machine(&loaded.machine).map(|machine| machine.keys).unwrap_or_default())
}

fn entries_with(loaded: &Loaded, board_keys: &crate::keys::Keys) -> Vec<Entry> {
    let Settings { sources, groups, ongoing, setup_skill, setup_command, dev_server, dev_worker, dev_url, dev_route, stage_on_main, rules_skill, test_commands, also_noticed, permission_mode, profile, sandbox, steps, require_accept, review_model, review_auto, review_profile, review_second, notify_command, vault_command, server_name, worker_name, channel, agent_updates, idle_minutes, auto_restart, ci_watch, comments_watch, comments_days, clean_on_release, docs_required, keeper, recover, machine_name, hub_url, hub_commands, hub_trust, hub_auto_approve, report_signals, report_text, report_inbox, propose_issues } = &loaded.settings;
    let names: Vec<&str> = sources.value.iter().map(|source| source.key()).collect();
    let (server_key, worker_key) = (board_keys.char(crate::keys::Act::Server), board_keys.char(crate::keys::Act::Worker));
    let mut entries = Vec::new();
    let mut key = |key: &'static str, value: String, layer: Layer, kind: Kind, scope: Scope, about: &str| {
        entries.push(Entry { key, value, layer, kind, scope, about: about.to_string() });
    };
    key("board.backlog.sources", quoted(&names), sources.layer, Kind::List, Scope::Machine,
        "Where the board's Backlog tab finds work: \"local\" (backlog.md next to the machine config), \"vault\" and \"linear\", in any combination; config.toml only.");
    key("board.backlog.ongoing", format!("{:?}", ongoing.value.key()), ongoing.layer, Kind::Choice(OngoingAt::ALL.map(OngoingAt::key).to_vec()), Scope::Machine,
        "Where the Backlog column shows ONGOING, the vault notes in progress: \"top\" above the backlog, \"bottom\" below it, \"hidden\" not at all; config.toml only.");
    let order: Vec<&str> = groups.value.iter().map(|group| group.key()).collect();
    key("board.groups", quoted(&order), groups.layer, Kind::List, Scope::Machine,
        &format!("The order of the Work column's groups, by name: {}; groups the list leaves out follow in this default order, so a partial list moves only the ones it names; config.toml only.", quoted(&Group::ALL.map(Group::key))));
    key("worktree.setup_skill", format!("{:?}", setup_skill.value), setup_skill.layer, Kind::Skill, Scope::Either,
        "The skill a task's Claude runs first to make its worktree runnable: kebbie's own, a skill of yours by name (<skill> or <plugin>:<skill>), or a Markdown file by an absolute or ~ path, whose text the task gets in its role.");
    key("worktree.setup_command", or_none(setup_command.value.as_ref()), setup_command.layer, Kind::Text, Scope::Either,
        "The repo's own command that prepares an existing worktree, run from inside it; the setup skill runs it. A path applies only in a repo that has the file; elsewhere, and when none, bin/setup-worktree when the repo has one.");
    key("worktree.dev_server", or_none(dev_server.value.as_ref()), dev_server.layer, Kind::Text, Scope::Either,
        &format!("The shell command the board's {server_key} key runs in a server tab of the task's Space, with PORT set to the worktree's port; without one there is no {server_key}."));
    key("worktree.dev_worker", or_none(dev_worker.value.as_ref()), dev_worker.layer, Kind::Text, Scope::Either,
        &format!("A second command, such as a job runner, the board's {worker_key} key runs in a worker tab of the task's Space, with the same PORT; without one there is no {worker_key}."));
    key("worktree.dev_url", dev_url.value.as_ref().map_or("\"\" (no link)".to_string(), |url| format!("{url:?}")), dev_url.layer, Kind::Text, Scope::Either,
        "The running dev server's address, shown on the board as a link: {worktree} (the worktree's folder), {repo}, {number} (its pool number) and {port} (its port) are filled in; a worktree without what it names gets no link, and \"\" turns the link off.");
    key("worktree.dev_route", dev_route.value.to_string(), dev_route.layer, Kind::Bool, Scope::Repo,
        "Whether kebbie routes writes a Caddy site for each of this repo's worktrees, at the host its dev_url names, proxied to the worktree's port; for a dev server on a remote machine opened in your laptop's browser. Off unless the repo turns it on; repo config only.");
    key("worktree.stage_on_main", stage_on_main.value.to_string(), stage_on_main.layer, Kind::Bool, Scope::Repo,
        "Whether kebbie stage may put a ready task's commits on this repo's local default branch, unpushed, so you can try them from the main checkout before you push main; for a repo you run from its main checkout, such as kebbie's own. Off unless the repo turns it on; repo config only.");
    key("worktree.server_name", format!("{:?}", server_name.value), server_name.layer, Kind::Text, Scope::Machine,
        &format!("The name of the tab the board's {server_key} starts the dev server in, and of the Space's tag in herdr's sidebar while it runs; config.toml only."));
    key("worktree.worker_name", format!("{:?}", worker_name.value), worker_name.layer, Kind::Text, Scope::Machine,
        &format!("The same for the worker tab {worker_key} starts; config.toml only."));
    let cleaned: Vec<&str> = clean_on_release.value.iter().map(String::as_str).collect();
    key("worktree.clean_on_release", quoted(&cleaned), clean_on_release.layer, Kind::List, Scope::Either,
        "Build output kebbie removes from a worktree when it goes back to the pool (done, drop, reset, a landed task closing), so the pool's disk use stays bounded; folders inside the worktree, each removed only when git ignores it and tracks nothing in it, and \"target\" only where the worktree has a Cargo.toml. [] keeps everything.");
    key("task.rules_skill", format!("{:?}", rules_skill.value), rules_skill.layer, Kind::Skill, Scope::Either,
        "The skill with the rules for how work is planned, built and committed; a task's Claude follows it, the reviewer checks a ready task against it, tidy reshapes commits by it. kebbie's own, a skill of yours by name (<skill> or <plugin>:<skill>), or a Markdown file by an absolute or ~ path, whose text the task and the reviewer get.");
    let own: Vec<&str> = test_commands.value.iter().map(String::as_str).collect();
    key("task.test_commands", quoted(&own), test_commands.layer, Kind::List, Scope::Either,
        &format!("Test commands of this repo that kebbie records when a task's Claude runs them, on top of the built-in {}; matched at the start of the command or of any part of its ; && || chain, after cd, export, VAR=value and wrappers such as timeout and nice. kebbie evidence, the board, the push dialog and the reviewer show the runs.", quoted(&crate::evidence::DEFAULT_COMMANDS)));
    key("task.also_noticed", format!("{:?}", also_noticed.value.key()), also_noticed.layer, Kind::Choice(AlsoNoticed::ALL.map(AlsoNoticed::key).to_vec()), Scope::Either,
        "What a task's Claude does with what it notices outside its task (a bug nearby, a stale doc, a refactor): \"backlog\" files it, in the vault's backlog with the vault plugin, else with kebbie backlog add; \"fix\" fixes a small related one in a commit of its own and files the rest; \"ask\" puts each to you with kebbie ask first. Each is listed under Also noticed on the sign-off card, and the reviewer counts an unrelated change under \"backlog\" or \"ask\" as a finding.");
    key("task.permission_mode", or_none(permission_mode.value.as_ref()), permission_mode.layer, Kind::Choice(std::iter::once("none").chain(PERMISSION_MODES).collect()), Scope::Either,
        &format!("The permission mode a task's Claude starts in, one of {}; none leaves it to Claude's own default. kebbie start --mode overrides it for one task, and a restarted task keeps the mode it started in. \"bypassPermissions\" runs every tool without asking, for an isolated machine only; a task still never pushes nor approves its own commits.", quoted(&PERMISSION_MODES)));
    key("task.profile", profile.value.as_ref().map_or(format!("none ({:?})", crate::profiles::TASK), |profile| format!("{profile:?}")), profile.layer, Kind::Profile, Scope::Either,
        &format!("The profile a task starts with when kebbie start names none with --profile: a file in {}/<name>.toml with the agent, model, extra role, skills, permission mode and rules it adds to kebbie's own; none starts the built-in task. A restarted task keeps the profile it started with; kebbie profiles lists them, and the Settings page picks from them.", crate::profiles::dir().display()));
    key("task.sandbox", sandbox.value.to_string(), sandbox.layer, Kind::Bool, Scope::Either,
        &format!("Whether a task's and the reviewer's shell commands run in Claude Code's sandbox: writes only in the worktree, the task's temp folder and {}, network only to {} and a profile's own hosts, no Unix sockets; kebbie's own task commands ({}) run outside it, and kebbie refuses its user-only commands to a sandboxed process. On Linux it needs bubblewrap, socat and the seccomp filter, or kebbie refuses to start tasks; false runs them unsandboxed, where a task can pass as you to kebbie's user-only commands. A profile's [sandbox] enabled wins over it.", crate::sandbox::WRITES.join(", "), crate::sandbox::DOMAINS.join(", "), crate::sandbox::TASK_COMMANDS.join(", ")));
    key("task.steps", steps.value.to_string(), steps.layer, Kind::Bool, Scope::Either,
        "Tasks work in steps, one commit each, listed with kebbie steps before they write code: you accept and push one step at a time while the task builds the next. Off, a task commits its own way and kebbie steps refuses. New tasks follow it; a running task once it restarts.");
    key("task.require_accept", require_accept.value.to_string(), require_accept.layer, Kind::Bool, Scope::Either,
        "Whether every commit must be accepted before it can be pushed: kebbie push, the board's push and push in order refuse while a commit they would take is not accepted, and a ready task with one waits under Needs you to have its next step accepted. Off, a push takes what is committed and accepting is yours to use or skip. Only with task.steps on; with it off this has no effect.");
    key("review.model", or_none(review_model.value.as_ref()), review_model.layer, Kind::Text, Scope::Either,
        "The model the default reviewer (review.profile) runs on, ahead of its profile's model; none leaves it to that profile, then to its agent's default.");
    key("review.profile", format!("{:?}", review_profile.value), review_profile.layer, Kind::ReviewProfile, Scope::Either,
        "The review profile kebbie review runs by default, and whose verdict holds a task's push until it passes: the built-in reviewer, or a profile of yours with kind = \"review\". Only a review profile can be named; the Settings page picks from them.");
    key("review.second", if review_second.value == Second::None { "none".to_string() } else { format!("{:?}", review_second.value.key()) }, review_second.layer, Kind::SecondOpinion, Scope::Either,
        "A second opinion from another review profile: its name offers it with s when you send a task to review (v on the board, kebbie review --second), \"always <profile>\" runs it with every review, none offers none. It never holds the push: its points show on the task's row and go to the lead, which passes them to the task like a review's findings.");
    key("review.auto", format!("{:?}", review_auto.value.key()), review_auto.layer, Kind::Choice(ReviewAuto::ALL.map(ReviewAuto::key).to_vec()), Scope::Either,
        "What kebbie does when the assessor it runs on a ready report judges that the task's work, its pushed steps included, needs an independent review: \"suggest\" shows review suggested and its reason on the task, and nothing starts until you or the lead send it; \"run\" starts the reviewer by itself without holding the push; \"require\" starts it and holds the push, from the ready report on, until it passes. An assessment that gives no answer reads as review not assessed, a suggestion in every mode.");
    key("notify.command", or_none(notify_command.value.as_ref()), notify_command.layer, Kind::Text, Scope::Machine,
        "The shell command kebbie runs when something needs you (a task needs you or is ready, a question waits, the lead's summary); KEBBIE_EVENT, KEBBIE_TASK, KEBBIE_REPO and KEBBIE_TEXT describe it, and $1 is the text. None sends no notifications; config.toml only.");
    key("vault.command", or_none(vault_command.value.as_ref()), vault_command.layer, Kind::Text, Scope::Machine,
        "The vault command kebbie runs for task notes, log lines and the board's snapshot, before its own arguments (about --json, task open, log append, ...), such as \"uvx mindin\"; it is kebbie's own, whatever agent runs the task. kebbie setup finds the candidates on this machine and writes your choice. None falls back to the Claude vault plugin's pinned command; config.toml only.");
    key("ci.watch", ci_watch.value.to_string(), ci_watch.layer, Kind::Bool, Scope::Either,
        "Whether kebbie watches GitHub Actions after a push and keeps the task open under CI running until every run for the pushed commit is green; a failed run moves the task to Needs you and its Claude gets the failing job's log. Needs origin on GitHub and gh logged in; false closes the task right after the push. A repo config can turn it off for one repo.");
    key("comments.watch", comments_watch.value.to_string(), comments_watch.layer, Kind::Bool, Scope::Either,
        "Whether kebbie watches the GitHub comments colleagues leave on the commits it pushed, reads each one independently with a read-only Claude, and shows it under COMMENTS on the board with a recommendation and a reply to send; kebbie never posts a reply. Needs origin on GitHub and gh logged in, as ci.watch does; your own comments are skipped. A repo config can turn it off for one repo.");
    key("comments.watch_days", comments_days.value.to_string(), comments_days.layer, Kind::Number, Scope::Either,
        "How many days after a push kebbie keeps checking its commits for new comments, asking gh once per repo every 12 minutes, or less often while gh fails; 1 to 90. A comment already on the board stays there until you mark its reply sent or dismiss it.");
    key("docs.required", docs_required.value.to_string(), docs_required.layer, Kind::Bool, Scope::Repo,
        "Whether this repo's docs must keep in step with its code: a task's Claude updates the README and docs/ when its change alters what a user sees or does, by the skill kebbie:keep-docs, and the reviewer fails a branch that changes such behaviour and leaves it undescribed in the README or docs/. Off unless the repo turns it on; repo config only. kebbie doctor suggests it for a public GitHub repo.");
    key("update.channel", format!("{:?}", channel.value.key()), channel.layer, Kind::Choice(vec![Channel::Release.key(), Channel::Main.key()]), Scope::Machine,
        "What kebbie update installs on this machine: \"release\", the highest vX.Y.Z tag on origin whose release binary is published; \"main\", origin's default branch as it is pushed. kebbie update --from <branch or path> runs other code until kebbie update --stable; config.toml only.");
    key("update.agents", format!("{:?}", agent_updates.value.key()), agent_updates.layer, Kind::Choice(AgentUpdates::ALL.map(AgentUpdates::key).to_vec()), Scope::Machine,
        "What happens when an agent that does not update itself, Codex, has a newer version, which kebbie checks once a day: \"ask\" offers it in a window after U on the board, and nothing installs without your yes; \"auto\" has U install it with the agent's own installer; \"notify\" only puts a notice on the board and a line in kebbie doctor. Either way kebbie restarts the agent's tasks on the new version once they are idle; Claude Code updates itself. config.toml only.");
    key("restart.auto", auto_restart.value.to_string(), auto_restart.layer, Kind::Bool, Scope::Machine,
        "Whether kebbie restarts the Claude sessions and Codex tasks that run older code than is installed, after kebbie updates or a new Claude Code, Codex, plugin or MCP config is installed; no other session is ever restarted. Each resumes its conversation with claude --resume or codex resume in its own pane, keeping its flags, and kebbie update restarts the idle ones at once. false restarts nothing on its own: the board still marks those sessions, and kebbie restart restarts them when you want; config.toml only.");
    key("restart.idle_minutes", idle_minutes.value.to_string(), idle_minutes.layer, Kind::Number, Scope::Machine,
        "How long a session that runs older code than is installed waits to be idle and out of focus before kebbie restarts it; a session mid-turn, the pane you are in and one with background work running are never restarted on their own; config.toml only.");
    key("keeper.enabled", keeper.value.to_string(), keeper.layer, Kind::Bool, Scope::Machine,
        "Whether one background kebbie keeper per machine runs the routines (stale-session restarts, the update check, held messages, delivery checks of typed prompts, scheduled [[routines]]), so they go on with the board closed; herdr's startup, the board and kebbie update start it. false runs them in an open board instead; config.toml only.");
    key("recover.enabled", recover.value.to_string(), recover.layer, Kind::Bool, Scope::Machine,
        "Whether kebbie resumes a task whose Claude an API error stopped: after overload, a server error, a rate limit or a dropped connection it says carry on after 1, 2, 4 and 8 minutes; after a usage limit, once the allowance resets. A login, billing, invalid request or refusal error goes to you under Needs you at once, as does any error that stops a Codex task. false leaves every such task idle; config.toml only.");
    key("machine.name", format!("{:?}", machine_name.value), machine_name.layer, Kind::Text, Scope::Machine,
        "The machine's one name: your phone shows it through the hub, and kebbie claims vault notes under it; kebbie hub connect and kebbie hub join use it, set it from --name or ask for it when unset. A task started from a vault note writes it with the task's name into the note's kebbie frontmatter key, and kebbie on another machine sharing the vault shows the note as worked on at this name and asks before starting it; the default is the host's short name. config.toml only.");
    key("hub.url", format!("{:?}", hub_url.value), hub_url.layer, Kind::Text, Scope::Machine,
        "The hub kebbie hub connect and the board's hub window connect this machine to: the kebbie hub unless set, or your own kebbie hub at an https address (http only on this machine). Choosing your own hub there sets it; the hub a machine is connected to stays in kebbie's state with its keys, so changing this key later moves nothing (kebbie hub move does). config.toml only.");
    key("hub.commands", format!("{:?}", hub_commands.value.key()), hub_commands.layer, Kind::Choice(HubCommands::ALL.map(HubCommands::key).to_vec()), Scope::Machine,
        "What this machine takes from your devices through the hub: \"answer-and-push\" takes answers to questions, choices on an agent's prompt and pushes; \"answer\" takes answers and prompt choices, and refuses a push; \"off\" takes nothing, and the board still posts. A command counts only from a device hub.trust lets in. config.toml only.");
    key("hub.trust", format!("{:?}", hub_trust.value.key()), hub_trust.layer, Kind::Choice(HubTrust::ALL.map(HubTrust::key).to_vec()), Scope::Machine,
        "Which of your devices this machine takes commands from: \"device\" takes them from each device you approve on this machine, and a new device's first command waits here until you approve it (N on the board, or kebbie hub devices approve <name>) or hub.auto_approve lets it in; \"account\" takes them from any device signed in to the account this machine is connected to, with nothing to approve. Every command names the device that sent it, and kebbie hub devices lists them. config.toml only.");
    key("hub.auto_approve", format!("{:?}", hub_auto_approve.value.key()), hub_auto_approve.layer, Kind::Choice(AutoApprove::ALL.map(AutoApprove::key).to_vec()), Scope::Machine,
        "What happens to a device that waits for your approval under hub.trust = \"device\": \"24h\" approves it 24 hours after its first command unless you deny it first; \"never\" keeps it waiting until you approve it. A command waits at most its hour, so one sent before the approval expires, and the device says it was not delivered. config.toml only.");
    key("report.signals", report_signals.value.to_string(), report_signals.layer, Kind::Bool, Scope::Machine,
        "Whether kebbie sends its maintainers error signals, counts of kebbie's own failures: when kebbie itself fails (a panic, and some internal errors), the keeper sends at most once a day a count per failure with only kebbie's version and commit, the OS and CPU architecture, the component (a fixed name from kebbie's code, such as keeper.push) and the error kind (panic, internal-error, corrupt-state). Never an error message, path, repo, task or host name. Once a day it also sends an install signal: a random id made for this install alone, kebbie's version, the OS and CPU architecture and the update channel. Each error signal is a line under messages on the board. Not set, nothing is sent and no install id is made; kebbie setup asks. true turns both on; false sends none, drops what was counted and deletes the install id; config.toml only.");
    key("report.text", format!("{:?}", report_text.value.key()), report_text.layer, Kind::Choice(ReportText::ALL.map(ReportText::key).to_vec()), Scope::Machine,
        "What happens to a bug or wish about kebbie, once you or the lead saved it, with its text redacted (paths under your home as ~, no repo names, remote URLs or tokens): \"ask\" shows it on the board as it would go to kebbie's maintainers and only your yes sends it; \"always\" sends it without asking, and says so under messages, once you arm it with kebbie issue report --always in your own terminal (until then it asks); \"never\" keeps it on this machine, though kebbie issue report <id> still sends one when you ask. Signed with this install's key; config.toml only.");
    key("report.inbox_token", if report_inbox.value { "set".to_string() } else { "none".to_string() }, report_inbox.layer, Kind::Secret, Scope::Machine,
        "For kebbie's maintainers: the kha_ token the issues endpoint issued. With it the keeper fetches the reports the endpoint lets through every 10 minutes, files each as a kebbie issue and acknowledges it, and kebbie doctor counts the ones waiting in quarantine; never in a repo config, and never shown.");
    key("issues.propose", propose_issues.value.to_string(), propose_issues.layer, Kind::Bool, Scope::Machine,
        "Whether the lead proposes saving a bug or wish about kebbie when kebbie itself cost something (an error, a workaround, a missing capability), one per turn at most, and saves it on your yes; false keeps the lead from proposing or filing them, and it files one only when you ask it to. Takes effect when the lead next restarts (L on the board); config.toml only.");
    entries
}

pub fn describe(loaded: &Loaded) -> String {
    let repo = match &loaded.repo {
        Some(path) => shown(path),
        None => "none: not in a git repo (--repo <path> names one)".to_string(),
    };
    let mut lines: Vec<String> = [
        ("repo config", repo),
        ("machine config", shown(&loaded.machine)),
        ("defaults", "built in".to_string()),
    ].into_iter().map(|(layer, file)| format!("{:<16}{file}", format!("{layer}:"))).collect();
    lines.push(String::new());
    lines.push(format!("Each key below may be set in config.toml or, for one repo, in {}/<repo>.toml (named after the main checkout's folder), which wins; a key marked config.toml only applies to every repo.", repos_dir().display()));
    lines.push(String::new());
    let (board_keys, key_errors) = board_keys(loaded);
    lines.extend(entries_with(loaded, &board_keys).iter().flat_map(|entry| [format!("{} = {}  ({})", entry.key, entry.value, entry.layer.label()), format!("  {}", entry.about)]));
    lines.push("[keys]  (config.toml only)\n  The board's keys, as name = \"key\" by action: a plain character for an action whose key is one (esc, ctrl+c, ⏎, the arrows and tab stay), and one not taken by another action where both act; moving two at once in the table works. An entry that breaks a rule is ignored, and named below. Help, hints and messages show the key in effect.".to_string());
    lines.extend(board_keys.describe(&key_errors).into_iter().map(|line| format!("  {line}")));
    lines.push(format!("[[routines]]  (config.toml or a repo config)\n  Checks kebbie runs on a schedule, each as a task that reports, plans or fixes, one table each with the keys {}; a repo config's run in its repo, one in config.toml names its repo. Listed below are config.toml's and this repo's; kebbie routines lists every one with its next run, and docs/reference/configuration.md shows each key.", crate::routines::KEYS.join(", ")));
    let repo_file = loaded.repo.clone().map(|file| { let stem = file.file_stem().map(|stem| stem.to_string_lossy().to_string()); (file, stem) });
    let (routines, errors) = crate::routines::load_files([(loaded.machine.clone(), None)].into_iter().chain(repo_file).collect());
    lines.extend(routines.iter().map(|routine| format!("  {}: {} in {}, {}{}", routine.id, routine.schedule.text(), routine.repo, routine.mode.key(), if routine.enabled { "" } else { ", off" })));
    lines.extend(errors.iter().map(|error| format!("  ignored: {error}")));
    lines.push(format!("[[actions]]  (repo config only)\n  The Areas page's own actions for the repo, one table each, with the keys {}; docs/reference/configuration.md shows each.", crate::actions::ACTION_KEYS.join(", ")));
    lines.join("\n")
}

// For scripts such as the sidebar sync: one machine-wide setting's value, plain.
pub fn get(key: &str) -> Result<String> {
    let settings = resolve(None)?.settings;
    match key {
        "worktree.server_name" => Ok(settings.server_name.value),
        "worktree.worker_name" => Ok(settings.worker_name.value),
        other => Err(format!("kebbie config --get knows worktree.server_name and worktree.worker_name, not {other}")),
    }
}

pub fn show(cwd: &Path, repo: Option<&str>) -> Result<String> {
    let repo = match repo {
        Some(path) => {
            let path = PathBuf::from(path);
            main_checkout(&path).map_err(|_| format!("--repo {}: not a git repository", path.display()))?;
            Some(path)
        }
        None => main_checkout(cwd).ok().map(|_| cwd.to_path_buf()),
    };
    Ok(format!("{}\n{}", describe(&resolve(repo.as_deref())?), crate::hub::config_line(crate::lead::state_location().ok().as_deref())))
}
