use std::path::Path;

use serde_json::json;

use crate::lead::state_dir;
use crate::peers::{self, Session};
use crate::sh::Result;
use crate::status::LEAD_LABEL;
use super::{HAND_KIND, Lock, Message, NOTICE_KIND, ROTATE_BYTES, RULES_KIND, TypeInto, USER_LABEL, append, append_in, load_in, new_id, now, part, send, send_locked, send_locked_in};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Informs,
    Asks,
}

// Every kind of word that reaches the lead. Informs never start a lead turn: they wait in its digest for the user's next prompt (live only while the user is away). Asks are typed into the lead's chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToLead {
    TaskStarted,
    TaskRestarted,
    TaskEnded,
    Pushed,
    PushCancelled,
    PushRebasing,
    Staged,
    Landed,
    CiGreen,
    CiHanded,
    CiDismissed,
    RebaseHanded,
    RebaseStuck,
    ReviewPassed,
    ReviewAgain,
    ReviewWithdrawn,
    RulesBreak,
    Notice,
    Findings,
    RulesCheck,
    Prompt,
    PushFailed,
    PushToLog,
    RedCi,
    CiOutside,
    ChainBlocked,
    NoteUnsettled,
    TaskStopped,
    Permission,
    Away,
    // A task's own `kebbie tell lead`, typed as any tell is, and what kebbie tells the lead to write for a Codex task.
    TaskMessage,
    QuestionCopy,
    BacklogItem,
}

impl ToLead {
    #[cfg_attr(not(test), allow(dead_code))]
    pub const ALL: [ToLead; 33] = [
        ToLead::TaskStarted, ToLead::TaskRestarted, ToLead::TaskEnded, ToLead::Pushed, ToLead::PushCancelled, ToLead::PushRebasing, ToLead::Staged, ToLead::Landed,
        ToLead::CiGreen, ToLead::CiHanded, ToLead::CiDismissed, ToLead::RebaseHanded, ToLead::RebaseStuck, ToLead::ReviewPassed, ToLead::ReviewAgain, ToLead::ReviewWithdrawn,
        ToLead::RulesBreak, ToLead::Notice, ToLead::Findings, ToLead::RulesCheck, ToLead::Prompt, ToLead::PushFailed, ToLead::PushToLog, ToLead::RedCi,
        ToLead::CiOutside, ToLead::ChainBlocked, ToLead::NoteUnsettled, ToLead::TaskStopped, ToLead::Permission, ToLead::Away, ToLead::TaskMessage, ToLead::QuestionCopy, ToLead::BacklogItem,
    ];

    // The one table: whether the lead has something to do, and the kind its log record carries (the away summary reads "push" and "after").
    pub fn route(self) -> (Class, Option<&'static str>) {
        use Class::{Asks, Informs};
        match self {
            ToLead::TaskStarted => (Informs, Some("after")),
            ToLead::TaskRestarted => (Informs, None),
            ToLead::TaskEnded => (Informs, None),
            ToLead::Pushed => (Informs, None),
            ToLead::PushCancelled => (Informs, Some("push")),
            ToLead::PushRebasing => (Informs, Some("push")),
            ToLead::Staged => (Informs, Some("push")),
            ToLead::Landed => (Informs, Some("landed")),
            ToLead::CiGreen => (Informs, Some("ci")),
            ToLead::CiHanded => (Informs, Some("ci")),
            ToLead::CiDismissed => (Informs, Some("ci")),
            ToLead::RebaseHanded => (Informs, Some("follow")),
            ToLead::RebaseStuck => (Informs, Some("follow")),
            ToLead::ReviewPassed => (Informs, None),
            ToLead::ReviewAgain => (Informs, None),
            ToLead::ReviewWithdrawn => (Informs, None),
            ToLead::RulesBreak => (Informs, None),
            ToLead::Notice => (Informs, Some(NOTICE_KIND)),
            ToLead::Findings => (Asks, None),
            ToLead::RulesCheck => (Asks, Some(RULES_KIND)),
            ToLead::Prompt => (Asks, Some(crate::menu::KIND)),
            ToLead::PushFailed => (Asks, Some("push")),
            ToLead::PushToLog => (Asks, Some("push")),
            ToLead::RedCi => (Asks, Some("ci")),
            ToLead::CiOutside => (Asks, Some("ci")),
            ToLead::ChainBlocked => (Asks, Some("after")),
            ToLead::NoteUnsettled => (Asks, None),
            ToLead::TaskStopped => (Asks, Some("recover")),
            ToLead::Permission => (Asks, None),
            ToLead::Away => (Asks, None),
            ToLead::TaskMessage => (Asks, None),
            ToLead::QuestionCopy => (Asks, Some("copy")),
            ToLead::BacklogItem => (Asks, Some("backlog")),
        }
    }

