use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kebbie_hub_api::ReportInbox;
use serde::{Deserialize, Serialize};

use super::http::{CALL, Client};
use crate::doctor::{Check, Level};
use crate::issues;
use crate::sh::Result;

pub const EVERY: u64 = 600;
const PAGE: usize = 100;
const FILE: &str = "inbox.json";

// Filed is keyed by report id, and the issue's id is derived from it, so neither a lost ack nor a crash files a report twice.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub filed: BTreeMap<u64, String>,
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
                slot.insert(issue);
                save(state, ledger)?;
            }
            client.post::<_, ()>(&format!("/v1/admin/reports/{}/ack", received.id), &()).map_err(|error| error.to_string())?;
        }
        match page.reports.last() {
            Some(last) if page.reports.len() >= PAGE => after = last.id,
            _ => return Ok(notes),
        }
    }
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
    let fact = format!("{} · {} filed · {checked}", settings.url(), ledger.filed.len());
    Some(if let Some(error) = &ledger.error {
        Check { level: Level::Warn, name, fact: format!("{fact}; last fetch failed: {error}"), fix: vec!["check [report] inbox_token against kebbie-hub admin token; the keeper retries every 10 minutes".to_string()] }
    } else if ledger.quarantined > 0 {
        Check { level: Level::Warn, name, fact: format!("{fact}; {} reports wait in quarantine: kebbie-hub admin reports --quarantine", ledger.quarantined), fix: vec!["kebbie-hub admin reports allow <key> or block <key> on the hub".to_string()] }
    } else {
        Check { level: Level::Ok, name, fact, fix: Vec::new() }
    })
}
