use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const DEFAULT: [&str; 2] = ["fable", "opus"];
const KEPT_OK: u64 = 7 * 86_400;
const KEPT_REFUSED: u64 = 86_400;
const PROBE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Wanted {
    #[default]
    Default,
    Off,
    Models(Vec<String>),
}

impl Wanted {
    pub fn to_json(&self) -> Value {
        match self {
            Wanted::Default => Value::Null,
            Wanted::Off => json!("off"),
            Wanted::Models(models) => json!(models),
        }
    }

    pub fn describe(&self) -> Option<String> {
        match self {
            Wanted::Default => None,
            Wanted::Off => Some("advisor off".to_string()),
            Wanted::Models(models) => Some(format!("advisor {}", models.join(" or "))),
        }
    }
}

pub fn parse(value: &toml::Value, source: &str) -> Result<Wanted, String> {
    let models: Vec<String> = match value {
        toml::Value::String(text) if text.trim() == "off" => return Ok(Wanted::Off),
        toml::Value::String(text) => vec![text.trim().to_string()],
        toml::Value::Array(items) => items.iter().map(|item| item.as_str().map(|text| text.trim().to_string())).collect::<Option<_>>().ok_or_else(|| format!("{source}: advisor: a list of model names, each a string"))?,
        _ => return Err(format!("{source}: advisor: a model name, a list of them, or \"off\"")),
    };
    if models.is_empty() || models.iter().any(String::is_empty) {
        return Err(format!("{source}: advisor: must not be empty; name a model, a list of them, or \"off\""));
    }
    Ok(Wanted::Models(models))
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
struct Tried {
    ok: bool,
    at: u64,
}

fn file(state: &Path) -> PathBuf {
    state.join("advisor.json")
}

// The first wanted advisor the account can use, each answer kept so a start asks at most once a week; Claude Code itself drops an advisor weaker than the session's model, so the account is all that is asked; without a profile's advisor, the user's own advisorModel holds.
pub fn chosen_in(state: &Path, wanted: &Wanted, own: bool, now: u64, probe: &dyn Fn(&str) -> Option<bool>) -> Option<String> {
    let candidates: Vec<String> = match wanted {
        Wanted::Off => return None,
        Wanted::Default if own => return None,
        Wanted::Default => DEFAULT.map(str::to_string).to_vec(),
        Wanted::Models(models) => models.clone(),
    };
    let mut tried: BTreeMap<String, Tried> = std::fs::read_to_string(file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let mut found = None;
    for advisor in candidates {
        let ok = match tried.get(&advisor) {
            Some(known) if now.saturating_sub(known.at) < if known.ok { KEPT_OK } else { KEPT_REFUSED } => known.ok,
            _ => match probe(&advisor) {
                Some(ok) => {
                    tried.insert(advisor.clone(), Tried { ok, at: now });
                    ok
                }
                None => false,
            },
        };
        if ok {
            found = Some(advisor);
            break;
        }
    }
    if let Ok(text) = serde_json::to_string_pretty(&tried) {
        let _ = crate::state::write(&file(state), text);
    }
    found
}

// A test never runs Claude Code: chosen_in takes its probe.
pub fn chosen(wanted: &Wanted) -> Option<String> {
    if cfg!(test) {
        return None;
    }
    let state = crate::lead::state_dir().ok()?;
    chosen_in(&state, wanted, own_advisor().is_some(), crate::messages::now(), &|advisor| probe_in(&state, advisor))
}

pub fn with_advisor(settings: &str, advisor: Option<&str>) -> String {
    let Some(advisor) = advisor else { return settings.to_string() };
    let Ok(mut written) = serde_json::from_str::<Value>(settings) else { return settings.to_string() };
    written["advisorModel"] = json!(advisor);
    written.to_string()
}

fn own_advisor() -> Option<String> {
    let dir = std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".claude"))).ok()?;
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).ok()?).ok()?;
    settings["advisorModel"].as_str().filter(|advisor| !advisor.is_empty()).map(str::to_string)
}

// One short haiku turn with the advisor on; its debug log says whether Claude Code turned the advisor on, and a failure that names no advisor refusal (no network, a timeout) is no answer.
fn probe_in(dir: &Path, advisor: &str) -> Option<bool> {
    let log = dir.join("advisor-probe.log");
    let _ = std::fs::remove_file(&log);
    let mut command = std::process::Command::new("claude");
    command.current_dir(dir).args(["-p", "--advisor", advisor, "--model", "haiku", "--output-format", "json", "--debug-file"]).arg(&log).arg("Reply with the single word ok");
    let done = crate::sh::output_with_timeout(command, PROBE).ok().flatten()?;
    let debug = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    verdict(done.code, &done.stdout, &format!("{}\n{debug}", done.stderr))
}

