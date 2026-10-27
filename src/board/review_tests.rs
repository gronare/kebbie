use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use serde_json::{Value, json};

use super::review::*;
use super::review_data::tests::{Repo, commit, repo, sh};
use super::review_data::{self as data, kebbie_bin};
use super::review_draw as draw;

type Calls = Rc<RefCell<Vec<Vec<String>>>>;

struct Fake {
    root: PathBuf,
    layout: Rc<RefCell<Value>>,
    modes: Rc<RefCell<Vec<Value>>>,
}

impl Fake {
    fn answer(&self, argv: &[String]) -> std::result::Result<String, String> {
        if argv[0] == kebbie_bin() {
            return self.kebbie(&argv[1..]);
        }
        if argv[0] == herdr() {
            return Ok(match argv[1..3].join(" ").as_str() {
                "pane layout" => json!({ "result": { "layout": self.layout.borrow().clone() } }).to_string(),
                "pane split" => json!({ "result": { "pane": { "pane_id": "p_nvim" } } }).to_string(),
                "pane move" => json!({ "result": { "move_result": { "pane": { "pane_id": "p_nvim2" } } } }).to_string(),
                _ => String::new(),
            });
        }
        if argv[0] == super::review_nvim::RPC {
            return match argv[2].as_str() {
                "nvim_get_mode" if !self.modes.borrow().is_empty() => Ok(self.modes.borrow_mut().remove(0).to_string()),
                "nvim_input" | "nvim_exec_lua" => Ok("1".to_string()),
                _ => Err("nvim is not running".to_string()),
            };
        }
        Err("nvim is not running".to_string())
    }

    fn accepted(&self) -> Vec<String> {
        accepted_in(&self.root)
    }

    fn kebbie(&self, args: &[String]) -> std::result::Result<String, String> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match args.as_slice() {
            ["accept", _, sha, "--withdraw"] => {
                let kept: Vec<String> = self.accepted().into_iter().filter(|kept| kept != sha).collect();
                accept_elsewhere(&self.root, &kept.join("\n"));
                Ok("Withdrawn".to_string())
            }
            ["accept", _, sha] => {
                let mut kept = self.accepted();
                kept.push(sha.to_string());
                accept_elsewhere(&self.root, &kept.join("\n"));
                Ok("Accepted".to_string())
            }
            _ => Err(format!("unexpected kebbie {args:?}")),
        }
    }
}

// The fake's acceptances live in an accepted folder of its own that the view watches, written as kebbie writes them, by a rename.
fn accepted_file(root: &Path) -> PathBuf {
    root.join("state/accepted/billing@app")
}

fn accepted_in(root: &Path) -> Vec<String> {
    fs::read_to_string(accepted_file(root)).unwrap_or_default().lines().map(str::to_string).collect()
}

fn accept_elsewhere(root: &Path, shas: &str) {
    crate::state::write(&accepted_file(root), shas).unwrap();
}

// kebbie's marks as the view reads them in-process: the steps when steps.json is there, else the commits with the fake's acceptances.
fn fake_marks(root: PathBuf, slot: PathBuf) -> impl FnMut(&Path) -> Option<data::Entries> {
    move |_: &Path| {
        if let Some(steps) = fs::read_to_string(root.join("steps.json")).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()) {
            return data::from_steps(&steps, "billing");
        }
        let accepted = accepted_in(&root);
        let log = sh(&slot, &["log", "--reverse", "--format=%H %s", "origin/main..HEAD"]);
        let commits: Vec<Value> = log.lines().map(|line| line.split_once(' ').unwrap()).map(|(sha, subject)| json!({ "sha": sha, "subject": subject, "accepted": accepted.contains(&sha.to_string()) })).collect();
        data::from_commits(&json!({ "commits": commits }), "billing")
    }
}

struct Opened {
    view: View,
    calls: Calls,
    layout: Rc<RefCell<Value>>,
    modes: Rc<RefCell<Vec<Value>>>,
    repo: Repo,
}

