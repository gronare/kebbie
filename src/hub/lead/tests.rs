use std::cell::RefCell;
use std::collections::BTreeMap;

use kebbie_hub_protocol::{Away, Kind, LeadMessage, Prefix, Turn, open_blob};

use super::*;

fn said(index: usize) -> Turn {
    Turn::Lead { id: format!("t-{index}"), at: 1_000 + index as u64, text: format!("reply {index}") }
}

fn thread(count: usize) -> Thread {
    Thread { turns: (0..count).map(said).collect(), working_since: None, replied_at: Some(1_000 + count as u64), ..Thread::default() }
}

type Hub = RefCell<BTreeMap<String, Vec<u8>>>;

fn sealed(dir: &Path, thread: &Thread, now: u64, hub: &Hub, puts: &RefCell<usize>) -> Box<LeadRef> {
    let put = |id: &Id, ciphertext: &[u8]| {
        *puts.borrow_mut() += 1;
        hub.borrow_mut().insert(id.to_string(), ciphertext.to_vec());
        Ok(())
    };
    let delete = |id: &Id| {
        hub.borrow_mut().remove(id.as_str());
        Ok(())
    };
    seal(dir, thread, now, put, delete).unwrap()
}

fn pages(lead: &LeadRef, hub: &Hub) -> Vec<LeadThread> {
    let mut pages = Vec::new();
    let mut next = Some(lead.thread.clone());
    while let Some(blob) = next {
        let page = LeadThread::read(&open_blob(&blob, &hub.borrow()[blob.id.as_str()]).unwrap()).unwrap();
        next = page.older.clone();
        pages.push(page);
    }
    pages
}

#[test]
fn the_thread_goes_up_in_pages_of_fifty_oldest_first_and_only_the_newest_page_changes_as_the_lead_talks() {
    let dir = crate::sh::test_dir("lead-pages");
    let (hub, puts) = (RefCell::new(BTreeMap::new()), RefCell::new(0));
    let first = sealed(&dir, &thread(120), 10, &hub, &puts);
    let read = pages(&first, &hub);
    assert_eq!(read.iter().map(|page| page.turns.len()).collect::<Vec<_>>(), [20, 50, 50], "the newest page first, each naming the one before");
    assert_eq!((read[2].older.as_ref(), read[2].turns[0].id()), (None, "t-0"));
    assert_eq!((first.thread.media_type.as_str(), *puts.borrow()), (LEAD_THREAD_MEDIA_TYPE, 3));

    assert_eq!(sealed(&dir, &thread(120), 20, &hub, &puts), first, "at rest the board names the same page");
    assert_eq!(*puts.borrow(), 3, "nothing is put again within the day");
    let renewed = sealed(&dir, &thread(120), 20 + REPUT, &hub, &puts);
    assert_eq!((renewed.thread.clone(), *puts.borrow()), (first.thread.clone(), 6), "a day on, each page is put again as the same blob, which the hub keeps alive");

    let grown = sealed(&dir, &thread(121), 30 + REPUT, &hub, &puts);
    assert_ne!(grown.thread.id, first.thread.id);
    assert_eq!(*puts.borrow(), 7, "only the newest page is sealed again");
    assert!(!hub.borrow().contains_key(first.thread.id.as_str()), "the page it replaced is deleted from the hub");
    assert_eq!(pages(&grown, &hub)[1].older, read[1].older, "the older pages are the same blobs");

    let quiet = sealed(&crate::sh::test_dir("lead-empty"), &Thread::default(), 10, &hub, &puts);
    assert!(pages(&quiet, &hub)[0].turns.is_empty(), "a lead that said nothing yet has one empty page");
}

