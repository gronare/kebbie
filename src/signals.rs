use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use kelpie_hub_api::{Bytes32, Bytes64, ReportChallenge, SignalUpload, SignalsAccepted};
use kelpie_hub_protocol::{ErrorSignals, MAX_SIGNAL_COUNT, MAX_SIGNAL_ROWS, Name, SignalCount, SignalKind, solve};
use serde::{Deserialize, Serialize};

use crate::doctor::{Check, Level};
use crate::hub::http::{CALL, Client};
use crate::sh::Result;

pub const EVERY: u64 = 3600;
pub const DAY: u64 = 86_400;
const LOG: &str = "signals.jsonl";
const BATCH: &str = "signals-batch.jsonl";
const LEDGER: &str = "signals.json";
const LOG_BYTES: u64 = 64 * 1024;
pub const KIND: &str = "signal";
pub const DEPENDENCY: &str = "dependency";
pub const CONTAINS: &str = "kelpie's version and commit, the OS and CPU architecture, the component (a fixed name from kelpie's code, such as keeper.push), the error kind (panic, internal-error or corrupt-state) and a count";

// Every field is a Name or the kind, so no message, path or name from the user's work fits in a line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Counted {
    version: Name,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<Name>,
    component: Name,
    kind: SignalKind,
    #[serde(default = "one")]
    count: u32,
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    Retry(String),
    Final(String),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub sent_at: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

fn dir(state: &Path) -> PathBuf {
    crate::hub::report::dir(state)
}

fn built() -> (Option<Name>, Option<Name>) {
    let commit = env!("KELPIE_BUILT_FROM").split(' ').next().and_then(Name::new);
    (Name::new(env!("CARGO_PKG_VERSION")), commit)
}

pub fn record_in(state: &Path, component: &Name, kind: SignalKind) {
    let (Some(version), commit) = built() else { return };
    let file = dir(state).join(LOG);
    if std::fs::metadata(&file).is_ok_and(|metadata| metadata.len() > LOG_BYTES) {
        return;
    }
    let line = Counted { version, commit, component: component.clone(), kind, count: 1 };
    if let (Ok(mut out), Ok(text)) = (crate::state::append_to(&file), serde_json::to_string(&line)) {
        let _ = writeln!(out, "{text}");
    }
}

fn enabled() -> bool {
    crate::config::report_settings().is_ok_and(|settings| settings.signals())
}

// The component is a name in kelpie's own code, never a value it read.
pub fn record_if(on: &dyn Fn() -> bool, state: &dyn Fn() -> Result<PathBuf>, component: &'static str, kind: SignalKind) {
    let Some(component) = Name::new(component) else { return };
    if on() && let Ok(state) = state() {
        record_in(&state, &component, kind);
    }
}

// Tests count through record_if with a state dir of their own, so none writes into the shared one.
pub fn record(component: &'static str, kind: SignalKind) {
    if !cfg!(test) {
        record_if(&enabled, &crate::lead::state_location, component, kind);
    }
}

pub fn internal(component: &'static str, kind: SignalKind, error: impl Into<String>) -> String {
    record(component, kind);
    error.into()
}

// A panic in kelpie's own source is named after its module; anywhere else (a dependency, the standard library) the location may hold a home path, so it is only "dependency".
pub fn panic_component(file: Option<&str>) -> Name {
    let own = file.and_then(|file| file.strip_prefix("src/")).and_then(|file| file.strip_suffix(".rs")).map(|module| module.trim_end_matches("/mod").replace('/', "."));
    own.and_then(|module| Name::new(&module)).or_else(|| Name::new(DEPENDENCY)).expect("dependency is a name")
}

pub fn install_panic_hook() {
    if !cfg!(test) {
        install_panic_hook_if(enabled(), crate::lead::state_location());
    }
}

pub fn install_panic_hook_if(on: bool, state: Result<PathBuf>) -> bool {
    let (true, Ok(state)) = (on, state) else { return false };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        on_panic(&state, info);
        previous(info);
    }));
    true
}

