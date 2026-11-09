use super::*;
use crate::agent::Agent;
use crate::board::widgets::TEXT;
use crate::config::Layer;
use crate::profile_wizard::Pick;

struct Fixture {
    dir: PathBuf,
    page: Page,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(name: &str, machine: &str, repo_file: Option<&str>) -> Fixture {
    let dir = crate::sh::test_dir(name);
    let repo = dir.join("app");
    std::fs::create_dir_all(&repo).unwrap();
    crate::repo::git(&repo, &["init", "--quiet"]).unwrap();
    std::fs::create_dir_all(dir.join("repos")).unwrap();
    std::fs::write(dir.join("config.toml"), machine).unwrap();
    if let Some(text) = repo_file {
        std::fs::write(dir.join("repos/app.toml"), text).unwrap();
    }
    let kebbie = dir.join("kebbie");
    std::fs::create_dir_all(kebbie.join("skills/work-rules")).unwrap();
    std::fs::write(kebbie.join("skills/work-rules/SKILL.md"), "---\nname: work-rules\n---\n# Work rules\n").unwrap();
    let paths = Paths { machine: dir.join("config.toml"), repos_dir: dir.join("repos"), claude_home: dir.join("claude"), kebbie, profiles: dir.join("profiles"), codex_home: dir.join("codex"), path: vec![dir.join("bin")], kebbie_command: "kebbie".to_string(), sessions: Some(dir.join("state/sessions")) };
    let mut page = Page::new(paths, Some(repo));
    page.load();
    Fixture { page, dir }
}

fn press(page: &mut Page, keys: &[KeyCode]) {
    for code in keys {
        page.handle(KeyEvent::from(*code));
    }
}

fn type_text(page: &mut Page, text: &str) {
    for character in text.chars() {
        page.handle(KeyEvent::from(KeyCode::Char(character)));
    }
}

fn go_to(page: &mut Page, key: &str) {
    page.filter = String::new();
    let sections = page.sections();
    let (section, row) = sections.iter().enumerate().find_map(|(at, (_, rows))| rows.iter().position(|index| page.entries[*index].key == key).map(|row| (at, row))).unwrap_or_else(|| panic!("{key} is not on the {:?} tab", page.tab));
    (page.section, page.row) = (section, row);
}

fn said(page: &Page) -> String {
    page.said.clone().map(|(text, _)| text).unwrap_or_default()
}

fn file(fixture: &Fixture, name: &str) -> String {
    std::fs::read_to_string(fixture.dir.join(name)).unwrap_or_default()
}

fn screen(page: &mut Page) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
    terminal.draw(|frame| page.draw_alone(frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_page_lists_what_kebbie_config_prints_with_each_layer() {
    let mut fixture = fixture("settings-list", "[task]\nsandbox = false\n", Some("[docs]\nrequired = true\n"));
    let loaded = config::resolve_at(fixture.dir.join("config.toml"), None).unwrap();
    let printed: Vec<&str> = config::entries(&loaded).iter().filter(|entry| entry.scope != Scope::Repo).map(|entry| entry.key).collect();
    let mut listed: Vec<&str> = fixture.page.entries.iter().map(|entry| entry.key).collect();
    let mut expected = printed.clone();
    listed.sort();
    expected.sort();
    assert_eq!(listed, expected, "the machine tab is kebbie config's keys, less the repo-only ones");
    let shown = screen(&mut fixture.page);
    assert!(shown.contains("Settings") && shown.contains(" This machine ") && shown.contains(" Repo: app "), "{shown}");
    assert!(shown.contains("▸ Tasks") && shown.contains("task.rules_skill") && shown.contains("\"kebbie:work-rules\""), "{shown}");
    assert!(shown.contains("task.sandbox") && shown.contains("○ off") && shown.lines().any(|line| line.contains("task.sandbox") && line.ends_with("machine")), "{shown}");
    assert!(shown.lines().any(|line| line.contains("task.rules_skill") && line.ends_with("default")), "{shown}");
    assert!(shown.contains("The skill with the rules for how work is planned"), "the selected key's text shows: {shown}");
    press(&mut fixture.page, &[KeyCode::Tab]);
    assert_eq!(fixture.page.tab, Tab::Repo);
    assert!(fixture.page.entries.iter().all(|entry| entry.scope != Scope::Machine), "a config.toml-only key never shows on the repo tab");
    assert!(fixture.page.entries.iter().any(|entry| entry.key == "docs.required"));
    go_to(&mut fixture.page, "docs.required");
    let shown = screen(&mut fixture.page);
    assert!(shown.lines().any(|line| line.contains("docs.required") && line.contains("● on") && line.ends_with("repo app")), "{shown}");
}

#[test]
fn space_turns_steps_off_for_the_machine_and_on_again_for_one_repo() {
    let mut fixture = fixture("settings-steps", "", None);
    go_to(&mut fixture.page, "task.steps");
    assert!(screen(&mut fixture.page).lines().any(|line| line.contains("task.steps") && line.contains("● on")), "on by default");
    press(&mut fixture.page, &[KeyCode::Char(' ')]);
    assert_eq!(file(&fixture, "config.toml"), "[task]\nsteps = false\n");
    assert!(said(&fixture.page).starts_with("✓ wrote task.steps = false to config.toml"), "{}", said(&fixture.page));
    press(&mut fixture.page, &[KeyCode::Tab]);
    go_to(&mut fixture.page, "task.steps");
    press(&mut fixture.page, &[KeyCode::Char(' ')]);
    assert_eq!(file(&fixture, "repos/app.toml"), "[task]\nsteps = true\n", "a repo keeps its own way of working");
    assert_eq!(fixture.page.current().unwrap().layer, Layer::Repo);
}

#[test]
fn space_turns_accepting_every_commit_on_for_one_repo_under_tasks() {
    let mut fixture = fixture("settings-require-accept", "", None);
    go_to(&mut fixture.page, "task.require_accept");
    let shown = screen(&mut fixture.page);
    assert!(shown.lines().any(|line| line.contains("task.require_accept") && line.contains("○ off") && line.ends_with("default")), "off by default: {shown}");
    press(&mut fixture.page, &[KeyCode::Tab]);
    go_to(&mut fixture.page, "task.require_accept");
    press(&mut fixture.page, &[KeyCode::Char(' ')]);
    assert_eq!(file(&fixture, "repos/app.toml"), "[task]\nrequire_accept = true\n");
    assert_eq!(fixture.page.current().unwrap().layer, Layer::Repo);
}

#[test]
fn space_toggles_a_boolean_in_the_file_of_the_tab_keeping_its_comments() {
    let mut fixture = fixture("settings-bool", "# mine\n[task]\nsandbox = true # build box\n", None);
    go_to(&mut fixture.page, "task.sandbox");
    press(&mut fixture.page, &[KeyCode::Char(' ')]);
    assert_eq!(file(&fixture, "config.toml"), "# mine\n[task]\nsandbox = false # build box\n");
    assert!(said(&fixture.page).starts_with("✓ wrote task.sandbox = false to config.toml · doctor: ok, config.toml parses"), "{}", said(&fixture.page));
    press(&mut fixture.page, &[KeyCode::Tab]);
    go_to(&mut fixture.page, "ci.watch");
    press(&mut fixture.page, &[KeyCode::Char(' ')]);
    assert_eq!(file(&fixture, "repos/app.toml"), "[ci]\nwatch = false\n", "the repo tab writes the repo's own file");
    assert!(!file(&fixture, "config.toml").contains("[ci]"));
    assert_eq!(fixture.page.current().unwrap().layer, Layer::Repo);
}

#[test]
fn a_choice_is_picked_from_its_list_and_none_removes_the_key() {
    let mut fixture = fixture("settings-choice", "", None);
    go_to(&mut fixture.page, "task.also_noticed");
    press(&mut fixture.page, &[KeyCode::Enter]);
    let shown = screen(&mut fixture.page);
    assert!(shown.contains("╭ task.also_noticed ") && shown.contains("│ Which value?") && shown.contains("│ What a task's Claude does with what it notices"), "the key is the title, its text the explanation: {shown}");
    assert!(shown.contains("│ ▌\"backlog\"           now ") && shown.contains("│  \"ask\"  ") && shown.contains("│ ↑ ↓ choose · ⏎ set · esc close"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "[task]\nalso_noticed = \"ask\"\n");
    go_to(&mut fixture.page, "task.permission_mode");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    assert!(file(&fixture, "config.toml").contains("permission_mode = \"acceptEdits\""));
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(matches!(&fixture.page.mode, Mode::Choose(chooser) if chooser.index == 2), "the pick opens on the value in effect");
    press(&mut fixture.page, &[KeyCode::Up, KeyCode::Up, KeyCode::Enter]);
    assert!(!file(&fixture, "config.toml").contains("permission_mode"), "none leaves it to Claude: the key goes");
}

fn click(page: &mut Page, shown: &str, text: &str) -> Effect {
    let (row, line) = shown.lines().enumerate().find(|(_, line)| line.contains(text)).unwrap_or_else(|| panic!("{text}: {shown}"));
    let column = line.chars().collect::<Vec<_>>().windows(text.chars().count()).position(|window| window.iter().collect::<String>() == text).unwrap();
    page.pointer(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: column as u16, row: row as u16, modifiers: KeyModifiers::NONE })
}

#[test]
fn a_click_selects_an_answer_takes_it_on_the_second_and_one_beside_the_pick_closes_it() {
    let mut fixture = fixture("settings-click", "", None);
    go_to(&mut fixture.page, "task.also_noticed");
    press(&mut fixture.page, &[KeyCode::Enter]);
    let shown = screen(&mut fixture.page);
    click(&mut fixture.page, &shown, "  \"ask\"  ");
    assert!(matches!(&fixture.page.mode, Mode::Choose(chooser) if chooser.index == 2), "the first click selects");
    assert_eq!(file(&fixture, "config.toml"), "", "and writes nothing");
    let shown = screen(&mut fixture.page);
    click(&mut fixture.page, &shown, "▌\"ask\"");
    assert_eq!(file(&fixture, "config.toml"), "[task]\nalso_noticed = \"ask\"\n", "the second takes it");
    press(&mut fixture.page, &[KeyCode::Enter]);
    let shown = screen(&mut fixture.page);
    click(&mut fixture.page, &shown, "esc close");
    assert!(fixture.page.is_normal(), "a key in the hints is pressed by a click");
    press(&mut fixture.page, &[KeyCode::Enter]);
    let shown = screen(&mut fixture.page);
    click(&mut fixture.page, &shown, "Settings");
    assert!(fixture.page.is_normal(), "a click beside the pick closes it");
    let wheel = |kind| MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE };
    let row = fixture.page.row;
    fixture.page.pointer(wheel(MouseEventKind::ScrollDown));
    assert_eq!(fixture.page.row, row + 1, "the wheel moves through the keys, kebbie config edit's too");
    press(&mut fixture.page, &[KeyCode::Up, KeyCode::Enter]);
    let index = match &fixture.page.mode { Mode::Choose(chooser) => chooser.index, _ => panic!("a pick") };
    fixture.page.pointer(wheel(MouseEventKind::ScrollUp));
    assert!(matches!(&fixture.page.mode, Mode::Choose(chooser) if chooser.index == index - 1), "and through a pick's answers");
}

#[test]
fn text_numbers_and_lists_are_edited_in_place_and_a_bad_value_writes_nothing() {
    let mut fixture = fixture("settings-text", "[restart]\nidle_minutes = 10\n", None);
    go_to(&mut fixture.page, "restart.idle_minutes");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Backspace, KeyCode::Backspace]);
    type_text(&mut fixture.page, "25");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "[restart]\nidle_minutes = 25\n");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Backspace, KeyCode::Backspace]);
    type_text(&mut fixture.page, "0");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(said(&fixture.page).starts_with("✗ ") && said(&fixture.page).contains("1 to 1440"), "{}", said(&fixture.page));
    assert_eq!(file(&fixture, "config.toml"), "[restart]\nidle_minutes = 25\n", "a value kebbie refuses is never written");
    press(&mut fixture.page, &[KeyCode::Enter]);
    type_text(&mut fixture.page, "x");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(said(&fixture.page).contains("\"25x\" is not a whole number"), "{}", said(&fixture.page));
    go_to(&mut fixture.page, "task.test_commands");
    press(&mut fixture.page, &[KeyCode::Enter]);
    type_text(&mut fixture.page, "bin/test, make check");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(file(&fixture, "config.toml").contains("[task]\ntest_commands = [\"bin/test\", \"make check\"]\n"), "{}", file(&fixture, "config.toml"));
    go_to(&mut fixture.page, "machine.name");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Char('u')]);
    fixture.page.handle(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_text(&mut fixture.page, "devbox");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(file(&fixture, "config.toml").contains("[machine]\nname = \"devbox\"\n"));
    press(&mut fixture.page, &[KeyCode::Enter]);
    type_text(&mut fixture.page, "zzz");
    press(&mut fixture.page, &[KeyCode::Esc]);
    assert!(file(&fixture, "config.toml").contains("name = \"devbox\""), "esc cancels the edit");
}

