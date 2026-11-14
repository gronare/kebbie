use std::io::Write;

use kebbie_hub_protocol::{LeadThread, Turn};
use serde_json::json;

use super::*;

const PHONE: &str = "dev_01K7Z0000000000000000000PH";

fn base() -> u64 {
    crate::typing::iso_millis("2026-11-14T10:00:00.000Z").unwrap() / 1000
}

fn stamp(at: u64) -> String {
    let offset = at - base();
    format!("2026-11-14T10:{:02}:{:02}.000Z", offset / 60, offset % 60)
}

fn you(id: &str, at: u64, text: &str) -> String {
    json!({ "type": "user", "uuid": id, "timestamp": stamp(at), "message": { "role": "user", "content": text } }).to_string()
}

fn kebbie_in(id: &str, at: u64, text: &str) -> String {
    json!({ "type": "user", "uuid": id, "isMeta": true, "timestamp": stamp(at), "message": { "role": "user", "content": format!("Another Claude session sent a message:\n<cross-session-message from=\"kebbie\" from-name=\"kebbie\">\n[from kebbie] {text}\n</cross-session-message>") } }).to_string()
}

fn lead(id: &str, at: u64, text: &str, stop: &str) -> String {
    json!({ "type": "assistant", "uuid": id, "timestamp": stamp(at), "message": { "stop_reason": stop, "content": [{ "type": "text", "text": text }] } }).to_string()
}

fn system(subtype: &str, at: u64) -> String {
    json!({ "type": "system", "subtype": subtype, "timestamp": stamp(at) }).to_string()
}

fn events(lines: &[String]) -> Vec<Event> {
    lines.iter().filter_map(|line| event(line)).collect()
}

fn sent(command: &str, at: u64, text: &str) -> Sent {
    Sent { command: command.to_string(), device: PHONE.to_string(), typed: typed("iPhone", text), text: text.to_string(), at, exp: at + 7 * 86_400, message: format!("log-{command}"), dir: PathBuf::from("/nowhere"), delivered: None }
}

fn no_waits(_: &str) -> Option<Waits> {
    None
}

fn pages_check(turns: &[Turn]) {
    for page in turns.chunks(kebbie_hub_protocol::MAX_LEAD_PAGE_TURNS) {
        LeadThread { turns: page.to_vec(), older: None }.check().unwrap();
    }
}

#[test]
fn the_thread_holds_your_words_and_the_leads_replies_to_you_and_none_of_its_tools_hooks_or_talk_with_kebbie() {
    let at = base();
    let lines = [
        you("u1", at + 1, "Run the shop suite"),
        lead("a0", at + 2, "Checking the suite first.", "tool_use"),
        json!({ "type": "user", "uuid": "r1", "timestamp": stamp(at + 3), "message": { "role": "user", "content": [{ "type": "tool_result", "content": "ok" }] } }).to_string(),
        lead("a1", at + 4, "**Done**\n- The suite is green", "end_turn"),
        system("stop_hook_summary", at + 5),
        lead("a2", at + 6, "Nothing to save.", "end_turn"),
        system("turn_duration", at + 7),
        kebbie_in("k1", at + 10, "refunds@shop is ready"),
        lead("a3", at + 11, "Noted: 1 update, nothing for you.", "end_turn"),
        system("turn_duration", at + 12),
        you("c1", at + 13, "<command-name>/clear</command-name>"),
        json!({ "type": "assistant", "uuid": "s1", "isSidechain": true, "timestamp": stamp(at + 14), "message": { "stop_reason": "end_turn", "content": [{ "type": "text", "text": "a subagent" }] } }).to_string(),
        json!({ "type": "attachment", "uuid": "q1", "timestamp": stamp(at + 15), "attachment": { "type": "queued_command", "commandMode": "prompt", "prompt": "and the lint" } }).to_string(),
        json!({ "type": "api-request-blob", "body": "\"type\":\"user\" in a request body" }).to_string(),
    ];
    let thread = assemble(&events(&lines), &Store::default(), &no_waits, None, at + 15, at + 20);
    let shown: Vec<(&str, &str)> = thread.turns.iter().map(|turn| match turn {
        Turn::You { text, .. } => ("you", text.as_deref().unwrap_or_default()),
        Turn::Lead { text, .. } => ("lead", text.as_str()),
    }).collect();
    assert_eq!(shown, [("you", "Run the shop suite"), ("lead", "**Done**\n- The suite is green"), ("you", "and the lint")]);
    assert_eq!((thread.replied_at, thread.working_since), (Some(at + 4), Some(at + 15)), "the queued prompt opened a turn that has not ended");
    assert!(thread.answers.is_empty(), "a reply at the computer while you are here is no notice");
    pages_check(&thread.turns);
    assert_eq!(assemble(&events(&lines), &Store::default(), &no_waits, None, at + 15, at + 15 + STALE_TURN).working_since, None, "a turn the transcript left half an hour ago is no longer shown working");
}

