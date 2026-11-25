use std::fs;

use crate::scenarios::lab::{Lab, stderr};

const DESIGNER: &str = "# UI work, mockup first\nabout = \"UI work\" # shown on the board\nmodel = \"opus\"\neffort = \"high\"\nrole = \"designer.md\"\n";

#[test]
fn a_profiles_model_effort_and_role_reach_the_task_it_starts_and_a_profile_that_does_not_load_starts_nothing() {
    let lab = Lab::new("profiles");
    lab.profile("designer", DESIGNER);
    fs::write(lab.home.join(".config/kebbie/profiles/designer.md"), "Mock every new surface first.\n").unwrap();
    let listed: serde_json::Value = serde_json::from_str(&lab.ok(&["profiles", "--json"])).unwrap();
    let designer = listed.as_array().unwrap().iter().find(|profile| profile["name"] == "designer").unwrap_or_else(|| panic!("{listed}"));
    assert_eq!((designer["model"].as_str(), designer["effort"].as_str()), (Some("opus"), Some("high")), "{designer}");

    lab.start("mockup", "Mock the search page", &["--profile", "designer"]);
    let start = lab.herdr_calls().into_iter().find(|call| call[..2] == ["agent", "start"]).unwrap();
    assert!(start.windows(2).any(|pair| pair == ["--model", "opus"]) && start.windows(2).any(|pair| pair == ["--effort", "high"]), "{start:?}");
    let role = fs::read_to_string(lab.state.join("roles/mockup@app.md")).unwrap();
    assert!(role.contains("Mock every new surface first."), "the profile's role follows kebbie's own: {role}");
    assert_eq!(lab.item("mockup")["profile"], "designer");
    assert_eq!(fs::read_to_string(lab.home.join(".config/kebbie/profiles/designer.toml")).unwrap(), DESIGNER, "a start writes nothing back into the profile");

    lab.profile("broken", "effort = \"huge\"\n");
    let refused = lab.run(&["-C", lab.app(), "start", "other", "x", "--profile", "broken"]);
    assert!(!refused.status.success() && stderr(&refused).contains("effort: \"huge\" is not an effort level"), "{}", stderr(&refused));
    assert!(!lab.on_board("other"), "nothing started for it");
}

#[test]
fn a_codex_profile_on_a_machine_that_did_not_set_codex_up_is_refused_and_marked_until_it_does() {
    let lab = Lab::new("codex-unset");
    lab.profile("coder", "agent = \"codex\"\n");
    let refused = lab.run(&["-C", lab.app(), "start", "search", "Add search", "--profile", "coder"]);
    assert!(!refused.status.success() && stderr(&refused).contains("Codex is not set up: kebbie setup codex"), "{}", stderr(&refused));
    assert!(!lab.worktree(1).exists() && !lab.on_board("search"), "nothing is claimed for it");
    assert!(lab.ok(&["profiles"]).contains("  ⚠ Codex is not set up: kebbie setup codex"));
    let doctor = crate::scenarios::lab::stdout(&lab.run(&["doctor"]));
    assert!(doctor.contains("profile coder: Codex is not set up: kebbie setup codex"), "{doctor}");
    lab.ok(&["setup", "codex", "--yes"]);
    assert!(!lab.ok(&["profiles"]).contains("Codex is not set up"));
    lab.start("search", "Add search", &["--profile", "coder"]);
}