#[test]
fn d_removes_the_key_so_the_layer_below_applies() {
    let mut fixture = fixture("settings-remove", "[ci]\nwatch = false\n", Some("# team\n[ci]\nwatch = true\n"));
    press(&mut fixture.page, &[KeyCode::Tab]);
    go_to(&mut fixture.page, "ci.watch");
    assert_eq!(fixture.page.current().unwrap().layer, Layer::Repo);
    press(&mut fixture.page, &[KeyCode::Char('d')]);
    assert_eq!(file(&fixture, "repos/app.toml"), "# team\n[ci]\n");
    let entry = fixture.page.current().unwrap();
    assert_eq!((entry.value.as_str(), entry.layer), ("false", Layer::Machine), "the machine's value applies once the repo's goes");
    assert!(said(&fixture.page).starts_with("✓ removed ci.watch from repos/app.toml"), "{}", said(&fixture.page));
}

#[test]
fn a_value_from_a_team_link_asks_whether_to_change_the_team_file_or_override_it_here() {
    let mut fixture = fixture("settings-team", "", None);
    let team = fixture.dir.join("team.toml");
    std::fs::write(&team, "[ci]\nwatch = true\n").unwrap();
    std::os::unix::fs::symlink(&team, fixture.dir.join("repos/app.toml")).unwrap();
    fixture.page.load();
    press(&mut fixture.page, &[KeyCode::Tab]);
    go_to(&mut fixture.page, "ci.watch");
    assert!(screen(&mut fixture.page).lines().any(|line| line.contains("ci.watch") && line.ends_with("team link")));
    press(&mut fixture.page, &[KeyCode::Char(' ')]);
    let shown = screen(&mut fixture.page);
    assert!(shown.contains("change the team's file") && shown.contains("override it here"), "{shown}");
    assert_eq!(std::fs::read_to_string(&team).unwrap(), "[ci]\nwatch = true\n", "nothing is written before the answer");
    press(&mut fixture.page, &[KeyCode::Char('t')]);
    assert_eq!(std::fs::read_to_string(&team).unwrap(), "[ci]\nwatch = false\n");
    press(&mut fixture.page, &[KeyCode::Char(' '), KeyCode::Char('o')]);
    let own = fixture.dir.join("repos/app.toml");
    assert!(!std::fs::symlink_metadata(&own).unwrap().file_type().is_symlink());
    assert_eq!((std::fs::read_to_string(&own).unwrap(), std::fs::read_to_string(&team).unwrap()), ("[ci]\nwatch = true\n".to_string(), "[ci]\nwatch = false\n".to_string()));
}

#[test]
fn a_skill_key_offers_built_in_a_name_a_file_or_a_copy_of_kebbies_to_edit() {
    let mut fixture = fixture("settings-skill", "", None);
    go_to(&mut fixture.page, "task.rules_skill");
    press(&mut fixture.page, &[KeyCode::Enter]);
    let shown = screen(&mut fixture.page);
    assert!(shown.contains("│ Which skill?") && shown.contains("▌\"kebbie:work-rules\"      built in") && shown.contains(" Your skill, by name      <skill> or <plugin>:<skill>") && shown.contains("A Markdown file, by path") && shown.contains("Copy kebbie's to"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    type_text(&mut fixture.page, "~/code/team/RULES.md");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "[task]\nrules_skill = \"~/code/team/RULES.md\"\n");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Enter]);
    type_text(&mut fixture.page, "team-rules");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "[task]\nrules_skill = \"team-rules\"\n");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    let copied = fixture.dir.join("claude/skills/work-rules/SKILL.md");
    assert_eq!(std::fs::read_to_string(&copied).unwrap(), "---\nname: work-rules\n---\n# Work rules\n");
    assert_eq!(file(&fixture, "config.toml"), "[task]\nrules_skill = \"work-rules\"\n");
    assert!(said(&fixture.page).contains("copied kebbie's skill to") && said(&fixture.page).contains("work-rules/SKILL.md"), "{}", said(&fixture.page));
    std::fs::write(&copied, "mine").unwrap();
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    assert_eq!(std::fs::read_to_string(&copied).unwrap(), "mine", "a copy already there is the user's and stays");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "[task]\nrules_skill = \"kebbie:work-rules\"\n");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    type_text(&mut fixture.page, "RULES.md");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(said(&fixture.page).contains("a file goes by its absolute or ~ path"), "{}", said(&fixture.page));
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down, KeyCode::Enter]);
    type_text(&mut fixture.page, "team/rules");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(said(&fixture.page).contains("not a skill name or a Markdown file"), "{}", said(&fixture.page));
    assert_eq!(file(&fixture, "config.toml"), "[task]\nrules_skill = \"kebbie:work-rules\"\n");
}

#[test]
fn slash_filters_the_keys_tab_switches_and_esc_leaves() {
    let mut fixture = fixture("settings-filter", "", None);
    press(&mut fixture.page, &[KeyCode::Char('/')]);
    type_text(&mut fixture.page, "idle_m");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(fixture.page.current().map(|entry| entry.key), Some("restart.idle_minutes"));
    assert_eq!(fixture.page.sections()[0].1.len(), 1);
    assert!(screen(&mut fixture.page).contains("Search: idle_m"));
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Esc)), Effect::Handled), "the first esc clears the search");
    assert!(fixture.page.filter.is_empty());
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Esc)), Effect::Left));
    let mut alone = Page::new(Paths { machine: fixture.dir.join("config.toml"), repos_dir: fixture.dir.join("repos"), claude_home: fixture.dir.join("claude"), kebbie: fixture.dir.join("kebbie"), profiles: fixture.dir.join("profiles"), codex_home: fixture.dir.join("codex"), path: Vec::new(), kebbie_command: "kebbie".to_string(), sessions: Some(fixture.dir.join("state/sessions")) }, None);
    alone.load();
    press(&mut alone, &[KeyCode::Tab]);
    assert_eq!(alone.tab, Tab::Machine);
    assert!(said(&alone).contains("No repo here"));
}

#[test]
fn a_file_that_does_not_parse_is_shown_and_left_alone() {
    let mut fixture = fixture("settings-broken", "[task]\nsandbox = \"yes\"\n", None);
    assert!(fixture.page.entries.is_empty());
    let shown = screen(&mut fixture.page).split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(shown.contains("task.sandbox: must be true or false"), "the error shows, wherever the path before it makes it wrap: {shown}");
    press(&mut fixture.page, &[KeyCode::Char(' '), KeyCode::Char('d')]);
    assert_eq!(file(&fixture, "config.toml"), "[task]\nsandbox = \"yes\"\n");
}

const CODEX_BUILD: &str = "# for small builds\nabout = \"Codex, building: a small, well-defined task done by another model family\"\nagent = \"codex\" # not Claude\nmodel = \"gpt-6.1-sol\"\neffort = \"medium\"\nrole = \"codex.md\"\n";

fn agents_fixture(name: &str) -> Fixture {
    let fixture = fixture(name, "", None);
    let dir = &fixture.dir;
    for folder in ["profiles", "codex", "claude"] {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
    }
    std::fs::write(dir.join("profiles/codex.toml"), "about = \"Codex on its own config\"\nagent = \"codex\"\neffort = \"medium\"\nrole = \"codex.md\"\n").unwrap();
    std::fs::write(dir.join("profiles/codex.md"), "Run the tests before each commit.\n").unwrap();
    std::fs::write(dir.join("profiles/designer.toml"), "about = \"UI work: mockup first\"\nmodel = \"opus\"\neffort = \"high\"\nrole = \"designer.md\"\n").unwrap();
    std::fs::write(dir.join("profiles/designer.md"), "Measure in the browser.\n").unwrap();
    std::fs::write(dir.join("kebbie/Cargo.toml"), "[package]\nname = \"kebbie\"\nversion = \"0.30.0\"\n").unwrap();
    for role in ["roles/task.md", "skills/review/SKILL.md"] {
        std::fs::create_dir_all(dir.join("kebbie").join(role).parent().unwrap()).unwrap();
        std::fs::write(dir.join("kebbie").join(role), "kebbie's own role\n").unwrap();
    }
    std::fs::write(dir.join("profiles/codex-build.toml"), CODEX_BUILD).unwrap();
    std::fs::write(dir.join("profiles/reviewer.toml"), "model = \"opus\"\neffort = \"max\"\n").unwrap();
    std::fs::write(dir.join("codex/config.toml"), "model = \"gpt-5.1-codex-max\"\n").unwrap();
    std::fs::write(dir.join("codex/models_cache.json"), crate::agent::codex::CACHE_FIXTURE).unwrap();
    std::fs::write(dir.join("claude/settings.json"), "{\"model\": \"opus\"}").unwrap();
    crate::agent::codex::install(&dir.join("codex"), "kebbie").unwrap();
    let mut fixture = fixture;
    fixture.page.load();
    fixture
}

