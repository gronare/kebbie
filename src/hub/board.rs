use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;

use kebbie_hub_protocol::{Agent, AgentStatus, Board, Checkout, Ci, CiState, Counts, Git, Group, Item, Later, LaterState, NextPart, NextRow, Prompt, PromptOption, PushPermission, Question, QuestionOption, Recover, Report, RoutineSummary, SecondOpinion, Step, StepState, Steps, Verdict, VerdictOutcome};

use super::blobs::{Attachment, PLAN_LIMIT};
use crate::menu::Menu;
use crate::next::{self, Part};
use crate::status::{self, Surface};

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

// What the board reads beyond the items: the menu on each blocked agent's screen by address, each profile's model by name, and the repos whose steps must be accepted.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    pub menus: BTreeMap<String, Menu>,
    pub models: BTreeMap<String, String>,
    pub home: Option<String>,
    pub accepting: BTreeSet<String>,
}

fn group(group: status::Group) -> Option<Group> {
    Some(match group {
        status::Group::NeedsYou => Group::NeedsYou,
        status::Group::Working => Group::Working,
        status::Group::InReview => Group::ToReview,
        status::Group::ReadyToPush => Group::ReadyToPush,
        status::Group::CiRunning => Group::CiRunning,
        status::Group::Idle => Group::Idle,
        status::Group::Queued | status::Group::Parked => return None,
    })
}

fn agent(item: &status::Item, seen: &Seen) -> Option<Agent> {
    let running = item.agent.as_ref()?;
    let status = match running.status.as_str() {
        "working" => AgentStatus::Working,
        "blocked" => AgentStatus::Blocked,
        "idle" => AgentStatus::Idle,
        "done" => AgentStatus::Done,
        _ => return None,
    };
    let model = item.profile.as_ref().and_then(|profile| seen.models.get(profile)).cloned();
    Some(Agent { status, kind: Some(running.kind.key().to_string()), model })
}

fn report(item: &status::Item) -> Option<Report> {
    let message = item.report.as_ref()?;
    let report = message.report.as_ref()?;
    Some(Report { state: report.state.clone(), next: message.next_for_you().map(str::to_string), summary: message.text.clone(), at: message.at })
}

// The plan reaches the phone as a blob the relay attaches, so its path or vault key stays on the machine.
fn question(item: &status::Item) -> Option<Question> {
    let message = item.question.as_ref()?;
    let asked = message.question.as_ref()?;
    Some(Question {
        id: message.id.clone(),
        text: message.text.clone(),
        options: asked.options.iter().enumerate().map(|(index, (label, meaning))| QuestionOption { label: label.clone(), meaning: meaning.clone(), comment: asked.comment.contains(&index) }).collect(),
        recommend: u8::try_from(asked.recommend).ok().filter(|recommend| *recommend > 0),
        kept_for_user: asked.waits,
        plan: None,
        plan_at: None,
        at: message.at,
    })
}

// A screen shows the task's paths and the user's home; the phone gets them relative, with kebbie's own path as its name.
fn local_paths_out(text: &str, item: &status::Item, seen: &Seen) -> String {
    let mut text = text.to_string();
    if let Ok(exe) = std::env::current_exe() {
        text = text.replace(&exe.to_string_lossy().to_string(), "kebbie");
    }
    for root in [item.path.as_ref(), item.repo.as_ref()].into_iter().flatten() {
        let root = root.to_string_lossy().to_string();
        text = text.replace(&format!("{root}/"), "").replace(&root, ".");
    }
    if let Some(home) = seen.home.as_deref().filter(|home| home.len() > 1) {
        text = text.replace(&format!("{}/", home.trim_end_matches('/')), "~/");
    }
    text
}

// The prompt carries the id kebbie choose takes, worked out from the screen as read, before any path leaves it.
fn prompt(item: &status::Item, address: &str, seen: &Seen) -> Option<Prompt> {
    let running = item.agent.as_ref().filter(|agent| agent.status == "blocked")?;
    let menu = seen.menus.get(address)?;
    let options = menu.options.iter().enumerate().map(|(index, label)| PromptOption {
        label: local_paths_out(label, item, seen),
        takes_text: crate::menu::takes_text(label),
        key: Some(crate::menu::key_of(label, running.kind).map_or_else(|| (index + 1).to_string(), str::to_string)),
    }).collect();
    Some(Prompt { id: crate::menu::fingerprint(address, menu), question: local_paths_out(&menu.question, item, seen), options })
}