#[test]
fn a_message_from_the_phone_waits_behind_the_turn_or_a_draft_and_counts_as_delivered_once_the_transcript_has_it() {
    let at = base();
    let store = Store { sent: vec![sent("cmd_one", at + 2, "Do the last-admin subtask first."), sent("cmd_two", at + 3, "And hold the push.")], ..Store::default() };
    let waiting = |id: &str| (id == "log-cmd_two").then_some(Waits::Draft);
    let before = [you("u1", at + 1, "Start refunds"), lead("a1", at + 2, "Starting.", "end_turn"), system("turn_duration", at + 3)];
    let thread = assemble(&events(&before), &store, &waiting, None, at + 3, at + 4);
    let deliveries: Vec<Option<Delivery>> = thread.turns.iter().filter_map(|turn| match turn { Turn::You { delivery, device: Some(_), .. } => Some(*delivery), _ => None }).collect();
    assert_eq!(deliveries, [Some(Delivery::WaitingTurn), Some(Delivery::WaitingDraft)]);
    assert!(thread.delivered.is_empty());
    pages_check(&thread.turns);

    let mut after = before.to_vec();
    after.extend([you("u2", at + 9, &typed("iPhone", "Do the last-admin subtask first.")), lead("a2", at + 10, "Understood. I take the subtask now.\nMore below.", "end_turn"), system("turn_duration", at + 11)]);
    let thread = assemble(&events(&after), &store, &waiting, None, at + 11, at + 12);
    assert_eq!(thread.delivered, [("cmd_one".to_string(), at + 9)]);
    assert!(thread.turns.iter().all(|turn| !matches!(turn, Turn::You { text: Some(text), device: None, .. } if text.starts_with("(from"))), "the typed message shows once, as the phone's");
    assert!(matches!(&thread.turns[2], Turn::You { id, device: Some(_), delivery: Some(Delivery::Delivered { at: delivered }), .. } if id == "cmd_one" && *delivered == at + 9), "{:?}", thread.turns);
    assert_eq!(thread.answers, [("a2".to_string(), "Understood. I take the subtask now.\nMore below.".to_string())], "the lead's reply to the phone is a notice");
    assert_eq!(first_line(&thread.answers[0].1), "Understood. I take the subtask now.");
    pages_check(&thread.turns);
}

#[test]
fn away_mode_is_a_turn_of_yours_and_every_reply_while_you_are_away_is_shown_and_told() {
    let at = base();
    let store = Store { switches: vec![Switch { id: "cmd_away".to_string(), at: at + 5, by: at + 5, on: true, device: Some(PHONE.to_string()) }], ..Store::default() };
    let lines = [
        you("u1", at + 1, "Hello"),
        lead("a1", at + 2, "Hi.", "end_turn"),
        system("turn_duration", at + 3),
        kebbie_in("k1", at + 6, &format!("{} Work by the away rules", crate::permit::AWAY_ON)),
        lead("a2", at + 7, "Understood. I settle questions of method myself.", "end_turn"),
        system("turn_duration", at + 8),
        kebbie_in("k2", at + 9, "refunds@shop is ready"),
        lead("a3", at + 10, "Pushed refunds.", "end_turn"),
        system("turn_duration", at + 11),
    ];
    let thread = assemble(&events(&lines), &store, &no_waits, Some(at + 5), at + 11, at + 12);
    assert!(matches!(&thread.turns[2], Turn::You { away: Some(true), text: None, delivery: None, device: Some(_), .. }), "{:?}", thread.turns);
    let replies: Vec<&str> = thread.turns.iter().filter_map(|turn| match turn { Turn::Lead { text, .. } => Some(text.as_str()), _ => None }).collect();
    assert_eq!(replies, ["Hi.", "Understood. I settle questions of method myself.", "Pushed refunds."]);
    assert_eq!(thread.answers.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["a2", "a3"]);
    pages_check(&thread.turns);
}