fn go_to_profile(page: &mut Page, name: &str) {
    page.filter = String::new();
    page.section = page.sections().iter().position(|(section, _)| section == PROFILES).expect("a Profiles section");
    page.row = page.agents.profiles.iter().position(|profile| profile.name == name).unwrap_or_else(|| panic!("no profile {name}"));
}

fn screen_at(page: &mut Page, width: u16, height: u16) -> (String, ratatui::buffer::Buffer) {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| page.draw_alone(frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let text = (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n");
    (text, buffer)
}

fn color_of(buffer: &ratatui::buffer::Buffer, line: &str, word: &str) -> Option<Color> {
    let lines: Vec<String> = (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect()).collect();
    let y = lines.iter().position(|text| text.contains(line))?;
    let x = lines[y].chars().collect::<Vec<_>>().windows(word.chars().count()).position(|window| window.iter().collect::<String>() == word)?;
    buffer[(x as u16, y as u16)].fg.into()
}

fn line_of<'a>(shown: &'a str, text: &str) -> &'a str {
    shown.lines().find(|line| line.contains(text)).unwrap_or_else(|| panic!("{text}: {shown}"))
}

#[test]
fn the_profiles_section_keeps_kebbies_profiles_apart_from_yours_with_each_roles_two_layers() {
    let mut fixture = agents_fixture("settings-profiles");
    let names: Vec<String> = fixture.page.sections().into_iter().map(|(name, _)| name).collect();
    let at = names.iter().position(|name| name == PROFILES).unwrap();
    assert_eq!((names[at - 1].as_str(), names[at + 1].as_str()), ("Review & docs", "Updates"));
    go_to_profile(&mut fixture.page, "designer");
    let (shown, buffer) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("▸ Profiles 6"), "{shown}");
    assert!(screen_at(&mut fixture.page, 300, 30).0.contains(&format!("writes {}", fixture.dir.join("profiles/<name>.toml").display())));
    let at = |text: &str| shown.find(text).unwrap_or_else(|| panic!("{text}: {shown}"));
    assert!(at(" FROM KEBBIE") < at(" task ") && at(" ops ") < at(" YOURS") && at(" YOURS") < at(" codex "), "kebbie's three, then the user's: {shown}");
    assert!(line_of(&shown, "FROM KEBBIE").ends_with(" FROM KEBBIE  3") && line_of(&shown, "YOURS").ends_with(" YOURS  3"), "each group headed as the board's, with its count: {shown}");
    assert!(line_of(&shown, "FROM KEBBIE").len() < line_of(&shown, "follow kebbie's updates; your settings on them are kept").len(), "{shown}");
    assert!(screen_at(&mut fixture.page, 300, 30).0.contains(&format!("{} · an update never writes here", fixture.dir.join("profiles").display())));
    assert!(line_of(&shown, " task ").ends_with("task         claude · opus                          kebbie's task role"), "{shown}");
    assert!(line_of(&shown, " reviewer ").ends_with("reviewer     claude · opus · max                    kebbie's review role"), "{shown}");
    assert!(line_of(&shown, " codex ").ends_with("codex        codex · gpt-5.1-codex-max ⚠ · medium   + codex.md"), "{shown}");
    assert!(line_of(&shown, " codex-build ").ends_with("codex-build  codex · gpt-6.1-sol · medium           + codex.md"), "a shared role shows on both: {shown}");
    assert!(line_of(&shown, " designer ").ends_with("▌ designer     claude · opus · high                   + designer.md"), "{shown}");
    assert!(line_of(&shown, "role:  ").ends_with("    role:  kebbie's task role, from kebbie 0.30.0, updates with kebbie") && line_of(&shown, "+ designer.md, ").ends_with("         + designer.md, yours: read after kebbie's role"), "{shown}");
    assert!(shown.contains("    m model  e effort  r role  n new profile  ⏎ open file  x delete"), "{shown}");
    assert!(shown.contains("  designer: claude, opus, high · role designer.md"), "the line under the page sums the selected profile up: {shown}");
    assert!(shown.lines().any(|line| line.trim_start() == "↑ ↓ profile · m model · e effort · r role · n new · tab repo · esc quit"), "{shown}");
    let (wide, _) = screen_at(&mut fixture.page, 150, 30);
    assert!(wide.lines().any(|line| line.trim_start() == "↑ ↓ profile · ← → section · m model · e effort · r role · n new · ⏎ open · d default · x delete · tab machine / repo · / search · esc quit"), "the section's keys and the page's own, as on every section: {wide}");
    assert_eq!(color_of(&buffer, " task ", "opus"), Some(MUTED), "the agent's own default shows dim");
    assert_eq!(color_of(&buffer, " reviewer ", "opus"), Some(TEXT), "the profile's own model does not");
    assert_eq!(color_of(&buffer, " codex  ", "gpt-5.1-codex-max"), Some(RED));
    assert!(shown.contains("dim = the agent's own default:") && shown.contains("Claude's from"), "{shown}");
    go_to_profile(&mut fixture.page, "codex");
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("⚠ codex runs gpt-5.1-codex-max from Codex's config.toml, which Codex does not offer") && shown.contains("account; pick a model for it with m."), "the selected profile's ⚠ comes first: {shown}");
    go_to_profile(&mut fixture.page, "task");
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("    m model  e effort  r role  n new profile  ⏎ open file  d default"), "kebbie's own keep d and cannot be deleted: {shown}");
    assert!(line_of(&shown, "role:  ").ends_with("kebbie's task role, from kebbie 0.30.0, updates with kebbie") && !shown.contains("yours: read after") && shown.contains("  task: claude, opus by Claude's default"), "kebbie's role alone: {shown}");
    press(&mut fixture.page, &[KeyCode::Tab]);
    assert!(!fixture.page.sections().iter().any(|(name, _)| name == PROFILES), "profiles are this machine's, never a repo's");
}

#[test]
fn the_profiles_section_fits_a_narrow_window_dropping_the_role_column_and_wrapping_the_keys() {
    let mut fixture = agents_fixture("settings-profiles-narrow");
    go_to_profile(&mut fixture.page, "codex-build");
    let (shown, _) = screen_at(&mut fixture.page, 60, 40);
    let rows: Vec<String> = shown.lines().map(|line| line.chars().skip(24).collect::<String>()).collect();
    let row = |text: &str| rows.iter().find(|line| line.contains(text)).cloned().unwrap_or_else(|| panic!("{text}: {shown}"));
    assert!(rows.iter().all(|line| line.chars().count() <= 34), "nothing runs past the rows: {shown}");
    assert!(!row("▌ codex-b…").contains("codex.md") && row(" codex  ").ends_with('…'), "the role column goes, a name or a row too long ends in …: {shown}");
    assert!(row("+ codex.md, yours").starts_with("         + codex.md"), "the selected row still names its role: {shown}");
    assert!(rows.iter().any(|line| line.trim_start().starts_with("m model  e effort")) && rows.iter().any(|line| line.contains("x delete")), "the keys wrap onto a second line: {shown}");
    assert!(row(" YOURS  3").starts_with(" YOURS") && rows.iter().any(|line| line.starts_with("  ") && line.ends_with('…') && line.contains('/')), "the folder's path is cut, not run off: {shown}");
    let (wide, _) = screen_at(&mut fixture.page, 100, 40);
    assert!(line_of(&wide, " codex-build ").ends_with("codex-build  codex · gpt-6.1-sol · medium           + codex.md") && wide.contains("    m model  e effort  r role  n new profile  ⏎ open file  x delete"), "{wide}");
}

#[test]
fn r_on_one_of_your_profiles_picks_a_role_file_with_who_shares_it_or_makes_one_that_overwrites_none() {
    let mut fixture = agents_fixture("settings-role-pick");
    let read = |name: &str| std::fs::read_to_string(fixture.dir.join("profiles").join(name)).unwrap_or_default();
    go_to_profile(&mut fixture.page, "designer");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("╭ role · designer ") && shown.contains("│ Which role?") && shown.contains("│ Your lines, read after kebbie's task role."), "{shown}");
    assert!(shown.contains("│ ▌designer.md         only designer · now") && shown.contains("│  codex.md            shared with codex, codex-build"), "{shown}");
    assert!(shown.contains("│  New role            designer-2.md, an empty file") && shown.contains("│  Copy of designer.md to change it for this profile alone") && shown.contains("│  None                kebbie's role alone"), "{shown}");
    assert!(shown.contains("│ A shared file changes for every profile that uses it.") && shown.contains("│ ↑ ↓ choose · ⏎ set · o open the file · esc close"), "{shown}");
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Char('o'))), Effect::Open(file) if file == fixture.dir.join("profiles/designer.md")), "o opens the selected file");
    press(&mut fixture.page, &[KeyCode::Char('r'), KeyCode::Down, KeyCode::Enter]);
    assert_eq!(read("designer.toml"), "about = \"UI work: mockup first\"\nmodel = \"opus\"\neffort = \"high\"\nrole = \"codex.md\"\n", "only the role line changes");
    assert_eq!(read("designer.md"), "Measure in the browser.\n", "the file it left stays");
    assert!(said(&fixture.page).starts_with("✓ wrote role = \"codex.md\" to profiles/designer.toml"), "{}", said(&fixture.page));
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("│ ▌codex.md            shared with codex, codex-build, designer ·") && shown.contains("│                      now  ") && shown.contains("│  designer.md         used by none") && shown.contains("Copy of codex.md"), "{shown}");
    let Mode::Choose(chooser) = &fixture.page.mode else { panic!("a pick") };
    let copy = chooser.choices.iter().position(|choice| choice.label == "Copy of codex.md").unwrap();
    press(&mut fixture.page, &vec![KeyCode::Down; copy]);
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Enter)), Effect::Open(file) if file == fixture.dir.join("profiles/designer-2.md")), "a copy opens to be changed");
    assert_eq!((read("designer-2.md"), read("designer.md")), ("Run the tests before each commit.\n".to_string(), "Measure in the browser.\n".to_string()), "named after the profile, beside the file that has its name");
    assert!(read("designer.toml").contains("role = \"designer-2.md\"\n"));
    fixture.page.opened(&fixture.dir.join("profiles/designer-2.md"), Ok(true));
    assert_eq!(said(&fixture.page), "profiles/designer-2.md: read after kebbie's role by designer");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    let Mode::Choose(chooser) = &fixture.page.mode else { panic!("a pick") };
    let new = chooser.choices.iter().position(|choice| choice.label == "New role").unwrap();
    press(&mut fixture.page, &vec![KeyCode::Down; new]);
    press(&mut fixture.page, &[KeyCode::Char('o')]);
    assert!(said(&fixture.page).contains("o opens a role file; this one has none yet") && matches!(fixture.page.mode, Mode::Choose(_)), "{}", said(&fixture.page));
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Enter)), Effect::Open(file) if file == fixture.dir.join("profiles/designer-3.md")));
    assert_eq!(read("designer-3.md"), "", "a new role is an empty file");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    press(&mut fixture.page, &[KeyCode::Down; 9]);
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(!read("designer.toml").contains("role"), "None, the last, removes the key: {}", read("designer.toml"));
    assert!(fixture.dir.join("profiles/designer-3.md").exists(), "and leaves every file");
}