    pub fn class(self) -> Class {
        self.route().0
    }

    pub fn kind(self) -> Option<&'static str> {
        self.route().1
    }
}

// kebbie's word to the lead, routed by its class: an inform to the digest, an ask into the lead's chat (held while it cannot take it, and kept until a lead runs).
pub fn to_lead(what: ToLead, text: &str) -> Result<()> {
    let dir = state_dir()?;
    if what == ToLead::RulesCheck {
        return ask_lead(text).map(|_| ());
    }
    to_lead_in(&dir, what, &crate::status::with_titles_in(&dir, text), &mut tell_lead)
}

pub(super) fn to_lead_in(dir: &Path, what: ToLead, text: &str, ask: &mut dyn FnMut(&str, Option<&str>) -> Result<()>) -> Result<()> {
    match what.class() {
        Class::Informs => fyi_or_live_in(dir, text, what.kind(), ask),
        Class::Asks => ask(text, what.kind()),
    }
}

// kebbie's own word to a task, typed in as any message is.
pub fn tell_from_kebbie(recipients: &[&Session], text: &str, sessions: &[Session]) -> Result<()> {
    send("kebbie", recipients, text, sessions).map(|_| ())
}

// kebbie's word to the lead that must not be lost: with no lead session yet, it waits in the log and reaches the lead once one runs.
fn tell_lead(text: &str, kind: Option<&str>) -> Result<()> {
    let text = &crate::status::with_titles(text);
    let sessions = peers::sessions().unwrap_or_default();
    let _lock = Lock::take(true)?;
    let Some(lead) = sessions.iter().find(|session| session.address == LEAD_LABEL) else {
        let id = new_id("lead");
        return append(&json!({ "id": id, "at": now(), "from": "kebbie", "to": LEAD_LABEL, "text": text, "state": "held", "kind": kind }));
    };
    send_locked("kebbie", &[lead], text, &sessions, None, kind).map(|_| ())
}

// kebbie's word about itself (an update, a resume): held behind a draft like any message, but the draft stays the user's own business; the lead reads it with the user's next prompt.
pub fn notice_locked(session: &Session, text: &str, sessions: &[Session]) -> Result<Vec<String>> {
    notice_locked_in(&state_dir()?, session, text, sessions, &mut crate::inbox::prompt)
}

pub(super) fn notice_locked_in(dir: &Path, session: &Session, text: &str, sessions: &[Session], type_into: TypeInto) -> Result<Vec<String>> {
    if session.address == LEAD_LABEL && crate::permit::away_since_in(dir).is_none() {
        log_for_lead_in(dir, text, Some(NOTICE_KIND))?;
        return Ok(vec![format!("Logged for {LEAD_LABEL}.")]);
    }
    send_locked_in(dir, "kebbie", &[session], text, sessions, None, Some(NOTICE_KIND), type_into)
}

// While the user is away the lead answers for them and writes their summary, so it hears kebbie's notices live.
pub(super) fn fyi_or_live_in(dir: &Path, text: &str, kind: Option<&str>, live: &mut dyn FnMut(&str, Option<&str>) -> Result<()>) -> Result<()> {
    if crate::permit::away_since_in(dir).is_some() {
        return live(text, kind);
    }
    log_for_lead_in(dir, text, kind)
}

pub fn log_for_user_in(dir: &Path, text: &str, kind: &str) -> Result<()> {
    append_in(dir, &json!({ "id": new_id(kind), "at": now(), "from": "kebbie", "to": USER_LABEL, "text": text, "state": "logged", "kind": kind }), ROTATE_BYTES)
}

pub(super) fn log_for_lead_in(dir: &Path, text: &str, kind: Option<&str>) -> Result<()> {
    append_in(dir, &json!({ "id": new_id("fyi"), "at": now(), "from": "kebbie", "to": LEAD_LABEL, "text": text, "state": "logged", "kind": kind, "fyi": true }), ROTATE_BYTES)
}

// kebbie's word to a task that must reach it: held while its box holds the user's draft or it waits for an answer, kept for a session that is not running yet, and typed again when its Claude did not take it.
pub fn hand_to(to: &str, text: &str) -> Result<()> {
    let sessions = peers::sessions().unwrap_or_default();
    let _lock = Lock::take(true)?;
    hand_to_in(&state_dir()?, to, text, &sessions, &mut crate::inbox::prompt)
}

