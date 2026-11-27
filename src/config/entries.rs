use std::path::{Path, PathBuf};

use super::{load_machine, repos_dir, resolve, shown, AgentUpdates, AlsoNoticed, ApproveFrom, AutoApprove, BeforePush, Channel, HubTrust, Layer, Loaded, OngoingAt, ReportText, ReviewAuto, Second, Setting, Settings, PERMISSION_MODES};
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
    LeadProfile,
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

// A repo config's "" shows as unset, so it reads apart from a key nobody set.
fn or_unset(setting: &Setting<Option<String>>) -> String {
    if setting.value.is_none() && setting.layer == Layer::Repo { "unset".to_string() } else { or_none(setting.value.as_ref()) }
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
    let Settings { sources, groups, ongoing, backlog_template, next_question, tab_questions, setup_skill, setup_command, dev_server, dev_worker, dev_url, dev_route, stage_on_main, rules_skill, test_commands, also_noticed, permission_mode, profile, sandbox, steps, require_accept, review_model, review_auto, review_profile, review_second, investigate_profile, notify_command, vault_command, server_name, worker_name, channel, agent_updates, idle_minutes, auto_restart, lead_context, ci_watch, ci_deploy, ci_deploy_window, push_branch: _, tests_per_commit, tests_before_push, tests_full_command, tests_lint_command, comments_watch, comments_days, clean_on_release, release_command, docs_required, keeper, worker, link_networks, build_slots, build_commands, disk_trim_below, disk_builds_below, recover, machine_name, hub_url, hub_commands, hub_trust, hub_auto_approve, hub_approve_from, report_signals, report_text, report_inbox, propose_issues, lead_profile, writer_profile, ideas_machine } = &loaded.settings;
    let names: Vec<&str> = sources.value.iter().map(|source| source.key()).collect();
    let (server_key, worker_key) = (board_keys.char(crate::keys::Act::Server), board_keys.char(crate::keys::Act::Worker));
    let mut entries = Vec::new();
    let mut key = |key: &'static str, value: String, layer: Layer, kind: Kind, scope: Scope, about: &str| {
        entries.push(Entry { key, value, layer, kind, scope, about: about.to_string() });
    };
    key("board.backlog.sources", quoted(&names), sources.layer, Kind::List, Scope::Machine,
        "Where the board's Backlog tab finds work and where new items are written: \"local\" (the backlog folder next to the machine config) or \"vault\", one of them, and \"linear\", which is only read; config.toml only.");
    key("board.backlog.ongoing", format!("{:?}", ongoing.value.key()), ongoing.layer, Kind::Choice(OngoingAt::ALL.map(OngoingAt::key).to_vec()), Scope::Machine,
        "Where the Backlog page shows ONGOING, the vault notes in progress: \"top\" above the backlog, \"bottom\" below it, \"hidden\" not at all; config.toml only.");
    key("board.backlog.template", or_none(backlog_template.value.as_ref()), backlog_template.layer, Kind::Text, Scope::Machine,
        "A file with the sections local backlog items are written in, as JSON or TOML: sections, each with a key, a heading and whether it is required, and questions, the key of the section that holds the open questions; a relative path is read from the folder of config.toml. Unset, local items use kebbie's own: Background, Goal, What we know, Open questions, Out of scope and Source. Vault items follow the vault's task template. config.toml only.");
    let order: Vec<&str> = groups.value.iter().map(|group| group.key()).collect();
    key("board.groups", quoted(&order), groups.layer, Kind::List, Scope::Machine,
        &format!("The order of the Work column's groups, by name: {}; groups the list leaves out follow in this default order, so a partial list moves only the ones it names; config.toml only.", quoted(&Group::ALL.map(Group::key))));
    key("board.next_question", next_question.value.to_string(), next_question.layer, Kind::Bool, Scope::Machine,
        "Whether the questions window moves on to the next waiting question after an answer is sent; false closes it back to the board with the answer's notice. config.toml only.");
    key("board.tab_questions", tab_questions.value.to_string(), tab_questions.layer, Kind::Bool, Scope::Machine,
        "Whether tab and shift-tab move between the waiting questions in the questions window; false keeps the window on the question you opened. config.toml only.");
    key("worktree.setup_skill", format!("{:?}", setup_skill.value), setup_skill.layer, Kind::Skill, Scope::Either,
        "The skill a task's Claude runs first to make its worktree runnable: kebbie's own, a skill of yours by name (<skill> or <plugin>:<skill>), or a Markdown file by an absolute or ~ path, whose text the task gets in its role.");
    key("worktree.setup_command", or_unset(setup_command), setup_command.layer, Kind::Text, Scope::Either,
        "The repo's own command that prepares an existing worktree, run from inside it; the setup skill runs it. A path applies only in a repo that has the file; elsewhere, and when none, bin/setup-worktree when the repo has one.");
    key("worktree.dev_server", or_unset(dev_server), dev_server.layer, Kind::Text, Scope::Either,
        &format!("The shell command the board's {server_key} key runs in a server tab of the task's Space, with PORT set to the worktree's port; without one there is no {server_key}."));
    key("worktree.dev_worker", or_unset(dev_worker), dev_worker.layer, Kind::Text, Scope::Either,
        &format!("A second command, such as a job runner, the board's {worker_key} key runs in a worker tab of the task's Space, with the same PORT; without one there is no {worker_key}."));
    key("worktree.dev_url", dev_url.value.as_ref().map_or("\"\" (no link)".to_string(), |url| format!("{url:?}")), dev_url.layer, Kind::Text, Scope::Either,
        "The running dev server's address, shown on the board as a link: {worktree} (the worktree's folder), {repo}, {number} (its pool number) and {port} (its port) are filled in; a worktree without what it names gets no link, and \"\" turns the link off.");
    key("worktree.dev_route", dev_route.value.to_string(), dev_route.layer, Kind::Bool, Scope::Repo,
        "Whether kebbie routes writes a Caddy site for each of this repo's worktrees, at the host its dev_url names, proxied to the worktree's port; for a dev server on a remote machine opened in your laptop's browser. Off unless the repo turns it on; repo config only.");
    key("worktree.stage_on_main", stage_on_main.value.to_string(), stage_on_main.layer, Kind::Bool, Scope::Repo,
        "Whether kebbie try may put a ready task's commits on this repo's local default branch, unpushed, so you can try them from the main checkout before you push main; for a repo you run from its main checkout, such as kebbie's own. Off unless the repo turns it on; repo config only.");
    key("worktree.server_name", format!("{:?}", server_name.value), server_name.layer, Kind::Text, Scope::Machine,
        &format!("The name of the tab the board's {server_key} starts the dev server in, and of the Space's tag in herdr's sidebar while it runs; config.toml only."));
    key("worktree.worker_name", format!("{:?}", worker_name.value), worker_name.layer, Kind::Text, Scope::Machine,
        &format!("The same for the worker tab {worker_key} starts; config.toml only."));
    let cleaned: Vec<&str> = clean_on_release.value.iter().map(String::as_str).collect();
    key("worktree.clean_on_release", quoted(&cleaned), clean_on_release.layer, Kind::List, Scope::Either,
        "Build output kebbie removes from a worktree when it goes back to the pool (done, drop, reset, a landed task closing), so the pool's disk use stays bounded; folders inside the worktree, each removed only when git ignores it and tracks nothing in it, and \"target\" only where the worktree has a Cargo.toml. [] keeps everything.");
    key("worktree.release_command", or_unset(release_command), release_command.layer, Kind::Text, Scope::Either,
        "The repo's own command that gives back what a worktree's setup made outside it, such as its own database copy, run from inside the worktree once it is back on the default branch, so a task's own changes to it never run, when it goes back to the pool (done, drop, reset, a landed task closing), before its build output is removed. Its result shows in the freed line; a failure, or a run past 2 minutes, is reported and the worktree is freed all the same. A path applies only in a worktree that has the file.");
    key("task.rules_skill", format!("{:?}", rules_skill.value), rules_skill.layer, Kind::Skill, Scope::Either,
        "The skill with the rules for how work is planned, built and committed; a task's Claude follows it, the reviewer checks a ready task against it, tidy reshapes commits by it. kebbie's own, a skill of yours by name (<skill> or <plugin>:<skill>), or a Markdown file by an absolute or ~ path, whose text the task and the reviewer get.");
    let own: Vec<&str> = test_commands.value.iter().map(String::as_str).collect();
    key("task.test_commands", quoted(&own), test_commands.layer, Kind::List, Scope::Either,
        &format!("Test commands of this repo that kebbie records when a task's Claude runs them, on top of the built-in {}; matched at the start of the command or of any part of its ; && || chain, after cd, export, VAR=value and wrappers such as timeout and nice. kebbie evidence, the board, the push dialog and the reviewer show the runs, and kebbie names these commands to every agent that runs or asks for tests: the task's role, the reviewer, the assessor, the investigator and the lead.", quoted(&crate::evidence::DEFAULT_COMMANDS)));
    key("task.also_noticed", format!("{:?}", also_noticed.value.key()), also_noticed.layer, Kind::Choice(AlsoNoticed::ALL.map(AlsoNoticed::key).to_vec()), Scope::Either,
        "What a task's Claude does with what it notices outside its task (a bug nearby, a stale doc, a refactor): \"backlog\" writes it out by the skill kebbie:write-backlog-item and files it where new items go, in the vault's backlog or with kebbie backlog add; \"fix\" fixes a small related one in a commit of its own and files the rest; \"ask\" puts each to you with kebbie ask first. Each is listed under Also noticed on the sign-off card, and the reviewer counts an unrelated change under \"backlog\" or \"ask\" as a finding.");
    key("task.permission_mode", or_none(permission_mode.value.as_ref()), permission_mode.layer, Kind::Choice(std::iter::once("none").chain(PERMISSION_MODES).collect()), Scope::Either,
        &format!("The permission mode a task's Claude starts in, one of {}; none leaves it to Claude's own default. kebbie start --mode overrides it for one task, and a restarted task keeps the mode it started in. \"bypassPermissions\" runs every tool without asking, for an isolated machine only; a task still never pushes nor approves its own commits.", quoted(&PERMISSION_MODES)));
    key("task.profile", profile.value.as_ref().map_or(format!("none ({:?})", crate::profiles::TASK), |profile| format!("{profile:?}")), profile.layer, Kind::Profile, Scope::Either,
        &format!("The profile a task starts with when kebbie start names none with --profile: a file in {}/<name>.toml with the agent, model, extra role, skills, permission mode and rules it adds to kebbie's own; none starts the built-in task. A restarted task keeps the profile it started with; kebbie profiles lists them, and the Settings page picks from them.", crate::profiles::dir().display()));
    key("task.sandbox", sandbox.value.to_string(), sandbox.layer, Kind::Bool, Scope::Either,
        &format!("Whether a task's and the reviewer's shell commands run in Claude Code's sandbox: writes only in the worktree, the task's temp folder and {}, network only to {} and a profile's own hosts, no Unix sockets; kebbie's own task commands ({}) run outside it, and kebbie refuses its user-only commands to a sandboxed process. On Linux it needs bubblewrap, socat and the seccomp filter, or kebbie refuses to start tasks; false runs them unsandboxed, where a task can pass as you to kebbie's user-only commands. A profile's [sandbox] enabled wins over it.", crate::sandbox::WRITES.join(", "), crate::sandbox::DOMAINS.join(", "), crate::sandbox::TASK_COMMANDS.join(", ")));
    key("task.steps", steps.value.to_string(), steps.layer, Kind::Bool, Scope::Either,
        "Tasks work in subtasks, each a titled part of the work in small commits, listed with kebbie subtasks before they write code and marked ready with kebbie subtasks done: you accept each subtask and push the accepted run from the top while the task builds on. Off, a task commits its own way and kebbie subtasks refuses. New tasks follow it; a running task once it restarts.");
    key("task.require_accept", require_accept.value.to_string(), require_accept.layer, Kind::Bool, Scope::Either,
        "Whether every commit must be accepted before it can be pushed: kebbie push, the board's push and push in order refuse while a commit they would take is not accepted, and a ready task with one waits under Needs you to have its next subtask accepted. Off, a push takes what is committed and accepting is yours to use or skip. Only with task.steps on; with it off this has no effect.");
    key("review.model", or_unset(review_model), review_model.layer, Kind::Text, Scope::Either,
        "The model the default reviewer (review.profile) runs on, ahead of its profile's model; none leaves it to that profile, then to its agent's default.");
    key("review.profile", format!("{:?}", review_profile.value), review_profile.layer, Kind::ReviewProfile, Scope::Either,
        "The review profile kebbie review runs by default, and whose verdict holds a task's push until it passes: the built-in reviewer, or a profile of yours with kind = \"review\". Only a review profile can be named; the Settings page picks from them.");
    key("review.second", if review_second.value == Second::None { "none".to_string() } else { format!("{:?}", review_second.value.key()) }, review_second.layer, Kind::SecondOpinion, Scope::Either,
        "A second opinion from another review profile: its name offers it with s when you send a task to review (v on the board, kebbie review --second), \"always <profile>\" runs it with every review, none offers none. It never holds the push: its points show on the task's row and go to the lead, which passes them to the task like a review's findings.");
    key("review.auto", format!("{:?}", review_auto.value.key()), review_auto.layer, Kind::Choice(ReviewAuto::ALL.map(ReviewAuto::key).to_vec()), Scope::Either,
        "What kebbie does when the assessor it runs on a ready report judges that the task's work, its pushed subtasks included, needs an independent review: \"suggest\" tells the lead review suggested and its reason and shows it in the review menu, and nothing starts until you send it, or the lead on your word; \"run\" starts the reviewer by itself without holding the push; \"require\" starts it and holds the push, from the ready report on, until it passes. An assessment that gives no answer reads as review not assessed, a suggestion in every mode.");
    key("investigate.profile", format!("{:?}", investigate_profile.value), investigate_profile.layer, Kind::Text, Scope::Either,
        &format!("The profile kebbie's investigator runs with when a task raises an obstacle (kebbie obstacle) or a review questions scope or intent: any profile in {}/<name>.toml or a built-in, whose agent, model, effort and role it takes (a Claude profile with its model and effort, or a Codex one); \"investigator\" is the built-in one, Claude at high effort. It reads the code, the vault and the web, and decides workaround, subtask, task or ask, once per obstacle. none runs no investigator: obstacles go to the lead, which decides them with kebbie obstacles decide.", crate::profiles::dir().display()));
    key("notify.command", or_none(notify_command.value.as_ref()), notify_command.layer, Kind::Text, Scope::Machine,
        "The shell command kebbie runs when something needs you (a task needs you or is ready, a question waits, the lead's summary); KEBBIE_EVENT, KEBBIE_TASK, KEBBIE_REPO and KEBBIE_TEXT describe it, and $1 is the text. None sends no notifications; config.toml only.");
    key("vault.command", or_none(vault_command.value.as_ref()), vault_command.layer, Kind::Text, Scope::Machine,
        "The vault command kebbie runs for task notes, log lines and the board's snapshot, before its own arguments (about --json, task open, log append, ...), such as \"uvx mindin\"; it is kebbie's own, whatever agent runs the task. kebbie setup finds the candidates on this machine and writes your choice. None falls back to the Claude vault plugin's pinned command; config.toml only.");
    key("ci.watch", ci_watch.value.to_string(), ci_watch.layer, Kind::Bool, Scope::Either,
        "Whether kebbie watches GitHub Actions after a push and keeps the task open under CI running until every run for the pushed commit is green; a failed run moves the task to Needs you and its Claude gets the failing job's log. Needs origin on GitHub and gh logged in; false closes the task right after the push. A repo config can turn it off for one repo.");
    key("ci.deploy", or_unset(ci_deploy), ci_deploy.layer, Kind::Text, Scope::Either,
        "The name of the GitHub Actions workflow that deploys this repo, for a task in stages (kebbie stages): a stage whose requirement is deploy is met once a run of this workflow on its pushed commit is green. None takes CI green on the pushed commit as the deploy. Needs ci.watch on.");
    key("ci.deploy_window", or_none(ci_deploy_window.value.as_ref()), ci_deploy_window.layer, Kind::Text, Scope::Repo,
        "When the repo's deploys may run, such as \"mon-fri 08:00-17:00\", in this machine's time zone, or in UTC with UTC after it. A run GitHub cancels while the window is shut keeps the task under CI running, and in the window's next opening kebbie re-runs it (gh run rerun) and closes the task once it is green; a newer push on the branch is followed instead. A run cancelled while the window stayed open goes to Needs you, as without the key. Needs ci.watch on; repo config only.");
    key("tests.per_commit", tests_per_commit.value.to_string(), tests_per_commit.layer, Kind::Bool, Scope::Either,
        "Whether kebbie tests each of a task's unpushed commits on its own once the task reports ready: the keeper runs the test commands the task's own recorded test runs used (else task.test_commands) on one commit at a time, in a throwaway worktree of kebbie's, and remembers each commit's result by its sha, so a commit is tested once. A commit the task's own recorded runs already tested is not run again, and a task in the sandbox gets only those. The throwaway worktree is first prepared with the repo's setup (worktree.setup_command, else bin/setup-worktree), and a setup that fails leaves the commits not tested. The push order on the board and the phone marks each commit passed or failed; nothing waits on it. Left at its default it runs only in a repo with such a setup; true runs it without one, false runs none. With a slow suite this costs a suite run per commit.");
    key("tests.before_push", format!("{:?}", tests_before_push.value.key()), tests_before_push.layer, Kind::Choice(BeforePush::ALL.map(BeforePush::key).to_vec()), Scope::Either,
        "Which tests must have passed before a task reports ready and before any push of it, from the board, the lead or your phone: \"related\" leaves it to the task, which runs the tests its change touches; \"full\" also refuses both until kebbie recorded a green run of tests.full_command on the task's HEAD, and the refusal names the command to run. A repo without tests.full_command stays on related. kebbie doctor suggests full for a repo whose recorded full runs are short.");
    key("tests.full_command", or_none(tests_full_command.value.as_ref()), tests_full_command.layer, Kind::Text, Scope::Repo,
        "The command that runs this repo's whole test suite, such as \"cargo test --locked\" or \"uv run pytest -q\", as CI runs it; under tests.before_push = \"full\" a run of it, with flags of its own at most, counts as the full run, and kebbie records it as a test run whether or not task.test_commands lists it; repo config only.");
    key("tests.lint_command", or_none(tests_lint_command.value.as_ref()), tests_lint_command.layer, Kind::Text, Scope::Repo,
        "The command that lints this repo as CI does, such as \"cargo clippy --locked --all-targets -- -D warnings\": once set, a task's ready report and every push of it, from the board, the lead or your phone, wait until kebbie recorded a green run of exactly this command on the task's HEAD, also after the push line rebased it, whatever tests.before_push says; kebbie records its runs beside the test runs, and they never count as a test run; repo config only.");
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
        "Whether kebbie restarts the Claude sessions and Codex tasks that run older code than is installed, after kebbie updates or a new Claude Code, Codex, plugin or MCP config is installed; no other session is ever restarted. Each resumes its conversation with claude --resume or codex resume in its own pane, keeping its flags, and kebbie update restarts the idle ones at once. false restarts nothing on its own: the board still marks those sessions, and kebbie restart restarts them when you want. It also starts the lead fresh with its own handover once its context passes restart.lead_context; config.toml only.");
    key("restart.idle_minutes", idle_minutes.value.to_string(), idle_minutes.layer, Kind::Number, Scope::Machine,
        "How long a session that runs older code than is installed waits to be idle and out of focus before kebbie restarts it; a session mid-turn, the pane you are in and one with background work running are never restarted on their own; config.toml only.");
    key("restart.lead_context", lead_context.value.to_string(), lead_context.layer, Kind::Number, Scope::Machine,
        "How full the lead's context may get, in percent of its model's window, before kebbie has it write a handover and starts it fresh to read it, as the lead never compacts its conversation on its own. It waits until the lead is idle and you are out of its pane, as restart.auto does, and the board's Work page offers L from this share on; config.toml only.");
    key("keeper.enabled", keeper.value.to_string(), keeper.layer, Kind::Bool, Scope::Machine,
        "Whether one background kebbie keeper per machine runs the routines (stale-session restarts, the update check, held messages, delivery checks of typed prompts, scheduled [[routines]]), so they go on with the board closed; herdr's startup, the board and kebbie update start it. false runs them in an open board instead; config.toml only.");
    key("worker.enabled", worker.value.to_string(), worker.layer, Kind::Bool, Scope::Machine,
        "Whether this machine runs jobs for the other machines of its hub account: its keeper tells the hub it is a worker and runs, one at a time and in your account, the commands their kebbie job run sends, on the worktree each sends along, in a folder per task under ~/.local/share/kebbie/worker, and takes their kebbie run --on commands over a direct link: its keeper listens on port 47011 on its Tailscale and WireGuard addresses and on loopback only, and is rung through the hub's relay otherwise. Turning it on is your yes to every machine on the account's signed list. config.toml only.");
    key("link.networks", quoted(&link_networks.value.iter().map(|network| network.key()).collect::<Vec<_>>()), link_networks.layer, Kind::List, Scope::Machine,
        "The private networks kebbie run --on links over, in the order to try them: \"tailscale\" and \"wireguard\". On a worker, its keeper listens on and announces only the addresses on the networks listed; on the machine that runs kebbie run, it dials the listed networks in this order, each next one 150 ms after the one before unless that one failed first, and never an address on a network left out. [] links only through the hub's relay. config.toml only.");
    key("builds.slots", build_slots.value.to_string(), build_slots.layer, Kind::Number, Scope::Machine,
        "How many task builds run at once on this machine, 1 to 64. kebbie puts a guard before each command in builds.commands on a task's PATH: a build takes a slot, and a task that finds every slot taken is told its place in line and its command exits; the keeper tells the next in line when a slot frees and holds the slot 3 minutes for it, then passes it on; a build that holds its slot 30 minutes while others wait loses it. kebbie update goes first in line and waits at most 10 minutes, the board shows \"waiting for a build slot, 2nd\" on a waiting task's row, and the guard runs cargo with sccache as its RUSTC_WRAPPER when sccache is on PATH and no wrapper is set, so a machine compiles each dependency once. Without a keeper running, a build that finds no slot free for it goes ahead without waiting; config.toml only.");
    key("builds.commands", quoted(&build_commands.value.iter().map(String::as_str).collect::<Vec<_>>()), build_commands.layer, Kind::List, Scope::Machine,
        "The commands that take a build slot, each a program and the words its arguments start with, such as \"xcodebuild\" or \"npm run build\". \"cargo\" counts only the cargo commands that compile: build, check, test, clippy, bench, doc, install, rustc, rustdoc, fix, nextest and llvm-cov; cargo run takes none, as what it runs may never end. A task started after a change has the new list; config.toml only.");
    key("disk.trim_below", format!("{:?}", disk_trim_below.value.shown()), disk_trim_below.layer, Kind::Text, Scope::Machine,
        "Free space, as \"30 GB\" or \"15%\" of the disk, below which the keeper, on Linux only, trims build output (worktree.clean_on_release) from the worktrees nobody uses on that disk, every 2 minutes; it watches the disks under the worktrees and under sccache's cache (SCCACHE_DIR, or ~/.cache/sccache), whose cache it never trims. The order: free pool worktrees first, then tasks waiting for a push, then tasks whose Claude has finished, each only while its Claude is not working, it is not building or waiting for a build slot, and nothing runs inside it but its agent, the agent's shells and the tools the agent starts itself (a dev server run through a shell keeps it); it stops once every watched disk is above the limit again. When that is not enough, or on macOS, where kebbie cannot see which processes use a worktree and trims nothing, the board shows a line under Needs you naming the disk and its free space; config.toml only.");
    key("disk.builds_below", format!("{:?}", disk_builds_below.value.shown()), disk_builds_below.layer, Kind::Text, Scope::Machine,
        "Free space, as \"10 GB\" or \"5%\", below which a task's build takes no build slot and does not run: the guard says how much is free and exits with status 75, and a task in line whose own disk is below the limit gets no build turn, keeping its place while the turn goes to the next task whose disk has room. Below disk.trim_below when both use the same unit; config.toml only.");
    key("recover.enabled", recover.value.to_string(), recover.layer, Kind::Bool, Scope::Machine,
        "Whether kebbie resumes a task whose Claude an API error stopped: after overload, a server error, a rate limit or a dropped connection it says carry on after 1, 2, 4 and 8 minutes; after a usage limit, once the allowance resets. A login, billing, invalid request or refusal error goes to you under Needs you at once, as does any error that stops a Codex task. false leaves every such task idle; config.toml only.");
    key("machine.name", format!("{:?}", machine_name.value), machine_name.layer, Kind::Text, Scope::Machine,
        "The machine's one name: your phone shows it through the hub, and kebbie claims vault notes under it; kebbie hub connect and kebbie hub join use it, set it from --name or ask for it when unset. A task started from a vault note writes it with the task's name into the note's kebbie frontmatter key, and kebbie on another machine sharing the vault shows the note as worked on at this name and asks before starting it; the default is the host's short name. config.toml only.");
    key("hub.url", format!("{:?}", hub_url.value), hub_url.layer, Kind::Text, Scope::Machine,
        "The hub kebbie hub connect and the board's hub window connect this machine to: the kebbie hub unless set, or your own kebbie hub at an https address (http only on this machine). Choosing your own hub there sets it; the hub a machine is connected to stays in kebbie's state with its keys, so changing this key later moves nothing (kebbie hub move does). config.toml only.");
    key("hub.commands", quoted(&hub_commands.value.keys()), hub_commands.layer, Kind::List, Scope::Machine,
        "What this machine takes from your devices through the hub, as a list of kinds: \"answer\" takes answers to a task's questions and to a backlog item's open questions; \"push\" pushes a task; \"prompt\" picks a choice on an agent's prompt; \"idea\" adds an idea to the backlog, as kebbie backlog add does; \"start\" starts a backlog item as a task, as g on the board does; \"edit\" changes a backlog item's title or one of its sections; \"review\" opens a task's step for review, sends line comments to its Claude, marks files read and accepts the step; \"lead\" takes your messages to the lead, typed into its chat as your own word, and switches away mode. Unset, it takes all eight; [] takes nothing, and the board still posts. The earlier single words still read: \"answer-and-push\" is [\"answer\", \"push\", \"prompt\"], \"answer\" is [\"answer\", \"prompt\"] and \"off\" is []. A command counts only from a device hub.trust lets in. config.toml only.");
    key("hub.trust", format!("{:?}", hub_trust.value.key()), hub_trust.layer, Kind::Choice(HubTrust::ALL.map(HubTrust::key).to_vec()), Scope::Machine,
        "Which of your devices this machine takes commands from: \"device\" takes them from each device you approve on this machine, and a new device's first command waits here until you approve it (N on the board, or kebbie hub devices approve <name>) or hub.auto_approve lets it in; \"account\" takes them from any device signed in to the account this machine is connected to, with nothing to approve. Every command names the device that sent it, and kebbie hub devices lists them. config.toml only.");
    key("hub.auto_approve", format!("{:?}", hub_auto_approve.value.key()), hub_auto_approve.layer, Kind::Choice(AutoApprove::ALL.map(AutoApprove::key).to_vec()), Scope::Machine,
        "What happens to a device that waits for your approval under hub.trust = \"device\": \"24h\" approves it 24 hours after its first command unless you deny it first; \"never\" keeps it waiting until you approve it. A command waits at most its hour, so one sent before the approval expires, and the device says it was not delivered. config.toml only.");
    key("hub.approve_from", format!("{:?}", hub_approve_from.value.key()), hub_approve_from.layer, Kind::Choice(ApproveFrom::ALL.map(ApproveFrom::key).to_vec()), Scope::Machine,
        "Where a new device that waits for your approval under hub.trust = \"device\" can be approved: \"devices\" lets a device you approved on this machine approve or deny it too, from the notice that says it is waiting or from the phone's board, as well as N on the board or kebbie hub devices; \"machine\" takes that decision only here, and the phone shows the waiting device without the buttons. Either way a waiting device never approves another. config.toml only.");
    key("report.signals", report_signals.value.to_string(), report_signals.layer, Kind::Bool, Scope::Machine,
        "Whether kebbie sends its maintainers error signals, counts of kebbie's own failures: when kebbie itself fails (a panic, and some internal errors), the keeper sends at most once a day a count per failure with only kebbie's version and commit, the OS and CPU architecture, the component (a fixed name from kebbie's code, such as keeper.push) and the error kind (panic, internal-error, corrupt-state). Never an error message, path, repo, task or host name. Once a day it also sends an install signal: a random id made for this install alone, kebbie's version, the OS and CPU architecture and the update channel. Each error signal is a line under messages on the board. Not set, nothing is sent and no install id is made; kebbie setup asks. true turns both on; false sends none, drops what was counted and deletes the install id; config.toml only.");
    key("report.text", format!("{:?}", report_text.value.key()), report_text.layer, Kind::Choice(ReportText::ALL.map(ReportText::key).to_vec()), Scope::Machine,
        "What happens to a bug or wish about kebbie, once you or the lead saved it, with its text redacted (paths under your home as ~, no repo names, remote URLs or tokens): \"ask\" shows it on the board as it would go to kebbie's maintainers and only your yes sends it; \"always\" sends it without asking, and says so under messages, once you arm it with kebbie issue report --always in your own terminal (until then it asks); \"never\" keeps it on this machine, though kebbie issue report <id> still sends one when you ask. Signed with this install's key; config.toml only.");
    key("report.inbox_token", if report_inbox.value { "set".to_string() } else { "none".to_string() }, report_inbox.layer, Kind::Secret, Scope::Machine,
        "For kebbie's maintainers: the kha_ token the issues endpoint issued. With it the keeper fetches the reports the endpoint lets through every 10 minutes, files each as a kebbie issue and acknowledges it, and kebbie doctor counts the ones waiting in quarantine; never in a repo config, and never shown.");
    key("issues.propose", propose_issues.value.to_string(), propose_issues.layer, Kind::Bool, Scope::Machine,
        "Whether the lead saves a bug or wish about kebbie by itself when kebbie itself cost something (an error, a workaround, a missing capability), with no question to you; false keeps the lead from filing them, and it files one only when you ask it to. Takes effect when the lead next restarts (L on the board); config.toml only.");
    key("lead.profile", lead_profile.value.as_ref().map_or("none (Claude Code's own model)".to_string(), |profile| format!("{profile:?}")), lead_profile.layer, Kind::LeadProfile, Scope::Machine,
        &format!("The profile the lead runs with: a Claude profile in {}/<name>.toml whose model and effort the lead's Claude starts with, so the lead can run another model than your other Claude sessions; its role, skills and rules are a task's and stay off the lead. none starts it on the model and effort your Claude Code settings name. Takes effect when the lead starts or restarts (L on the board, kebbie lead --restart, or the restart after an update); kebbie profiles lists them, and the Settings page picks from them. config.toml only.", crate::profiles::dir().display()));
    key("writer.profile", format!("{:?}", writer_profile.value), writer_profile.layer, Kind::Text, Scope::Machine,
        &format!("The profile kebbie's writer runs with when the lead is asked to write out a backlog item (an idea from your phone, a short item from kebbie backlog add or n, Write it out): any profile in {}/<name>.toml or a built-in, whose agent, model and effort it takes. It researches the code, the notes and the web and writes the item out apart from the lead's chat; the lead reviews it and its open questions, then saves it. \"writer\" is the built-in one, Claude at medium effort. none has the lead write it out in its own chat. config.toml only.", crate::profiles::dir().display()));
    key("hub.ideas_machine", ideas_machine.value.as_ref().map_or("none".to_string(), |name| format!("{name:?}")), ideas_machine.layer, Kind::Text, Scope::Machine,
        "The machine.name of the machine that writes out new backlog items, from your phone and, when the backlog is the vault, from kebbie backlog add and n on your other machines; an item whose repo has a checkout on only one machine goes there instead, and one it has not taken within 5 minutes is added where you added it. The phone's New idea sends there while it is awake, and to an awake machine while it sleeps. An item you asked to start once it is written starts on the machine it goes to; a task already running stays where it started. Set it on that machine, or the same on all. config.toml only.");
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