impl Drop for Opened {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.repo.root);
    }
}

impl Opened {
    fn sent(&self, what: &str) -> Vec<Vec<String>> {
        self.calls.borrow().iter().filter(|argv| argv.join(" ").contains(what)).cloned().collect()
    }

    fn clear(&self) {
        self.calls.borrow_mut().clear();
    }

    fn press(&mut self, code: KeyCode) {
        self.view.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn shown(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.view.dir.join("shown.json")).unwrap()).unwrap()
    }

    fn screen(&mut self, width: u16, height: u16) -> Vec<String> {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw::draw(frame, &mut self.view)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>()).collect()
    }

    fn click(&mut self, x: u16, y: u16) {
        self.view.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE });
    }
}

fn open_with(repo: Repo, panes: Panes) -> Opened {
    let calls: Calls = Rc::default();
    let layout = Rc::new(RefCell::new(Value::Null));
    let modes = Rc::new(RefCell::new(Vec::new()));
    let fake = Fake { root: repo.root.clone(), layout: layout.clone(), modes: modes.clone() };
    let log = calls.clone();
    let marks = fake_marks(repo.root.clone(), repo.slot.clone());
    let watched = vec![repo.root.join("state/accepted")];
    let mut view = View::new(&repo.dir, &repo.slot, panes, Box::new(move |argv: &[String]| {
        log.borrow_mut().push(argv.to_vec());
        fake.answer(argv)
    }), Box::new(marks));
    view.watched = watched;
    view.stamp = None;
    Opened { view, calls, layout, modes, repo }
}

fn open(repo: Repo) -> Opened {
    open_with(repo, Panes::default())
}

fn with_steps(repo: &Repo, accepted_first: bool) {
    let step = |index: usize, title: &str, sha: Option<&str>, accepted: bool| match sha {
        Some(sha) => json!({ "index": index, "title": title, "extra": false, "state": "committed", "sha": sha, "subject": title, "accepted": accepted }),
        None => json!({ "index": index, "title": title, "extra": false, "state": if index == 4 { "in-progress" } else { "planned" } }),
    };
    let steps = [step(1, "Change a", Some(&repo.shas[0]), accepted_first), step(2, "Add b", Some(&repo.shas[1]), false), step(3, "Add c", Some(&repo.shas[2]), false), step(4, "Write the docs", None, false), step(5, "Release", None, false)];
    fs::write(repo.root.join("steps.json"), json!({ "task": "billing@app", "listed": true, "pushed": 1, "steps": steps }).to_string()).unwrap();
}

fn row<'a>(screen: &'a [String], text: &str) -> &'a str {
    screen.iter().find(|line| line.contains(text)).unwrap_or_else(|| panic!("{text} on the screen:\n{}", screen.join("\n")))
}

#[test]
fn a_narrow_review_with_steps_opens_on_the_first_step_to_accept_with_its_message_and_files() {
    let repo = repo("narrow-steps");
    with_steps(&repo, true);
    let mut opened = open(repo);
    assert_eq!(opened.view.selected.as_deref(), Some(opened.repo.shas[1].as_str()), "step 2 is accepted, so it opens on step 3");
    let screen = opened.screen(80, 20);
    assert!(row(&screen, "review").contains("review billing · since origin/main · 1 accepted · 2 to accept") && row(&screen, "review").trim_end().ends_with("? keys"), "{}", screen.join("\n"));
    assert!(row(&screen, "all").contains("▕ all ▏ ▕ ↑ 1 pushed ▏ ▕ ✓ 2 ▏ ▕ ● 3 ▏ ▕ ● 4 ▏ ▕ ◐ 5 ▏ ▕ ○ 6 ▏"), "{}", screen.join("\n"));
    assert!(row(&screen, "Add b").contains(&format!("▎ Add b  {}", &opened.repo.shas[1][..7])));
    assert!(row(&screen, "Why b exists.").contains("▎ Why b exists."), "the body under the subject");
    assert!(row(&screen, "FILES").contains("FILES 1 · step 3"));
    assert!(row(&screen, "+1").contains("▌ +1    -0    b"), "{}", screen.join("\n"));
    assert!(screen[19].starts_with("← → step · ↑ ↓ file · a accept · r read · / filter · tab to the diff"), "{}", screen[19]);
    assert!(screen.iter().all(|line| line.chars().count() == 80));
}

