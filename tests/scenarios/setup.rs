use std::fs;

use crate::lab::{Lab, stdout};

fn row<'a>(said: &'a str, step: &str) -> &'a str {
    said.lines().find(|line| line.split_whitespace().nth(1) == Some(step)).unwrap_or_else(|| panic!("no {step} row: {said}"))
}

fn doctor_line(doctor: &str, name: &str) -> String {
    doctor.lines().find(|line| line.get(6..).is_some_and(|rest| rest.starts_with(name))).unwrap_or_else(|| panic!("no {name} line: {doctor}")).to_string()
}

#[test]
fn setup_yes_on_a_new_home_installs_the_claude_plugin_and_herdrs_keys_and_ends_on_the_doctors_count() {
    let lab = Lab::new("setup");
    let said = lab.ok(&["setup", "--yes"]);
    assert!(row(&said, "claude").starts_with("done") && row(&said, "claude").contains("installed kebbie@kebbie in Claude Code"), "{said}");
    assert!(row(&said, "keys").starts_with("done") && row(&said, "keys").contains("added prefix+space"), "{said}");
    assert!(row(&said, "link").starts_with("skipped"), "without the current link there is nothing to link: {said}");
    assert!(said.lines().any(|line| line.starts_with("Setup done · doctor: ")), "{said}");
    let plugin = fs::read_to_string(lab.root.join("claude-plugin.log")).unwrap();
    assert_eq!(plugin.lines().map(|line| line.split_whitespace().take(3).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>(), ["plugin marketplace add", "plugin install kebbie@kebbie"], "{plugin}");
    let keys = fs::read_to_string(lab.home.join(".config/herdr/config.toml")).unwrap();
    assert!(keys.contains("kebbie"), "{keys}");
    let again = lab.ok(&["setup", "--yes"]);
    assert!(!row(&again, "keys").starts_with("done"), "a second run adds no keys: {again}");
    assert_eq!(fs::read_to_string(lab.home.join(".config/herdr/config.toml")).unwrap(), keys);
}

#[test]
fn setup_codex_yes_installs_the_plugin_and_its_rules_file_while_codexs_config_stays_as_it_was() {
    let lab = Lab::new("setup-codex");
    let codex = lab.home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    let config = "# mine\nmodel = \"gpt-5.1-codex\"\n";
    fs::write(codex.join("config.toml"), config).unwrap();
    let doctor = stdout(&lab.run(&["doctor"]));
    assert!(doctor_line(&doctor, "codex plugin").starts_with("warn"), "{doctor}");
    let said = lab.ok(&["setup", "codex", "--yes"]);
    assert!(said.starts_with("kebbie's Codex plugin installed at "), "{said}");
    assert!(said.lines().any(|line| line.starts_with("done") && line.contains("codex approvals  ~/.codex/rules/kebbie.rules")), "the plugin comes with its rules file, printed: {said}");
    assert!(codex.join("rules/kebbie.rules").is_file());
    assert_eq!(fs::read_to_string(codex.join("config.toml")).unwrap(), config, "without a vault no server table or other key is added to Codex's own config");
    let doctor = stdout(&lab.run(&["doctor"]));
    assert!(doctor_line(&doctor, "codex plugin").starts_with("ok") && doctor_line(&doctor, "codex approvals").starts_with("ok"), "{doctor}");
    let again = lab.ok(&["setup", "codex", "--yes"]);
    assert!(again.starts_with("kebbie's Codex plugin is up to date at ") && !again.contains("done"), "a second run writes nothing: {again}");
}

#[test]
fn setup_yes_sets_up_every_agent_found_and_each_agents_own_setup_writes_its_own_part() {
    let lab = Lab::new("setup-agents");
    let claude = lab.ok(&["setup", "claude", "--yes"]);
    assert!(claude.lines().next().unwrap().contains("plugin") && !claude.contains("Codex"), "{claude}");
    assert!(!lab.home.join(".codex").exists(), "kebbie setup claude writes nothing of Codex's");
    let both = lab.ok(&["setup", "--yes"]);
    assert!(row(&both, "codex").starts_with("done") && row(&both, "codex").contains(".codex/plugins/cache/kebbie-tasks"), "a Codex found is ticked by default, so --yes installs its plugin: {both}");
    assert!(lab.home.join(".codex/rules/kebbie.rules").is_file(), "and its rules file: {both}");
    let again = lab.ok(&["setup", "--yes"]);
    assert!(again.lines().filter(|line| line.split_whitespace().nth(1) == Some("codex")).all(|line| line.starts_with("already")), "{again}");
}

#[test]
fn the_doctor_in_the_suite_reads_only_the_lab_and_the_systems_own_tools() {
    let lab = Lab::new("setup-doctor");
    let doctor = stdout(&lab.run(&["doctor"]));
    let root = lab.root.display().to_string();
    let outside = doctor.replace(&root, "<lab>");
    let facts: Vec<&str> = outside.lines().filter(|line| !line.trim_start().starts_with("fix:")).collect();
    for host in [" /Users/", " /home/", " /opt/homebrew", ".cargo/bin", " /usr/local/bin"] {
        let leaked: Vec<&&str> = facts.iter().filter(|line| line.contains(host)).collect();
        assert!(leaked.is_empty(), "{host} reached the doctor: {leaked:?}");
    }
    for tool in ["herdr", "claude", "codex"] {
        assert!(doctor_line(&doctor, tool).contains(&root), "{tool} is the lab's: {doctor}");
    }
    assert!(!doctor.contains("push block"), "no checkout around the lab, so no push block line: {doctor}");
}
