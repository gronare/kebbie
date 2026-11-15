use super::*;

#[test]
fn a_repos_jobs_come_from_its_config_on_the_master_as_a_command_or_a_table_that_picks_the_worker() {
    let jobs = |text: &str| parse(&text.parse::<toml::Table>().unwrap());
    let read = jobs("[jobs]\nlint = \"swiftlint\"\n\n[jobs.build]\nrun = \"xcodebuild -scheme App build\"\non = \"macos\"\n").unwrap();
    assert_eq!(read.get("lint"), Some(&Def { run: "swiftlint".to_string(), on: None }));
    assert_eq!(read.get("build"), Some(&Def { run: "xcodebuild -scheme App build".to_string(), on: Some("macos".to_string()) }));
    assert!(jobs("[worktree]\ndev_url = \"x\"\n").unwrap().is_empty(), "a repo without jobs has none");
    for (text, said) in [("jobs = 3\n", "must be a table of jobs"), ("[jobs]\nbuild = \"  \"\n", "jobs.build: must be a non-empty string"), ("[jobs.build]\non = \"macos\"\n", "jobs.build: needs run"), ("[jobs.build]\nrun = \"x\"\nwhere = \"mac\"\n", "jobs.build.where: unknown key"), ("[jobs]\nbuild = 3\n", "must be a command, or a table")] {
        assert!(jobs(text).unwrap_err().contains(said), "{text}");
    }
}