fn git(item: &status::Item) -> Option<Git> {
    let git = item.git.as_ref()?;
    Some(Git { upstream: git.upstream.trim_start_matches("origin/").to_string(), ahead: count(git.ahead), behind: count(git.behind), uncommitted: count(git.uncommitted), approved: git.approved })
}

fn verdict(item: &status::Item) -> Option<Verdict> {
    let shown = item.verdict.as_ref()?;
    let verdict = shown.verdict.as_ref();
    Some(Verdict {
        verdict: verdict.map(|verdict| if verdict.ok { VerdictOutcome::Ok } else { VerdictOutcome::Changes }),
        findings: verdict.map(|verdict| count(verdict.findings)),
        sha: verdict.map(|verdict| verdict.sha.clone()),
        stale: shown.stale,
        running: shown.running,
        held: Some(shown.held),
    })
}

fn ci(item: &status::Item) -> Option<Ci> {
    Some(match &item.ci.as_ref()?.state {
        crate::ci::State::Running => Ci { state: CiState::Running, failure: None },
        crate::ci::State::Failed { what, .. } => Ci { state: CiState::Failed, failure: Some(what.clone()) },
    })
}

fn steps(item: &status::Item, seen: &Seen) -> Option<Steps> {
    let steps = &item.steps;
    if !steps.listed() {
        return None;
    }
    let items = steps.items.iter().map(|step| {
        let (state, sha) = match &step.state {
            crate::tasks::StepState::Planned => (StepState::Planned, None),
            crate::tasks::StepState::InProgress => (StepState::InProgress, None),
            crate::tasks::StepState::Committed { sha, accepted: false, .. } => (StepState::Committed, Some(sha.clone())),
            crate::tasks::StepState::Committed { sha, accepted: true, .. } => (StepState::Accepted, Some(sha.clone())),
        };
        Step { number: count(step.index + 1), title: step.title.clone(), state, sha }
    }).collect();
    Some(Steps { pushed: count(steps.pushed), items, required: Some(seen.accepting.contains(&item.place)) })
}

// Only what the protocol allow-lists: no path, pane, Space, dev server address or host leaves the machine.
pub fn item(item: &status::Item, seen: &Seen) -> Option<Item> {
    if !item.is_task() {
        return None;
    }
    let group = group(item.group())?;
    let address = crate::peers::address(&item.name, &item.place);
    Some(Item {
        group,
        urgency: item.urgency(),
        summary: item.summary_on(Surface::Phone),
        next_for_you: item.next_for_you().map(str::to_string),
        agent: agent(item, seen),
        report: report(item),
        question: question(item),
        prompt: prompt(item, &address, seen),
        git: git(item),
        head: item.git.as_ref().and_then(|git| git.head.clone()),
        verdict: verdict(item),
        push_permission: item.permit.map(|permit| match permit { crate::permit::Permit::Task => PushPermission::Task, crate::permit::Permit::Repo => PushPermission::Repo }),
        push_waits: item.push_note(),
        follows_main: item.follows,
        ci: ci(item),
        recover: item.recover_note().map(|note| Recover { note }),
        routine: item.routine.clone(),
        card: None,
        steps: steps(item, seen),
        title: item.title.clone(),
        profile: item.profile.clone(),
        recipe: item.recipe.as_ref().map(|recipe| crate::recipe::shown_name(&recipe.name).to_string()),
        assessment: item.assessment.as_ref().map(crate::assessment::Word::text),
        second_opinion: item.second.as_ref().map(|second| SecondOpinion { by: second.by.clone(), points: second.points.map(count), running: second.running }),
        address,
    })
}