#[test]
fn r_on_a_kebbie_profile_is_a_menu_that_shows_kebbies_role_and_adds_your_lines_in_your_own_folder() {
    let mut fixture = agents_fixture("settings-role-menu");
    go_to_profile(&mut fixture.page, "task");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("╭ role · task ") && shown.contains("│ task is kebbie's: its role comes with kebbie and an        │") && shown.contains("│ update replaces it, so it is not edited here."), "{shown}");
    assert!(shown.contains("│ ▸ Show kebbie's role ") && shown.contains("│   roles/task.md, read-only") && shown.contains("│   Add your lines ") && shown.contains("│   profiles/task.md, read after kebbie's role by every task"), "{shown}");
    assert!(line_of(&shown, "▸ Show kebbie's role").contains(" s   │") && line_of(&shown, "  Add your lines").contains(" a   │") && shown.contains("│ ↑ ↓ choose · ⏎ run · esc close"), "the menu's keys, as the task menu shows them: {shown}");
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Enter)), Effect::Show(file) if file == fixture.dir.join("kebbie/roles/task.md")), "kebbie's role is shown, never edited");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Char('a'))), Effect::Open(file) if file == fixture.dir.join("profiles/task.md")));
    assert_eq!(file(&fixture, "profiles/task.toml"), "role = \"task.md\"\n", "the role goes in the user's own file for task");
    assert_eq!((file(&fixture, "profiles/task.md"), file(&fixture, "kebbie/roles/task.md")), (String::new(), "kebbie's own role\n".to_string()));
    fixture.page.said = None;
    let (shown, _) = screen_at(&mut fixture.page, 120, 30);
    assert!(line_of(&shown, " task ").ends_with("kebbie's task role + task.md") && shown.contains("         + task.md, yours: read after kebbie's role"), "{shown}");
    assert!(shown.contains("task: claude, opus by Claude's default · role task.md · your settings in profiles/task.toml"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Char('a'))), Effect::Open(file) if file == fixture.dir.join("profiles/task.md")), "a second a opens the same lines");
    std::fs::write(fixture.dir.join("profiles/ops.md"), "Mine.\n").unwrap();
    std::fs::write(fixture.dir.join("profiles/helper.toml"), "role = \"ops.md\"\n").unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "ops");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    assert!(screen_at(&mut fixture.page, 100, 30).0.contains("│   profiles/ops.md, shared with helper, read after kebbie's"), "a file already there says who else reads it: {}", screen_at(&mut fixture.page, 100, 30).0);
    press(&mut fixture.page, &[KeyCode::Char('a')]);
    assert_eq!((file(&fixture, "profiles/ops.toml"), file(&fixture, "profiles/ops.md")), ("role = \"ops.md\"\n".to_string(), "Mine.\n".to_string()), "a file already there is the user's and stays");
    go_to_profile(&mut fixture.page, "reviewer");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("New profile from reviewer"), "a profile made from the reviewer is a review profile: {shown}");
    assert!(shown.contains("│   skills/review/SKILL.md, read-only") && shown.contains("profiles/reviewer.md, read after kebbie's role by every") && shown.contains("│ review  "), "{shown}");
    assert!(matches!(click(&mut fixture.page, &shown, "Add your lines"), Effect::Open(file) if file == fixture.dir.join("profiles/reviewer.md")), "one click runs a choice, as in the board's menus");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(matches!(click(&mut fixture.page, &shown, "skills/review/SKILL.md, read-only"), Effect::Show(file) if file == fixture.dir.join("kebbie/skills/review/SKILL.md")), "its second line too");
}

#[test]
fn a_role_that_cannot_be_read_or_a_profile_that_cannot_be_written_is_said_and_leaves_no_file() {
    let mut fixture = agents_fixture("settings-role-errors");
    let profiles = fixture.dir.join("profiles");
    go_to_profile(&mut fixture.page, "designer");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    let Mode::Choose(chooser) = &fixture.page.mode else { panic!("a pick") };
    let copy = chooser.choices.iter().position(|choice| choice.label == "Copy of designer.md").unwrap();
    std::fs::remove_file(profiles.join("designer.md")).unwrap();
    std::fs::create_dir(profiles.join("designer.md")).unwrap();
    press(&mut fixture.page, &vec![KeyCode::Down; copy]);
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Enter)), Effect::Handled));
    assert!(said(&fixture.page).starts_with("✗ designer.md: ") && !profiles.join("designer-2.md").exists(), "a copy it cannot read is said, never an empty file: {}", said(&fixture.page));
    let locked = fixture.dir.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("task.toml"), "effort = \"high\"\n").unwrap();
    std::os::unix::fs::symlink(locked.join("task.toml"), profiles.join("task.toml")).unwrap();
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o555)).unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "task");
    press(&mut fixture.page, &[KeyCode::Char('r')]);
    let effect = fixture.page.handle(KeyEvent::from(KeyCode::Char('a')));
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert!(matches!(effect, Effect::Handled) && said(&fixture.page).starts_with("✗ ") && !profiles.join("task.md").exists(), "a role whose profile cannot be written goes again: {}", said(&fixture.page));
}

fn wizard(page: &Page) -> &Wizard {
    match &page.mode {
        Mode::Wizard(wizard) => wizard,
        _ => panic!("the wizard is not open"),
    }
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().to_string()).collect();
    names.sort();
    names
}

fn fake_codex(fixture: &Fixture) {
    let bin = fixture.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    crate::executable::write(bin.join("codex"), "#!/bin/sh\necho codex-cli 0.163.0\n").unwrap();
}