#[test]
fn a_wide_review_without_steps_names_its_tabs_after_the_commits_and_the_whole_branch_lists_every_file() {
    let repo = repo("wide-commits");
    accept_elsewhere(&repo.root, &repo.shas[0]);
    let mut opened = open(repo);
    assert_eq!(opened.view.selected.as_deref(), Some(opened.repo.shas[1].as_str()));
    let screen = opened.screen(160, 30);
    assert!(row(&screen, "all").contains("▕ all ▏ ▕ ✓ 1 ▏ ▕ ● 2 ▏ ▕ ● 3 ▏") && !row(&screen, "all").contains("pushed"), "{}", screen.join("\n"));
    assert!(row(&screen, "FILES").contains("FILES 1 · commit 2"));
    opened.press(KeyCode::Left);
    opened.press(KeyCode::Left);
    let screen = opened.screen(160, 30);
    assert!(row(&screen, "Every unpushed commit").contains("Every unpushed commit as one diff: commits 1–3."), "{}", screen.join("\n"));
    fs::write(opened.repo.slot.join("d"), "new\n").unwrap();
    opened.view.refresh();
    assert!(row(&opened.screen(160, 30), "Every unpushed commit").contains("commits 1–3, and the changes not committed yet."));
    assert!(row(&screen, "FILES").contains("FILES 3 · commits 1–3"));
    for file in ["a", "b", "c"] {
        assert!(screen.iter().any(|line| line.contains(&format!("{file} ")) && line.contains('+')), "{file}");
    }
    assert_eq!(Layout::of(160), Layout::Wide);
    assert_eq!(Layout::of(119), Layout::Narrow);
}

#[test]
fn the_nvim_pane_moves_beside_the_view_when_the_review_grows_wide_and_back_under_it_when_narrow() {
    let mut opened = open_with(repo("layout"), Panes { view: Some("p_view".into()), nvim: Some("p_nvim".into()) });
    let pane = |id: &str, x: u64, width: u64| json!({ "pane_id": id, "rect": { "x": x, "y": 0, "width": width, "height": 40 } });
    *opened.layout.borrow_mut() = json!({ "tab_id": "w1:t1", "workspace_id": "w1", "zoomed": false, "panes": [pane("p_claude", 0, 60), pane("p_view", 60, 160), pane("p_nvim", 60, 160)] });
    opened.view.relayout(160);
    let argv = |args: &[&str]| std::iter::once(herdr()).chain(args.iter().map(|arg| arg.to_string())).collect::<Vec<String>>();
    assert_eq!(opened.sent("pane move"), [
        argv(&["pane", "move", "p_nvim", "--new-tab", "--workspace", "w1", "--label", "kebbie review", "--no-focus"]),
        argv(&["pane", "move", "p_nvim", "--tab", "w1:t1", "--split", "right", "--target-pane", "p_view", "--ratio", "0.4", "--no-focus"]),
    ], "out to a tab of its own and back beside the view, as herdr leaves a move within the tab undone");
    assert_eq!((opened.view.layout, opened.view.panes.nvim.as_deref()), (Layout::Wide, Some("p_nvim2")));
    opened.clear();
    *opened.layout.borrow_mut() = json!({ "tab_id": "w1:t1", "zoomed": false, "panes": [pane("p_claude", 0, 60), pane("p_view", 60, 64), pane("p_nvim2", 124, 96)] });
    opened.view.relayout(64);
    assert!(opened.sent("pane move").is_empty(), "the view and nvim side by side are still 160 wide");
    *opened.layout.borrow_mut() = json!({ "tab_id": "w1:t1", "zoomed": false, "panes": [pane("p_claude", 0, 100), pane("p_view", 100, 40), pane("p_nvim2", 140, 60)] });
    opened.view.relayout(40);
    assert!(opened.sent("pane move")[1].contains(&"down".to_string()) && opened.view.layout == Layout::Narrow);
    opened.clear();
    *opened.layout.borrow_mut() = json!({ "tab_id": "w1:t1", "zoomed": true, "panes": [pane("p_view", 0, 220)] });
    opened.view.relayout(220);
    assert!(opened.sent("pane move").is_empty(), "a zoomed view moves nothing");
    let region = region(&json!({ "panes": [pane("p_view", 10, 50), pane("p_nvim", 60, 90)] }), "p_view", Some("p_nvim"));
    assert_eq!(region.map(|place| place.width), Some(140));
}

