use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const DEFAULT: [&str; 2] = ["fable", "opus"];
const KEPT_OK: u64 = 7 * 86_400;
const KEPT_REFUSED: u64 = 86_400;
const PROBE: Duration = Duration::from_secs(90);

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

fn key(advisor: &str, model: Option<&str>) -> String {
    format!("{advisor} with {}", model.unwrap_or("the default model"))
}

// The first wanted advisor the account can use with this model, each answer kept so a start probes at most once a week; without a profile's advisor, the user's own advisorModel in Claude Code holds.
pub fn chosen_in(state: &Path, wanted: &Wanted, model: Option<&str>, own: bool, now: u64, probe: &dyn Fn(&str, Option<&str>) -> bool) -> Option<String> {
    let candidates: Vec<String> = match wanted {
        Wanted::Off => return None,
        Wanted::Default if own => return None,
        Wanted::Default => DEFAULT.map(str::to_string).to_vec(),
        Wanted::Models(models) => models.clone(),
    };
    let mut tried: BTreeMap<String, Tried> = std::fs::read_to_string(file(state)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let mut found = None;
    for advisor in candidates {
        let key = key(&advisor, model);
        let ok = match tried.get(&key) {
            Some(known) if now.saturating_sub(known.at) < if known.ok { KEPT_OK } else { KEPT_REFUSED } => known.ok,
            _ => {
                let ok = probe(&advisor, model);
                tried.insert(key, Tried { ok, at: now });
                ok
            }
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
pub fn chosen(wanted: &Wanted, model: Option<&str>) -> Option<String> {
    if cfg!(test) {
        return None;
    }
    let state = crate::lead::state_dir().ok()?;
    chosen_in(&state, wanted, model, own_advisor().is_some(), crate::messages::now(), &|advisor, model| probe_in(&state, advisor, model))
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

// One short turn through Claude Code itself, which refuses an advisor the account or the model cannot use.
fn probe_in(dir: &Path, advisor: &str, model: Option<&str>) -> bool {
    let mut command = std::process::Command::new("claude");
    command.current_dir(dir).args(["-p", "--advisor", advisor, "--output-format", "json"]);
    if let Some(model) = model {
        command.args(["--model", model]);
    }
    command.arg("Reply with the single word ok");
    match crate::sh::output_with_timeout(command, PROBE) {
        Ok(Some(done)) if done.code == Some(0) => answered(&done.stdout),
        _ => false,
    }
}

fn answered(stdout: &str) -> bool {
    serde_json::from_str::<Value>(stdout.trim()).is_ok_and(|answer| answer["is_error"] == false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn the_first_advisor_the_account_takes_is_chosen_and_each_answer_is_kept_for_a_while() {
        let dir = crate::sh::test_dir("advisor-chosen");
        let asked = RefCell::new(Vec::new());
        let probe = |advisor: &str, model: Option<&str>| {
            asked.borrow_mut().push(format!("{advisor}/{}", model.unwrap_or("-")));
            advisor == "opus"
        };
        assert_eq!(chosen_in(&dir, &Wanted::Default, None, false, 1_000, &probe), Some("opus".to_string()), "an account without fable gets opus");
        assert_eq!(*asked.borrow(), ["fable/-", "opus/-"]);
        assert_eq!(chosen_in(&dir, &Wanted::Default, None, false, 2_000, &probe), Some("opus".to_string()));
        assert_eq!(asked.borrow().len(), 2, "a kept answer is not asked again");
        assert_eq!(chosen_in(&dir, &Wanted::Default, None, false, 1_000 + KEPT_REFUSED, &probe), Some("opus".to_string()));
        assert_eq!(asked.borrow()[2..], ["fable/-".to_string()], "a refusal is asked again after a day, an answer that worked after a week");
        assert_eq!(chosen_in(&dir, &Wanted::Models(vec!["fable".into()]), Some("haiku"), false, 5_000, &probe), None, "none the account takes leaves the advisor off");
        assert_eq!(asked.borrow().last().unwrap(), "fable/haiku", "each model is asked on its own, as an advisor fits some models only");
        asked.borrow_mut().clear();
        assert_eq!(chosen_in(&dir, &Wanted::Default, None, true, 5_000, &probe), None, "the user's own advisorModel holds without a profile's");
        assert_eq!(chosen_in(&dir, &Wanted::Off, None, false, 5_000, &probe), None);
        assert!(asked.borrow().is_empty(), "off and the user's own ask nothing");
        std::fs::remove_dir_all(dir).unwrap();
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
        assert!(answered("{\"is_error\":false,\"result\":\"ok\"}") && !answered("{\"is_error\":true}") && !answered("Error: The model \"x\" cannot be used as an advisor."));
    }
}