#[test]
fn n_walks_seven_steps_to_a_profile_that_loads_with_its_role_and_never_a_copy_of_kebbies_role() {
    let mut fixture = agents_fixture("settings-wizard");
    fake_codex(&fixture);
    let profiles = fixture.dir.join("profiles");
    let before = files_in(&profiles);
    go_to_profile(&mut fixture.page, "designer");
    press(&mut fixture.page, &[KeyCode::Char('n')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("╭ new profile ") && shown.contains("│ step 1 of 7") && shown.contains("│ Its name?") && shown.contains("│ ⏎ next · esc cancel"), "{shown}");
    type_text(&mut fixture.page, "task");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(wizard(&fixture.page).step == 0 && screen_at(&mut fixture.page, 100, 34).0.contains("task is kebbie's own profile"), "a built-in's name is refused");
    press(&mut fixture.page, &[KeyCode::Backspace; 4]);
    type_text(&mut fixture.page, "codex");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(wizard(&fixture.page).step == 0 && screen_at(&mut fixture.page, 100, 34).0.contains("codex is a profile already"), "an existing name is refused");
    type_text(&mut fixture.page, "-Big");
    assert!(screen_at(&mut fixture.page, 100, 34).0.contains("lower-case letters, digits and dashes, starting with a letter"));
    press(&mut fixture.page, &[KeyCode::Backspace; 9]);
    type_text(&mut fixture.page, "designer-codex");
    press(&mut fixture.page, &[KeyCode::Enter]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("│ step 2 of 7") && shown.contains("│ What is it for?") && shown.contains("│ ▌Tasks               does the work in a worktree") && shown.contains("│  Review              reads a task's commits and gives a verdict"), "a profile made from a task's is a task's unless you say: {shown}");
    press(&mut fixture.page, &[KeyCode::Enter]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("│ step 3 of 7") && shown.contains("│ Start from which profile?") && shown.contains("│ ▌designer ") && shown.contains("│  ops ") && !shown.contains("│  reviewer"), "it starts from the selected profile, never the reviewer: {shown}");
    press(&mut fixture.page, &[KeyCode::Enter]);
    press(&mut fixture.page, &[KeyCode::Down]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("│ step 4 of 7") && shown.contains("│ Which agent?") && shown.contains("│ It runs the task; the model comes next."), "{shown}");
    assert!(shown.contains("│  Claude Code         kebbie's own default") && shown.contains("│ ▌Codex               codex-cli 0.163.0 · kebbie's plugin ") && shown.contains("│                      installed "), "Codex's version and kebbie's plugin, as kebbie profiles says them: {shown}");
    assert!(shown.contains("│ ↑ ↓ choose · ⏎ next · ← back · esc cancel"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(screen_at(&mut fixture.page, 100, 34).0.contains("│ ▌Codex's default     gpt-5.1-codex-max, not offered to you ⚠"), "the same answers as m, with no now");
    press(&mut fixture.page, &[KeyCode::Up; 9]);
    press(&mut fixture.page, &[KeyCode::Enter]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("│ step 6 of 7") && shown.contains("│ How much effort?") && shown.contains("│ The levels GPT-6.1-Sol takes, from Codex's list."), "{shown}");
    let high = wizard(&fixture.page).efforts.1.iter().position(|answer| answer.then == Pick::Value(Some("high".to_string()))).unwrap();
    press(&mut fixture.page, &[KeyCode::Up; 9]);
    press(&mut fixture.page, &vec![KeyCode::Down; high]);
    press(&mut fixture.page, &[KeyCode::Enter]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("│ step 7 of 7") && shown.contains("│ Your lines, read after kebbie's task role, which updates with") && shown.contains("│ kebbie."), "{shown}");
    assert!(shown.contains("│ ▌New role            designer-codex.md, empty, yours to write") && shown.contains("│  Copy of designer.md the same lines, changed for this one alone"), "{shown}");
    assert!(shown.contains("│  codex.md            shared with codex, codex-build") && shown.contains("│  designer.md         shared with designer") && shown.contains("│  None                kebbie's role alone"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Enter]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("│ check") && shown.contains("│ Add to ") && shown.contains("│ designer-codex.toml") && shown.contains("│ designer-codex.md  a copy of designer.md"), "{shown}");
    assert!(shown.contains("│ about = \"UI work: mockup first\"\n") || shown.contains("│ about = \"UI work: mockup first\""), "{shown}");
    assert!(shown.contains("│ agent = \"codex\"") && shown.contains("│ model = \"gpt-6.1-sol\"") && shown.contains("│ effort = \"high\"") && shown.contains("│ role = \"designer-codex.md\""), "{shown}");
    assert!(shown.contains("│ y add · g add and open the role · ← back · esc cancel"), "{shown}");
    assert_eq!(files_in(&profiles), before, "nothing is written before y or g");
    let effect = fixture.page.handle(KeyEvent::from(KeyCode::Char('g')));
    assert!(matches!(effect, Effect::Open(file) if file == profiles.join("designer-codex.md")), "g opens the role");
    assert_eq!(file(&fixture, "profiles/designer-codex.toml"), "about = \"UI work: mockup first\"\nagent = \"codex\"\nmodel = \"gpt-6.1-sol\"\neffort = \"high\"\nrole = \"designer-codex.md\"\n");
    assert_eq!(file(&fixture, "profiles/designer-codex.md"), "Measure in the browser.\n", "a copy of the user's role, never kebbie's");
    assert!(files_in(&profiles).iter().all(|name| !file(&fixture, &format!("profiles/{name}")).contains("kebbie's own role")), "kebbie's role text is in no file of the user's");
    let made = profiles::load_in(&profiles, "designer-codex").unwrap();
    assert_eq!((made.agent, made.model.as_deref(), made.effort.as_deref()), (Agent::Codex, Some("gpt-6.1-sol"), Some("high")));
    assert_eq!(fixture.page.profile().map(|profile| profile.name), Some("designer-codex".to_string()), "the new row is selected");
    assert!(said(&fixture.page).starts_with("✓ added profiles/designer-codex.toml, its role designer-codex.md"), "{}", said(&fixture.page));
}

#[test]
fn a_codex_profile_from_one_with_claudes_rules_leaves_them_out_and_esc_anywhere_writes_nothing() {
    let mut fixture = agents_fixture("settings-wizard-codex");
    let profiles = fixture.dir.join("profiles");
    std::fs::write(profiles.join("careful.toml"), "for = \"risky changes\"\npermission_mode = \"plan\"\ndeny = [\"Bash(rm:*)\"]\n").unwrap();
    fixture.page.load();
    let before = files_in(&profiles);
    go_to_profile(&mut fixture.page, "careful");
    press(&mut fixture.page, &[KeyCode::Char('n')]);
    type_text(&mut fixture.page, "careful-codex");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter, KeyCode::Down, KeyCode::Enter, KeyCode::Enter, KeyCode::Enter]);
    let roles = wizard(&fixture.page).roles();
    press(&mut fixture.page, &vec![KeyCode::Down; roles.len() - 1]);
    press(&mut fixture.page, &[KeyCode::Enter]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("│ permission_mode = \"plan\"") && !shown.contains("│ deny =") && shown.contains("│ Left out: deny, which only Claude Code reads."), "{shown}");
    assert!(shown.contains("│ y add · g add and open the file"), "with no role, g opens the profile's file: {shown}");
    press(&mut fixture.page, &[KeyCode::Left]);
    assert_eq!(wizard(&fixture.page).step, 6, "← goes back a step");
    press(&mut fixture.page, &[KeyCode::Esc]);
    assert!(fixture.page.is_normal());
    assert_eq!(files_in(&profiles), before, "esc writes nothing");
    go_to_profile(&mut fixture.page, "careful");
    press(&mut fixture.page, &[KeyCode::Char('n')]);
    type_text(&mut fixture.page, "careful-codex");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    click(&mut fixture.page, &shown, " Codex   ");
    assert_eq!((wizard(&fixture.page).step, wizard(&fixture.page).agent()), (3, Agent::Codex), "a click selects an answer");
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    click(&mut fixture.page, &shown, "▌Codex");
    assert_eq!(wizard(&fixture.page).step, 4, "and a second goes on");
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    click(&mut fixture.page, &shown, "Settings");
    assert_eq!(wizard(&fixture.page).step, 4, "a click beside the steps leaves them open");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Enter]);
    press(&mut fixture.page, &vec![KeyCode::Down; roles.len() - 1]);
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Char('y')]);
    assert_eq!(file(&fixture, "profiles/careful-codex.toml"), "for = \"risky changes\"\nagent = \"codex\"\npermission_mode = \"plan\"\n");
    assert!(profiles::load_in(&profiles, "careful-codex").is_ok());
    go_to_profile(&mut fixture.page, "task");
    press(&mut fixture.page, &[KeyCode::Char('r'), KeyCode::Char('n')]);
    assert_eq!(wizard(&fixture.page).from().map(|profile| profile.name.as_str()), Some("task"), "n in task's role menu starts from task");
    type_text(&mut fixture.page, "mine");
    press(&mut fixture.page, &[KeyCode::Enter; 7]);
    press(&mut fixture.page, &[KeyCode::Char('y')]);
    assert_eq!((file(&fixture, "profiles/mine.md"), file(&fixture, "profiles/mine.toml").contains("role = \"mine.md\"")), (String::new(), true), "a new role starts empty: kebbie's lines come from kebbie, read first");
}

#[test]
fn the_wizard_follows_the_start_it_was_given_and_a_click_while_typing_a_model_takes_that_answer() {
    let mut fixture = agents_fixture("settings-wizard-follow");
    go_to_profile(&mut fixture.page, "designer");
    press(&mut fixture.page, &[KeyCode::Char('n')]);
    type_text(&mut fixture.page, "follow");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Enter]);
    let at = wizard(&fixture.page).starts.iter().position(|profile| profile.name == "codex-build").unwrap();
    let from = wizard(&fixture.page).start;
    press(&mut fixture.page, &vec![if at > from { KeyCode::Down } else { KeyCode::Up }; at.abs_diff(from)]);
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(wizard(&fixture.page).agent(), Agent::Codex, "the agent step opens on the chosen start's agent");
    press(&mut fixture.page, &[KeyCode::Left]);
    let designer = wizard(&fixture.page).starts.iter().position(|profile| profile.name == "designer").unwrap();
    press(&mut fixture.page, &vec![KeyCode::Down; designer.saturating_sub(at)]);
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(wizard(&fixture.page).agent(), Agent::Claude, "and follows a start chosen again");
    press(&mut fixture.page, &[KeyCode::Enter]);
    let typed = wizard(&fixture.page).models.1.iter().position(|answer| answer.then == Pick::Type).unwrap();
    press(&mut fixture.page, &[KeyCode::Up; 9]);
    press(&mut fixture.page, &vec![KeyCode::Down; typed]);
    press(&mut fixture.page, &[KeyCode::Enter]);
    type_text(&mut fixture.page, "claude-half");
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    click(&mut fixture.page, &shown, "the newest Sonnet");
    let shown_after = screen_at(&mut fixture.page, 100, 34).0;
    assert!(wizard(&fixture.page).typing.is_none() && wizard(&fixture.page).step == 4 && wizard(&fixture.page).model() == Some("sonnet".to_string()), "the click leaves the typing and selects that answer, nothing half-typed taken: {shown_after}");
    assert!(!shown_after.contains("claude-half"), "{shown_after}");
}

#[test]
fn y_on_the_wizard_keeps_a_role_file_that_appeared_meanwhile_and_says_a_copy_it_cannot_read() {
    let mut fixture = agents_fixture("settings-wizard-guard");
    let profiles = fixture.dir.join("profiles");
    go_to_profile(&mut fixture.page, "designer");
    press(&mut fixture.page, &[KeyCode::Char('n')]);
    type_text(&mut fixture.page, "guard");
    for _ in 0..7 {
        press(&mut fixture.page, &[KeyCode::Enter]);
        assert!(matches!(fixture.page.mode, Mode::Wizard(_)), "{}", screen_at(&mut fixture.page, 100, 34).0);
    }
    assert_eq!(wizard(&fixture.page).chosen_role(), Role::New("guard.md".to_string()));
    std::fs::write(profiles.join("guard.md"), "Someone's lines.\n").unwrap();
    press(&mut fixture.page, &[KeyCode::Char('y')]);
    assert_eq!(wizard(&fixture.page).error.as_deref(), Some("guard.md is in the folder already"));
    assert_eq!((file(&fixture, "profiles/guard.md"), profiles.join("guard.toml").exists()), ("Someone's lines.\n".to_string(), false), "a file this call did not write stays");
    std::fs::remove_file(profiles.join("guard.md")).unwrap();
    press(&mut fixture.page, &[KeyCode::Left, KeyCode::Down, KeyCode::Enter]);
    assert!(matches!(wizard(&fixture.page).chosen_role(), Role::Copy { .. }));
    std::fs::remove_file(profiles.join("designer.md")).unwrap();
    std::fs::create_dir(profiles.join("designer.md")).unwrap();
    press(&mut fixture.page, &[KeyCode::Char('y')]);
    assert!(wizard(&fixture.page).error.as_deref().is_some_and(|error| error.starts_with("designer.md: ")), "{:?}", wizard(&fixture.page).error);
    assert!(!profiles.join("guard.md").exists() && !profiles.join("guard.toml").exists(), "a copy it cannot read writes nothing");
}