#[test]
fn tab_moves_focus_from_the_steps_to_the_files_to_nvim_and_back_and_shift_tab_goes_the_other_way() {
    let mut opened = open_with(repo("focus"), Panes { view: Some("p_view".into()), nvim: Some("p_nvim".into()) });
    assert_eq!(opened.view.focus, Panel::Steps);
    opened.press(KeyCode::Tab);
    assert_eq!(opened.view.focus, Panel::Files);
    assert!(opened.sent("pane focus").is_empty(), "within the view no herdr pane moves");
    opened.press(KeyCode::Tab);
    assert_eq!(opened.view.focus, Panel::Diff);
    assert_eq!(opened.sent("pane focus"), [focus_argv("p_view", "down")], "into nvim below");
    opened.clear();
    opened.press(KeyCode::Tab);
    assert_eq!(opened.view.focus, Panel::Steps, "nvim hands its tab to the view, which takes the focus back");
    assert_eq!(opened.sent("pane focus"), [focus_argv("p_nvim", "up")]);
    opened.clear();
    opened.press(KeyCode::BackTab);
    assert_eq!(opened.view.focus, Panel::Diff, "shift-tab from the steps goes to the diff");
    opened.press(KeyCode::BackTab);
    assert_eq!(opened.view.focus, Panel::Files);
    assert_eq!(opened.sent("pane focus"), [focus_argv("p_view", "down"), focus_argv("p_nvim", "up")]);
    opened.view.layout = Layout::Wide;
    opened.clear();
    opened.press(KeyCode::Tab);
    assert_eq!(opened.sent("pane focus"), [focus_argv("p_view", "right")], "beside the view on a wide review");
    let screen = opened.screen(80, 20);
    assert!(row(&screen, "tab").contains("tab back to the steps"), "{}", screen.join("\n"));
}

#[test]
fn m_collapses_the_message_to_the_row_of_tabs_and_the_review_remembers_it() {
    let mut opened = open(repo("collapse"));
    assert!(opened.screen(80, 20).iter().any(|line| line.contains("Why b exists.") || line.contains("Change a")));
    opened.press(KeyCode::Char('m'));
    let screen = opened.screen(80, 20);
    assert!(!screen.iter().any(|line| line.contains("▎")), "no message: {}", screen.join("\n"));
    assert!(row(&screen, "all").contains("● 1"), "the tabs stay");
    assert!(opened.view.dir.join("collapsed").exists());
    let again = View::new(&opened.view.dir, &opened.view.root, Panes::default(), Box::new(|_: &[String]| Err("off".to_string())), Box::new(|_: &Path| None));
    assert!(again.collapsed, "a view opened again on the review keeps it collapsed");
    opened.press(KeyCode::Char('m'));
    assert!(opened.screen(80, 20).iter().any(|line| line.contains("▎")));
}