fn verdict(code: Option<i32>, stdout: &str, said: &str) -> Option<bool> {
    let answer: Value = serde_json::from_str(stdout.trim()).unwrap_or_default();
    let text = format!("{said}\n{}", answer["result"].as_str().unwrap_or_default());
    if REFUSALS.iter().any(|refusal| text.contains(refusal)) {
        return Some(false);
    }
    (code == Some(0) && answer["is_error"] == false && said.contains(ENABLED)).then_some(true)
}

const ENABLED: &str = "Server-side tool enabled with";
const REFUSALS: [&str; 5] = ["cannot be used as an advisor", "advisor_incompatible", "advisor model is not compatible", "isn't available for your account", "is not in the list of available models"];

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn the_first_advisor_the_account_takes_is_chosen_and_each_answer_is_kept_for_a_while() {
        let dir = crate::sh::test_dir("advisor-chosen");
        let asked = RefCell::new(Vec::new());
        let offline = std::cell::Cell::new(false);
        let probe = |advisor: &str| {
            asked.borrow_mut().push(advisor.to_string());
            if offline.get() { None } else { Some(advisor == "opus") }
        };
        assert_eq!(chosen_in(&dir, &Wanted::Default, false, 1_000, &probe), Some("opus".to_string()), "an account without fable gets opus");
        assert_eq!(*asked.borrow(), ["fable", "opus"]);
        assert_eq!(chosen_in(&dir, &Wanted::Default, false, 2_000, &probe), Some("opus".to_string()));
        assert_eq!(asked.borrow().len(), 2, "a kept answer is not asked again");
        assert_eq!(chosen_in(&dir, &Wanted::Default, false, 1_000 + KEPT_REFUSED, &probe), Some("opus".to_string()));
        assert_eq!(asked.borrow()[2..], ["fable".to_string()], "a refusal is asked again after a day, an answer that worked after a week");
        offline.set(true);
        assert_eq!(chosen_in(&dir, &Wanted::Models(vec!["sonnet".into()]), false, 5_000, &probe), None, "no answer leaves the advisor off for this start");
        offline.set(false);
        assert_eq!(chosen_in(&dir, &Wanted::Models(vec!["sonnet".into()]), false, 5_001, &probe), None);
        assert_eq!(asked.borrow().iter().filter(|advisor| *advisor == "sonnet").count(), 2, "and is not kept, so the next start asks again");
        asked.borrow_mut().clear();
        assert_eq!(chosen_in(&dir, &Wanted::Default, true, 5_000, &probe), None, "the user's own advisorModel holds without a profile's");
        assert_eq!(chosen_in(&dir, &Wanted::Off, false, 5_000, &probe), None);
        assert!(asked.borrow().is_empty(), "off and the user's own ask nothing");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_probe_counts_only_what_claude_code_says_about_the_advisor() {
        let ok = "{\"is_error\":false,\"result\":\"ok\"}";
        assert_eq!(verdict(Some(0), ok, "[DEBUG] [AdvisorTool] Server-side tool enabled with claude-fable-5-2 as the advisor model"), Some(true));
        assert_eq!(verdict(Some(1), "", "Error: The model \"x\" cannot be used as an advisor."), Some(false));
        assert_eq!(verdict(Some(0), "{\"is_error\":true,\"result\":\"API Error: claude-fable-5-2 isn't available for your account\"}", ""), Some(false));
        assert_eq!(verdict(Some(0), ok, "[DEBUG] nothing about it"), None, "an advisor Claude Code never turned on is no answer");
        assert_eq!(verdict(Some(1), "", "Network error. Please check your internet connection."), None, "a failure that names no refusal is kept for nothing");
    }

    #[test]
    fn a_profile_names_one_advisor_a_list_or_off_and_the_settings_take_the_chosen_one() {
        let parsed = |text: &str| parse(&toml::from_str::<toml::Table>(&format!("advisor = {text}")).unwrap()["advisor"], "x.toml");
        assert_eq!(parsed("\"fable\""), Ok(Wanted::Models(vec!["fable".into()])));
        assert_eq!(parsed("[\"fable\", \"opus\"]"), Ok(Wanted::Models(vec!["fable".into(), "opus".into()])));
        assert_eq!(parsed("\"off\""), Ok(Wanted::Off));
        assert!(parsed("[]").unwrap_err().contains("must not be empty") && parsed("3").unwrap_err().contains("a model name"));
        assert_eq!(serde_json::from_str::<Value>(&with_advisor("{\"hooks\":{}}", Some("fable"))).unwrap(), json!({ "hooks": {}, "advisorModel": "fable" }));
        assert_eq!(with_advisor("{\"hooks\":{}}", None), "{\"hooks\":{}}");
    }
}