#[test]
fn x_deletes_your_profile_and_its_role_only_when_no_other_reads_it_and_never_kebbies_or_an_open_tasks() {
    let mut fixture = agents_fixture("settings-delete");
    let exists = |fixture: &Fixture, name: &str| fixture.dir.join("profiles").join(name).exists();
    go_to_profile(&mut fixture.page, "task");
    press(&mut fixture.page, &[KeyCode::Char('x')]);
    assert!(fixture.page.is_normal() && said(&fixture.page).starts_with("✗ task is kebbie's and stays"), "{}", said(&fixture.page));
    go_to_profile(&mut fixture.page, "codex");
    press(&mut fixture.page, &[KeyCode::Char('x')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("╭ delete ") && shown.contains("│ codex ") && shown.contains("│ profiles/codex.toml goes; its role codex.md stays, as codex-build") && shown.contains("│ Delete this profile?  y / n"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Char('z')]);
    assert!(screen_at(&mut fixture.page, 100, 30).0.contains("press y or n") && exists(&fixture, "codex.toml"), "another key asks again");
    press(&mut fixture.page, &[KeyCode::Char('y')]);
    assert!(!exists(&fixture, "codex.toml") && exists(&fixture, "codex.md"), "a role another profile reads stays");
    assert_eq!(said(&fixture.page), "✓ removed profiles/codex.toml; its role codex.md stays for codex-build");
    std::fs::create_dir_all(fixture.dir.join("state/sessions")).unwrap();
    std::fs::write(fixture.dir.join("state/sessions/billing@app"), "{\"session\":\"s\",\"mode\":null,\"profile\":\"designer\",\"agent\":\"claude\"}").unwrap();
    std::fs::write(fixture.dir.join("state/sessions/billing@app.prompt"), "designer").unwrap();
    go_to_profile(&mut fixture.page, "designer");
    press(&mut fixture.page, &[KeyCode::Char('x')]);
    assert!(fixture.page.is_normal() && said(&fixture.page).starts_with("✗ designer is the profile of billing@app, still open"), "{}", said(&fixture.page));
    std::fs::remove_file(fixture.dir.join("state/sessions/billing@app")).unwrap();
    press(&mut fixture.page, &[KeyCode::Char('x')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("│ profiles/designer.toml goes, and its role designer.md, which no"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Char('n')]);
    assert!(fixture.page.is_normal() && exists(&fixture, "designer.toml"), "n keeps it");
    std::fs::write(fixture.dir.join("profiles/broken.toml"), "agent = \"gemini\"\nrole = \"designer.md\"\n").unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "designer");
    press(&mut fixture.page, &[KeyCode::Char('x')]);
    assert!(screen_at(&mut fixture.page, 100, 30).0.contains("stays, as broken"), "a file that does not load still reads its role: {}", screen_at(&mut fixture.page, 100, 30).0);
    press(&mut fixture.page, &[KeyCode::Char('y')]);
    assert!(!exists(&fixture, "designer.toml") && exists(&fixture, "designer.md") && said(&fixture.page) == "✓ removed profiles/designer.toml; its role designer.md stays for broken", "{}", said(&fixture.page));
    std::fs::remove_file(fixture.dir.join("profiles/broken.toml")).unwrap();
    std::fs::write(fixture.dir.join("profiles/designer.toml"), "role = \"designer.md\"\n").unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "designer");
    press(&mut fixture.page, &[KeyCode::Char('x'), KeyCode::Char('y')]);
    assert!(!exists(&fixture, "designer.toml") && !exists(&fixture, "designer.md"), "a role no other profile reads goes with it");
    assert_eq!(said(&fixture.page), "✓ removed profiles/designer.toml and its role designer.md");
    std::fs::write(fixture.dir.join("config.toml"), "[task]\nprofile = \"codex-build\"\n").unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "codex-build");
    press(&mut fixture.page, &[KeyCode::Char('x')]);
    assert!(said(&fixture.page).starts_with("✗ task.profile names codex-build in ") && exists(&fixture, "codex-build.toml"), "{}", said(&fixture.page));
    std::fs::write(fixture.dir.join("config.toml"), "").unwrap();
    std::fs::write(fixture.dir.join("repos/app.toml"), "[task]\nprofile = \"codex-build\"\n").unwrap();
    fixture.page.load();
    press(&mut fixture.page, &[KeyCode::Char('x')]);
    assert!(said(&fixture.page).starts_with(&format!("✗ task.profile names codex-build in {}", fixture.dir.join("repos/app.toml").display())) && exists(&fixture, "codex-build.toml"), "a repo config's task.profile keeps it too: {}", said(&fixture.page));
    std::fs::write(fixture.dir.join("repos/app.toml"), "").unwrap();
    std::fs::write(fixture.dir.join("config.toml"), "[[routines]]\nid = \"nightly\"\ntitle = \"Nightly check\"\nrepo = \"app\"\nat = \"07:00\"\nprompt = \"Check it\"\nprofile = \"codex-build\"\n").unwrap();
    fixture.page.load();
    press(&mut fixture.page, &[KeyCode::Char('x')]);
    assert!(said(&fixture.page).starts_with("✗ codex-build is the profile the routine \"Nightly check\" starts its tasks with") && exists(&fixture, "codex-build.toml"), "{}", said(&fixture.page));
}