#[test]
fn a_accepts_the_selected_commit_through_kebbie_moves_to_the_next_waiting_and_withdraws_an_accepted_one() {
    let mut opened = open(repo("accept"));
    let shas = opened.repo.shas.clone();
    assert_eq!(opened.view.selected.as_deref(), Some(shas[0].as_str()));
    opened.press(KeyCode::Char('a'));
    let accept = |sha: &str, withdraw: bool| { let mut argv = vec![kebbie_bin(), "accept".to_string(), "billing".to_string(), sha.to_string()]; if withdraw { argv.push("--withdraw".to_string()); } argv };
    assert_eq!(opened.sent("accept billing"), [accept(&shas[0], false)]);
    assert_eq!(opened.view.selected.as_deref(), Some(shas[1].as_str()), "on to the next commit waiting");
    assert_eq!(opened.view.entries.items[0].mark, data::Mark::Accepted, "the mark is kebbie's, read back");
    opened.press(KeyCode::Left);
    opened.clear();
    opened.press(KeyCode::Char('a'));
    assert_eq!(opened.sent("--withdraw"), [accept(&shas[0], true)]);
    assert_eq!(opened.view.entries.items[0].mark, data::Mark::Committed);
    accept_elsewhere(&opened.repo.root, &shas[2]);
    opened.view.refresh();
    assert_eq!(opened.view.entries.items[2].mark, data::Mark::Accepted, "an acceptance given elsewhere shows here");
    opened.press(KeyCode::Left);
    opened.clear();
    opened.press(KeyCode::Char('a'));
    assert!(opened.sent("accept billing").is_empty(), "nothing is accepted on all");
    assert!(opened.screen(80, 20).iter().any(|line| line.contains("Pick a step first")));
}

#[test]
fn with_steps_kebbies_own_mark_decides_and_steps_without_a_commit_are_skipped() {
    let repo = repo("steps-accept");
    with_steps(&repo, true);
    let mut opened = open(repo);
    let shas = opened.repo.shas.clone();
    for _ in 0..5 {
        opened.press(KeyCode::Right);
    }
    assert_eq!(opened.view.selected.as_deref(), Some(shas[2].as_str()), "planned steps have no commit to show");
    opened.press(KeyCode::Left);
    opened.press(KeyCode::Left);
    opened.press(KeyCode::Char('a'));
    assert_eq!(opened.sent("accept billing")[0].last().map(String::as_str), Some("--withdraw"), "step 2 is accepted by kebbie's steps");
}

#[test]
fn choosing_a_file_shows_it_in_nvim_at_once_with_the_commits_own_diff() {
    let mut opened = open(repo("show"));
    let shas = opened.repo.shas.clone();
    let shown = opened.shown();
    assert_eq!((shown["path"].as_str(), shown["title"].as_str(), shown["short"].as_str(), shown["index"].as_u64(), shown["count"].as_u64()), (Some("a"), Some("commit 1"), Some(&shas[0][..7]), Some(1), Some(1)));
    let diff: Vec<String> = serde_json::from_value(shown["diff"].clone()).unwrap();
    assert_eq!(diff[diff.len() - 4..], [format!("{}^", shas[0]), shas[0].clone(), "--".to_string(), "a".to_string()]);
    assert_eq!(opened.sent("nvim_get_mode"), [rpc_argv(&opened.view.dir, "nvim_get_mode", None)]);
    assert_eq!(rpc_argv(Path::new("/r"), "nvim_exec_lua", Some("return KebbieReview.load()")), ["nvim-rpc", "/r/nvim.sock", "nvim_exec_lua", "return KebbieReview.load()"]);
    opened.clear();
    opened.press(KeyCode::Left);
    opened.press(KeyCode::Down);
    assert_eq!(opened.shown()["path"], "b", "the second file of the whole branch");
    assert_eq!(opened.shown()["title"], "commits 1–3");
    assert_eq!(opened.sent("nvim_get_mode").len(), 2, "one per file shown: a, then b");
    opened.press(KeyCode::Down);
    let screen = opened.screen(80, 24);
    let y = screen.iter().position(|line| line.contains(" a ") && line.contains('+')).unwrap();
    opened.click(10, y as u16);
    assert_eq!((opened.shown()["path"].as_str(), opened.view.focus), (Some("a"), Panel::Files), "a click picks the file");
    let tab_y = screen.iter().position(|line| line.contains("▕ ● 2 ▏")).unwrap();
    let tab_x = screen[tab_y].chars().collect::<Vec<_>>().windows(3).position(|cells| cells.iter().collect::<String>() == "● 2").unwrap();
    opened.click(tab_x as u16, tab_y as u16);
    assert_eq!(opened.view.selected.as_deref(), Some(shas[1].as_str()), "a click picks the step");
    assert_eq!(opened.shown()["path"], "b");
}