fn line(row: &next::Row) -> String {
    let said = match row.part {
        Part::Push => return row.what.clone(),
        Part::Stopped => row.short.clone(),
        Part::Read | Part::Device => row.what.clone(),
    };
    [Some(said), next::waits_wide(&row.waits)].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

// The rows N and kebbie next show, in their order, worked out by the same function.
pub fn next_rows(items: &[status::Item]) -> (Vec<NextRow>, Counts) {
    let next = next::of(items);
    let rows = next.rows.iter().filter_map(|row| Some((row, match row.part { Part::Stopped => NextPart::Stopped, Part::Push => NextPart::Push, Part::Read => NextPart::Read, Part::Device => return None }))).enumerate().map(|(at, (row, part))| NextRow {
        part,
        address: row.address.clone(),
        rank: count(at + 1),
        line: line(row),
        since: row.since,
        waits_behind: count(row.waits.len()),
    }).collect();
    (rows, Counts { working: count(next.working), queued: count(next.queued), parked: count(next.parked) })
}

// Queued and parked tasks leave the groups; the phone lists them apart, a parked one with whether a question waits on it.
fn later(item: &status::Item) -> Option<Later> {
    if !item.is_task() {
        return None;
    }
    let state = match item.group() {
        status::Group::Queued => LaterState::Queued,
        status::Group::Parked => LaterState::Parked,
        _ => return None,
    };
    let line = match state {
        LaterState::Queued => item.queued_note().unwrap_or_default(),
        LaterState::Parked => item.summary_on(Surface::Phone),
    };
    let asks = item.question.is_some() || item.agent.as_ref().is_some_and(|agent| agent.status == "blocked");
    Some(Later { address: crate::peers::address(&item.name, &item.place), title: item.title.clone(), state, line, until: item.parked.and_then(|parked| parked.until), question: asks })
}

pub fn routines(state: &Path, now: u64) -> Vec<RoutineSummary> {
    let offset = crate::routines::local_offset(now);
    crate::routines::load().0.into_iter().map(|routine| {
        let kept = crate::routines::load_state(state, &routine.id);
        let next_run = Some(crate::routines::next_run(routine.schedule, &kept, now, offset)).filter(|at| routine.enabled && *at > now);
        RoutineSummary { id: routine.id, title: routine.title, repo: routine.repo, mode: routine.mode.key().to_string(), next_run, last_outcome: kept.last_outcome.map(|outcome| outcome.key().to_string()) }
    }).collect()
}

// The board's revision is the relay's to set when it seals the board.
pub fn build(items: &[status::Item], checkouts: &[status::Checkout], routines: Vec<RoutineSummary>, away_since: Option<u64>, seen: &Seen) -> Board {
    let (next, counts) = next_rows(items);
    Board {
        rev: 0,
        kebbie: env!("CARGO_PKG_VERSION").to_string(),
        away_since,
        checkouts: checkouts.iter().map(|checkout| Checkout { repo: checkout.name.clone(), free_worktrees: count(checkout.free_slots), busy_worktrees: count(checkout.busy_slots) }).collect(),
        routines,
        items: items.iter().filter_map(|shown| item(shown, seen)).collect(),
        sessions_outside_tasks: count(items.iter().filter(|item| !item.is_task()).count()),
        next: Some(next),
        counts: Some(counts),
        later: Some(items.iter().filter_map(later).collect()),
        accepts: Some(super::commands::accepts(crate::config::hub_commands())),
        devices: None,
        backlog: None,
    }
}

// A screen that cannot be read leaves that task's prompt off the board this time.
fn seen(items: &[status::Item]) -> Seen {
    let menus = items.iter().filter(|item| item.is_task()).filter_map(|item| {
        let agent = item.agent.as_ref().filter(|agent| agent.status == "blocked")?;
        let screen = crate::herdr::read_agent(&agent.pane).ok()?;
        Some((crate::peers::address(&item.name, &item.place), crate::menu::parse_for(&screen, agent.kind)?))
    }).collect();
    let models = crate::profiles::load_all_in(&crate::profiles::dir()).0.into_iter().filter_map(|profile| Some((profile.name, profile.model?))).collect();
    let accepting = items.iter().filter_map(|item| Some((item.place.clone(), item.repo.as_ref()?))).filter(|(_, repo)| crate::tasks::accept_required(repo)).map(|(place, _)| place).collect();
    Seen { menus, models, home: std::env::var("HOME").ok(), accepting }
}

// HEAD still makes the change the card was written for, by the push dialog's rule; the answer is kept per (card, HEAD) so the board loop runs git only when either moves.
fn card_current(item: &status::Item, sha: &str) -> bool {
    static KNOWN: Mutex<BTreeMap<(String, String), bool>> = Mutex::new(BTreeMap::new());
    let Some((git, head)) = item.git.as_ref().and_then(|git| Some((git, git.head.as_deref()?))) else { return false };
    if head == sha {
        return true;
    }
    let Some(path) = item.path.as_ref() else { return false };
    let key = (sha.to_string(), head.to_string());
    if let Some(known) = KNOWN.lock().ok().and_then(|known| known.get(&key).copied()) {
        return known;
    }
    let current = crate::tasks::same_code(path, &git.upstream, sha, head);
    if let Ok(mut known) = KNOWN.lock() {
        if known.len() >= 256 {
            known.clear();
        }
        known.insert(key, current);
    }
    current
}

pub fn attachments(items: &[status::Item], state: &Path) -> Vec<Attachment> {
    items.iter().filter(|item| item.is_task() && group(item.group()).is_some()).flat_map(|item| {
        let address = crate::peers::address(&item.name, &item.place);
        let asked = item.question.as_ref().and_then(|message| message.question.as_ref()).filter(|asked| asked.plan.is_some());
        let at = asked.and_then(|asked| asked.plan_at).and_then(|millis| u64::try_from(millis / 1000).ok());
        let plan = asked.and_then(crate::questions::plan_text).filter(|text| !text.trim().is_empty() && text.len() <= PLAN_LIMIT).map(|text| Attachment::Plan { address: address.clone(), text, at });
        let card = crate::report::card_in(state, &address).filter(|card| !card.text.is_empty() && card.text.len() <= PLAN_LIMIT).map(|card| {
            let current = card_current(item, &card.sha);
            Attachment::Card { address: address.clone(), text: card.text, sha: card.sha, current }
        });
        [plan, card].into_iter().flatten()
    }).collect()
}

pub fn gather(state: &Path, now: u64) -> crate::sh::Result<(Board, Vec<Attachment>)> {
    let items = status::gather()?;
    let checkouts = status::checkouts()?;
    let mut board = build(&items, &checkouts, routines(state, now), crate::permit::away_since_in(state), &seen(&items));
    let (backlog, bodies) = super::backlog::gather(state);
    board.backlog = Some(backlog);
    Ok((board, attachments(&items, state).into_iter().chain(bodies).collect()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::messages::{Message, Question as Asked};
    use crate::phase::Phase;
    use crate::status::tests::{in_phase, task};

    pub(crate) fn asking(name: &str, place: &str) -> status::Item {
        let mut item = task(Some("idle"), Some("needs-you"), 1);
        item.name = name.to_string();
        item.place = place.to_string();
        item.question = Some(Message {
            id: "1761069540-4411-0-question".to_string(), at: 1_761_069_540, from: format!("{name}@{place}"), to: "you".to_string(), text: "Keep the old endpoint for a release?".to_string(), held: false, report: None, ended: false,
            question: Some(Asked { options: vec![("Keep it".to_string(), "deprecate now".to_string()), ("Remove it".to_string(), "drop it in this task".to_string())], recommend: 2, plan: Some("/Users/someone/plans/refunds.md".to_string()), plan_at: None, comment: Vec::new(), answered: false, waits: false, relayed: None }),
            about: None, kind: None, landed: None, why: None, forced: false, fyi: false,
        });
        in_phase(&mut item, Phase::NeedsYou(crate::phase::Why::Question));
        item
    }

    #[test]
    fn a_task_reaches_the_board_with_its_question_and_nothing_local() {
        let mut item = asking("refunds", "shop");
        item.path = Some("/Users/someone/projects/wt3.shop".into());
        item.repo = Some("/Users/someone/projects/shop".into());
        item.workspace = Some("w_secret_space".to_string());
        item.url = Some("http://localhost:3003".to_string());
        item.agent.as_mut().unwrap().pane = "w1:p9".to_string();
        item.git.as_mut().unwrap().head = Some("9f3c2a1e5b7d4c6a8b0e2f4d6c8a0b2e4f6d8c0a".to_string());
        let board = build(&[item], &[], Vec::new(), None, &Seen::default());
        let shown = &board.items[0];
        assert_eq!(shown.address, "refunds@shop");
        assert_eq!(shown.group, Group::NeedsYou);
        let question = shown.question.as_ref().unwrap();
        assert_eq!((question.text.as_str(), question.options.len(), question.recommend), ("Keep the old endpoint for a release?", 2, Some(2)));
        assert_eq!(question.options[1].label, "Remove it");
        assert!(!question.options[0].comment && !question.options[1].comment, "neither option takes a comment");
        assert!(question.plan.is_none(), "a plan path stays on the machine");
        assert_eq!(shown.git.as_ref().unwrap().upstream, "main");
        let text = serde_json::to_string(&board).unwrap();
        for local in ["/Users/someone", "w_secret_space", "localhost:3003", "w1:p9"] {
            assert!(!text.contains(local), "{local} leaked: {text}");
        }
    }

    #[test]
    fn an_option_that_takes_the_users_comment_tells_the_phone_so() {
        let mut item = asking("refunds", "shop");
        item.question.as_mut().unwrap().question.as_mut().unwrap().comment = vec![1];
        let board = build(&[item], &[], Vec::new(), None, &Seen::default());
        let options = &board.items[0].question.as_ref().unwrap().options;
        assert_eq!(options.iter().map(|option| option.comment).collect::<Vec<_>>(), [false, true], "the phone asks for a comment exactly where kebbie refuses an answer without one");
    }

    #[test]
    fn a_task_with_steps_reaches_the_board_with_each_step_and_a_task_without_them_carries_none() {
        use crate::tasks::{Step as Listed, StepState as Listing, Steps as Listings};
        let committed = |sha: &str, accepted: bool| Listing::Committed { sha: sha.to_string(), subject: "Route charges through the gateway".to_string(), accepted };
        let mut item = task(Some("working"), Some("working"), 2);
        item.name = "stripe".to_string();
        item.place = "billing".to_string();
        item.steps = Listings { pushed: 1, items: vec![
            Listed { index: 1, title: "Charge through a PaymentGateway".to_string(), state: committed("2222222bbbb", true), extra: false },
            Listed { index: 2, title: "Add Stripe as the gateway".to_string(), state: committed("3333333cccc", false), extra: false },
            Listed { index: 3, title: "Remove the old payment client".to_string(), state: Listing::InProgress, extra: false },
            Listed { index: 4, title: "Docs".to_string(), state: Listing::Planned, extra: false },
        ] };
        let mut plain = task(Some("working"), Some("working"), 2);
        plain.name = "search".to_string();
        let mut pushed_only = plain.clone();
        pushed_only.name = "dates".to_string();
        pushed_only.steps = Listings { pushed: 2, items: Vec::new() };
        let board = build(&[item, plain, pushed_only], &[], Vec::new(), None, &Seen::default());
        let steps = board.items[0].steps.as_ref().unwrap();
        assert_eq!(steps.pushed, 1);
        let shown: Vec<(u32, &str, StepState, Option<&str>)> = steps.items.iter().map(|step| (step.number, step.title.as_str(), step.state, step.sha.as_deref())).collect();
        assert_eq!(shown, [
            (2, "Charge through a PaymentGateway", StepState::Accepted, Some("2222222bbbb")),
            (3, "Add Stripe as the gateway", StepState::Committed, Some("3333333cccc")),
            (4, "Remove the old payment client", StepState::InProgress, None),
            (5, "Docs", StepState::Planned, None),
        ]);
        assert!(!serde_json::to_string(&board).unwrap().contains("Route charges"), "a commit's subject stays on the machine");
        assert_eq!(board.items[1].steps, None);
        assert!(!serde_json::to_string(&board.items[1]).unwrap().contains("steps"), "a task without steps sends the board an older hub reads");
        assert_eq!(board.items[2].steps.as_ref().map(|steps| (steps.pushed, steps.items.len())), Some((2, 0)));
    }

    #[test]
    fn sessions_outside_tasks_are_counted_and_queued_tasks_wait_off_the_board() {
        let mut session = task(Some("working"), None, 0);
        session.git = None;
        let mut queued = task(None, None, 0);
        queued.git = None;
        queued.queued = Some(status::Queued { after: Vec::new(), blocked: None, waiting: "waits for 18:00".to_string(), names: Vec::new(), base: "main".to_string(), at: None, backlog: false });
        let mut blocked = queued.clone();
        blocked.name = "rounding".to_string();
        blocked.queued.as_mut().unwrap().blocked = Some("refunds was dropped".to_string());
        let mut working = task(Some("working"), Some("working"), 2);
        working.name = "search".to_string();
        let board = build(&[session, queued, blocked, working], &[status::Checkout { name: "web".to_string(), changed: 3, free_slots: 2, busy_slots: 4 }], Vec::new(), Some(7), &Seen::default());
        assert_eq!(board.sessions_outside_tasks, 1);
        assert_eq!(board.items.iter().map(|item| (item.address.as_str(), item.group)).collect::<Vec<_>>(), [("rounding@web", Group::NeedsYou), ("search@web", Group::Working)]);
        assert_eq!((board.checkouts[0].repo.as_str(), board.checkouts[0].free_worktrees, board.checkouts[0].busy_worktrees), ("web", 2, 4));
        assert_eq!(board.away_since, Some(7));
    }

    #[test]
    fn next_on_the_board_is_kebbie_nexts_order_with_its_counts_and_no_board_keys() {
        use crate::next::tests::{asking as waiting_on_you, queued_after, ready, result, working};
        let mut prompted = waiting_on_you("hub", "kebbie", 3);
        prompted.question = None;
        prompted.agent.as_mut().unwrap().status = "blocked".to_string();
        prompted.agent.as_mut().unwrap().kind = crate::agent::Agent::Codex;
        prompted.agent.as_mut().unwrap().answer = status::answer_in_space(crate::agent::Agent::Codex);
        let items = [ready("copy-typo", "web", 180), ready("card-component", "web", 60), queued_after("pricing", "web", &["card-component@web"]), waiting_on_you("billing-vat", "web", 42), prompted, result("research", "web", 10), working("search")];
        let board = build(&items, &[], Vec::new(), None, &Seen::default());
        let expected = crate::next::of(&items);
        let rows = board.next.as_ref().unwrap();
        assert_eq!(rows.iter().map(|row| row.address.as_str()).collect::<Vec<_>>(), expected.rows.iter().map(|row| row.address.as_str()).collect::<Vec<_>>(), "the same order as N and kebbie next");
        assert_eq!(rows.iter().map(|row| row.rank).collect::<Vec<_>>(), (1..=rows.len() as u32).collect::<Vec<_>>());
        assert_eq!(rows.iter().map(|row| row.part).collect::<Vec<_>>(), [NextPart::Stopped, NextPart::Stopped, NextPart::Push, NextPart::Push, NextPart::Read]);
        let card = rows.iter().find(|row| row.address == "card-component@web").unwrap();
        assert_eq!((card.line.as_str(), card.waits_behind), ("2 commits · pricing waits on it", 1));
        let codex = rows.iter().find(|row| row.address == "hub@kebbie").unwrap();
        assert_eq!(codex.line, "answer Codex's prompt", "{rows:?}");
        assert!(rows.iter().all(|row| !row.line.contains("goes there") && !row.line.contains(" v ")), "{rows:?}");
        assert_eq!(board.counts, Some(Counts { working: count(expected.working), queued: 1, parked: 0 }));
        let empty = build(&[working("one")], &[], Vec::new(), None, &Seen::default());
        assert_eq!(empty.next, Some(Vec::new()), "an empty Next is sent, so the phone can say nothing needs you");
    }

    const CODEX_APPROVAL: &str = "  Would you like to run the following command?\n\n  Environment: local\n\n  Reason: May I record that this task is waiting on the lead?\n\n  $ /Users/u/.local/bin/kebbie report --state blocked --next nothing 'Read-only\n  inspection complete.' in /Users/u/projects/wt2.kebbie/src\n\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands that start with `/Users/u/.local/bin/kebbie\n     report` (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";

    #[test]
    fn a_codex_approval_reaches_the_board_as_its_prompt_with_the_id_kebbie_choose_takes_and_no_local_path() {
        let mut item = task(Some("blocked"), Some("working"), 1);
        item.name = "hub".to_string();
        item.place = "kebbie".to_string();
        item.path = Some("/Users/u/projects/wt2.kebbie".into());
        item.title = Some("Clarify machine removal".to_string());
        let agent = item.agent.as_mut().unwrap();
        agent.kind = crate::agent::Agent::Codex;
        agent.answer = status::answer_in_space(crate::agent::Agent::Codex);
        let menu = crate::menu::parse_for(CODEX_APPROVAL, crate::agent::Agent::Codex).unwrap();
        let seen = Seen { menus: BTreeMap::from([("hub@kebbie".to_string(), menu.clone())]), models: BTreeMap::from([("task".to_string(), "gpt-6.1-sol".to_string())]), home: Some("/Users/u".to_string()), accepting: BTreeSet::new() };
        let board = build(std::slice::from_ref(&item), &[], Vec::new(), None, &seen);
        let shown = &board.items[0];
        let prompt = shown.prompt.as_ref().unwrap();
        assert_eq!(prompt.id, crate::menu::fingerprint("hub@kebbie", &menu), "the id is the one the lead's relay and kebbie choose use");
        assert_eq!(prompt.options.iter().map(|option| (option.key.as_deref(), option.takes_text)).collect::<Vec<_>>(), [(Some("y"), false), (Some("p"), false), (Some("esc"), false)]);
        assert_eq!(prompt.options[0].label, "Yes, proceed (y)");
        assert!(prompt.question.starts_with("Would you like to run the following command?\nEnvironment: local\nReason: May I record"), "{}", prompt.question);
        assert!(prompt.question.contains("$ ~/.local/bin/kebbie report") && prompt.question.ends_with("inspection complete.' in src"), "{}", prompt.question);
        assert_eq!(shown.agent.as_ref().map(|agent| (agent.status, agent.kind.as_deref(), agent.model.as_deref())), Some((AgentStatus::Blocked, Some("codex"), Some("gpt-6.1-sol"))));
        assert_eq!(shown.title.as_deref(), Some("Clarify machine removal"));
        assert_eq!(shown.next_for_you.as_deref(), Some("answer Codex's prompt in its Space or through the lead"));
        assert!(!serde_json::to_string(&board).unwrap().contains("/Users/u"), "no local path leaves the machine");
        assert!(board.accepts.as_deref().is_some_and(|kinds| kinds.contains(&kebbie_hub_protocol::Kind::PromptAnswer)), "the phone offers a prompt's options only to a machine that takes the answer");
        let mut claude = item.clone();
        claude.agent.as_mut().unwrap().kind = crate::agent::Agent::Claude;
        let numbered = crate::menu::parse_for("Bash command\n  cp a b\nDo you want to proceed?\n❯ 1. Yes\n  2. No, and tell Claude what to do differently (esc)\n", crate::agent::Agent::Claude).unwrap();
        let seen = Seen { menus: BTreeMap::from([("hub@kebbie".to_string(), numbered)]), ..Seen::default() };
        let keys: Vec<Option<String>> = build(&[claude], &[], Vec::new(), None, &seen).items[0].prompt.as_ref().unwrap().options.iter().map(|option| option.key.clone()).collect();
        assert_eq!(keys, [Some("1".to_string()), Some("2".to_string())], "a Claude menu is answered by number");
        item.agent.as_mut().unwrap().status = "idle".to_string();
        assert_eq!(build(&[item], &[], Vec::new(), None, &seen).items[0].prompt, None, "only a blocked agent's screen is a prompt");
    }

    #[test]
    fn queued_and_parked_tasks_are_listed_apart_with_their_line_and_whether_a_question_waits() {
        use crate::next::tests::{asking as waiting_on_you, queued_after};
        let mut parked = waiting_on_you("herdr-app", "kebbie", 30);
        parked.parked = Some(crate::tasks::Parked { until: Some(1_761_100_000) });
        let mut quiet = task(Some("idle"), Some("working"), 0);
        quiet.name = "quiet".to_string();
        quiet.parked = Some(crate::tasks::Parked { until: None });
        let mut queued = queued_after("mail-dns", "shop", &["billing-site@shop"]);
        queued.title = Some("Mail DNS to Migadu".to_string());
        queued.queued.as_mut().unwrap().waiting = "waits for billing-site to push".to_string();
        let board = build(&[parked, quiet, queued], &[], Vec::new(), None, &Seen::default());
        assert!(board.items.is_empty(), "they leave the groups");
        let later: Vec<(&str, LaterState, &str, Option<u64>, bool)> = board.later.as_ref().unwrap().iter().map(|later| (later.address.as_str(), later.state, later.line.as_str(), later.until, later.question)).collect();
        assert_eq!(later[0], ("herdr-app@kebbie", LaterState::Parked, later[0].2, Some(1_761_100_000), true));
        assert_eq!(later[1].1, LaterState::Parked);
        assert!(!later[1].4);
        assert_eq!(later[2], ("mail-dns@shop", LaterState::Queued, "waits for billing-site to push", None, false));
        assert_eq!(board.later.as_ref().unwrap()[2].title.as_deref(), Some("Mail DNS to Migadu"));
        assert_eq!(board.counts.as_ref().map(|counts| (counts.queued, counts.parked)), Some((1, 2)));
    }

    #[test]
    fn texts_for_the_phone_name_the_tasks_own_agent_and_no_board_key() {
        let mut held = task(Some("idle"), Some("ready"), 2);
        held.verdict = Some(crate::status::tests::in_review(false, 2, false, false));
        held.agent.as_mut().unwrap().kind = crate::agent::Agent::Codex;
        held.recipe = Some(crate::recipe::Recipe { name: "small-fix".to_string(), why: "one file".to_string(), by: "lead".to_string() });
        held.assessment = Some(crate::assessment::Word::Suggested { reason: "touches money".to_string() });
        assert!(held.summary().ends_with("withdraw with v to push without a pass"), "the board keeps its key: {}", held.summary());
        let shown = &build(std::slice::from_ref(&held), &[], Vec::new(), None, &Seen::default()).items[0];
        assert!(shown.summary.ends_with("review: 2 findings · the push waits for a pass"), "{}", shown.summary);
        assert_eq!(shown.verdict.as_ref().unwrap().held, Some(true));
        assert_eq!(shown.second_opinion, None);
        let mut seconded = held.clone();
        seconded.second = Some(crate::review::SecondShown { by: "codex-review".to_string(), points: Some(2), running: false, stale: false, file: "/state/second/billing@web.md".into() });
        let second = build(&[seconded], &[], Vec::new(), None, &Seen::default()).items[0].second_opinion.clone();
        assert_eq!(second, Some(SecondOpinion { by: "codex-review".to_string(), points: Some(2), running: false }), "the second opinion goes without its file");
        assert_eq!((shown.recipe.as_deref(), shown.assessment.as_deref(), shown.profile.as_deref()), (Some(crate::recipe::shown_name("small-fix")), Some("review suggested: touches money"), Some("task")));
        let mut fixing = task(Some("idle"), Some("ready"), 1);
        fixing.agent.as_mut().unwrap().kind = crate::agent::Agent::Codex;
        fixing.push_waits = Some(crate::tasks::PendingPush { by: crate::tasks::PushBy::User, work: crate::tasks::PushWork::Ci, at: 5, ahead: 0, after: None });
        let board = build(std::slice::from_ref(&fixing), &[], Vec::new(), None, &Seen::default());
        assert_eq!(board.items[0].push_waits.as_deref(), Some("pushing for you · its Codex fixes the red CI"));
        let mut stepped = task(Some("working"), Some("working"), 1);
        stepped.steps = crate::tasks::Steps { pushed: 1, items: Vec::new() };
        assert_eq!(build(std::slice::from_ref(&stepped), &[], Vec::new(), None, &Seen::default()).items[0].steps.as_ref().unwrap().required, Some(false), "without task.require_accept push takes what is committed");
        let accepting = Seen { accepting: BTreeSet::from(["web".to_string()]), ..Seen::default() };
        assert_eq!(build(&[stepped], &[], Vec::new(), None, &accepting).items[0].steps.as_ref().unwrap().required, Some(true), "with task.require_accept in its repo each step waits on your accept");
    }

    #[test]
    fn a_card_stays_current_through_a_clean_rebase_and_not_after_a_new_code_commit_and_git_runs_once_per_head() {
        use crate::repo::git;
        let dir = crate::sh::test_dir("board-card-rebase");
        let run = |args: &[&str]| git(&dir, &[&["-c", "user.name=Ada", "-c", "user.email=ada@example.com"], args].concat()).unwrap();
        run(&["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(dir.join("a"), "one\n").unwrap();
        run(&["add", "a"]);
        run(&["commit", "--quiet", "-m", "first"]);
        run(&["checkout", "--quiet", "-b", "refunds"]);
        std::fs::write(dir.join("refund"), "cents\n").unwrap();
        run(&["add", "refund"]);
        run(&["commit", "--quiet", "-m", "Refund in cents"]);
        let card = git(&dir, &["rev-parse", "HEAD"]).unwrap();
        run(&["checkout", "--quiet", "main"]);
        std::fs::write(dir.join("b"), "two\n").unwrap();
        run(&["add", "b"]);
        run(&["commit", "--quiet", "-m", "main moves"]);
        run(&["checkout", "--quiet", "refunds"]);
        run(&["rebase", "--quiet", "main"]);
        let rebased = git(&dir, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(rebased, card);
        let mut item = task(Some("idle"), Some("ready"), 1);
        item.path = Some(dir.clone());
        item.git.as_mut().unwrap().upstream = "main".to_string();
        item.git.as_mut().unwrap().head = Some(rebased.clone());
        assert!(card_current(&item, &card), "a clean rebase onto a moved main keeps the card current");
        std::fs::write(dir.join("refund"), "whole crowns\n").unwrap();
        run(&["commit", "--quiet", "-am", "Refund in crowns"]);
        let changed = git(&dir, &["rev-parse", "HEAD"]).unwrap();
        item.git.as_mut().unwrap().head = Some(changed);
        assert!(!card_current(&item, &card), "a new code commit makes the card not current");
        std::fs::remove_dir_all(&dir).unwrap();
        item.git.as_mut().unwrap().head = Some(rebased);
        assert!(card_current(&item, &card), "the answer for this card and HEAD is kept, so git does not run again");
    }
}
