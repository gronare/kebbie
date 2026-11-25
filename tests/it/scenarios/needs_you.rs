use std::fs;
use std::time::Duration;

use serde_json::json;

use crate::scenarios::lab::{CODEX_SCREEN, Lab, eventually, stderr, stdout};

const WELCOME: &str = "  Welcome to Codex, OpenAI's command-line coding agent\n\n  Choose how you want to use Codex.\n\n› 1. Sign in with ChatGPT\n     Usage included with Plus, Pro, Business, and Enterprise plans\n\n  2. Sign in with Device Code\n\n  Press enter to continue\n";
const TRUST: &str = "  Trust this folder? Codex can read and edit files in it.\n\n› 1. Trust and continue\n  2. Quit\n";
const HOOKS: &str = "  Hooks need review\n  1 hook is new or changed.\n› 1. Review hooks\n  2. Trust all and continue\n  3. Continue without trusting (hooks won't run)\n";
const MODEL: &str = "  Codex just got an upgrade. Introducing\n  gpt-5.1-codex-max.\n\n› 1. Try new model\n  2. Use existing model\n\n  enter/esc confirm · ⌃c quit\n";
const UPDATE: &str = "\n  Update available · 0.163.0 → 0.164.0\n  Release notes: https://github.com/openai/codex/releases/latest\n\n› 1. Update now (runs `npm install -g @openai/codex`)\n  2. Skip\n  3. Skip until next version\n\n  enter continue · esc skip\n";
const APPROVAL: &str = "• Running kebbie report --state blocked --next nothing 'Read-only…\n    + Show details\n\n\n  Would you like to run the following command?\n\n  Environment: local\n\n  Reason: May I record that this task is waiting on the lead?\n\n  $ kebbie report --state blocked --next nothing 'Read-only'\n\n\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands that start with `kebbie report` (p)\n  3. No, and tell Codex what to do differently (esc)\n\n  Press enter to confirm or esc to cancel\n";

fn codex_lab(name: &str) -> Lab {
    let lab = Lab::new(name);
    lab.profile("coder", "agent = \"codex\"\n");
    lab.ok(&["setup", "codex", "--yes"]);
    lab
}

#[test]
fn codexs_start_up_screens_hold_its_prompt_under_needs_you_and_it_goes_in_once_the_user_answered() {
    let lab = codex_lab("codex-start-up");
    let screens = [("welcome", WELCOME, "you to sign in"), ("trust", TRUST, "whether you trust its folder"), ("hooks", HOOKS, "whether to trust the hooks it found new or changed"), ("model", MODEL, "which model to use, after its notice of a new one"), ("update", UPDATE, "whether to update it first")];
    for (task, screen, _) in screens {
        lab.screen(&format!("p_{task}"), screen);
        lab.start(task, &format!("Work on {task}"), &["--profile", "coder"]);
        assert!(lab.prompts_to(task).is_empty(), "{task}: nothing is typed into a start-up screen");
        assert_eq!(lab.item(task)["group"], "NEEDS YOU", "{task}: {}", lab.item(task));
    }
    lab.keeper_round();
    let told = lab.to_lead();
    for (task, _, asks) in screens {
        let line = format!("{task}@app waits on Codex, which asks {asks} before it takes the task's prompt");
        assert_eq!(told.iter().filter(|text| text.contains(&line)).count(), 1, "the lead hears it once: {line}\n{told:?}");
        lab.screen(&format!("p_{task}"), CODEX_SCREEN);
    }
    lab.keeper_round();
    for (task, _, _) in screens {
        assert_eq!(lab.prompts_to(task), [format!("Work on {task}")], "{task}: the held prompt goes in once the screen is answered");
        assert_ne!(lab.item(task)["group"], "NEEDS YOU", "{task}");
    }
}

fn relayed(lab: &Lab) -> Option<String> {
    lab.to_lead().iter().rev().find(|text| text.contains("waits on its Codex's own prompt (prompt")).map(|text| text.split("--prompt ").nth(1).unwrap().split(|character: char| !character.is_ascii_alphanumeric()).next().unwrap().to_string())
}