#[test]
fn r_marks_the_file_read_and_slash_filters_the_list() {
    let mut opened = open(repo("read-filter"));
    opened.press(KeyCode::Left);
    opened.press(KeyCode::Char('r'));
    let screen = opened.screen(80, 24);
    assert!(row(&screen, " a ").contains("✓ read"), "{}", screen.join("\n"));
    assert_eq!(opened.view.current().map(|file| file.path.as_str()), Some("b"), "on to the next file");
    opened.press(KeyCode::Char('/'));
    for character in "!b".chars() {
        opened.press(KeyCode::Char(character));
    }
    opened.press(KeyCode::Enter);
    assert_eq!(opened.view.visible().iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), ["a", "c"]);
    opened.press(KeyCode::Esc);
    assert_eq!(opened.view.visible().len(), 3, "esc clears the filter");
}

#[test]
fn a_commit_that_leaves_the_branch_gives_way_to_the_whole_branch() {
    let mut opened = open(repo("amended"));
    opened.press(KeyCode::Right);
    opened.press(KeyCode::Right);
    commit(&opened.repo.slot, "c", "three, reworded\n", "unused");
    sh(&opened.repo.slot, &["reset", "--quiet", "--soft", "HEAD~2"]);
    sh(&opened.repo.slot, &["commit", "--quiet", "-m", "Add c, reworded"]);
    opened.view.refresh();
    assert_eq!(opened.view.selected, None);
    assert_eq!(opened.view.visible().len(), 3);
}

#[test]
fn the_view_opens_nvim_below_itself_in_the_review_dir_and_closes_it_on_q() {
    let mut opened = open_with(repo("nvim"), Panes { view: Some("p_view".into()), nvim: None });
    opened.view.open_nvim(Path::new("/k/scripts/review.lua"));
    let dir = opened.view.dir.to_string_lossy().to_string();
    assert_eq!(opened.sent("pane split")[0][3..], ["p_view", "--direction", "down", "--ratio", "0.45", "--cwd", &opened.view.root.to_string_lossy(), "--no-focus"].map(String::from));
    assert_eq!(opened.sent("pane run")[0][3..], ["p_nvim".to_string(), format!("REVIEW_DIR='{dir}' nvim --listen '{dir}/nvim.sock' -S '/k/scripts/review.lua'; exit")]);
    opened.press(KeyCode::Char('q'));
    assert!(opened.view.quit);
    opened.view.close();
    assert_eq!(opened.sent("pane close")[0][3], "p_nvim");
    assert!(!opened.view.dir.join("shown.json").exists());
}

#[test]
fn a_hard_wrapped_body_reflows_to_a_narrow_panel_and_lists_trailers_and_code_keep_their_lines() {
    let repo = repo("reflow");
    commit(&repo.slot, "d", "four\n", "Charge through a PaymentGateway\n\nCheckout built its own Klarna client, so the tests could not\nswap the provider. It now takes the gateway it is given, and\nthe app passes the one the config names.\n\n- the gateway is passed in\n- the config names it\n\n    Checkout.new(cart, gateway: g)\n\nRefs: #12");
    let mut opened = open(repo);
    opened.press(KeyCode::Right);
    opened.press(KeyCode::Right);
    opened.press(KeyCode::Right);
    let screen = opened.screen(50, 40);
    assert!(screen.iter().any(|line| line.contains("▎ tests could not swap the provider. It now")), "one paragraph, wrapped to the panel:\n{}", screen.join("\n"));
    for kept in ["▎ - the gateway is passed in", "▎ - the config names it", "▎     Checkout.new(cart, gateway: g)", "▎ Refs: #12"] {
        assert!(screen.iter().any(|line| line.contains(kept)), "{kept}:\n{}", screen.join("\n"));
    }
    use super::review_draw::{Para, reflow};
    assert_eq!(reflow(&["", "one", "two", "", "1. first", "   more", "2) second", "next", "Signed-off-by: Ada"]), [
        Para::Text("one two".into()), Para::Blank, Para::Text("1. first".into()), Para::Kept("   more".into()), Para::Text("2) second next".into()), Para::Kept("Signed-off-by: Ada".into()),
    ]);
}

