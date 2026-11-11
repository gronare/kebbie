use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use crate::board::widgets::{cut, text_width};
use crate::notice::{Notice, Tone};
use crate::peers;
use crate::phase::{Phase, Why};
use crate::sh::Result;
use crate::status::{Group, Item};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Device,
    Stopped,
    Push,
    Read,
}

impl Part {
    pub const ALL: [Part; 4] = [Part::Device, Part::Stopped, Part::Push, Part::Read];

    pub fn title(self) -> &'static str {
        match self {
            Part::Device => "APPROVE OR DENY",
            Part::Stopped => "STOPPED ON YOU",
            Part::Push => "PUSH, IN THIS ORDER",
            Part::Read => "TO READ",
        }
    }

    #[cfg(test)]
    pub fn board_group(self) -> Group {
        match self {
            Part::Push => Group::ReadyToPush,
            Part::Device | Part::Stopped | Part::Read => Group::NeedsYou,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub part: Part,
    pub address: String,
    pub name: String,
    pub icon: char,
    pub what: String,
    pub short: String,
    pub said: String,
    pub since: Option<u64>,
    pub waits: Vec<String>,
    pub after: Vec<String>,
    pub path: Option<PathBuf>,
}

impl Row {
    pub fn age(&self, now: u64) -> String {
        self.since.map(|since| waited(now.saturating_sub(since))).unwrap_or_default()
    }

    pub fn when(&self, now: u64) -> String {
        let age = self.age(now);
        match self.part {
            _ if age.is_empty() => age,
            Part::Device | Part::Stopped => format!("waiting {age}"),
            Part::Push => format!("ready {age}"),
            Part::Read => age,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Next {
    pub rows: Vec<Row>,
    pub working: usize,
    pub queued: usize,
    pub parked: usize,
    pub coming: Option<String>,
    pub parked_until: Option<u64>,
}

pub fn waited(seconds: u64) -> String {
    match seconds {
        0..60 => "now".to_string(),
        60..3600 => format!("{} min", seconds / 60),
        3600..86400 => format!("{} h", seconds / 3600),
        _ => format!("{} d", seconds / 86400),
    }
}

fn phase(item: &Item) -> Option<&Phase> {
    item.phase.as_ref().map(|record| &record.phase)
}

fn reads(item: &Item) -> bool {
    matches!(phase(item), Some(Phase::NeedsYou(Why::NothingToPush))) && !item.wants_you_now()
}

fn ci_red(item: &Item) -> bool {
    matches!(phase(item), Some(Phase::CiRed { .. }))
}

// Its agent was handed the failing log and fixes it; only its word that the failure is outside the code, an agent that is gone or a question stops it on you.
fn red_ci_waits_on_you(item: &Item) -> bool {
    item.ci_outside.is_some() || item.agent.is_none() || item.wants_you_now()
}

pub fn part(item: &Item) -> Option<Part> {
    if !item.is_task() {
        return None;
    }
    match item.group() {
        Group::NeedsYou if reads(item) => Some(Part::Read),
        Group::NeedsYou if ci_red(item) && !red_ci_waits_on_you(item) => None,
        Group::NeedsYou => Some(Part::Stopped),
        Group::ReadyToPush if item.push_waits.is_none() => Some(Part::Push),
        _ => None,
    }
}

fn address(item: &Item) -> String {
    peers::address(&item.name, &item.place)
}

// The tasks a ready task's report says go out before it.
fn declared_after(item: &Item) -> &[String] {
    item.report.as_ref().and_then(|message| message.report.as_ref()).filter(|report| report.state == "ready").map_or(&[], |report| report.after.as_slice())
}

fn waits_for(item: &Item, dep: &str) -> bool {
    item.queued.as_ref().is_some_and(|queued| queued.after.iter().any(|(after, state)| after == dep && state != "done")) || declared_after(item).iter().any(|after| after == dep)
}

// The tasks that wait on this one, queued or ready to push after it, through the tasks that wait on those, in any repo.
fn waiting_on(items: &[Item], on: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut frontier = vec![on.to_string()];
    while let Some(dep) = frontier.pop() {
        for item in items.iter().filter(|item| item.is_task()) {
            let me = address(item);
            if me != on && !found.contains(&me) && waits_for(item, &dep) {
                found.push(me.clone());
                frontier.push(me);
            }
        }
    }
    items.iter().filter(|item| found.contains(&address(item))).map(Item::titled).collect()
}

// The tasks still open that this one is to be pushed after.
fn open_before(items: &[Item], item: &Item) -> Vec<String> {
    let after = declared_after(item);
    items.iter().filter(|other| other.is_task() && after.contains(&address(other))).map(|other| if other.place == item.place { other.titled() } else { address(other) }).collect()
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

fn first_line(text: &str) -> &str {
    text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("")
}

fn before_dot(text: &str) -> String {
    text.split(" · ").next().unwrap_or(text).to_string()
}

pub fn waits_wide(waits: &[String]) -> Option<String> {
    match waits {
        [] => None,
        [one] => Some(format!("{one} waits on it")),
        many => Some(format!("{} tasks wait on it", many.len())),
    }
}

fn stopped(item: &Item) -> (char, String, String, Option<u64>) {
    let since = item.phase.as_ref().map(|record| record.since);
    let go = crate::keys::key(crate::keys::Act::Go).to_string();
    if let Some(message) = item.question.as_ref().filter(|message| message.question.is_some()) {
        let plan = message.question.as_ref().is_some_and(|question| question.plan.is_some());
        let (what, short) = if plan { ("approve its plan".to_string(), "approve its plan".to_string()) } else { (format!("answer its question: {}", first_line(&message.text)), "answer its question".to_string()) };
        return ('?', what, short, Some(message.at));
    }
    if ci_red(item) && !item.wants_you_now() {
        let failed = item.ci.as_ref().and_then(|watch| match watch.state { crate::ci::State::Failed { at, .. } => Some(at), crate::ci::State::Running => None });
        let agent = item.agent.as_ref().map_or("agent", |agent| agent.kind.name());
        let why = match (&item.ci_outside, &item.agent) {
            (Some(reason), _) => format!("its {agent} says outside the code: {reason}"),
            (None, None) => "no agent to fix it".to_string(),
            (None, Some(_)) => format!("its {agent} stopped"),
        };
        return ('✗', format!("CI red after push · {why} · needs your call"), "CI red, needs your call".to_string(), failed.or(since));
    }
    let what = item.wanted(&go).or_else(|| item.recover_note()).unwrap_or_else(|| item.summary());
    let asks = item.agent.as_ref().is_some_and(|agent| agent.status == "blocked") || item.held_by_draft.is_some() || matches!(phase(item), Some(Phase::NeedsYou(Why::Report | Why::Question)));
    let icon = if item.queued.is_some() { '⧗' } else if asks { '?' } else { '!' };
    let since = if matches!(phase(item), Some(Phase::NeedsYou(Why::Report))) { item.report.as_ref().map(|report| report.at).or(since) } else { since };
    (icon, what.clone(), before_dot(&what), since)
}

fn row(items: &[Item], item: &Item, part: Part) -> Row {
    let waits = waiting_on(items, &address(item));
    let after = open_before(items, item);
    let since = item.phase.as_ref().map(|record| record.since);
    let (icon, what, short, said, since) = match part {
        Part::Stopped | Part::Device => {
            let (icon, what, short, since) = stopped(item);
            (icon, what, short.clone(), short, since)
        }
        Part::Push => {
            let commits = plural(item.git.as_ref().map_or(0, |git| git.ahead), "commit");
            let wide = waits_wide(&waits);
            let before = (!after.is_empty()).then(|| format!("after {}", after.join(", ")));
            let what = [Some(commits.clone()), before.clone(), wide.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            let short = match (&before, waits.len()) {
                (Some(before), _) => before.clone(),
                (None, 0) => commits,
                (None, 1) => "1 waits on it".to_string(),
                (None, count) => format!("{count} wait on it"),
            };
            let said = match (before, wide) {
                (Some(before), _) => format!("push {before}"),
                (None, wide) => wide.map_or("push".to_string(), |wide| format!("push · {wide}")),
            };
            ('↑', what, short, said, since)
        }
        Part::Read => {
            let text = item.report.as_ref().map(|report| first_line(&report.text).to_string()).filter(|text| !text.is_empty());
            let what = text.map_or("result ready".to_string(), |text| format!("result ready · {text}"));
            ('◆', what, "result ready".to_string(), "read the result".to_string(), item.report.as_ref().map(|report| report.at).or(since))
        }
    };
    Row { part, address: address(item), name: item.titled(), icon, what, short, said, since, waits, after, path: item.path.clone() }
}

fn shares_files(one: &Item, other: &Item) -> bool {
    one.built.files.iter().any(|file| other.built.files.contains(file))
}

fn built_before(one: &Item, other: &Item) -> bool {
    matches!((one.built.first_at, other.built.first_at), (Some(one), Some(other)) if one < other)
}

// Two pushes of the same files go in the order they were built, among pushes that as many tasks wait on.
fn same_files_in_build_order(rows: &mut Ranked) {
    for at in 0..rows.len() {
        let tier = rows[at].0.waits.len();
        let earlier = (0..at).filter(|before| rows[*before].0.waits.len() == tier).find(|before| shares_files(rows[at].1, rows[*before].1) && built_before(rows[at].1, rows[*before].1));
        if let Some(before) = earlier {
            let moved = rows.remove(at);
            rows.insert(before, moved);
        }
    }
}

fn coming(items: &[Item]) -> Option<String> {
    let working: Vec<&Item> = items.iter().filter(|item| item.is_task() && item.group() == Group::Working).collect();
    let stepped = working.iter().filter_map(|item| {
        let total = item.steps.pushed + item.steps.items.len();
        let done = item.steps.pushed + item.steps.items.iter().filter(|step| matches!(step.state, crate::tasks::StepState::Committed { .. })).count();
        (total > 0).then_some((*item, (done + 1).min(total), total))
    }).max_by(|(_, done, total), (_, other_done, other_total)| (done * other_total).cmp(&(other_done * total)));
    if let Some((item, step, total)) = stepped {
        return Some(format!("{} (step {step} of {total})", item.titled()));
    }
    working.iter().min_by_key(|item| item.phase.as_ref().map_or(u64::MAX, |record| record.since)).map(|item| item.titled())
}

type Ranked<'a> = Vec<(Row, &'a Item)>;

// What needs the user, in the order to do it, from the board's own items; nothing is kept to work it out.
pub fn of(items: &[Item]) -> Next {
    let mut next = Next::default();
    let mut parts: Ranked = Vec::new();
    for item in items.iter().filter(|item| item.is_task()) {
        match (part(item), item.group()) {
            (Some(part), _) => parts.push((row(items, item, part), item)),
            (None, Group::Queued) => next.queued += 1,
            (None, Group::Parked) => next.parked += 1,
            (None, Group::Idle) => {}
            (None, _) => next.working += 1,
        }
    }
    parts.sort_by(|(one, _), (other, _)| {
        let key = |row: &Row| (Part::ALL.iter().position(|part| *part == row.part), Reverse(row.waits.len()), row.since.unwrap_or(u64::MAX));
        key(one).cmp(&key(other)).then_with(|| one.name.cmp(&other.name))
    });
    let (mut pushes, rest): (Ranked, Ranked) = parts.into_iter().partition(|(row, _)| row.part == Part::Push);
    same_files_in_build_order(&mut pushes);
    let (stopped, read): (Ranked, Ranked) = rest.into_iter().partition(|(row, _)| row.part == Part::Stopped);
    next.rows = stopped.into_iter().chain(pushes).chain(read).map(|(row, _)| row).collect();
    next.coming = coming(items);
    next.parked_until = items.iter().filter_map(|item| item.parked.and_then(|parked| parked.until)).min();
    next
}

// A device waiting for approval stops its command on the user, so it goes above everything a task waits on.
fn device_row(request: &crate::hub::devices::Request, now: u64) -> Row {
    let wait = match request.until {
        Some(until) => crate::hub::devices::auto_in(until, now),
        None => "approve it to let it make changes".to_string(),
    };
    Row {
        part: Part::Device,
        address: request.id.as_str().to_string(),
        name: request.name.clone(),
        icon: '?',
        what: format!("{} · wants to make changes · {wait}", crate::hub::devices::kind_name(request.kind)),
        short: "approve or deny".to_string(),
        said: "approve or deny it".to_string(),
        since: Some(request.first_seen),
        waits: Vec::new(),
        after: Vec::new(),
        path: None,
    }
}

impl Next {
    pub fn with_devices(mut self, requests: &[crate::hub::devices::Request], now: u64) -> Next {
        let mut rows: Vec<Row> = requests.iter().map(|request| device_row(request, now)).collect();
        rows.sort_by_key(|row| row.since);
        rows.append(&mut self.rows);
        self.rows = rows;
        self
    }

    pub fn pushes(&self) -> Vec<&Row> {
        self.rows.iter().filter(|row| row.part == Part::Push).collect()
    }

    // The push group's place in the numbered list, as 3–5.
    pub fn push_span(&self) -> Option<String> {
        let first = self.rows.iter().position(|row| row.part == Part::Push)? + 1;
        let last = first + self.pushes().len() - 1;
        Some(if first == last { first.to_string() } else { format!("{first}–{last}") })
    }

    pub fn footer(&self) -> String {
        [(self.working > 0).then(|| format!("{} working", self.working)), (self.queued > 0).then(|| format!("{} queued", self.queued)), (self.parked > 0).then(|| format!("{} parked", self.parked))].into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }

    // What is on its way in when nothing needs the user, so they know whether to wait.
    pub fn on_its_way(&self, now: u64, offset: &dyn Fn(i64) -> i64) -> Vec<String> {
        let mut lines = Vec::new();
        if self.working > 0 {
            lines.push(match &self.coming {
                Some(coming) => format!("{} working · next to come: {coming}", self.working),
                None => format!("{} working", self.working),
            });
        }
        let parked = match (self.parked, self.parked_until) {
            (0, _) => None,
            (count, Some(at)) => Some(format!("{count} parked until {}", crate::when::shown(at, now, offset))),
            (count, None) => Some(format!("{count} parked")),
        };
        let rest: Vec<String> = [(self.queued > 0).then(|| format!("{} queued", self.queued)), parked].into_iter().flatten().collect();
        if !rest.is_empty() {
            lines.push(rest.join(" · "));
        }
        lines
    }

    pub fn text(&self, now: u64, offset: &dyn Fn(i64) -> i64) -> String {
        if self.rows.is_empty() {
            let mut lines = vec!["Nothing needs you.".to_string()];
            lines.extend(self.on_its_way(now, offset));
            return lines.join("\n");
        }
        let number = self.rows.len().to_string().len();
        let name = self.rows.iter().map(|row| text_width(&row.name)).max().unwrap_or(0).min(28);
        let said = self.rows.iter().map(|row| text_width(&row.said)).max().unwrap_or(0).min(48);
        let pad = |text: &str, width: usize| {
            let shown = if text_width(text) > width { cut(text, width) } else { text.to_string() };
            let used = text_width(&shown);
            format!("{shown}{}", " ".repeat(width.saturating_sub(used)))
        };
        let mut lines: Vec<String> = self.rows.iter().enumerate().map(|(index, row)| {
            format!("{:<number$}  {} {} {} {}", index + 1, row.icon, pad(&row.name, name), pad(&row.said, said), row.when(now)).trim_end().to_string()
        }).collect();
        let footer = self.footer();
        if !footer.is_empty() {
            lines.push(footer);
        }
        lines.join("\n")
    }
}

pub fn print() -> Result<String> {
    let now = crate::messages::now();
    let devices = crate::lead::state_dir().map(|state| crate::hub::devices::requests(&state, now)).unwrap_or_default();
    Ok(of(&crate::status::gather()?).with_devices(&devices, now).text(now, &crate::recover::local_offset))
}

// Each push in turn; the first that is refused stops the rest, which stay ready.
pub fn push_in_order(order: &[(String, PathBuf)], mut push: impl FnMut(&Path) -> Result<String>) -> Result<String> {
    let mut sent: Vec<String> = Vec::new();
    for (at, (name, path)) in order.iter().enumerate() {
        if let Err(why) = push(path) {
            let why = Notice::parse(&why).map_or(why.clone(), |notice| notice.headline);
            let left: Vec<String> = order[at + 1..].iter().map(|(name, _)| name.clone()).collect();
            let mut notice = Notice::new(Tone::Failed, format!("{name} did not push, so the rest wait")).detail("why", why);
            if !sent.is_empty() {
                notice = notice.detail("in the push line", crate::tasks::and_list(&sent));
            }
            if !left.is_empty() {
                notice = notice.detail("still ready", crate::tasks::and_list(&left));
            }
            return notice.next("The rest stay under Ready to push; push them once it is sorted.").err();
        }
        sent.push(name.clone());
    }
    Notice::new(Tone::Moving, format!("Pushing {} in order", plural(sent.len(), "task"))).next("Each is rebased onto main and goes out in turn; a conflict or red tests go back to its agent.").detail("order", sent.join(", ")).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::status::tests::{in_phase, task};
    use crate::status::{Built, Queued};

    pub(crate) fn now() -> u64 {
        crate::messages::now()
    }

    pub(crate) fn named(mut item: Item, name: &str, place: &str) -> Item {
        item.name = name.to_string();
        item.place = place.to_string();
        item.path = Some(PathBuf::from(format!("/work/{name}")));
        item
    }

    fn since(mut item: Item, minutes_ago: u64) -> Item {
        item.phase.as_mut().unwrap().since = now() - minutes_ago * 60;
        item
    }

    pub(crate) fn ready(name: &str, place: &str, minutes_ago: u64) -> Item {
        let mut item = named(task(Some("idle"), Some("ready"), 2), name, place);
        in_phase(&mut item, Phase::Ready { head: None });
        since(item, minutes_ago)
    }

    pub(crate) fn asking(name: &str, place: &str, minutes_ago: u64) -> Item {
        let mut item = named(task(Some("idle"), Some("needs-you"), 0), name, place);
        let mut question = crate::messages::fold(r#"{"id":"q1","at":10,"from":"x@web","to":"lead","text":"Round per invoice or per line?","state":"asked","question":{"options":[{"label":"Invoice","meaning":"one rounding"},{"label":"Line","meaning":"each line"}],"recommend":1}}"#).remove(0);
        question.from = format!("{name}@{place}");
        question.at = now() - minutes_ago * 60;
        item.question = Some(question);
        in_phase(&mut item, Phase::NeedsYou(Why::Question));
        since(item, minutes_ago)
    }

    pub(crate) fn result(name: &str, place: &str, minutes_ago: u64) -> Item {
        let mut item = named(task(Some("idle"), Some("ready"), 0), name, place);
        item.report.as_mut().unwrap().text = "recommends staying on the TUI".to_string();
        item.report.as_mut().unwrap().at = now() - minutes_ago * 60;
        in_phase(&mut item, Phase::NeedsYou(Why::NothingToPush));
        since(item, minutes_ago)
    }

    pub(crate) fn queued_after(name: &str, place: &str, after: &[&str]) -> Item {
        let mut item = named(task(None, None, 0), name, place);
        item.git = None;
        item.phase = None;
        item.agent = None;
        item.queued = Some(Queued { after: after.iter().map(|address| (address.to_string(), "ready to push".to_string())).collect(), blocked: None, waiting: "Waits.".to_string(), names: Vec::new(), base: "main".to_string(), at: None, backlog: false });
        item
    }

    pub(crate) fn working(name: &str) -> Item {
        let mut item = named(task(Some("working"), None, 1), name, "web");
        in_phase(&mut item, Phase::Working);
        item
    }

    fn order(items: &[Item]) -> Vec<String> {
        of(items).rows.iter().map(|row| row.name.clone()).collect()
    }

    #[test]
    fn a_task_waiting_on_your_answer_comes_before_any_push() {
        let items = [ready("card-component", "web", 300), asking("billing-vat", "web", 1)];
        assert_eq!(order(&items), ["billing-vat", "card-component"]);
        assert_eq!(of(&items).rows.iter().map(|row| row.part).collect::<Vec<_>>(), [Part::Stopped, Part::Push]);
    }

    #[test]
    fn a_task_with_a_step_to_accept_under_require_is_stopped_on_you_not_pushed_in_order() {
        let mut held = ready("billing", "web", 30);
        held.to_accept = Some(crate::status::tests::waiting_to_accept(2, 1));
        let items = [ready("card-component", "web", 300), held];
        let next = of(&items);
        assert_eq!(next.rows.iter().map(|row| (row.name.as_str(), row.part)).collect::<Vec<_>>(), [("billing", Part::Stopped), ("card-component", Part::Push)]);
        assert_eq!((next.rows[0].said.as_str(), next.rows[0].icon), ("accept step 2", '!'));
        assert_eq!(next.pushes().len(), 1, "push in order leaves it out");
    }

    #[test]
    fn of_two_questions_the_one_more_tasks_wait_behind_comes_first() {
        let items = [asking("old-question", "web", 90), asking("blocking", "web", 5), queued_after("pricing", "web", &["blocking@web"])];
        assert_eq!(order(&items), ["blocking", "old-question"]);
        let level = [asking("newer", "web", 5), asking("older", "web", 90)];
        assert_eq!(order(&level), ["older", "newer"], "with no task behind either, the one that waited longest");
    }

    #[test]
    fn a_push_other_tasks_wait_on_goes_before_an_older_one() {
        let items = [ready("copy-typo", "web", 180), ready("card-component", "web", 60), queued_after("pricing", "web", &["card-component@web"]), queued_after("hero", "web", &["pricing@web"])];
        let next = of(&items);
        assert_eq!(order(&items), ["card-component", "copy-typo"]);
        assert_eq!(next.rows[0].waits, ["pricing", "hero"], "a task that waits on one that waits on it counts too");
        assert_eq!(next.rows[0].what, "2 commits · 2 tasks wait on it");
    }

    #[test]
    fn a_wait_from_another_repos_task_counts_as_one_in_its_own() {
        let items = [ready("old-fix", "theme", 180), ready("fix-grid", "theme", 20), queued_after("bump-pins", "shop", &["fix-grid@theme"])];
        assert_eq!(order(&items), ["fix-grid", "old-fix"], "the push a task in another repo waits on goes first");
        assert_eq!(of(&items).rows[0].waits, ["bump-pins"]);
    }

    fn ready_after(name: &str, place: &str, minutes_ago: u64, after: &[&str]) -> Item {
        let mut item = ready(name, place, minutes_ago);
        if let Some(report) = item.report.as_mut().and_then(|message| message.report.as_mut()) {
            report.after = after.iter().map(|address| address.to_string()).collect();
        }
        item
    }

    #[test]
    fn the_half_of_a_task_declared_to_go_after_the_other_pushes_after_it_and_says_so() {
        let items = [ready_after("payload-v2", "web", 300, &["payload-v2@api"]), ready("copy-typo", "web", 200), ready("payload-v2", "api", 10)];
        let next = of(&items);
        assert_eq!(next.rows.iter().map(|row| row.address.as_str()).collect::<Vec<_>>(), ["payload-v2@api", "payload-v2@web", "copy-typo@web"], "the api half goes first though it was ready last");
        let web = &next.rows[1];
        assert_eq!((web.what.as_str(), web.short.as_str(), web.said.as_str()), ("2 commits · after payload-v2@api", "after payload-v2@api", "push after payload-v2@api"));
        assert_eq!(next.rows[0].waits, ["payload-v2"], "the api half shows the half that waits on it");
        let alone = [ready_after("payload-v2", "web", 300, &["payload-v2@api"]), ready("copy-typo", "web", 200)];
        let next = of(&alone);
        assert!(next.rows.iter().all(|row| row.after.is_empty()) && next.rows[0].address == "payload-v2@web", "once the api half has gone, the wait is over: {:?}", next.rows);
    }

    #[test]
    fn two_ready_tasks_in_the_same_files_push_in_the_order_they_were_built() {
        let mut first_built = ready("second-ready", "web", 10);
        first_built.built = Built { files: vec!["app/price.rb".to_string()], first_at: Some(100) };
        let mut later_built = ready("first-ready", "web", 60);
        later_built.built = Built { files: vec!["app/price.rb".to_string(), "README.md".to_string()], first_at: Some(200) };
        assert_eq!(order(&[later_built.clone(), first_built.clone()]), ["second-ready", "first-ready"]);
        first_built.built.files = vec!["app/other.rb".to_string()];
        assert_eq!(order(&[later_built, first_built]), ["first-ready", "second-ready"], "different files keep the ready order");
    }

    #[test]
    fn equal_pushes_go_oldest_ready_first() {
        let items = [ready("newest", "web", 5), ready("oldest", "web", 180), ready("middle", "web", 60)];
        assert_eq!(order(&items), ["oldest", "middle", "newest"]);
    }

    #[test]
    fn a_result_a_decision_waits_on_comes_first_then_the_oldest() {
        let items = [result("old-research", "web", 300), result("new-research", "web", 10), result("needed", "web", 5), queued_after("decision", "web", &["needed@web"])];
        assert_eq!(order(&items), ["needed", "old-research", "new-research"]);
        assert!(of(&items).rows.iter().all(|row| row.part == Part::Read && row.icon == '◆'));
    }

    #[test]
    fn working_queued_and_parked_tasks_stay_out_of_next_and_count_in_the_footer() {
        let mut parked = asking("parked", "web", 30);
        parked.parked = Some(crate::tasks::Parked { until: Some(now() + 3600) });
        let mut pushing = ready("in-line", "web", 30);
        pushing.push_waits = Some(crate::tasks::PendingPush { by: crate::tasks::PushBy::User, work: crate::tasks::PushWork::Turn, at: 5, ahead: 1, after: None });
        let items = [working("one"), working("two"), queued_after("waits", "web", &["one@web"]), parked, pushing];
        let next = of(&items);
        assert!(next.rows.is_empty(), "{:?}", next.rows);
        assert_eq!(next.footer(), "3 working · 1 queued · 1 parked", "a push in the line is kebbie's work now");
        assert_eq!(next.text(now(), &|_| 0).lines().next(), Some("Nothing needs you."));
    }

    #[test]
    fn a_parked_task_that_wakes_with_a_question_enters_next() {
        let mut item = asking("billing-vat", "web", 30);
        item.parked = Some(crate::tasks::Parked { until: Some(now() + 60) });
        assert!(of(std::slice::from_ref(&item)).rows.is_empty());
        item.parked = None;
        assert_eq!(order(&[item]), ["billing-vat"]);
    }

    #[test]
    fn red_ci_is_on_you_only_when_its_claude_cannot_fix_it() {
        let red = |outside: Option<&str>| {
            let mut item = named(task(Some("idle"), Some("ready"), 0), "booking-calendar", "web");
            in_phase(&mut item, Phase::CiRed { sha: "abc".to_string() });
            item.ci_outside = outside.map(str::to_string);
            item
        };
        let fixing = of(&[red(None)]);
        assert!(fixing.rows.is_empty() && fixing.working == 1, "its agent has the log and fixes it");
        let outside = of(&[red(Some("the deploy window was closed"))]);
        assert_eq!((outside.rows[0].icon, outside.rows[0].short.as_str()), ('✗', "CI red, needs your call"));
        assert!(outside.rows[0].what.starts_with("CI red after push · its Claude says outside the code"), "{}", outside.rows[0].what);
        let mut codex = red(Some("a flake"));
        codex.agent.as_mut().unwrap().kind = crate::agent::Agent::Codex;
        assert!(of(&[codex]).rows[0].what.starts_with("CI red after push · its Codex says outside the code"), "the task's own agent is named");
    }

    #[test]
    fn push_all_stops_at_the_first_failure_and_leaves_the_rest_ready() {
        let order: Vec<(String, PathBuf)> = ["card-component", "fix-grid", "copy-typo"].iter().map(|name| (name.to_string(), PathBuf::from(format!("/work/{name}")))).collect();
        let mut tried = Vec::new();
        let outcome = push_in_order(&order, |path| {
            tried.push(path.to_path_buf());
            if path.ends_with("fix-grid") { Notice::new(Tone::Failed, "Not pushed: it is in review").err() } else { Ok("in line".to_string()) }
        });
        assert_eq!(tried, [PathBuf::from("/work/card-component"), PathBuf::from("/work/fix-grid")], "copy-typo is never pushed");
        let notice = Notice::parse(&outcome.unwrap_err()).unwrap();
        assert_eq!(notice.headline, "fix-grid did not push, so the rest wait");
        assert!(notice.details.contains(&("why".to_string(), "Not pushed: it is in review".to_string())) && notice.details.contains(&("still ready".to_string(), "copy-typo".to_string())), "{:?}", notice.details);
        let all = push_in_order(&order, |_| Ok(String::new())).unwrap();
        assert!(Notice::parse(&all).unwrap().headline == "Pushing 3 tasks in order");
    }

    #[test]
    fn waits_read_in_minutes_hours_and_days() {
        assert_eq!([waited(30), waited(42 * 60), waited(3600), waited(3 * 86400)], ["now", "42 min", "1 h", "3 d"]);
    }
}