// The hook is handed the panic's message too, and reads only its location.
fn on_panic(state: &Path, info: &std::panic::PanicHookInfo) {
    record_in(state, &panic_component(info.location().map(|location| location.file())), SignalKind::Panic);
}

fn ledger_file(state: &Path) -> PathBuf {
    dir(state).join(LEDGER)
}

pub fn ledger(state: &Path) -> Ledger {
    std::fs::read_to_string(ledger_file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save_ledger(state: &Path, ledger: &Ledger) -> Result<()> {
    crate::state::write(&ledger_file(state), serde_json::to_string_pretty(ledger).map_err(|error| error.to_string())?)
}

type Folded = BTreeMap<(Name, Option<Name>), BTreeMap<(Name, SignalKind), u32>>;

fn fold(texts: &[String]) -> Folded {
    let mut folded = Folded::new();
    for line in texts.iter().flat_map(|text| text.lines()) {
        let Ok(counted) = serde_json::from_str::<Counted>(line) else { continue };
        let commit_ok = counted.commit.as_ref().is_none_or(|commit| commit.as_str().len() <= kelpie_hub_protocol::MAX_COMMIT_CHARS && commit.as_str().chars().all(|character| character.is_ascii_hexdigit()));
        if !commit_ok || counted.count == 0 {
            continue;
        }
        let count = folded.entry((counted.version, counted.commit)).or_default().entry((counted.component, counted.kind)).or_default();
        *count = count.saturating_add(counted.count).min(MAX_SIGNAL_COUNT);
    }
    folded
}

fn lines(folded: &Folded) -> String {
    folded.iter().flat_map(|((version, commit), rows)| rows.iter().map(move |((component, kind), count)| Counted { version: version.clone(), commit: commit.clone(), component: component.clone(), kind: *kind, count: *count }))
        .filter_map(|counted| serde_json::to_string(&counted).ok()).map(|line| format!("{line}\n")).collect()
}

fn uploads(folded: &Folded) -> Vec<ErrorSignals> {
    let os = Name::new(std::env::consts::OS).unwrap_or_else(|| Name::new("unknown").expect("unknown is a name"));
    let arch = Name::new(std::env::consts::ARCH).unwrap_or_else(|| Name::new("unknown").expect("unknown is a name"));
    let mut out = Vec::new();
    for ((version, commit), rows) in folded {
        let rows: Vec<SignalCount> = rows.iter().map(|((component, kind), count)| SignalCount { component: component.clone(), kind: *kind, count: *count }).collect();
        for chunk in rows.chunks(MAX_SIGNAL_ROWS) {
            out.push(ErrorSignals { version: version.clone(), commit: commit.clone(), os: os.clone(), arch: arch.clone(), counts: chunk.to_vec() });
        }
    }
    out
}

fn without(folded: &mut Folded, sent: &ErrorSignals) {
    let key = (sent.version.clone(), sent.commit.clone());
    if let Some(rows) = folded.get_mut(&key) {
        for row in &sent.counts {
            rows.remove(&(row.component.clone(), row.kind));
        }
        if rows.is_empty() {
            folded.remove(&key);
        }
    }
}

pub fn summary(signals: &ErrorSignals) -> String {
    let rows = signals.counts.iter().map(|row| format!("{} {} (×{})", row.component, row.kind.key(), row.count)).collect::<Vec<_>>().join(", ");
    let built = signals.commit.as_ref().map_or(signals.version.to_string(), |commit| format!("{} ({commit})", signals.version));
    format!("Sent kelpie's maintainers an error signal, a count of kelpie's own failures: {rows}; with kelpie {built} on {} {} and nothing else. [report] signals = false in config.toml turns these off.", signals.os, signals.arch)
}

// A refusal the hub would give again writes its batch off, so one bad batch never holds back every later count.
pub fn tick_in(state: &Path, enabled: bool, now: u64, send: &mut dyn FnMut(&ErrorSignals) -> std::result::Result<(), Refusal>) -> Result<Vec<String>> {
    let log = dir(state).join(LOG);
    let batch = dir(state).join(BATCH);
    if !enabled {
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(&batch);
        return Ok(Vec::new());
    }
    let mut ledger = ledger(state);
    if ledger.sent_at.is_some_and(|at| at / DAY == now / DAY) {
        return Ok(Vec::new());
    }
    let mut texts: Vec<String> = std::fs::read_to_string(&batch).ok().into_iter().collect();
    let taken = dir(state).join(format!(".{LOG}.{}", std::process::id()));
    if std::fs::rename(&log, &taken).is_ok() {
        texts.extend(std::fs::read_to_string(&taken).ok());
    }
    let mut folded = fold(&texts);
    crate::state::write(&batch, lines(&folded))?;
    let _ = std::fs::remove_file(&taken);
    if folded.is_empty() {
        let _ = std::fs::remove_file(&batch);
        return Ok(Vec::new());
    }
    let mut notes = Vec::new();
    let mut refused = None;
    for upload in uploads(&folded) {
        match send(&upload) {
            Err(Refusal::Retry(error)) => {
                ledger.error = Some(error.clone());
                save_ledger(state, &ledger)?;
                return Err(error);
            }
            Err(Refusal::Final(error)) => {
                notes.push(format!("signals: the hub refused a batch, so it was dropped: {error}"));
                refused = Some(error);
            }
            Ok(()) => {
                let said = summary(&upload);
                let _ = crate::messages::log_for_user_in(state, &said, KIND);
                notes.push(format!("signals: {said}"));
            }
        }
        without(&mut folded, &upload);
        crate::state::write(&batch, lines(&folded))?;
    }
    let _ = std::fs::remove_file(&batch);
    ledger = Ledger { sent_at: Some(now), error: refused };
    save_ledger(state, &ledger)?;
    Ok(notes)
}

fn refusal(error: crate::hub::http::HubError) -> Refusal {
    if error.is_final() { Refusal::Final(error.to_string()) } else { Refusal::Retry(error.to_string()) }
}

pub fn send(state: &Path, url: &str, signals: &ErrorSignals) -> std::result::Result<(), Refusal> {
    let key = crate::hub::report::install_key(state).map_err(Refusal::Retry)?;
    let client = Client::new(&crate::hub::hub_url(url).map_err(Refusal::Retry)?, None, CALL);
    let challenge: ReportChallenge = client.get("/v1/reports/challenge").map_err(refusal)?;
    if challenge.difficulty > crate::hub::report::MAX_WORK {
        return Err(Refusal::Retry(format!("the signals endpoint asks for {} bits of work, more than kelpie does ({}); nothing was sent", challenge.difficulty, crate::hub::report::MAX_WORK)));
    }
    let counter = solve(&challenge.nonce.0, &signals.digest(), challenge.difficulty);
    let upload = SignalUpload { signature: Bytes64(key.sign_signals(&challenge.nonce.0, signals)), key: Bytes32(key.public()), signals: signals.clone(), challenge, counter };
    client.post::<_, SignalsAccepted>("/v1/signals", &upload).map(|_| ()).map_err(refusal)
}

// A board runs this too while the keeper is off, so two of them never send the same counts or report twice.
pub fn one_sender(state: &Path, run: impl FnOnce() -> Result<Vec<String>>) -> Result<Vec<String>> {
    use std::os::fd::AsRawFd;
    let file = crate::state::append_to(&dir(state).join(".send.lock")).map_err(|error| error.to_string())?;
    // SAFETY: flock only reads the descriptor, which the File keeps open until it is dropped, and dropping it releases the lock.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(Vec::new());
    }
    run()
}

pub fn tick(state: &Path) -> Result<Vec<String>> {
    let settings = crate::config::report_settings()?;
    let url = settings.url().to_string();
    one_sender(state, || tick_in(state, settings.signals(), crate::messages::now(), &mut |signals| send(state, &url, signals)))
}

pub fn doctor(state: &Path, settings: &crate::config::Report, now: u64) -> Check {
    let name = "maintainers".to_string();
    let armed = crate::hub::report::armed(state);
    let text = match settings.text() {
        crate::config::ReportText::Ask => "each bug you run into in kelpie is shown to you and goes to its maintainers only on your yes ([report] text = \"ask\")",
        crate::config::ReportText::Always if armed => "each bug you run into in kelpie goes to its maintainers redacted, without asking, each send a line under messages ([report] text = \"always\", armed)",
        crate::config::ReportText::Always => "[report] text = \"always\" is set but not armed, so each bug you run into is still shown to you and sent only on your yes",
        crate::config::ReportText::Never => "the bugs you run into in kelpie stay on this machine ([report] text = \"never\")",
    };
    let unarmed = settings.text() == crate::config::ReportText::Always && !armed;
    let arm = || vec![format!("run {} in your own terminal to send them without asking", crate::hub::report::ARM_COMMAND)];
    if !settings.signals() {
        let fact = format!("error signals off ([report] signals = false); {text}");
        return if unarmed { Check { level: Level::Warn, name, fact, fix: arm() } } else { Check { level: Level::Ok, name, fact, fix: Vec::new() } };
    }
    let ledger = ledger(state);
    let sent = ledger.sent_at.map_or("none sent yet".to_string(), |at| format!("last sent {} ago", crate::keeper::span(now.saturating_sub(at))));
    let fact = format!("error signals on, at most once a day, holding only {CONTAINS} ({sent}); {text}");
    match ledger.error {
        Some(error) => Check { level: Level::Warn, name, fact: format!("{fact}; the last send failed: {error}"), fix: vec!["the keeper tries again within the hour; [report] signals = false in config.toml turns them off".to_string()] },
        None if unarmed => Check { level: Level::Warn, name, fact, fix: arm() },
        None => Check { level: Level::Ok, name, fact, fix: Vec::new() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_792_540_800;

    fn name(text: &str) -> Name {
        Name::new(text).unwrap()
    }

    fn log_lines(state: &Path) -> Vec<String> {
        std::fs::read_to_string(dir(state).join(LOG)).unwrap_or_default().lines().map(str::to_string).collect()
    }

    #[test]
    fn a_panic_is_named_after_kelpies_own_module_and_anything_else_is_a_dependency() {
        assert_eq!(panic_component(Some("src/board/draw.rs")), name("board.draw"));
        assert_eq!(panic_component(Some("src/tasks/mod.rs")), name("tasks"));
        assert_eq!(panic_component(Some("src/main.rs")), name("main"));
        for elsewhere in ["/Users/carl/.cargo/registry/src/index.crates.io-1/ratatui-0.30.0/src/buffer.rs", "/rustc/abc123/library/core/src/option.rs", "src/Weird Name.rs", "../greenhouse/src/x.rs"] {
            assert_eq!(panic_component(Some(elsewhere)), name(DEPENDENCY), "{elsewhere}");
        }
        assert_eq!(panic_component(None), name(DEPENDENCY));
    }

    thread_local! {
        static PREVIOUS_RAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    #[test]
    fn the_hook_is_installed_only_with_signals_on_records_no_message_and_still_calls_the_hook_before_it() {
        let state = crate::sh::test_dir("signals-panic");
        let original = std::sync::Arc::new(std::panic::take_hook());
        let forward = std::sync::Arc::clone(&original);
        std::panic::set_hook(Box::new(move |info| {
            PREVIOUS_RAN.with(|ran| ran.set(true));
            forward(info);
        }));
        assert!(!install_panic_hook_if(false, Ok(state.clone())), "off installs nothing");
        assert!(!install_panic_hook_if(true, Err("no state dir".to_string())));
        let _ = std::panic::catch_unwind(|| panic!("with signals off"));
        assert!(PREVIOUS_RAN.with(|ran| ran.replace(false)) && log_lines(&state).is_empty(), "with signals off a panic records nothing");
        assert!(install_panic_hook_if(true, Ok(state.clone())));
        let caught = std::panic::catch_unwind(|| panic!("push of billing@greenhouse failed in /Users/carl/work/greenhouse with token ghp_abcdefghijklmnop1234"));
        let _ = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| original(info)));
        assert!(caught.is_err());
        assert!(PREVIOUS_RAN.with(|ran| ran.get()), "the hook before kelpie's still runs");
        let recorded = log_lines(&state);
        let line: serde_json::Value = recorded.iter().filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok()).find(|line| line["component"] == "signals").expect("this test's panic was recorded");
        assert_eq!((line["component"].as_str(), line["kind"].as_str(), line["count"].as_u64()), (Some("signals"), Some("panic"), Some(1)));
        let keys: Vec<&str> = line.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(keys.iter().all(|key| ["version", "commit", "component", "kind", "count"].contains(key)), "{keys:?}");
        for private in ["billing", "greenhouse", "/Users", "carl", "ghp_", "push of"] {
            assert!(recorded.iter().all(|line| !line.contains(private)), "{private} in {recorded:?}");
        }
    }

    #[test]
    fn record_counts_only_with_signals_on_and_only_a_name() {
        let state = crate::sh::test_dir("signals-record");
        let here = || Ok(state.clone());
        record_if(&|| false, &here, "keeper.push", SignalKind::InternalError);
        assert!(log_lines(&state).is_empty(), "off records nothing");
        record_if(&|| true, &here, "keeper push: /Users/carl", SignalKind::InternalError);
        assert!(log_lines(&state).is_empty(), "a component that is not a name records nothing");
        record_if(&|| true, &|| Err("no state dir".to_string()), "keeper.push", SignalKind::InternalError);
        record_if(&|| true, &here, "keeper.push", SignalKind::InternalError);
        assert_eq!(log_lines(&state).len(), 1);
    }

    #[test]
    fn a_batch_the_hub_refuses_for_good_is_dropped_and_later_counts_still_go() {
        let state = crate::sh::test_dir("signals-final");
        record_in(&state, &name("keeper.push"), SignalKind::InternalError);
        let notes = tick_in(&state, true, NOW, &mut |_| Err(Refusal::Final("the hub answered 400 bad-count".to_string()))).unwrap();
        assert!(notes[0].contains("refused a batch, so it was dropped"), "{notes:?}");
        assert_eq!(ledger(&state).error.as_deref(), Some("the hub answered 400 bad-count"), "doctor still says why");
        assert!(crate::messages::load_in(&state).is_empty(), "a dropped batch is not said to be sent");
        record_in(&state, &name("board.draw"), SignalKind::Panic);
        let mut sent = Vec::new();
        tick_in(&state, true, NOW + DAY, &mut |signals| { sent.push(signals.clone()); Ok(()) }).unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].counts.iter().map(|row| row.component.to_string()).collect::<Vec<_>>(), ["board.draw"], "the refused count is gone, the new one goes");
        assert_eq!(ledger(&state).error, None);
    }

    #[test]
    fn a_line_with_a_commit_that_is_not_short_hex_is_dropped() {
        let state = crate::sh::test_dir("signals-commit");
        let long = "a".repeat(41);
        let text = format!("{{\"version\":\"0.26.3\",\"commit\":\"zzz\",\"component\":\"a\",\"kind\":\"panic\"}}\n{{\"version\":\"0.26.3\",\"commit\":\"{long}\",\"component\":\"b\",\"kind\":\"panic\"}}\n{{\"version\":\"0.26.3\",\"commit\":\"16983b9\",\"component\":\"c\",\"kind\":\"panic\"}}\n{{\"version\":\"0.26.3\",\"commit\":\"16983b9\",\"component\":\"d\",\"kind\":\"panic\",\"count\":0}}\n");
        crate::state::write(&dir(&state).join(BATCH), text).unwrap();
        let mut sent = Vec::new();
        tick_in(&state, true, NOW, &mut |signals| { sent.push(signals.clone()); Ok(()) }).unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].counts.iter().map(|row| row.component.to_string()).collect::<Vec<_>>(), ["c"]);
        assert_eq!(sent[0].check(), Ok(()));
    }

    #[test]
    fn a_counted_line_holds_no_text_however_it_was_tampered_with() {
        let state = crate::sh::test_dir("signals-tampered");
        let file = dir(&state).join(BATCH);
        let good = r#"{"version":"0.26.3","commit":"16983b9","component":"keeper.push","kind":"internal-error","count":2}"#;
        let tampered = [
            r#"{"version":"0.26.3","component":"keeper.push","kind":"internal-error","message":"git refused /Users/carl/app"}"#,
            r#"{"version":"0.26.3","component":"/Users/carl/app","kind":"panic"}"#,
            r#"{"version":"0.26.3","component":"push failed: auth","kind":"panic"}"#,
            r#"{"version":"0.26.3","component":"keeper.push","kind":"git-said-no"}"#,
        ];
        crate::state::write(&file, format!("{good}\n{}\n", tampered.join("\n"))).unwrap();
        let mut sent = Vec::new();
        tick_in(&state, true, NOW, &mut |signals| { sent.push(signals.clone()); Ok(()) }).unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].counts, vec![SignalCount { component: name("keeper.push"), kind: SignalKind::InternalError, count: 2 }]);
        let wire = serde_json::to_string(&sent[0]).unwrap();
        assert!(!wire.contains("/Users") && !wire.contains("message") && !wire.contains("git"), "{wire}");
    }

    #[test]
    fn counts_go_out_once_a_day_folded_and_each_send_is_a_line_under_messages() {
        let state = crate::sh::test_dir("signals-daily");
        for _ in 0..3 {
            record_in(&state, &name("keeper.push"), SignalKind::InternalError);
        }
        record_in(&state, &name("board.draw"), SignalKind::Panic);
        let mut sent = Vec::new();
        let notes = tick_in(&state, true, NOW, &mut |signals| { sent.push(signals.clone()); Ok(()) }).unwrap();
        assert_eq!(sent.len(), 1, "one upload per version");
        let rows: Vec<(String, SignalKind, u32)> = sent[0].counts.iter().map(|row| (row.component.to_string(), row.kind, row.count)).collect();
        assert_eq!(rows, [("board.draw".to_string(), SignalKind::Panic, 1), ("keeper.push".to_string(), SignalKind::InternalError, 3)]);
        assert_eq!((sent[0].version.as_str(), sent[0].os.as_str(), sent[0].arch.as_str()), (env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH));
        assert_eq!(notes.len(), 1);
        let shown = crate::messages::load_in(&state);
        let line = shown.last().unwrap();
        assert_eq!((line.from.as_str(), line.to.as_str(), line.kind.as_deref()), ("kelpie", crate::messages::USER_LABEL, Some(KIND)));
        assert!(line.text.starts_with("Sent kelpie's maintainers an error signal, a count of kelpie's own failures: board.draw panic (×1), keeper.push internal-error (×3); with kelpie ") && line.text.ends_with("[report] signals = false in config.toml turns these off."), "{}", line.text);

        record_in(&state, &name("keeper.push"), SignalKind::InternalError);
        assert!(tick_in(&state, true, NOW + 3600, &mut |_| panic!("a second send the same day")).unwrap().is_empty());
        assert_eq!(log_lines(&state).len(), 1, "what came after waits for tomorrow");
        let mut tomorrow = Vec::new();
        tick_in(&state, true, NOW + DAY, &mut |signals| { tomorrow.push(signals.clone()); Ok(()) }).unwrap();
        assert_eq!(tomorrow[0].counts[0].count, 1, "nothing is counted twice");
        assert!(tick_in(&state, true, NOW + 2 * DAY, &mut |_| panic!("nothing to send")).unwrap().is_empty(), "a quiet day sends nothing");
    }

    #[test]
    fn a_failed_send_keeps_the_counts_for_the_next_round() {
        let state = crate::sh::test_dir("signals-retry");
        record_in(&state, &name("keeper.push"), SignalKind::InternalError);
        let failed = tick_in(&state, true, NOW, &mut |_| Err(Refusal::Retry("the hub is down".to_string()))).unwrap_err();
        assert_eq!(failed, "the hub is down");
        assert_eq!(ledger(&state).error.as_deref(), Some("the hub is down"));
        record_in(&state, &name("keeper.push"), SignalKind::InternalError);
        let mut sent = Vec::new();
        tick_in(&state, true, NOW + 60, &mut |signals| { sent.push(signals.clone()); Ok(()) }).unwrap();
        assert_eq!(sent[0].counts[0].count, 2);
        assert_eq!(ledger(&state).error, None);
    }

    #[test]
    fn with_signals_off_nothing_is_recorded_sent_or_kept() {
        let state = crate::sh::test_dir("signals-off");
        record_in(&state, &name("keeper.push"), SignalKind::InternalError);
        assert!(tick_in(&state, false, NOW, &mut |_| panic!("signals are off")).unwrap().is_empty());
        assert!(log_lines(&state).is_empty() && !dir(&state).join(BATCH).exists(), "what was counted is dropped");
        assert!(crate::messages::load_in(&state).is_empty());
        let off = crate::config::Report { signals: Some(false), ..Default::default() };
        let check = doctor(&state, &off, NOW);
        assert!(check.level == Level::Ok && check.fact.starts_with("error signals off ([report] signals = false); each bug you run into in kelpie is shown to you and goes to its maintainers only on your yes"), "{}", check.fact);
    }

    #[test]
    fn doctor_says_what_a_signal_holds_and_how_reports_are_sent() {
        let state = crate::sh::test_dir("signals-doctor");
        let check = doctor(&state, &crate::config::Report::default(), NOW);
        assert!(check.level == Level::Ok && check.fact.contains("error signals on, at most once a day, holding only kelpie's version and commit, the OS and CPU architecture") && check.fact.contains("none sent yet") && check.fact.contains("[report] text = \"ask\""), "{}", check.fact);
        let always = crate::config::Report { text: Some(crate::config::ReportText::Always), ..Default::default() };
        let unarmed = doctor(&state, &always, NOW);
        assert!(unarmed.level == Level::Warn && unarmed.fact.contains("[report] text = \"always\" is set but not armed") && unarmed.fix == ["run kelpie issue report --always in your own terminal to send them without asking"], "{unarmed:?}");
        crate::hub::report::arm_in(&state, &crate::permit::Caller::User, true, crate::config::ReportText::Always, NOW).unwrap();
        let armed = doctor(&state, &always, NOW);
        assert!(armed.level == Level::Ok && armed.fact.contains("goes to its maintainers redacted, without asking"), "{armed:?}");
    }

    #[test]
    fn a_second_sender_does_nothing_while_the_first_sends() {
        let state = crate::sh::test_dir("signals-lock");
        let first = one_sender(&state, || one_sender(&state, || panic!("a second sender ran")).map(|_| vec!["first".to_string()])).unwrap();
        assert_eq!(first, ["first"]);
        assert_eq!(one_sender(&state, || Ok(vec!["later".to_string()])).unwrap(), ["later"], "the lock goes with its sender");
    }

    #[test]
    fn the_keeper_checks_for_signals_every_hour() {
        assert!(crate::keeper::routines().iter().any(|routine| routine.name == "signals" && routine.every == EVERY));
    }
}