#[test]
fn m_on_a_codex_profile_picks_from_codexs_own_list_and_writes_model_alone() {
    let mut fixture = agents_fixture("settings-codex-model");
    go_to_profile(&mut fixture.page, "codex-build");
    press(&mut fixture.page, &[KeyCode::Char('m')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("╭ model · codex-build ") && shown.contains("│ Which model?") && shown.contains("│ Codex's own list for your account."), "{shown}");
    assert!(shown.contains("│ ▌GPT-6.1-Sol         gpt-6.1-sol · now") && shown.contains("│  GPT-6-Astra         gpt-6-astra") && shown.contains("│  GPT-6.1 Terra       gpt-6.1-terra"), "{shown}");
    assert!(shown.contains("│  Codex's default     gpt-5.1-codex-max, not offered to you ⚠") && shown.contains("│  Type a model…       any id Codex accepts"), "{shown}");
    assert!(shown.contains("│ A task already running keeps its model; the next start takes it.") && shown.contains("│ ↑ ↓ choose · ⏎ set · esc close"), "{shown}");
    assert!(matches!(&fixture.page.mode, Mode::Choose(chooser) if !chooser.choices.iter().any(|choice| choice.about == "gpt-5.1-codex-max")), "a model Codex hides is not offered, only named");
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Enter]);
    assert_eq!(file(&fixture, "profiles/codex-build.toml"), CODEX_BUILD.replace("gpt-6.1-sol", "gpt-6-astra"), "the one line changes, comments and all else stay");
    assert!(said(&fixture.page).starts_with("✓ wrote model = \"gpt-6-astra\" to profiles/codex-build.toml · codex-build runs codex on gpt-6-astra, effort medium"), "{}", said(&fixture.page));
    press(&mut fixture.page, &[KeyCode::Char('m'), KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    assert!(matches!(fixture.page.mode, Mode::Edit { typed: Typed::Model, .. }));
    fixture.page.handle(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_text(&mut fixture.page, "gpt-7-preview");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(file(&fixture, "profiles/codex-build.toml").contains("\nmodel = \"gpt-7-preview\"\n"), "any id is taken");
    assert!(screen_at(&mut fixture.page, 100, 30).0.contains("gpt-7-preview ⚠"), "an id Codex does not list is warned about");
    press(&mut fixture.page, &[KeyCode::Char('m')]);
    assert!(screen_at(&mut fixture.page, 100, 30).0.contains("│ ▌gpt-7-preview       not offered to you ⚠ · now"), "the model in effect is an answer of its own: {}", screen_at(&mut fixture.page, 100, 30).0);
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(file(&fixture, "profiles/codex-build.toml").contains("\nmodel = \"gpt-7-preview\"\n") && said(&fixture.page).starts_with("codex-build has that model already"), "⏎ keeps it: {}", said(&fixture.page));
    press(&mut fixture.page, &[KeyCode::Char('m'), KeyCode::Down, KeyCode::Enter]);
    assert!(!file(&fixture, "profiles/codex-build.toml").contains("model ="), "Codex's default removes the key");
    assert!(said(&fixture.page).starts_with("✓ removed model from profiles/codex-build.toml · codex-build runs codex on gpt-5.1-codex-max by Codex's default"), "{}", said(&fixture.page));
    std::fs::remove_file(fixture.dir.join("codex/models_cache.json")).unwrap();
    fixture.page.load();
    press(&mut fixture.page, &[KeyCode::Char('m')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("No list from Codex at ") && shown.contains("Codex's default     gpt-5.1-codex-max") && shown.contains("Type a model…") && !shown.contains("GPT-6"), "{shown}");
    assert!(!shown.contains("not offered"), "without Codex's list nothing is called not offered: {shown}");
}

#[test]
fn a_pick_in_a_narrow_window_wraps_each_about_under_its_own_column_and_a_click_takes_the_row_it_hits() {
    let mut fixture = agents_fixture("settings-narrow-pick");
    go_to_profile(&mut fixture.page, "codex-build");
    press(&mut fixture.page, &[KeyCode::Char('m')]);
    let (shown, _) = screen_at(&mut fixture.page, 60, 40);
    let rows: Vec<&str> = shown.lines().collect();
    let at = rows.iter().position(|line| line.contains("│  Codex's default ")).unwrap_or_else(|| panic!("{shown}"));
    assert!(rows[at + 1].starts_with(&format!("{}│ {}", " ".repeat(rows[at].find('│').unwrap()), " ".repeat(21))) && rows[at + 1].contains("⚠"), "the about goes on under its column, not from the edge: {shown}");
    assert!(rows.iter().all(|line| line.chars().count() <= 60), "{shown}");
    click(&mut fixture.page, &shown, "GPT-6.1 Terra");
    assert!(matches!(&fixture.page.mode, Mode::Choose(chooser) if chooser.choices[chooser.index].label == "GPT-6.1 Terra"), "a click takes the row drawn there");
    let (wide, _) = screen_at(&mut fixture.page, 100, 40);
    assert!(wide.contains("│  Codex's default     gpt-5.1-codex-max, not offered to you ⚠"), "{wide}");
}

#[test]
fn e_offers_the_levels_codex_lists_for_the_profiles_model_ultra_among_them() {
    let mut fixture = agents_fixture("settings-codex-effort");
    go_to_profile(&mut fixture.page, "codex-build");
    press(&mut fixture.page, &[KeyCode::Char('e')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("╭ effort · codex-build ") && shown.contains("│ How much effort?") && shown.contains("│ The levels GPT-6.1-Sol takes, from Codex's list."), "{shown}");
    assert!(shown.contains("│  low                 Codex's default for this model") && shown.contains("│ ▌medium              now") && shown.contains("│  ultra               only some Codex models"), "{shown}");
    assert!(shown.contains("│  Default             leave it to Codex: low"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    assert_eq!(file(&fixture, "profiles/codex-build.toml"), CODEX_BUILD.replace("effort = \"medium\"", "effort = \"ultra\""));
    assert_eq!(fixture.page.profile().unwrap().effort.as_deref(), Some("ultra"));
    go_to_profile(&mut fixture.page, "codex");
    press(&mut fixture.page, &[KeyCode::Char('e')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("╭ effort · codex ") && shown.contains("The levels gpt-5.1-codex-max takes"), "a hidden model's own levels: {shown}");
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Enter]);
    assert!(!file(&fixture, "profiles/codex.toml").contains("effort"), "Default leaves it to Codex");
    std::fs::write(fixture.dir.join("codex/config.toml"), "model = \"gpt-5.1-codex-max\"\nmodel_reasoning_effort = \"high\"\n").unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "codex-build");
    press(&mut fixture.page, &[KeyCode::Char('e')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("│  high                Codex's default, from its config.toml") && shown.contains("│  Default             leave it to Codex: high") && !shown.contains("Codex's default for this model"), "Codex's own config.toml wins over the model's default: {shown}");
}

#[test]
fn m_on_a_built_in_claude_profile_writes_its_own_file_and_d_takes_it_away_again() {
    let mut fixture = agents_fixture("settings-claude-model");
    go_to_profile(&mut fixture.page, "task");
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Enter)), Effect::Handled));
    assert!(said(&fixture.page).contains("task is kebbie's and has no file of yours until you set something: m, e or r writes profiles/task.toml"), "{}", said(&fixture.page));
    press(&mut fixture.page, &[KeyCode::Char('m')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("╭ model · task ") && shown.contains("│ Claude Code's model names."), "{shown}");
    assert!(matches!(&fixture.page.mode, Mode::Choose(chooser) if chooser.choices[0].about == format!("opus, from {}", fixture.dir.join("claude/settings.json").display()) && chooser.choices[0].now));
    assert!(shown.contains("│ ▌Claude's default    opus, from") && shown.contains("│  opus                the newest Opus") && shown.contains("│  fable               the newest Fable") && shown.contains("│  Type a model…       a full id, as claude-opus-5-5"), "{shown}");
    assert!(shown.contains("│ task is kebbie's: a model you set goes in profiles/task.toml,") && shown.contains("│ which an update never writes, and kebbie's role and rules stay") && shown.contains("│ as they ship. d on the row removes that file again.") && shown.contains("│ A task already running keeps its model; the next start takes it."), "{shown}");
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    assert_eq!(file(&fixture, "profiles/task.toml"), "model = \"sonnet\"\n");
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(line_of(&shown, " task ").contains("claude · sonnet"), "{shown}");
    fixture.page.said = None;
    assert!(screen_at(&mut fixture.page, 100, 30).0.contains("  task: claude, sonnet · your settings in profiles/task.toml"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Char('e')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("╭ effort · task ") && shown.contains("Claude Code's effort levels, for sonnet.") && !shown.contains("ultra") && shown.contains("leave it to Claude"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Up, KeyCode::Enter]);
    assert_eq!(file(&fixture, "profiles/task.toml"), "model = \"sonnet\"\neffort = \"max\"\n");
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Enter)), Effect::Open(file) if file == fixture.dir.join("profiles/task.toml")), "⏎ opens the file once there is one");
    press(&mut fixture.page, &[KeyCode::Char('m'), KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    fixture.page.handle(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(said(&fixture.page).contains("✗ profiles/task.toml: model: must not be empty"), "{}", said(&fixture.page));
    assert_eq!(file(&fixture, "profiles/task.toml"), "model = \"sonnet\"\neffort = \"max\"\n", "a value the parser refuses is never written");
    press(&mut fixture.page, &[KeyCode::Char('d')]);
    assert!(!fixture.dir.join("profiles/task.toml").exists());
    assert!(said(&fixture.page).starts_with("✓ removed profiles/task.toml, which held nothing else · task runs claude on opus by Claude's default"), "{}", said(&fixture.page));
    press(&mut fixture.page, &[KeyCode::Char('d')]);
    assert!(said(&fixture.page).starts_with("task has no model or effort of its own"), "{}", said(&fixture.page));
    go_to_profile(&mut fixture.page, "codex-build");
    press(&mut fixture.page, &[KeyCode::Char('d')]);
    assert_eq!(file(&fixture, "profiles/codex-build.toml"), "# for small builds\nabout = \"Codex, building: a small, well-defined task done by another model family\"\nagent = \"codex\" # not Claude\nrole = \"codex.md\"\n", "a file profile keeps everything but the two keys");
}

#[test]
fn a_profile_file_that_does_not_load_is_a_red_row_whose_enter_opens_it_kebbies_own_among_them() {
    let mut fixture = agents_fixture("settings-profile-broken");
    std::fs::write(fixture.dir.join("profiles/broken.toml"), "agent = \"gemini\"\n").unwrap();
    std::fs::write(fixture.dir.join("profiles/task.toml"), "effort = \"huge\"\n").unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "ops");
    let (shown, buffer) = screen_at(&mut fixture.page, 140, 44);
    assert!(shown.contains("▸ Profiles 7") && line_of(&shown, "FROM KEBBIE").ends_with("FROM KEBBIE  2") && line_of(&shown, "DOES NOT LOAD").ends_with(" DOES NOT LOAD  2"), "{shown}");
    assert!(line_of(&shown, " task         profiles").ends_with(" task         profiles/task.toml") && line_of(&shown, " broken  ").ends_with(" broken       profiles/broken.toml"), "kebbie's own task, hidden by its file, has a row too: {shown}");
    assert_eq!(color_of(&buffer, " broken  ", "broken"), Some(RED));
    fixture.page.row = fixture.page.agents.profiles.len() + fixture.page.agents.broken.iter().position(|(name, _, _)| name == "broken").unwrap();
    let (shown, _) = screen_at(&mut fixture.page, 140, 44);
    assert!(shown.contains("    profiles/broken.toml: agent: \"gemini\" is not an agent kebbie runs") && shown.contains("    ⏎ open file") && shown.contains("broken does not load, so no task can start with it; ⏎ opens its file"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Char('m'), KeyCode::Char('x')]);
    assert!(fixture.page.is_normal(), "a file that does not load has no picks");
    assert!(matches!(fixture.page.handle(KeyEvent::from(KeyCode::Enter)), Effect::Open(file) if file == fixture.dir.join("profiles/broken.toml")));
    fixture.page.opened(&fixture.dir.join("profiles/broken.toml"), Ok(true));
    assert!(said(&fixture.page).starts_with("✗ profiles/broken.toml: agent:"), "{}", said(&fixture.page));
    fixture.page.opened(&fixture.dir.join("profiles/codex.toml"), Ok(true));
    assert_eq!(said(&fixture.page), "profiles/codex.toml loads");
}

#[test]
fn the_reviewer_row_shows_review_model_and_m_changes_review_model_never_reviewer_toml() {
    let mut fixture = agents_fixture("settings-review-model");
    std::fs::write(fixture.dir.join("config.toml"), "# mine\n[review]\nmodel = \"sonnet\"\n").unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "reviewer");
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(line_of(&shown, " reviewer ").contains("claude · sonnet · max"), "the model kebbie review runs: {shown}");
    assert!(shown.contains("  reviewer: claude, sonnet from review.model (machine), max · your settings in\n  profiles/reviewer.toml"), "and where it comes from: {shown}");
    press(&mut fixture.page, &[KeyCode::Char('m')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("Claude Code's model names; this sets review.model") && shown.contains("│  The profile's       opus, from reviewer.toml") && shown.contains("│ ▌sonnet              the newest Sonnet · now"), "{shown}");
    assert!(shown.contains("This writes review.model in "), "{shown}");
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "# mine\n[review]\nmodel = \"haiku\"\n");
    assert_eq!(file(&fixture, "profiles/reviewer.toml"), "model = \"opus\"\neffort = \"max\"\n", "the reviewer's model has one place");
    assert!(said(&fixture.page).starts_with("✓ wrote review.model = \"haiku\" to config.toml"), "{}", said(&fixture.page));
    press(&mut fixture.page, &[KeyCode::Char('m'), KeyCode::Up, KeyCode::Up, KeyCode::Up, KeyCode::Enter]);
    assert!(!file(&fixture, "config.toml").contains("model ="), "the default removes review.model: {}", file(&fixture, "config.toml"));
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(line_of(&shown, " reviewer ").contains("claude · opus · max") && said(&fixture.page).starts_with("✓ removed review.model"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Char('e'), KeyCode::Up, KeyCode::Enter]);
    assert_eq!(file(&fixture, "profiles/reviewer.toml"), "model = \"opus\"\neffort = \"xhigh\"\n", "e keeps editing the profile's effort");
    assert!(!file(&fixture, "config.toml").contains("effort"));
    std::fs::write(fixture.dir.join("config.toml"), "[review]\nmodel = \"sonnet\"\n").unwrap();
    fixture.page.load();
    press(&mut fixture.page, &[KeyCode::Char('d')]);
    assert_eq!((file(&fixture, "config.toml"), fixture.dir.join("profiles/reviewer.toml").exists()), ("[review]\n".to_string(), false), "d takes the reviewer back to its defaults, review.model with them");
    assert!(said(&fixture.page).starts_with("✓ removed review.model from config.toml") && said(&fixture.page).contains("✓ removed profiles/reviewer.toml"), "{}", said(&fixture.page));
}

fn with_review_profiles(fixture: &mut Fixture) {
    std::fs::write(fixture.dir.join("profiles/codex-review.toml"), "kind = \"review\"\nagent = \"codex\"\nmodel = \"gpt-6.1-sol\"\neffort = \"high\"\n").unwrap();
    std::fs::write(fixture.dir.join("profiles/careful-review.toml"), "kind = \"review\"\nmodel = \"sonnet\"\n").unwrap();
    fixture.page.load();
}

fn answers(page: &Page) -> Vec<String> {
    match &page.mode {
        Mode::Choose(chooser) => chooser.choices.iter().map(|choice| choice.label.clone()).collect(),
        _ => panic!("no pick open"),
    }
}

#[test]
fn the_reviewer_picks_list_review_profiles_alone_and_write_review_profile_and_review_second() {
    let mut fixture = agents_fixture("settings-review-picks");
    with_review_profiles(&mut fixture);
    go_to(&mut fixture.page, "review.profile");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(answers(&fixture.page), ["reviewer", "careful-review", "codex-review"], "review profiles only, no task's");
    let shown = screen(&mut fixture.page);
    for line in ["│ Who reviews by default?", "│ Its verdict holds the push. Review profiles only.", "│ ▌reviewer            claude · opus · max · now", "│  codex-review        codex · gpt-6.1-sol · high"] {
        assert!(shown.contains(line), "{line}: {shown}");
    }
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "[review]\nprofile = \"careful-review\"\n");
    go_to(&mut fixture.page, "review.second");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(answers(&fixture.page), ["None", "reviewer", "codex-review", "Always: reviewer", "Always: codex-review"], "every review profile but the default reviewer, offered or always");
    assert!(screen(&mut fixture.page).contains("│ Second opinion?") && screen(&mut fixture.page).contains("s is not offered · now"), "{}", screen(&mut fixture.page));
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "[review]\nprofile = \"careful-review\"\nsecond = \"codex-review\"\n");
    press(&mut fixture.page, &[KeyCode::Enter]);
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "[review]\nprofile = \"careful-review\"\nsecond = \"always codex-review\"\n");
    go_to(&mut fixture.page, "task.profile");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(!answers(&fixture.page).iter().any(|label| label.contains("review")), "a review profile never starts a task: {:?}", answers(&fixture.page));
}

