use std::path::Path;

use kebbie_hub_protocol::{Agent, AgentStatus, Board, Checkout, Ci, CiState, Git, Group, Item, PushPermission, Question, QuestionOption, Recover, Report, RoutineSummary, Step, StepState, Steps, Verdict, VerdictOutcome};

use crate::status;

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn group(group: status::Group) -> Option<Group> {
    Some(match group {
        status::Group::NeedsYou => Group::NeedsYou,
        status::Group::Working => Group::Working,
        status::Group::InReview => Group::ToReview,
        status::Group::ReadyToPush => Group::ReadyToPush,
        status::Group::CiRunning => Group::CiRunning,
        status::Group::Idle => Group::Idle,
        status::Group::Queued => return None,
    })
}

fn agent(item: &status::Item) -> Option<Agent> {
    let status = match item.agent.as_ref()?.status.as_str() {
        "working" => AgentStatus::Working,
        "blocked" => AgentStatus::Blocked,
        "idle" => AgentStatus::Idle,
        "done" => AgentStatus::Done,
        _ => return None,
    };
    Some(Agent { status })
}

fn report(item: &status::Item) -> Option<Report> {
    let message = item.report.as_ref()?;
    let report = message.report.as_ref()?;
    Some(Report { state: report.state.clone(), next: message.next_for_you().map(str::to_string), summary: message.text.clone(), at: message.at })
}

// A plan lives on this machine as a file or a vault note, so the phone gets the question without it.
fn question(item: &status::Item) -> Option<Question> {
    let message = item.question.as_ref()?;
    let asked = message.question.as_ref()?;
    Some(Question {
        id: message.id.clone(),
        text: message.text.clone(),
        options: asked.options.iter().map(|(label, meaning)| QuestionOption { label: label.clone(), meaning: meaning.clone() }).collect(),
        recommend: u8::try_from(asked.recommend).ok().filter(|recommend| *recommend > 0),
        kept_for_user: asked.waits,
        plan: None,
        at: message.at,
    })
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
    })
}

fn ci(item: &status::Item) -> Option<Ci> {
    Some(match &item.ci.as_ref()?.state {
        crate::ci::State::Running => Ci { state: CiState::Running, failure: None },
        crate::ci::State::Failed { what, .. } => Ci { state: CiState::Failed, failure: Some(what.clone()) },
    })
}

fn steps(item: &status::Item) -> Option<Steps> {
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
    Some(Steps { pushed: count(steps.pushed), items })
}

// Only what the protocol allow-lists: no path, pane, Space, dev server address or host leaves the machine.
pub fn item(item: &status::Item) -> Option<Item> {
    if !item.is_task() {
        return None;
    }
    let group = group(item.group())?;
    Some(Item {
        address: crate::peers::address(&item.name, &item.place),
        group,
        urgency: item.urgency(),
        summary: item.summary(),
        next_for_you: item.next_for_you().map(str::to_string),
        agent: agent(item),
        report: report(item),
        question: question(item),
        prompt: None,
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
        steps: steps(item),
    })
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
pub fn build(items: &[status::Item], checkouts: &[status::Checkout], routines: Vec<RoutineSummary>, away_since: Option<u64>) -> Board {
    Board {
        rev: 0,
        kebbie: env!("CARGO_PKG_VERSION").to_string(),
        away_since,
        checkouts: checkouts.iter().map(|checkout| Checkout { repo: checkout.name.clone(), free_worktrees: count(checkout.free_slots), busy_worktrees: count(checkout.busy_slots) }).collect(),
        routines,
        items: items.iter().filter_map(item).collect(),
        sessions_outside_tasks: count(items.iter().filter(|item| !item.is_task()).count()),
    }
}

pub fn gather(state: &Path, now: u64) -> crate::sh::Result<Board> {
    let items = status::gather()?;
    let checkouts = status::checkouts()?;
    Ok(build(&items, &checkouts, routines(state, now), crate::permit::away_since_in(state)))
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
            question: Some(Asked { options: vec![("Keep it".to_string(), "deprecate now".to_string()), ("Remove it".to_string(), "drop it in this task".to_string())], recommend: 2, plan: Some("/Users/someone/plans/refunds.md".to_string()), plan_at: None, comment: Vec::new(), answered: false, waits: false }),
            about: None, kind: None, landed: None, why: None, forced: false, fyi: false,
        });
        in_phase(&mut item, Phase::NeedsYou(crate::phase::Why::Question));
        item
    }

    #[test]
    fn a_task_reaches_the_board_with_its_question_and_nothing_local() {
        let mut item = asking("refunds", "greenhouse");
        item.path = Some("/Users/someone/projects/wt3.greenhouse".into());
        item.repo = Some("/Users/someone/projects/greenhouse".into());
        item.workspace = Some("w_secret_space".to_string());
        item.url = Some("http://localhost:3003".to_string());
        item.agent.as_mut().unwrap().pane = "w1:p9".to_string();
        item.git.as_mut().unwrap().head = Some("9f3c2a1e5b7d4c6a8b0e2f4d6c8a0b2e4f6d8c0a".to_string());
        let board = build(&[item], &[], Vec::new(), None);
        let shown = &board.items[0];
        assert_eq!(shown.address, "refunds@greenhouse");
        assert_eq!(shown.group, Group::NeedsYou);
        let question = shown.question.as_ref().unwrap();
        assert_eq!((question.text.as_str(), question.options.len(), question.recommend), ("Keep the old endpoint for a release?", 2, Some(2)));
        assert_eq!(question.options[1].label, "Remove it");
        assert!(question.plan.is_none(), "a plan path stays on the machine");
        assert_eq!(shown.git.as_ref().unwrap().upstream, "main");
        let text = serde_json::to_string(&board).unwrap();
        for local in ["/Users/someone", "w_secret_space", "localhost:3003", "w1:p9"] {
            assert!(!text.contains(local), "{local} leaked: {text}");
        }
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
        let board = build(&[item, plain, pushed_only], &[], Vec::new(), None);
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
        let board = build(&[session, queued, blocked, working], &[status::Checkout { name: "web".to_string(), changed: 3, free_slots: 2, busy_slots: 4 }], Vec::new(), Some(7));
        assert_eq!(board.sessions_outside_tasks, 1);
        assert_eq!(board.items.iter().map(|item| (item.address.as_str(), item.group)).collect::<Vec<_>>(), [("rounding@web", Group::NeedsYou), ("search@web", Group::Working)]);
        assert_eq!((board.checkouts[0].repo.as_str(), board.checkouts[0].free_worktrees, board.checkouts[0].busy_worktrees), ("web", 2, 4));
        assert_eq!(board.away_since, Some(7));
    }
}
