use std::fs;

use crate::scenarios::lab::Lab;

const MODELS: &str = r#"{"fetched_at":"2026-10-30T08:00:00Z","client_version":"0.163.0","models":[
{"slug":"gpt-6.1-sol","display_name":"GPT-6.1-Sol","default_reasoning_level":"low","supported_reasoning_levels":[{"effort":"low","description":""}],"visibility":"list","priority":1},
{"slug":"gpt-6-astra","display_name":"GPT-6-Astra","default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"medium","description":""}],"visibility":"list","priority":2}
]}"#;

fn notices(lab: &Lab) -> Vec<String> {
    lab.messages().into_iter().filter(|message| message["to"] == "you" && message["kind"] == "agents").filter_map(|message| message["text"].as_str().map(str::to_string)).collect()
}

#[test]
fn the_daily_agent_check_turns_a_codex_model_gone_from_codexs_list_into_one_board_notice() {
    let lab = Lab::new("agent-check");
    lab.ok(&["setup", "codex", "--yes"]);
    lab.profile("coder", "agent = \"codex\"\nmodel = \"gpt-5.2-codex\"\n");
    fs::write(lab.home.join(".codex/models_cache.json"), MODELS).unwrap();
    let due = || fs::write(lab.state.join("agent-check.json"), r#"{"checked_at":0,"findings":[]}"#).unwrap();
    due();
    lab.keeper_round();
    let told = notices(&lab);
    assert_eq!(told.len(), 1, "{told:?}\n{}", lab.keeper_log());
    assert!(told[0].starts_with("coder: gpt-5.2-codex is gone from Codex's models; its tasks start on ") && told[0].ends_with("pick one with m on the Settings page"), "{told:?}");
    assert!(!told[0].contains("is out"), "the latest Codex is the one installed, so no update is told: {told:?}");
    let saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(lab.state.join("agent-check.json")).unwrap()).unwrap();
    assert_eq!(saved["latest"], "0.163.0", "the suite's latest version, fetched from nowhere");
    lab.keeper_round();
    assert_eq!(notices(&lab).len(), 1, "a finding the check has told is not told again the same day");
    let mut seen = saved.clone();
    seen["checked_at"] = serde_json::json!(0);
    fs::write(lab.state.join("agent-check.json"), seen.to_string()).unwrap();
    lab.keeper_round();
    assert_eq!(notices(&lab).len(), 1, "nor on the next day's check");
}