#[test]
fn review_model_follows_the_default_reviewer_wherever_review_profile_points() {
    let mut fixture = agents_fixture("settings-review-model");
    with_review_profiles(&mut fixture);
    std::fs::write(fixture.dir.join("config.toml"), "[review]\nprofile = \"careful-review\"\nmodel = \"opus\"\n").unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "careful-review");
    assert!(screen_at(&mut fixture.page, 120, 34).0.contains("careful-review: claude, opus from review.model (machine)"), "{}", screen_at(&mut fixture.page, 120, 34).0);
    go_to_profile(&mut fixture.page, "reviewer");
    assert!(!screen_at(&mut fixture.page, 120, 34).0.contains("reviewer: claude, opus from review.model"), "the built-in reviewer is no longer the default one");
}

#[test]
fn a_reviewer_on_an_agent_setup_did_not_choose_is_not_offered() {
    let mut fixture = agents_fixture("settings-review-unchosen");
    with_review_profiles(&mut fixture);
    crate::agent::codex::uninstall(&fixture.dir.join("codex")).unwrap();
    fixture.page.load();
    go_to(&mut fixture.page, "review.profile");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(answers(&fixture.page), ["reviewer", "careful-review"]);
    press(&mut fixture.page, &[KeyCode::Esc]);
    go_to(&mut fixture.page, "review.second");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(answers(&fixture.page), ["None", "careful-review", "Always"], "one candidate: its Always needs no name");
}

#[test]
fn the_wizard_asks_what_a_profile_is_for_and_a_review_one_takes_only_a_reviewers_keys() {
    let mut fixture = agents_fixture("settings-wizard-review");
    go_to_profile(&mut fixture.page, "designer");
    press(&mut fixture.page, &[KeyCode::Char('n')]);
    type_text(&mut fixture.page, "codex-review");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("│ step 2 of 7") && shown.contains("│ ▌Review              reads a task's commits and gives a verdict"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(wizard(&fixture.page).starts.iter().map(|profile| profile.name.as_str()).collect::<Vec<_>>(), ["reviewer"], "a review profile starts from a review profile");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Down]);
    assert!(screen_at(&mut fixture.page, 100, 34).0.contains("│ It reads the commits; the model comes next."));
    assert_eq!(wizard(&fixture.page).agent(), Agent::Codex);
    press(&mut fixture.page, &[KeyCode::Enter]);
    press(&mut fixture.page, &[KeyCode::Up; 9]);
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Enter]);
    let roles = wizard(&fixture.page).roles();
    press(&mut fixture.page, &vec![KeyCode::Down; roles.len() - 1]);
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Char('y')]);
    let written = file(&fixture, "profiles/codex-review.toml");
    assert!(written.starts_with("kind = \"review\"\n") && written.contains("agent = \"codex\"\nmodel = \"gpt-6.1-sol\"\n"), "{written}");
    for key in ["skills", "permission_mode", "deny", "allow", "sandbox"] {
        assert!(!written.contains(key), "{key}: {written}");
    }
    let made = profiles::load_in(&fixture.dir.join("profiles"), "codex-review").unwrap();
    assert!(made.review && profiles::for_task(&fixture.dir.join("profiles"), "codex-review").is_err());
    assert!(said(&fixture.page).contains("review.profile or review.second on this page"), "{}", said(&fixture.page));
}

#[test]
fn a_value_in_effect_that_no_answer_offers_is_one_of_its_own_and_enter_keeps_it() {
    let mut fixture = agents_fixture("settings-keep-current");
    let profiles = fixture.dir.join("profiles");
    std::fs::write(profiles.join("codex-build.toml"), CODEX_BUILD.replace("effort = \"medium\"", "effort = \"persistent\"")).unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "codex-build");
    press(&mut fixture.page, &[KeyCode::Char('e')]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 30);
    assert!(shown.contains("│ ▌persistent          not a level this model lists · now"), "{shown}");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(file(&fixture, "profiles/codex-build.toml").contains("effort = \"persistent\"") && said(&fixture.page).starts_with("codex-build has that effort already"), "⏎ never rewrites it to the first level: {}", said(&fixture.page));
    std::fs::write(profiles.join("broken.toml"), "agent = \"gemini\"\n").unwrap();
    std::fs::write(fixture.dir.join("config.toml"), "[task]\nprofile = \"broken\"\n").unwrap();
    fixture.page.load();
    go_to(&mut fixture.page, "task.profile");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(screen(&mut fixture.page).contains("│ ▌\"broken\"            no profile loads by this name · now"), "{}", screen(&mut fixture.page));
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "[task]\nprofile = \"broken\"\n", "⏎ keeps the name, never none");
    std::fs::write(profiles.join("odd.toml"), "agent = \"codex\"\nmodel = \"gpt-7-preview\"\neffort = \"persistent\"\n").unwrap();
    fixture.page.load();
    go_to_profile(&mut fixture.page, "odd");
    press(&mut fixture.page, &[KeyCode::Char('n')]);
    type_text(&mut fixture.page, "odd-copy");
    press(&mut fixture.page, &[KeyCode::Enter; 4]);
    assert!(screen_at(&mut fixture.page, 100, 34).0.contains("│ ▌gpt-7-preview       not offered to you ⚠"), "the start's model is offered as it is: {}", screen_at(&mut fixture.page, 100, 34).0);
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Enter]);
    assert_eq!((wizard(&fixture.page).model().as_deref(), wizard(&fixture.page).effort().as_deref()), (Some("gpt-7-preview"), Some("persistent")), "the wizard keeps the start's model and effort on ⏎");
}

#[test]
fn without_codex_chosen_in_setup_no_codex_is_offered_and_a_codex_profile_says_how_to_set_it_up() {
    let mut fixture = agents_fixture("settings-no-codex");
    fake_codex(&fixture);
    crate::agent::codex::uninstall(&fixture.dir.join("codex")).unwrap();
    fixture.page.load();
    assert_eq!(fixture.page.agents.chosen, [Agent::Claude]);
    go_to_profile(&mut fixture.page, "codex-build");
    let (shown, _) = screen_at(&mut fixture.page, 120, 34);
    assert!(line_of(&shown, " codex-build ").contains("codex · gpt-6.1-sol ⚠ · medium"), "{shown}");
    assert!(shown.contains("⚠ Codex is not set up: kebbie setup codex"), "the selected row says what to run: {shown}");
    press(&mut fixture.page, &[KeyCode::Char('m')]);
    assert!(matches!(&fixture.page.mode, Mode::Choose(_)), "m still edits its file");
    press(&mut fixture.page, &[KeyCode::Esc]);
    go_to_profile(&mut fixture.page, "designer");
    press(&mut fixture.page, &[KeyCode::Char('n')]);
    type_text(&mut fixture.page, "writer");
    press(&mut fixture.page, &[KeyCode::Enter, KeyCode::Enter]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("│ step 3 of 7") && shown.contains("│ ▌designer ") && !shown.contains("│  codex"), "a Codex profile is no start: {shown}");
    assert!(wizard(&fixture.page).starts.iter().all(|profile| profile.agent == Agent::Claude));
    press(&mut fixture.page, &[KeyCode::Enter]);
    let (shown, _) = screen_at(&mut fixture.page, 100, 34);
    assert!(shown.contains("│ step 4 of 7") && shown.contains("│ ▌Claude Code         kebbie's own default") && !shown.contains("│  Codex "), "the one agent setup chose, selected: {shown}");
    assert_eq!(wizard(&fixture.page).agents, [(Agent::Claude, "kebbie's own default".to_string())]);
    press(&mut fixture.page, &[KeyCode::Down, KeyCode::Enter]);
    assert_eq!((wizard(&fixture.page).step, wizard(&fixture.page).agent()), (4, Agent::Claude));
    press(&mut fixture.page, &[KeyCode::Esc]);
    go_to(&mut fixture.page, "task.profile");
    press(&mut fixture.page, &[KeyCode::Enter]);
    let shown = screen(&mut fixture.page);
    assert!(shown.contains("\"designer\"") && !shown.contains("\"codex"), "a task.profile answer only on an agent setup chose: {shown}");
    press(&mut fixture.page, &[KeyCode::Esc]);
    std::fs::write(fixture.dir.join("config.toml"), "[task]\nprofile = \"codex-build\"\n").unwrap();
    fixture.page.load();
    go_to(&mut fixture.page, "task.profile");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(screen(&mut fixture.page).contains("│ ▌\"codex-build\"       Codex is not set up: kebbie setup codex ·"), "{}", screen(&mut fixture.page));
}

#[test]
fn task_profile_is_picked_from_the_profiles_there_are_and_none_removes_it() {
    let mut fixture = agents_fixture("settings-task-profile");
    go_to(&mut fixture.page, "task.profile");
    assert!(fixture.page.hint_lines().0.to_string().contains("⏎ pick"));
    press(&mut fixture.page, &[KeyCode::Enter]);
    let shown = screen(&mut fixture.page);
    assert!(shown.contains("│ Which profile?") && shown.contains("│ ▌none (task)         the built-in task · now") && shown.contains("│  \"ops\"               work on other machines over ssh") && shown.contains("\"codex-build\"") && shown.contains("\"codex\""), "{shown}");
    assert!(!shown.contains("\"reviewer\""), "the reviewer is never a task's profile: {shown}");
    let Mode::Choose(chooser) = &fixture.page.mode else { panic!("a pick") };
    let at = chooser.choices.iter().position(|choice| choice.label == "\"codex-build\"").unwrap();
    press(&mut fixture.page, &vec![KeyCode::Down; at]);
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert_eq!(file(&fixture, "config.toml"), "[task]\nprofile = \"codex-build\"\n");
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(matches!(&fixture.page.mode, Mode::Choose(chooser) if chooser.index == at), "the pick opens on the profile in effect");
    press(&mut fixture.page, &[KeyCode::Esc]);
    press(&mut fixture.page, &[KeyCode::Enter]);
    press(&mut fixture.page, &vec![KeyCode::Up; at]);
    press(&mut fixture.page, &[KeyCode::Enter]);
    assert!(!file(&fixture, "config.toml").contains("profile"), "none starts the built-in task: the key goes");
}

#[test]
fn the_title_line_ends_at_the_right_edge() {
    let mut fixture = fixture("settings-title-edge", "", None);
    let (shown, buffer) = screen_at(&mut fixture.page, 100, 30);
    let row = shown.lines().position(|line| line.contains("Settings")).unwrap();
    let last = buffer.area.width - 3;
    let end: String = (last - 3..=last).map(|x| buffer[(x, row as u16)].symbol().to_string()).collect();
    assert_eq!(end, "edit", "the hint ends in the title area's last column: {shown}");
}
