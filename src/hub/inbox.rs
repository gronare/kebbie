use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use kebbie_hub_api::ReportInbox;
use serde::{Deserialize, Serialize};

use super::http::{CALL, Client, HubError};
use crate::doctor::{Check, Level};
use crate::issues;
use crate::sh::Result;

pub const EVERY: u64 = 600;
const PAGE: usize = 100;
const FILE: &str = "inbox.json";
pub const FILED_KIND: &str = "report-filed";

#[derive(Clone, Debug, Deserialize)]
pub struct SignalInbox {
    pub signals: Vec<ReceivedSignal>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReceivedSignal {
    pub id: u64,
    pub version: String,
    pub component: String,
    pub kind: String,
    pub installs: u64,
    pub days: Vec<SignalDay>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SignalDay {
    pub day: String,
    pub installs: u64,
}

// Filed is keyed by report id, and the issue's id is derived from it, so neither a lost ack nor a crash files a report twice.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub filed: BTreeMap<u64, String>,
    #[serde(default)]
    pub signals: BTreeSet<String>,
    #[serde(default)]
    pub quarantined: u64,
    #[serde(default)]
    pub checked_at: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

fn file(state: &Path) -> PathBuf {
    super::report::dir(state).join(FILE)
}

pub fn load(state: &Path) -> Ledger {
    std::fs::read_to_string(file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save(state: &Path, ledger: &Ledger) -> Result<()> {
    crate::state::write(&file(state), serde_json::to_string_pretty(ledger).map_err(|error| error.to_string())?)
}

pub fn pull(state: &Path, url: &str, token: &str, now: u64) -> Result<Vec<String>> {
    let mut ledger = load(state);
    let pulled = pull_into(state, &mut ledger, url, token, now);
    ledger.checked_at = Some(now);
    ledger.error = pulled.as_ref().err().cloned();
    save(state, &ledger)?;
    pulled
}

fn pull_into(state: &Path, ledger: &mut Ledger, url: &str, token: &str, now: u64) -> Result<Vec<String>> {
    let client = Client::new(&super::hub_url(url)?, Some(token), CALL);
    let dir = issues::dir(state);
    let mut notes = Vec::new();
    let mut after = 0;
    loop {
        let page: ReportInbox = client.get(&format!("/v1/admin/reports?after={after}")).map_err(|error| error.to_string())?;
        ledger.quarantined = page.quarantined;
        for received in &page.reports {
            if let std::collections::btree_map::Entry::Vacant(slot) = ledger.filed.entry(received.id) {
                let issue = issues::file_report_in(&dir, received.id, &received.report, now)?;
                notes.push(format!("reports: filed kebbie issue {issue} from report {}", received.id));
                announce(state, &issues::find(&dir, &issue).map(|filed| filed.title).unwrap_or_default());
                slot.insert(issue);
                save(state, ledger)?;
            }
            client.post::<_, ()>(&format!("/v1/admin/reports/{}/ack", received.id), &()).map_err(|error| error.to_string())?;
        }
        match page.reports.last() {
            Some(last) if page.reports.len() >= PAGE => after = last.id,
            _ => break,
        }
    }
    pull_signals(state, ledger, &client, &mut notes, now)?;
    Ok(notes)
}

// An endpoint without a signal inbox, such as a kebbie hub, answers 404 and has none to file.
fn pull_signals(state: &Path, ledger: &mut Ledger, client: &Client, notes: &mut Vec<String>, now: u64) -> Result<()> {
    let dir = issues::dir(state);
    let mut after = 0;
    loop {
        let page: SignalInbox = match client.get(&format!("/v1/admin/signals?after={after}")) {
            Err(HubError::Status { status: 404, .. }) => return Ok(()),
            answer => answer.map_err(|error| error.to_string())?,
        };
        for received in &page.signals {
            let days: Vec<(String, u64)> = received.days.iter().map(|day| (day.day.clone(), day.installs)).collect();
            let signal = issues::Signal { version: &received.version, component: &received.component, kind: &received.kind, days: &days };
            match issues::file_signal_in(&dir, &signal, now) {
                Err(error) => {
                    notes.push(format!("signals: signal {} not filed: {error}", received.id));
                    continue;
                }
                Ok(issues::Filed::New { id, title }) => {
                    notes.push(format!("signals: filed kebbie issue {id} from signal {}", received.id));
                    announce(state, &title);
                    ledger.signals.insert(id);
                    save(state, ledger)?;
                }
                Ok(issues::Filed::Updated { id, .. }) => notes.push(format!("signals: updated kebbie issue {id} from signal {}", received.id)),
                Ok(issues::Filed::Same { .. }) => {}
            }
            client.post::<_, ()>(&format!("/v1/admin/signals/{}/ack", received.id), &serde_json::json!({ "installs": received.installs })).map_err(|error| error.to_string())?;
        }
        match page.signals.last() {
            Some(last) if page.signals.len() >= PAGE => after = last.id,
            _ => return Ok(()),
        }
    }
}

fn announce(state: &Path, title: &str) {
    let _ = crate::messages::log_for_user_in(state, &format!("New bug report: {title}"), FILED_KIND);
}

pub fn tick(state: &Path) -> Result<Vec<String>> {
    let settings = crate::config::report_settings()?;
    match &settings.inbox_token {
        Some(token) => pull(state, settings.url(), token, crate::messages::now()),
        None => Ok(Vec::new()),
    }
}

pub fn doctor(state: &Path, settings: &crate::config::Report, now: u64) -> Option<Check> {
    settings.inbox_token.as_ref()?;
    let ledger = load(state);
    let name = "reports".to_string();
    let checked = ledger.checked_at.map_or("not fetched yet".to_string(), |at| format!("fetched {} ago", crate::keeper::span(now.saturating_sub(at))));
    let fact = format!("{} · {} filed · {} from signals · {checked}", settings.url(), ledger.filed.len(), ledger.signals.len());
    Some(if let Some(error) = &ledger.error {
        Check { level: Level::Warn, name, fact: format!("{fact}; last fetch failed: {error}"), fix: vec!["check [report] inbox_token against the token the issues endpoint issued; the keeper retries every 10 minutes".to_string()] }
    } else if ledger.quarantined > 0 {
        Check { level: Level::Warn, name, fact: format!("{fact}; {} reports wait in quarantine", ledger.quarantined), fix: vec!["allow or block their senders on the issues endpoint".to_string()] }
    } else {
        Check { level: Level::Ok, name, fact, fix: Vec::new() }
    })
}