#[test]
fn the_first_diff_reaches_nvim_through_a_hit_enter_prompt_and_waits_out_one_the_user_answers() {
    let mut opened = open(repo("prompt"));
    assert_eq!(opened.view.pending, 59, "no nvim yet: one try made, the rest left for the loop");
    let dir = opened.view.dir.clone();
    let rpc = |method: &str, argument: Option<&str>| rpc_argv(&dir, method, argument);
    let mode = |mode: &str, blocking: bool| json!({ "mode": mode, "blocking": blocking });
    *opened.modes.borrow_mut() = vec![mode("r", true), mode("n", false)];
    opened.clear();
    opened.view.push();
    assert_eq!(*opened.calls.borrow(), [rpc("nvim_get_mode", None), rpc("nvim_input", Some("<CR>")), rpc("nvim_get_mode", None), rpc("nvim_exec_lua", Some("return KebbieReview.load()"))], "lazy.nvim's Press ENTER is pressed, then the diff loads");
    assert_eq!(opened.view.pending, 0);
    *opened.modes.borrow_mut() = vec![mode("r?", true)];
    opened.clear();
    opened.press(KeyCode::Right);
    assert_eq!(*opened.calls.borrow().iter().filter(|argv| argv[0] == "nvim-rpc").cloned().collect::<Vec<_>>(), [rpc("nvim_get_mode", None)], "a question the user is answering gets no key and no load");
    assert_eq!(opened.view.pending, 59, "tried again from the loop");
    *opened.modes.borrow_mut() = vec![mode("n", false)];
    opened.view.push();
    assert_eq!(opened.view.pending, 0);
}

#[test]
fn a_refresh_reads_kebbies_marks_and_the_files_again_only_once_something_they_stand_on_moved() {
    let repo = repo("stamp");
    let reads = Rc::new(std::cell::Cell::new(0));
    let counted = reads.clone();
    let mut inner = fake_marks(repo.root.clone(), repo.slot.clone());
    let mut view = View::new(&repo.dir, &repo.slot, Panes::default(), Box::new(|_: &[String]| Err("off".to_string())), Box::new(move |root: &Path| { counted.set(counted.get() + 1); inner(root) }));
    view.watched = vec![repo.root.join("state/accepted")];
    view.refresh();
    assert_eq!(reads.get(), 2, "a stamp of its own folders reads once");
    view.refresh();
    view.refresh();
    assert_eq!(reads.get(), 2, "nothing moved, nothing is read");
    fs::write(repo.slot.join("a"), "base\none\nedited\n").unwrap();
    view.refresh();
    assert_eq!(reads.get(), 3, "an edit in the worktree");
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(repo.slot.join("a"), "base\none\nedited again\n").unwrap();
    view.refresh();
    assert_eq!(reads.get(), 4, "a second edit to a file already changed");
    accept_elsewhere(&repo.root, &repo.shas[0]);
    view.refresh();
    assert_eq!((reads.get(), view.entries.items[0].mark), (5, data::Mark::Accepted), "an acceptance given elsewhere");
    commit(&repo.slot, "a", "base\none\nedited again\n", "Edit a");
    view.refresh();
    assert_eq!((reads.get(), view.entries.items.len()), (6, 4), "a new commit");
    let _ = fs::remove_dir_all(&repo.root);
}