pub(super) fn hand_to_in(dir: &Path, to: &str, text: &str, sessions: &[Session], type_into: TypeInto) -> Result<()> {
    match sessions.iter().find(|session| session.address == to) {
        Some(session) => send_locked_in(dir, "kebbie", &[session], text, sessions, None, Some(HAND_KIND), type_into).map(|_| ()),
        None => append_in(dir, &json!({ "id": new_id("held"), "at": now(), "from": "kebbie", "to": to, "text": text, "state": "held", "kind": HAND_KIND }), ROTATE_BYTES),
    }
}

// The caller holds the lock. A message for a session that has none now, delivered once it runs.
pub fn hold_for(from: &str, to: &str, text: &str, about: Option<&str>, kind: Option<&str>) -> Result<()> {
    let id = new_id("held");
    append(&json!({ "id": id, "at": now(), "from": from, "to": to, "text": text, "state": "held", "about": about, "kind": kind }))
}

// The caller holds the lock. The lead's copy of what the user is asked (about that question, so a copy still held when it is answered is dropped) and answers on the board: typed into its chat while the user is away, so it can settle a question of method; otherwise given with the user's next prompt to it.
pub fn copy_to_lead_locked(text: &str, about: Option<&str>, sessions: &[Session]) -> Result<()> {
    let text = &crate::status::with_titles(text);
    if crate::permit::away_since().is_none() {
        return append(&json!({ "id": new_id("copy"), "at": now(), "from": "kebbie", "to": LEAD_LABEL, "text": text, "state": "logged", "kind": ToLead::QuestionCopy.kind(), "about": about }));
    }
    match sessions.iter().find(|session| session.address == LEAD_LABEL) {
        Some(lead) => send_locked("kebbie", &[lead], text, sessions, about, ToLead::QuestionCopy.kind()).map(|_| ()),
        None => hold_for("kebbie", LEAD_LABEL, text, about, ToLead::QuestionCopy.kind()),
    }
}

// kebbie's own word to a task, held like any message while it waits for an answer.
pub fn notify(to: &str, text: &str) -> Result<()> {
    let sessions = peers::sessions()?;
    let Ok(recipients) = peers::resolve(to, "kebbie", &sessions) else { return Ok(()) };
    send("kebbie", &recipients, text, &sessions).map(|_| ())
}

// When a task starts in another repo, the halves already running learn of it.
pub fn announce(task: &str, repo: &str) -> Result<()> {
    let sessions = peers::sessions()?;
    let me = peers::address(task, repo);
    let Ok(halves) = peers::resolve(task, &me, &sessions) else { return Ok(()) };
    send("kebbie", &halves, &format!("{me} has started: the same task, in {repo}. Keep it informed of changes it depends on."), &sessions).map(|_| ())
}

// From kebbie itself, held like any message while the lead waits for an answer; the lead is asked once about a task at a sha.
pub fn ask_lead(text: &str) -> Result<String> {
    let sessions = peers::sessions()?;
    let _lock = Lock::take(true)?;
    ask_lead_in(&state_dir()?, text, &sessions, &mut crate::inbox::prompt, &|message, lead| lead.pane.as_deref().is_some_and(|pane| crate::typing::taken_by(pane, &part(message), message.at * 1000)))
}

// A check the lead lost (dropped at its inbox, or typed and never taken) is no longer pending, so it is asked again.
pub(super) fn ask_lead_in(dir: &Path, text: &str, sessions: &[Session], type_into: TypeInto, taken: &dyn Fn(&Message, &Session) -> bool) -> Result<String> {
    let lead = sessions.iter().find(|session| session.address == LEAD_LABEL).ok_or("no lead session to check the rules")?;
    let subject = crate::tasks::rules_check_subject(text);
    let pending = |message: &Message| message.held || crate::typing::watching_part_in(dir, LEAD_LABEL, &part(message)) || taken(message, lead);
    let asked = load_in(dir).into_iter().any(|message| message.to == LEAD_LABEL && message.kind.as_deref() == Some(RULES_KIND) && subject.is_some() && crate::tasks::rules_check_subject(&message.text) == subject && pending(&message));
    if asked {
        return Ok(format!("The lead already has this rules check ({}).", subject.unwrap_or_default()));
    }
    Ok(send_locked_in(dir, "kebbie", &[lead], text, sessions, None, Some(RULES_KIND), type_into)?.join("\n"))
}