#[test]
fn the_transcript_is_read_on_from_where_it_was_and_again_from_its_start_when_it_is_replaced() {
    let dir = crate::sh::test_dir("lead-reader");
    let file = dir.join("lead.jsonl");
    let at = base();
    std::fs::write(&file, format!("{}\n{}", you("u1", at, "one"), &you("u2", at + 1, "two")[..20])).unwrap();
    let mut reader = Reader::default();
    reader.read(&file).unwrap();
    assert_eq!(reader.events.len(), 1, "a line still being written waits for its end");
    let offset = reader.offset;
    let rest = you("u2", at + 1, "two")[20..].to_string();
    std::fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(format!("{rest}\n").as_bytes()).unwrap();
    reader.read(&file).unwrap();
    assert_eq!((reader.events.len(), reader.offset > offset), (2, true));
    reader.read(&file).unwrap();
    assert_eq!(reader.events.len(), 2, "nothing new, nothing read");
    std::fs::remove_file(&file).unwrap();
    std::fs::write(&file, format!("{}\n", you("u9", at, "fresh"))).unwrap();
    reader.read(&file).unwrap();
    assert!(matches!(reader.events.as_slice(), [Event::Input { text, .. }] if text == "fresh"));
}

#[test]
fn a_round_answers_what_the_lead_took_lets_go_of_what_expired_and_tells_a_reply_once() {
    let state = crate::sh::test_dir("lead-round");
    let file = state.join("lead.jsonl");
    let at = base();
    let message = crate::messages::hold_for_lead_in(&state, &typed("iPhone", "Hold the push")).unwrap();
    let late = crate::messages::hold_for_lead_in(&state, &typed("iPhone", "Too late")).unwrap();
    let (first, second) = ("cmd_0199a3e0-0000-7000-8000-000000000001", "cmd_0199a3e0-0000-7000-8000-000000000002");
    change(&state, |store| {
        store.sent.push(Sent { message: message.clone(), command: first.to_string(), ..sent("x", at + 2, "Hold the push") });
        store.sent.push(Sent { message: late.clone(), command: second.to_string(), exp: at + 3, ..sent("y", at + 2, "Too late") });
    }).unwrap();
    std::fs::write(&file, format!("{}\n{}\n{}\n", you("u1", at + 1, "Hello"), lead("a1", at + 1, "Hi.", "end_turn"), system("turn_duration", at + 1))).unwrap();
    let answered = std::cell::RefCell::new(Vec::new());
    let told = std::cell::RefCell::new(Vec::new());
    let answer = |_: &Path, result: CommandResult| {
        answered.borrow_mut().push((result.command.to_string(), result.outcome));
        Ok(())
    };
    let notify = |first: &str| told.borrow_mut().push(first.to_string());
    let mut reader = Reader::default();
    tick_from(&state, Some(&file), &mut reader, at + 4, &answer, &notify).unwrap();
    assert_eq!(answered.borrow().as_slice(), [(second.to_string(), Outcome::Refused { reason: Reason::Expired })]);
    assert!(!crate::messages::load_in(&state).iter().any(|held| held.id == late && held.held), "the expired message no longer waits");
    assert_eq!(load(&state).sent.len(), 1);
    std::fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(format!("{}\n{}\n{}\n", you("u2", at + 5, &typed("iPhone", "Hold the push")), lead("a2", at + 6, "Holding it.", "end_turn"), system("turn_duration", at + 7)).as_bytes()).unwrap();
    let thread = tick_from(&state, Some(&file), &mut reader, at + 8, &answer, &notify).unwrap();
    assert_eq!(answered.borrow().last().unwrap(), &(first.to_string(), Outcome::Done));
    assert_eq!(told.borrow().as_slice(), ["Holding it."]);
    assert_eq!(load(&state).sent[0].delivered, Some(at + 5));
    tick_from(&state, Some(&file), &mut reader, at + 9, &answer, &notify).unwrap();
    assert_eq!((answered.borrow().len(), told.borrow().len()), (2, 1), "a round with nothing new answers and tells nothing");
    pages_check(&thread.turns);
}