#[test]
fn a_codex_approval_mid_session_waits_under_needs_you_and_the_lead_answers_it_by_number_and_by_key() {
    let lab = codex_lab("codex-approval");
    lab.start("search", "Add the search page", &["--profile", "coder"]);
    assert_eq!(lab.prompts_to("search").len(), 1);
    let asking = |lab: &Lab| {
        lab.agent("search", |agent| agent["terminal_title"] = json!("[ ! ] Action Required | Add the search page | wt1.app"));
        lab.screen("p_search", APPROVAL);
        lab.screen("p_search.next", CODEX_SCREEN);
    };
    asking(&lab);
    let item = lab.item("search");
    assert_eq!((item["group"].as_str(), item["agent"]["status"].as_str()), (Some("NEEDS YOU"), Some("blocked")), "{item}");
    assert!(!item["title"].as_str().unwrap().contains("Action Required"), "{item}");
    lab.keeper_round();
    let id = relayed(&lab).unwrap_or_else(|| panic!("the lead hears the approval: {:?}", lab.to_lead()));
    let told = lab.to_lead().into_iter().rev().find(|text| text.contains(&id)).unwrap();
    assert!(told.contains("$ kebbie report --state blocked") && told.contains("1. Yes, proceed (y)  (focused)") && told.contains(&format!("kebbie choose search@app y --prompt {id}")), "{told}");

    let task = lab.run_as("search@app", &["choose", "search", "1", "--prompt", &id]);
    assert!(!task.status.success() && stderr(&task).contains("answering an agent's prompt in a task is the user's or the lead's"), "{}", stderr(&task));
    let chosen = lab.run_as("lead", &["choose", "search", "1", "--prompt", &id]);
    assert!(chosen.status.success() && stdout(&chosen).contains("Chose \"1. Yes, proceed (y)\""), "{}\n{}", stdout(&chosen), stderr(&chosen));
    lab.agent("search", |agent| agent["terminal_title"] = json!("⠋ Add the search page | wt1.app"));
    assert_ne!(lab.item("search")["group"], "NEEDS YOU");

    asking(&lab);
    lab.keeper_round();
    let again = relayed(&lab).expect("a second approval is told again");
    let keyed = lab.run_as("lead", &["choose", "search", "p", "--prompt", &again]);
    assert!(keyed.status.success() && stdout(&keyed).contains("Chose \"2. Yes, and don't ask again"), "{}\n{}", stdout(&keyed), stderr(&keyed));
    let keys: Vec<String> = lab.herdr_calls().into_iter().filter(|call| call[..2] == ["pane", "send-keys"] && call[2] == "p_search").map(|call| call[3..].join(" ")).collect();
    assert_eq!(keys, ["1", "2"], "a number and a key each press the option's digit");
}

fn iso(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64 + 719_468;
    let (era, day_of_era) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let (day, month) = (day_of_year - (153 * shifted + 2) / 5 + 1, if shifted < 10 { shifted + 3 } else { shifted - 9 });
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let clock = seconds % 86_400;
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z", clock / 3600, clock / 60 % 60, clock % 60)
}

#[test]
fn an_api_error_that_stops_a_claude_task_puts_it_under_needs_you_and_tells_the_lead_once() {
    let lab = Lab::new("api-error");
    let worktree = lab.start("search", "Add the search page", &[]);
    lab.agent("search", |agent| {
        agent["agent_session"] = json!({ "value": "s-search" });
        agent["cwd"] = json!(worktree.display().to_string());
    });
    let folder: String = worktree.to_string_lossy().chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect();
    let transcripts = lab.home.join(".claude/projects").join(folder);
    fs::create_dir_all(&transcripts).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let prompt = json!({ "type": "user", "uuid": "u1", "timestamp": iso(now - 60), "isSidechain": false, "message": { "role": "user", "content": "Add the search page" } });
    let error = json!({ "type": "assistant", "uuid": "e1", "timestamp": iso(now - 30), "isSidechain": false, "isApiErrorMessage": true, "error": "billing_error", "apiErrorStatus": 400,
        "message": { "model": "<synthetic>", "role": "assistant", "type": "message", "stop_reason": "stop_sequence", "content": [{ "type": "text", "text": "API Error: Your credit balance is too low" }] } });
    fs::write(transcripts.join("s-search.jsonl"), format!("{prompt}\n{error}\n")).unwrap();
    lab.keeper_round();
    let item = lab.item("search");
    assert_eq!(item["group"], "NEEDS YOU", "{item}");
    assert_eq!(item["recover"]["kind"], "stuck", "{item}");
    let told: Vec<String> = lab.to_lead().into_iter().filter(|text| text.contains("search@app stopped by a billing problem")).collect();
    assert!(told.len() == 1 && told[0].contains("Your credit balance is too low"), "{:?}", lab.to_lead());
    lab.keeper_round();
    assert_eq!(lab.to_lead().iter().filter(|text| text.contains("stopped by a billing problem")).count(), 1, "told once");
    assert!(eventually(Duration::from_secs(1), || lab.prompts_to("search").len() == 1), "kebbie does not resume it");
}

#[test]
fn a_claude_task_whose_own_title_opens_with_action_required_keeps_it_and_is_not_taken_for_a_waiting_codex() {
    let lab = Lab::new("claude-title");
    lab.start("banner", "Add the banner", &[]);
    lab.agent("banner", |agent| {
        agent["terminal_title"] = json!("Action Required banner on the settings page");
        agent["terminal_title_stripped"] = json!("Action Required banner on the settings page");
    });
    let item = lab.item("banner");
    assert_eq!((item["agent"]["title"].as_str(), item["agent"]["status"].as_str()), (Some("Action Required banner on the settings page"), Some("idle")), "{item}");
    assert_ne!(item["group"], "NEEDS YOU", "{item}");
}
