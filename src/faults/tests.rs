use super::*;
use super::watch::{Kind, Verdict};

fn state(name: &str) -> PathBuf {
    crate::sh::test_dir(&format!("faults-{name}"))
}

fn case(state: &Path, source: Source, from: &str, words: &str, now: u64) -> Case {
    let id = add_case(state, &NewCase { source, from, words, context: "Before it, the agent had said: done", fingerprint: None }, now).unwrap().unwrap();
    cases(state).into_iter().find(|case| case.id == id).unwrap()
}

fn new_fault(title: &str) -> Verdict {
    Verdict { case: String::new(), kind: Some(Kind::New), issue_kind: Some("bug".to_string()), title: Some(title.to_string()), happened: Some("the lead asked the user in its chat".to_string()), expected: Some("questions go on the board".to_string()), cause: Some("roles/lead.md lets the lead ask in its reply".to_string()), barrier: Some("role".to_string()), files: Some("roles/lead.md".to_string()), question: None, task: None, reason: "a rule about how a lead works".to_string() }
}

#[test]
fn the_same_words_from_the_same_chat_wait_as_one_case() {
    let state = state("dedup");
    let first = add_case(&state, &NewCase { source: Source::Chat, from: "lead", words: "you asked me in the chat again", context: "", fingerprint: None }, 10).unwrap();
    let again = add_case(&state, &NewCase { source: Source::Chat, from: "lead", words: "you asked me in the chat again", context: "", fingerprint: None }, 20).unwrap();
    let elsewhere = add_case(&state, &NewCase { source: Source::Chat, from: "billing@app", words: "you asked me in the chat again", context: "", fingerprint: None }, 30).unwrap();
    assert_eq!((first.as_deref(), again, elsewhere.as_deref()), (Some("c1"), None, Some("c2")));
    settle(&state, "c1", "no fault in kebbie", None).unwrap();
    assert_eq!(pending(&state).iter().map(|case| case.id.as_str()).collect::<Vec<_>>(), ["c2"]);
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn at_most_three_fix_tasks_start_in_a_day_and_the_fourth_waits_for_the_oldest_to_age() {
    let state = state("cap");
    for at in [100, 200, 300] {
        assert_eq!(next_start(&state, at), at);
        count_fix(&state, at, at).unwrap();
    }
    assert_eq!(next_start(&state, 400), 100 + DAY, "the fourth starts when the first of the day is a day old");
    assert_eq!(next_start(&state, 100 + DAY + 1), 100 + DAY + 1);
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn the_users_words_stay_on_this_machine_and_out_of_the_issues_text() {
    let state = state("private");
    let said = case(&state, Source::Chat, "lead", "why do you keep asking me in the chat, I said put it on the board", 10);
    let issue = watch::filed_fault(&state, &said, &new_fault("The lead asks the user in its chat instead of on the board"), 10).unwrap();
    assert!(!issue.body.contains("I said") && !issue.title.contains("I said"), "{issue:?}");
    assert_eq!((issue.by.as_str(), issue.cause.as_deref(), issue.barrier.as_deref()), (BY, Some("roles/lead.md lets the lead ask in its reply"), Some("role")));
    assert!(occurrences(&state, &issue.id)[0].quote.contains("I said put it on the board"));
    assert!(show_in(&state, &issue.id).unwrap().contains("I said put it on the board"));
    let report = crate::hub::report::payload(&issue, &crate::hub::report::Scrub::default());
    assert!(!report.text.contains("I said") && !report.text.contains("roles/lead.md lets"), "the report carries the issue's text alone: {}", report.text);
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_fault_without_a_kebbie_checkout_is_filed_and_waits_for_the_maintainers() {
    let state = state("no-checkout");
    let said = case(&state, Source::Board, "billing@app", "this plan again puts my question in the chat", 10);
    let (outcome, issue) = watch::act(&state, &said, &new_fault("A plan's question reaches the chat"), None, 10).unwrap();
    assert!(outcome.contains("waits for kebbie's maintainers"), "{outcome}");
    let issue = issues::find(&issues::dir(&state), &issue.unwrap()).unwrap();
    assert_eq!(issue.fix, None);
    assert_eq!(list_in(&state, false, true).lines().next().unwrap().split_whitespace().nth(1), Some("open"));
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_known_issue_from_another_install_is_never_this_machines_fault() {
    let state = state("untrusted");
    let dir = issues::dir(&state);
    issues::save(&dir, &Issue { id: "r9".to_string(), title: "x".to_string(), kind: "bug".to_string(), status: "open".to_string(), source: Some("report 9".to_string()), ..Issue::default() }).unwrap();
    let said = case(&state, Source::Chat, "lead", "this broke again today", 10);
    let verdict = Verdict { kind: Some(Kind::Known("r9".to_string())), ..Verdict::default() };
    assert!(watch::act(&state, &said, &verdict, None, 10).unwrap_err().contains("another install's report"));
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn the_watchers_answer_reads_one_block_per_case() {
    let answer = "I read roles/lead.md.\n\ncase: c1\nverdict: ignore\nreason: the user answers a question\n\ncase: c2\nverdict: new\nreason: a lead rule\nkind: bug\ntitle: The lead hands the user a command\nhappened: the lead told the user to run a command\nexpected: the lead runs it\ncause: roles/lead.md has no rule against it\nbarrier: Code\nfiles: roles/lead.md, src/lead.rs\nquestion: none\n\ncase: c3\nverdict: known 3f9a2c.\nreason: same fault\n\ncase: c4\nverdict: linked lead-asks-on-board\n\ncase: c5\nverdict: maybe\n";
    let verdicts = watch::parse(answer);
    assert_eq!(verdicts.iter().map(|verdict| (verdict.case.as_str(), verdict.kind.clone().unwrap())).collect::<Vec<_>>(), [("c1", Kind::Ignore), ("c2", Kind::New), ("c3", Kind::Known("3f9a2c".to_string())), ("c4", Kind::Linked("lead-asks-on-board".to_string()))], "a verdict kebbie cannot read is no verdict");
    let fenced = watch::parse("Here are the verdicts.\n\n```\ncase: c1\nverdict: new\nreason: r\ntitle: The lead asks in its chat\ncause: src/lead.rs has no Stop hook for the lead\nbarrier: code\n```\n\n```\ncase: c2\nverdict: known 6f6168\nreason: same\n```\n");
    assert_eq!(fenced.iter().map(|verdict| verdict.kind.clone().unwrap()).collect::<Vec<_>>(), [Kind::New, Kind::Known("6f6168".to_string())], "a block in a code fence reads the same");
    let new = &verdicts[1];
    assert_eq!((new.barrier.as_deref(), new.question.as_deref(), new.files.as_deref()), (Some("code"), None, Some("roles/lead.md, src/lead.rs")));
}

#[test]
fn a_case_waits_for_the_agents_turn_and_runs_come_apart() {
    let waiting = |at| Case { id: format!("c{at}"), at, source: Source::Chat, from: "lead".to_string(), words: "w".to_string(), context: String::new(), fingerprint: None, settled: None, issue: None, tries: 0, pointed: false };
    let cases = vec![waiting(1_000), waiting(1_100)];
    assert_eq!(watch::due(&cases, 0, 1_110), Vec::<String>::new(), "neither has waited two minutes");
    assert_eq!(watch::due(&cases, 0, 1_150), ["c1000"]);
    assert_eq!(watch::due(&cases, 1_200, 1_400), Vec::<String>::new(), "a run came less than five minutes ago");
    let spent = vec![Case { tries: 2, ..waiting(1_000) }];
    assert_eq!(watch::due(&spent, 0, 5_000), Vec::<String>::new(), "a case two runs gave no answer on is not tried again");
}

#[test]
fn a_fix_task_is_told_to_build_a_code_barrier_once_a_text_fix_did_not_hold() {
    let issue = Issue { id: "3f9a2c".to_string(), title: "The lead hands the user a command".to_string(), body: "What happened: x\nWhat I expected: y".to_string(), cause: Some("roles/lead.md".to_string()), barrier: Some("code".to_string()), history: vec!["files: src/lead.rs".to_string()], ..Issue::default() };
    let prompt = watch::fix_prompt(&issue, true, &["type sudo apt install mold (lead)".to_string()]);
    assert!(prompt.starts_with("The lead hands the user a command\n") && prompt.contains("kebbie issue 3f9a2c") && prompt.contains("- type sudo apt install mold (lead)") && prompt.contains("Files it names: src/lead.rs"), "{prompt}");
    assert!(prompt.contains("reproduce the fault as it was met and see it happen") && prompt.contains("--after-push naming the check that reproduces it on the pushed build"), "{prompt}");
    assert!(prompt.contains("a text fix did not hold") && prompt.contains(watch::barrier_line("code")), "{prompt}");
    assert!(!watch::fix_prompt(&Issue { barrier: Some("role".to_string()), ..issue }, false, &[]).contains("did not hold"));
}

#[test]
fn the_chats_give_the_users_own_turns_with_the_agents_answer_and_nothing_else() {
    let state = state("chat");
    let transcript = state.join("lead.jsonl");
    let user = |text: &str| serde_json::json!({ "type": "user", "uuid": "u", "timestamp": "2026-11-26T08:00:00.000Z", "message": { "role": "user", "content": text } }).to_string();
    let reply = |text: &str| serde_json::json!({ "type": "assistant", "uuid": "a", "timestamp": "2026-11-26T08:00:05.000Z", "message": { "stop_reason": "end_turn", "content": [{ "type": "text", "text": text }] } }).to_string();
    let end = serde_json::json!({ "type": "system", "subtype": "turn_duration", "timestamp": "2026-11-26T08:00:06.000Z" }).to_string();
    fs::write(&transcript, format!("{}\n", user("an old correction from before the watcher ran"))).unwrap();
    let chats = vec![capture::Chat { from: "lead".to_string(), path: transcript.clone(), kebbie_typed: Vec::new() }];
    let now = crate::messages::now() + 5;
    assert!(capture::tick_in(&state, &chats, now).is_empty(), "the history before the watcher ran is never weighed");
    let lines = [reply("Noted."), end.clone(), user("ja"), reply("Ok."), end.clone(), user("[from billing@app] the board is wrong again, please fix"), end.clone(), user("why is the push line standing still again? fix the cause"), reply("I re-queued the push."), end];
    let mut appended = fs::read_to_string(&transcript).unwrap();
    appended.push_str(&format!("{}\n", lines.join("\n")));
    fs::write(&transcript, appended).unwrap();
    assert_eq!(capture::tick_in(&state, &chats, now + 30), ["faults: 1 of the user's words to weigh"]);
    let [taken] = pending(&state).try_into().unwrap();
    assert_eq!((taken.source, taken.from.as_str(), taken.words.as_str()), (Source::Chat, "lead", "why is the push line standing still again? fix the cause"));
    assert!(taken.context.contains("The agent answered: I re-queued the push.") && taken.context.contains("Before it, the agent had said: Ok."), "{}", taken.context);
    assert!(capture::tick_in(&state, &chats, now + 60).is_empty(), "read once");
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_chat_that_began_after_the_watcher_first_looked_is_read_from_its_first_words() {
    let state = state("new-chat");
    let user = |text: &str| serde_json::json!({ "type": "user", "uuid": "u", "timestamp": "2026-11-26T08:00:00.000Z", "message": { "role": "user", "content": text } }).to_string();
    let end = serde_json::json!({ "type": "system", "subtype": "turn_duration", "timestamp": "2026-11-26T08:00:06.000Z" }).to_string();
    let now = crate::messages::now();
    assert!(capture::tick_in(&state, &[], now - 60).is_empty());
    let lead = state.join("restarted-lead.jsonl");
    let summary = serde_json::json!({ "type": "user", "uuid": "s", "isCompactSummary": true, "timestamp": "2026-11-26T08:00:00.000Z", "message": { "role": "user", "content": "This session is being continued from a previous conversation. The user said the push line is wrong again." } }).to_string();
    fs::write(&lead, format!("{summary}\n{}\n{end}\n", user("you restarted and the board still shows the wrong task, fix it"))).unwrap();
    let chat = |path: &Path| [capture::Chat { from: "lead".to_string(), path: path.to_path_buf(), kebbie_typed: Vec::new() }];
    assert_eq!(capture::tick_in(&state, &chat(&lead), now), ["faults: 1 of the user's words to weigh"]);
    let [taken] = pending(&state).try_into().unwrap();
    assert_eq!(taken.words, "you restarted and the board still shows the wrong task, fix it", "a compaction's summary is never the user's words");
    fs::write(&lead, format!("{}\n{}\n{end}\n", fs::read_to_string(&lead).unwrap().trim_end(), user("and the second thing is wrong too, look at it"))).unwrap();
    assert_eq!(capture::tick_in(&state, &[], now + 30), Vec::<String>::new(), "a chat missing from one tick keeps its mark");
    assert_eq!(capture::tick_in(&state, &chat(&lead), now + 60), ["faults: 1 of the user's words to weigh"]);
    assert_eq!(pending(&state).len(), 2);
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_chat_still_a_day_after_its_last_words_is_never_read_again() {
    let state = state("still-chat");
    let user = |text: &str| serde_json::json!({ "type": "user", "uuid": "u", "timestamp": "2026-11-26T08:00:00.000Z", "message": { "role": "user", "content": text } }).to_string();
    let end = serde_json::json!({ "type": "system", "subtype": "turn_duration", "timestamp": "2026-11-26T08:00:06.000Z" }).to_string();
    let now = crate::messages::now();
    assert!(capture::tick_in(&state, &[], now - 60).is_empty());
    let task = state.join("waiting-task.jsonl");
    fs::write(&task, format!("{}\n{end}\n", user("the board shows this task as done but it is still running"))).unwrap();
    let chat = [capture::Chat { from: "billing@app".to_string(), path: task, kebbie_typed: Vec::new() }];
    assert_eq!(capture::tick_in(&state, &chat, now), ["faults: 1 of the user's words to weigh"]);
    settle(&state, "c1", "no fault in kebbie", None).unwrap();
    assert!(capture::tick_in(&state, &chat, now + DAY + 1).is_empty(), "a task that waited a day keeps its place in its chat");
    assert!(capture::tick_in(&state, &[], now + DAY + 31).is_empty());
    assert!(capture::tick_in(&state, &chat, now + DAY + 61).is_empty(), "a still chat missing from one tick keeps its place too");
    assert!(pending(&state).is_empty());
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn the_prompts_kebbie_types_into_a_new_chat_are_never_the_users_words() {
    let state = state("start-prompt");
    let user = |text: &str| serde_json::json!({ "type": "user", "uuid": "u", "timestamp": "2026-11-26T08:00:00.000Z", "message": { "role": "user", "content": text } }).to_string();
    let reply = |text: &str| serde_json::json!({ "type": "assistant", "uuid": "a", "timestamp": "2026-11-26T08:00:05.000Z", "message": { "stop_reason": "end_turn", "content": [{ "type": "text", "text": text }] } }).to_string();
    let end = serde_json::json!({ "type": "system", "subtype": "turn_duration", "timestamp": "2026-11-26T08:00:06.000Z" }).to_string();
    let now = crate::messages::now();
    assert!(capture::tick_in(&state, &[], now - 60).is_empty());
    let prompt = "The push line is broken again:\n  a task falls out of it unpushed. Fix the cause.";
    let task = state.join("task.jsonl");
    fs::write(&task, format!("{}\n{}\n{end}\n{}\n{end}\n{}\n{end}\n", user("The push line is broken again: a task falls out of it unpushed. Fix the cause."), reply("On it."), user("this task is wrong already, it rebuilt the old screen"), user("Use the new card, the old one is broken"))).unwrap();
    let lead = state.join("lead.jsonl");
    fs::write(&lead, format!("{}\n{}\n{end}\n", user("kebbie started you on a new conversation, as your last one's context was nearly full. Read your handover in /s/kebbie/lead-handover.md before anything else and carry on from it. You have 2 open promises that kebbie keeps for you: kebbie promises lists them, and kebbie reminds you when each is due."), reply("Read it."))).unwrap();
    let chats = [capture::Chat { from: "billing@app".to_string(), path: task, kebbie_typed: vec![prompt.to_string(), "Use the new card, the old one is broken".to_string()] }, capture::Chat { from: "lead".to_string(), path: lead, kebbie_typed: Vec::new() }];
    assert_eq!(capture::tick_in(&state, &chats, now), ["faults: 1 of the user's words to weigh"]);
    let [taken] = pending(&state).try_into().unwrap();
    assert_eq!((taken.from.as_str(), taken.words.as_str()), ("billing@app", "this task is wrong already, it rebuilt the old screen"), "a task's start prompt, a prompt kebbie typed on its restart and the lead's handover are kebbie's words");
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_board_answer_is_a_case_only_when_it_corrects() {
    let state = state("board");
    let answer = |label: Option<&str>, comment: &str| capture::board_answer_in(&state, "billing@app", "Approve the plan for the export?", label, comment, 10).unwrap();
    assert_eq!(answer(Some("Approve"), ""), None);
    assert_eq!(answer(Some("Approve with a comment"), "fine"), None);
    assert!(answer(Some("Reject with a comment"), "this plan builds the instance, not the cause").is_some());
    assert!(answer(Some("Something is wrong"), "").is_some());
    assert!(answer(None, "you asked this twice already").is_some());
    assert!(answer(Some("Change it"), "the review asked the wrong thing").is_some(), "a review's Change it corrects");
    assert_eq!(answer(Some("Confirm"), ""), None);
    let words: Vec<String> = pending(&state).into_iter().map(|case| case.words).collect();
    assert_eq!(words, ["this plan builds the instance, not the cause", "Something is wrong", "you asked this twice already", "the review asked the wrong thing"]);
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_fault_seen_again_gets_one_fix_task_and_a_closed_one_stays_the_users_call() {
    use watch::{Step, step};
    let issue = |status: &str, fix: Option<&str>| Issue { id: "3f9a2c".to_string(), status: status.to_string(), fix: fix.map(str::to_string), ..Issue::default() };
    assert_eq!(step(&issue("open", None), None), Step::Queue { came_back: false });
    assert_eq!(step(&issue("open", Some("start-queues-overlap")), Some("start-queues-overlap@kebbie".to_string())), Step::Tell("start-queues-overlap@kebbie".to_string()), "a task on it hears of it, and no second one starts");
    assert_eq!(step(&issue("retry", Some("fault-3f9a2c")), None), Step::Wait("fault-3f9a2c".to_string()), "its fix ended but is not in use yet: no fault-3f9a2c-2");
    let rechecked = Issue { history: vec!["fixed in 0.42.0 (abc1234), checked automatically".to_string()], ..issue("fixed", Some("fault-3f9a2c")) };
    assert_eq!(step(&rechecked, None), Step::Queue { came_back: true }, "only the keeper's recheck puts a fix in use");
    assert_eq!(step(&issue("fixed", Some("fault-3f9a2c")), None), Step::Keep, "an issue closed by hand stays closed");
    assert_eq!(step(&issue("wontfix", None), None), Step::Keep);
    assert_eq!(step(&issue("duplicate", Some("x")), None), Step::Keep);
}

#[test]
fn an_issue_the_user_closed_is_never_reopened_by_the_watcher() {
    let state = state("wontfix");
    let dir = issues::dir(&state);
    issues::save(&dir, &Issue { id: "aa11bb".to_string(), title: "x".to_string(), kind: "bug".to_string(), status: "wontfix".to_string(), ..Issue::default() }).unwrap();
    let said = case(&state, Source::Chat, "lead", "that thing happened again today", 10);
    let (outcome, _) = watch::act(&state, &said, &Verdict { kind: Some(Kind::Known("aa11bb".to_string())), ..Verdict::default() }, None, 10).unwrap();
    assert!(outcome.contains("stays wontfix"), "{outcome}");
    assert_eq!(issues::find(&dir, "aa11bb").unwrap().status, "wontfix");
    assert!(!occurrences(&state, "aa11bb")[0].came_back);
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_task_the_watcher_names_is_kept_by_its_bare_name() {
    assert_eq!(watch::task_name("start-queues-overlap@kebbie (queued)"), "start-queues-overlap");
    assert_eq!(watch::task_name("`fault-3f9a2c`"), "fault-3f9a2c");
    let [known] = watch::parse("case: c6\nverdict: known 6f6168\nreason: same\ntask: Start köar en task (start-queues-overlap@kebbie) (queued)\n").try_into().unwrap();
    assert_eq!(known.task.as_deref(), Some("start-queues-overlap"), "the brief shows a task's title beside its address");
}

#[test]
fn the_watchers_question_reaches_a_vault_backlog_and_a_local_one() {
    use crate::backlog_file::{Shelf, file_item_on};
    use crate::config::Source as Backlog;
    for source in [Backlog::Vault, Backlog::Local] {
        let (state, dir) = (state("ask-state"), state("ask-dir"));
        let told = std::cell::RefCell::new(Vec::new());
        let tell = |text: &str| -> Result<()> { told.borrow_mut().push(text.to_string()); Ok(()) };
        let shelf = Shelf { dir: &dir, state: &state, tell: &tell, write: &tell };
        let said = case(&state, Source::Chat, "lead", "the board should not do that, or should it?", 10);
        let verdict = Verdict { kind: Some(Kind::Ask), title: Some("The board hides a parked task".to_string()), reason: "two readings".to_string(), ..Verdict::default() };
        let question = "Do you want parked tasks shown on the board?";
        let filed = watch::ask_with(&said, &verdict, question, |filing, from_lead| file_item_on(&shelf, &[source], filing, from_lead));
        assert!(filed.is_ok(), "{source:?}: {filed:?}");
        let held = told.borrow().join("\n") + &fs::read_dir(&dir).map(|entries| entries.filter_map(|entry| fs::read_to_string(entry.ok()?.path()).ok()).collect::<String>()).unwrap_or_default();
        assert!(held.contains(&format!("- [ ] {question}")), "{source:?}: the question waits as the item's open question: {held}");
        let _ = (fs::remove_dir_all(state), fs::remove_dir_all(dir));
    }
}

#[test]
fn a_case_settled_without_a_verdict_goes_back_to_the_lead_with_the_users_words() {
    let state = state("unweighed");
    let said = case(&state, Source::Board, "billing@app", "this plan builds the instance, not the cause", 10);
    let told = watch::unweighed(&said, "the watcher gave no verdict on it");
    assert!(told.starts_with("The fault watcher could not weigh case c1 (the user's answer on the board, from billing@app): the watcher gave no verdict on it."), "{told}");
    assert!(told.contains("\"this plan builds the instance, not the cause\"") && told.contains("kebbie:propose-change"), "{told}");
    let heard = std::cell::RefCell::new(Vec::new());
    let tell = |text: &str, _: bool| heard.borrow_mut().push(text.to_string());
    let cases = [said.clone(), case(&state, Source::Chat, "lead", "the push line stands still again", 20), case(&state, Source::Chat, "lead", "thanks, that looks right now", 30)];
    let answer = "case: c1\nverdict: new\nreason: r\n\ncase: c3\nverdict: ignore\nreason: thanks\n";
    let act = |case: &Case, _: &Verdict| if case.id == "c1" { Err("a new fault names no title".to_string()) } else { Ok(("no fault in kebbie: thanks".to_string(), None)) };
    watch::settle_all(&state, &cases, answer, act, tell).unwrap();
    assert!(pending(&state).is_empty());
    let heard_of: Vec<bool> = cases.iter().map(|case| heard.borrow().iter().any(|text| text.contains(&format!("case {} (", case.id)))).collect();
    assert_eq!(heard_of, [true, true, false], "a verdict not carried out and a missing verdict reach the lead, an ignore does not: {:?}", heard.borrow());
    let spent = Case { tries: 2, ..case(&state, Source::Chat, "lead", "the board lost my answer again", 40) };
    heard.borrow_mut().clear();
    assert_eq!(watch::drop_spent(&state, std::slice::from_ref(&spent), tell).len(), 1);
    assert!(pending(&state).is_empty() && heard.borrow()[0].contains("no answer on it in 2 runs") && heard.borrow()[0].contains("the board lost my answer again"), "{:?}", heard.borrow());
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_short_correction_is_weighed_and_a_one_word_answer_is_not() {
    assert!(capture::worth("board is wrong") && capture::worth("fel igen"));
    assert!(!capture::worth("ja") && !capture::worth("2") && !capture::worth("/clear now"));
}

#[test]
fn the_running_mark_stays_until_every_verdict_is_carried_out() {
    let state = state("running");
    let said = case(&state, Source::Chat, "lead", "the push line stands still again", 10);
    let ids = [said.id.clone()];
    let act = |_: &Case, _: &Verdict| {
        assert!(watch::is_running(&state), "a tick while the verdicts are carried out sees the watcher running");
        Ok(("no fault in kebbie: r".to_string(), None))
    };
    watch::run_with(&state, &ids, |_| Ok("case: c1\nverdict: ignore\nreason: r\n".to_string()), act, |_, _| {}).unwrap();
    assert!(!watch::is_running(&state) && pending(&state).is_empty());
    let again = case(&state, Source::Chat, "lead", "and the board is wrong too", 20);
    assert!(watch::run_with(&state, &[again.id], |_| Err("claude timed out".to_string()), |_: &Case, _: &Verdict| unreachable!(), |_, _| {}).is_err());
    assert!(!watch::is_running(&state), "a run that failed leaves no mark behind");
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_fix_task_reset_before_it_landed_lets_go_of_its_issue() {
    let state = state("unlanded");
    let dir = issues::dir(&state);
    let filed = |id: &str, fix: &str| issues::save(&dir, &Issue { id: id.to_string(), title: id.to_string(), kind: "bug".to_string(), status: "open".to_string(), by: BY.to_string(), fix: Some(fix.to_string()), ..Issue::default() }).unwrap();
    filed("aa0001", "fault-aa0001");
    filed("aa0002", "fault-aa0002");
    filed("aa0003", "fault-aa0003");
    let end = |task: &str, how: &str| crate::messages::fold(&serde_json::json!({ "id": format!("e-{task}"), "at": 5, "from": "kebbie", "to": format!("{task}@kebbie"), "text": format!("{task} ended ({how})"), "state": "ended" }).to_string());
    let log: Vec<crate::messages::Message> = [end("fault-aa0001", "reset"), end("fault-aa0002", "done")].into_iter().flatten().collect();
    let notes = watch::release_unlanded(&state, |task| (task != "fault-aa0003").then(|| watch::ended_how(&log, task)).flatten());
    assert_eq!(watch::ended_how(&log, "fault-aa0003"), None);
    assert_eq!(notes, ["faults: kebbie issue aa0001 waits for a fix again, as fault-aa0001 ended (reset)"]);
    let fix = |id: &str| issues::find(&dir, id).unwrap().fix;
    assert_eq!((fix("aa0001"), fix("aa0002").as_deref(), fix("aa0003").as_deref()), (None, Some("fault-aa0002"), Some("fault-aa0003")), "a landed fix waits for the recheck and a running one keeps its issue");
    assert_eq!(watch::step(&issues::find(&dir, "aa0001").unwrap(), None), watch::Step::Queue { came_back: false }, "its next occurrence queues a new fix");
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn the_fault_watcher_is_off_without_its_key_and_kebbie_faults_says_so() {
    assert!(!crate::dev::on(), "a machine whose config sets no kebbie_dev.fault_watcher runs no watcher");
    let state = state("off");
    assert!(capture::tick(&state).is_empty() && watch::tick(&state).is_empty() && pending(&state).is_empty());
    assert!(!dir(&state).join("read.json").exists(), "the watcher reads no chat while off");
    assert_eq!(list_in(&state, false, false), OFF);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&list_in(&state, true, false)).unwrap()["on"], false);
    let _ = fs::remove_dir_all(state);
}

#[test]
fn a_fault_the_lead_reports_is_weighed_at_once_and_alone() {
    let state = state("pointed");
    let waiting = case(&state, Source::Chat, "lead", "the board lost my answer again", 1_000);
    let id = add_pointed(&state, "kebbie forgot my plan again, fix it", 1_010).unwrap();
    let cases = pending(&state);
    let pointed = cases.iter().find(|case| case.id == id).unwrap();
    assert!(pointed.pointed && pointed.from == "lead" && pointed.context == POINTED_CONTEXT);
    assert_eq!(watch::due(&cases, 1_000, 1_011), [id.as_str()], "neither the two minutes nor the gap since the last run hold it, and the other case waits");
    assert!(watch::brief(&state, std::slice::from_ref(pointed), Path::new("/k"), "English").contains("(the user's words, which the lead reported as a fault in kebbie)"));
    settle(&state, &id, "no fault in kebbie: r", None).unwrap();
    assert_eq!(watch::due(&pending(&state), 0, 1_200), [waiting.id], "the rest are weighed as before once it is settled");
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn the_users_words_the_lead_reports_are_weighed_once() {
    let state = state("pointed-once");
    let held = case(&state, Source::Chat, "lead", "Look, the push line is stuck again.  Fix it now", 1_000);
    assert_eq!(add_pointed(&state, "the push line is stuck again. fix it now", 1_020).unwrap(), held.id, "the case the capture holds becomes the lead's");
    assert!(pending(&state).iter().all(|case| case.pointed) && pending(&state).len() == 1);
    assert!(add_pointed(&state, "  ", 1_030).is_err());
    let state2 = crate::sh::test_dir("faults-pointed-capture");
    let transcript = state2.join("lead.jsonl");
    let user = |text: &str| serde_json::json!({ "type": "user", "uuid": "u", "timestamp": "2026-11-26T08:00:00.000Z", "message": { "role": "user", "content": text } }).to_string();
    let end = serde_json::json!({ "type": "system", "subtype": "turn_duration", "timestamp": "2026-11-26T08:00:06.000Z" }).to_string();
    fs::write(&transcript, "").unwrap();
    let chats = vec![capture::Chat { from: "lead".to_string(), path: transcript.clone(), kebbie_typed: Vec::new() }];
    let typed = 1_795_680_000;
    assert!(capture::tick_in(&state2, &chats, typed - 60).is_empty());
    let id = add_pointed(&state2, "the board shows the wrong task again", typed + 40).unwrap();
    fs::write(&transcript, format!("{}\n{end}\n{}\n{end}\n", user("The board shows the wrong task again!"), user("and now the push line hangs too"))).unwrap();
    assert_eq!(capture::tick_in(&state2, &chats, typed + 600), ["faults: 1 of the user's words to weigh"], "the words the lead reported after they were typed are not weighed a second time");
    assert!(pending(&state2).iter().any(|case| case.id == id) && pending(&state2).iter().any(|case| case.words == "and now the push line hangs too"));
    assert!(!pointed_after(&state2, "lead", "the board shows the wrong task again", typed + 60), "words typed after the report are the user's anew");
    fs::remove_dir_all(state).unwrap();
    fs::remove_dir_all(state2).unwrap();
}

#[test]
fn only_the_lead_or_the_user_reports_a_fault_and_the_lead_may_only_where_the_watcher_is_on() {
    let state = state("report");
    let task = crate::permit::Caller::Task("billing@app".to_string());
    assert!(report_as(&state, &task, "the board is wrong", 10).unwrap_err().contains("kebbie fault is the lead's"));
    assert!(pending(&state).is_empty());
    let (off, on) = (crate::lead::settings_for("/opt/kebbie/kebbie", false), crate::lead::settings_for("/opt/kebbie/kebbie", true));
    assert!(!off.contains("kebbie fault:") && on.contains("Bash(/opt/kebbie/kebbie fault:*)"), "{off}");
    let said = report_as(&state, &crate::permit::Caller::Lead, "the board is wrong", 10).unwrap();
    assert!(said.starts_with("Case c1 goes to the fault watcher now") && said.contains("Its verdict comes to you as a message"), "{said}");
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_pointed_out_fault_starts_its_fix_at_once_past_the_daily_cap_and_two_build_at_a_time() {
    let none: [String; 0] = [];
    assert_eq!(watch::start_of(true, 100 + DAY, &none, 500), watch::Start { at: 500, after: Vec::new(), counted: false }, "the daily cap holds no fault the user pointed out, and it uses up none of the day's three");
    assert_eq!(watch::start_of(false, 100 + DAY, &none, 500), watch::Start { at: 100 + DAY, after: Vec::new(), counted: true }, "the watcher's own finds keep the cap");
    let one = ["fault-aa0001".to_string()];
    assert!(watch::start_of(true, 500, &one, 500).after.is_empty(), "a second fix builds beside the first");
    let two = ["fault-aa0001".to_string(), "fault-aa0002".to_string()];
    assert_eq!(watch::start_of(true, 500, &two, 500).after, ["fault-aa0002"], "a third waits behind the newest, so the waiting ones form one line");
    let state = state("in-flight");
    for task in ["fault-aa0001", "fault-aa0002", "fault-aa0003"] {
        started_fix(&state, task).unwrap();
    }
    assert_eq!(in_flight(&state, |_| true).unwrap(), ["fault-aa0001", "fault-aa0002", "fault-aa0003"], "queued ones count, in the order they were started");
    assert_eq!(watch::start_of(true, 500, &in_flight(&state, |_| true).unwrap(), 500).after, ["fault-aa0003"], "a fourth waits behind the third, never beside it");
    assert_eq!(in_flight(&state, |task| task != "fault-aa0001").unwrap(), ["fault-aa0002", "fault-aa0003"], "an ended one is let go");
    assert_eq!(in_flight(&state, |_| true).unwrap(), ["fault-aa0002", "fault-aa0003"]);
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn the_lead_hears_every_verdict_on_a_pointed_fault_as_a_message_and_confirms_once_the_fix_is_in_use() {
    let state = state("pointed-verdict");
    let ignored = add_pointed(&state, "the board is slow today", 10).unwrap();
    let filed = add_pointed(&state, "the lead asked me in the chat again", 11).unwrap();
    let lost = add_pointed(&state, "kebbie lost my answer", 12).unwrap();
    let plain = case(&state, Source::Chat, "billing@app", "the push line stands still", 13);
    let cases: Vec<Case> = pending(&state);
    let heard = std::cell::RefCell::new(Vec::new());
    let tell = |text: &str, asks: bool| heard.borrow_mut().push((text.to_string(), asks));
    let answer = format!("case: {ignored}\nverdict: ignore\nreason: the host was busy\n\ncase: {filed}\nverdict: new\nreason: r\n\ncase: {}\nverdict: ignore\nreason: thanks\n", plain.id);
    let act = |case: &Case, verdict: &Verdict| match verdict.kind {
        Some(Kind::New) => Ok((format!("filed kebbie issue aa0009: queued fault-aa0009 ({})", case.id), Some("aa0009".to_string()))),
        _ => Ok((format!("no fault in kebbie: {}", verdict.reason), None)),
    };
    watch::settle_all(&state, &cases, &answer, act, tell).unwrap();
    let heard = heard.into_inner();
    assert_eq!(heard.len(), 3, "each pointed case, and no word on the plain one: {heard:?}");
    assert!(heard.iter().all(|(_, asks)| *asks), "every word on a pointed case is one the lead acts on: {heard:?}");
    assert!(heard[0].0.contains(&format!("(case {ignored}, their words: \"the board is slow today\"): no fault in kebbie: the host was busy")) && heard[0].0.contains("Tell the user in one line what it found"), "{}", heard[0].0);
    assert!(heard[1].0.contains("queued fault-aa0009") && heard[1].0.contains("kebbie recipe <task>"), "{}", heard[1].0);
    assert!(heard[2].0.starts_with(&format!("The fault watcher could not weigh case {lost} (the user's words, which the lead reported as a fault in kebbie")), "{}", heard[2].0);
    assert_eq!(awaiting_confirm(&state), ["aa0009"]);
    let dir = issues::dir(&state);
    let issue = Issue { id: "aa0009".to_string(), title: "The lead asks in its chat".to_string(), kind: "bug".to_string(), status: "open".to_string(), by: BY.to_string(), fix: Some("fault-aa0009".to_string()), ..Issue::default() };
    issues::save(&dir, &issue).unwrap();
    record_occurrence(&state, "aa0009", &Occurrence { at: 11, case: filed.clone(), source: Source::Chat, from: "lead".to_string(), quote: "the lead asked me in the chat again".to_string(), came_back: false }).unwrap();
    let asked = std::cell::RefCell::new(Vec::new());
    let ask = |text: &str, asks: bool| asked.borrow_mut().push((text.to_string(), asks));
    assert!(watch::ask_confirm(&state, ask).is_empty(), "the fix is not in use yet");
    issues::save(&dir, &Issue { status: "fixed".to_string(), history: vec!["fixed: checked automatically".to_string()], ..issue }).unwrap();
    assert_eq!(watch::ask_confirm(&state, ask).len(), 1);
    assert!(watch::ask_confirm(&state, ask).is_empty(), "asked once");
    let asked = asked.into_inner();
    assert!(asked.len() == 1 && asked[0].1 && asked[0].0.contains("is in use in the installed kebbie: reproduce the fault as the user met it (their words: \"the lead asked me in the chat again\")"), "{asked:?}");
    assert!(awaiting_confirm(&state).is_empty());
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn a_watcher_that_cannot_start_counts_a_try_and_a_pointed_case_reaches_the_lead_instead_of_retrying_unheard() {
    let state = state("start-fails");
    let id = add_pointed(&state, "the board froze again", 10).unwrap();
    let fails = |_: &str| Err("kebbie found no claude on the keeper's PATH".to_string());
    for (at, tries) in [(20, 1), (50, 2)] {
        let said = watch::start_due(&state, &pending(&state), at, fails).unwrap();
        assert!(said.contains("could not start"), "{said}");
        assert_eq!(pending(&state)[0].tries, tries, "each failed start counts");
    }
    assert_eq!(watch::start_due(&state, &pending(&state), 80, fails), None, "a case two starts failed on is not started again");
    let heard = std::cell::RefCell::new(Vec::new());
    assert_eq!(watch::drop_spent(&state, &pending(&state), |text: &str, asks: bool| heard.borrow_mut().push((text.to_string(), asks))).len(), 1);
    assert!(pending(&state).is_empty());
    let heard = heard.into_inner();
    assert!(heard.len() == 1 && heard[0].1 && heard[0].0.contains(&format!("could not weigh case {id}")) && heard[0].0.contains("the board froze again"), "{heard:?}");
    let started = add_pointed(&state, "and the push line too", 90).unwrap();
    assert_eq!(watch::start_due(&state, &pending(&state), 100, |payload: &str| { assert_eq!(payload, started); Ok(()) }).as_deref(), Some("faults: the fault watcher weighs 1 case"));
    assert_eq!(pending(&state)[0].tries, 0, "a start that worked leaves the try to the run");
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn the_board_lists_the_open_faults_first_then_the_fixes_in_use_and_leaves_the_rest_off() {
    let state = state("board");
    let dir = issues::dir(&state);
    let fault = |id: &str, status: &str, fix: Option<&str>, history: &[&str]| Issue { id: id.to_string(), title: format!("fault {id}"), kind: "bug".to_string(), status: status.to_string(), by: BY.to_string(), fix: fix.map(str::to_string), history: history.iter().map(|line| line.to_string()).collect(), ..Issue::default() };
    issues::save(&dir, &fault("aa0001", "fixed", Some("fault-aa0001"), &["fixed in 0.42.0 (abc1234), checked automatically"])).unwrap();
    issues::save(&dir, &fault("aa0002", "open", Some("fault-aa0002"), &[])).unwrap();
    issues::save(&dir, &fault("aa0003", "wontfix", None, &[])).unwrap();
    issues::save(&dir, &fault("aa0004", "open", None, &[])).unwrap();
    issues::save(&dir, &Issue { id: "bb0001".to_string(), title: "a wish the user saved".to_string(), kind: "wish".to_string(), status: "open".to_string(), ..Issue::default() }).unwrap();
    let shown: Vec<(String, String)> = board(&state).into_iter().map(|fault| (fault.id, fault.word)).collect();
    assert_eq!(shown, [("aa0002".to_string(), "fixing in fault-aa0002".to_string()), ("aa0004".to_string(), "open".to_string()), ("aa0001".to_string(), "in use".to_string())]);
    fs::remove_dir_all(state).unwrap();
}