#[test]
fn a_thread_longer_than_the_phone_pages_back_through_starts_at_its_newest_thousand_turns() {
    let dir = crate::sh::test_dir("lead-long");
    let (hub, puts) = (RefCell::new(BTreeMap::new()), RefCell::new(0));
    let read = pages(&sealed(&dir, &thread(MAX_PAGES * 50 + 70), 10, &hub, &puts), &hub);
    assert_eq!(read.len(), MAX_PAGES);
    assert_eq!(read.last().unwrap().turns[0].id(), "t-100", "the oldest pages fall away whole");
}

fn asked<'a>(command: &'a Id, device: &'a Id, at: u64, dir: &'a Path) -> ToLead<'a> {
    ToLead { command, device, at, exp: at + Kind::LeadMessage.default_ttl(), dir }
}

#[test]
fn a_message_from_a_device_waits_in_the_log_as_your_word_until_the_lead_takes_it_and_a_machine_without_a_lead_refuses_it() {
    let state = crate::sh::test_dir("lead-take");
    let (command, device) = (Id::new(Prefix::Command).unwrap(), Id::new(Prefix::Device).unwrap());
    let phone = Caller::Hub { account: "acc".to_string(), name: "iPhone".to_string() };
    let message = Payload::LeadMessage(LeadMessage { text: "Hold the push".to_string() });
    let flushed = RefCell::new(0);
    let flush = || *flushed.borrow_mut() += 1;
    let refused = take_in(&state, &phone, &asked(&command, &device, 100, &state), &message, 100, &flush);
    assert_eq!(refused, Taken::Now((Outcome::Refused { reason: Reason::NoLead }, "Not delivered: no lead has run on this machine yet; start kebbie there and send it again".to_string())));
    std::fs::write(state.join("lead-session"), "abc").unwrap();
    assert!(matches!(take_in(&state, &phone, &asked(&command, &device, 100, &state), &message, 100, &flush), Taken::Later(_)));
    assert_eq!(*flushed.borrow(), 1, "it is tried at once");
    let held = crate::messages::load_in(&state);
    assert!(matches!(held.as_slice(), [one] if one.held && one.from == crate::messages::USER_LABEL && one.text == "(from iPhone) Hold the push" && one.kind.as_deref() == Some(crate::messages::DEVICE_KIND)), "{held:?}");
    assert!(matches!(take_in(&state, &phone, &asked(&command, &device, 100, &state), &message, 101, &flush), Taken::Later(said) if said.contains("already waits")));
    assert_eq!((crate::messages::load_in(&state).len(), lead_thread::load(&state).sent.len()), (1, 1), "the same command is held once");
}

#[test]
fn of_two_away_switches_the_one_asked_for_last_wins_whichever_arrives_last() {
    let state = crate::sh::test_dir("lead-away");
    let device = Id::new(Prefix::Device).unwrap();
    let phone = Caller::Hub { account: "acc".to_string(), name: "iPhone".to_string() };
    let (later, earlier) = (Id::new(Prefix::Command).unwrap(), Id::new(Prefix::Command).unwrap());
    let on = take_in(&state, &phone, &asked(&later, &device, 200, &state), &Payload::Away(Away { on: true }), 210, &|| {});
    assert!(matches!(&on, Taken::Now((Outcome::Done, text)) if text.starts_with("Away mode is on")), "{on:?}");
    assert_eq!(crate::permit::away_since_in(&state), Some(210));
    let stale = take_in(&state, &phone, &asked(&earlier, &device, 150, &state), &Payload::Away(Away { on: false }), 220, &|| {});
    assert_eq!(stale, Taken::Now((Outcome::Done, "Away mode stays on: a switch asked for after this one decided it.".to_string())));
    assert_eq!(crate::permit::away_since_in(&state), Some(210));
    let switches = lead_thread::load(&state).switches;
    assert_eq!(switches.iter().map(|switch| (switch.id.as_str(), switch.on, switch.device.as_deref())).collect::<Vec<_>>(), [(later.as_str(), true, Some(device.as_str()))], "the switch is a turn of the thread, from the phone");
}
