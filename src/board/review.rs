use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, DisableFocusChange, EnableFocusChange, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde_json::Value;

use crate::sh::{Result, quote};
use super::terminal::{init_terminal, restore_terminal};
use super::review_data::{self as data, Entries, File, Mark, Marks, Runner, Stamp};
use super::review_draw as draw;
use super::review_nvim::{self as nvim, LOAD, Next, RPC};
use super::widgets::{GREEN, RED, YELLOW};

pub const WIDE: u16 = 120;
const REFRESH: Duration = Duration::from_secs(2);
const RETRIES: u32 = 60;
const RETRY: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Panel {
    Steps,
    Files,
    Diff,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layout {
    Narrow,
    Wide,
}

impl Layout {
    pub fn of(width: u16) -> Layout {
        if width >= WIDE { Layout::Wide } else { Layout::Narrow }
    }

    fn side(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Layout::Narrow => ("down", "up", "0.45"),
            Layout::Wide => ("right", "left", "0.4"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    All,
    Entry(usize),
    File(usize),
    Panel(Panel),
}

#[derive(Default)]
pub struct Panes {
    pub view: Option<String>,
    pub nvim: Option<String>,
}

pub struct View {
    pub dir: PathBuf,
    pub root: PathBuf,
    pub base: String,
    pub since: String,
    pub branch: String,
    pub entries: Entries,
    pub uncommitted: bool,
    pub selected: Option<String>,
    pub files: Vec<File>,
    pub file: usize,
    pub scroll: usize,
    pub focus: Panel,
    pub back_to: Panel,
    pub collapsed: bool,
    pub filter: String,
    pub typing: bool,
    pub help: bool,
    pub note: Option<(String, Color)>,
    pub layout: Layout,
    pub hits: Vec<(Rect, Hit)>,
    pub list_rows: usize,
    pub follow: bool,
    pub quit: bool,
    pub panes: Panes,
    pub sent: Option<Value>,
    pub pending: u32,
    run: Box<Runner>,
    marks: Box<Marks>,
    upstream: Option<String>,
    pub watched: Vec<PathBuf>,
    pub stamp: Option<Stamp>,
}

pub fn herdr() -> String {
    crate::herdr::bin()
}

pub fn nvim_socket(dir: &Path) -> PathBuf {
    dir.join("nvim.sock")
}

pub fn rpc_argv(dir: &Path, method: &str, argument: Option<&str>) -> Vec<String> {
    [RPC, &nvim_socket(dir).to_string_lossy(), method].into_iter().chain(argument).map(str::to_string).collect()
}

pub fn focus_argv(from: &str, direction: &str) -> Vec<String> {
    [herdr().as_str(), "pane", "focus", "--pane", from, "--direction", direction].map(str::to_string).to_vec()
}

pub struct Place {
    pub width: u16,
    pub zoomed: bool,
    pub tab: String,
    pub workspace: String,
}

// herdr leaves a move within one tab undone, so nvim goes out to a tab of its own and comes back on the other side; it runs on throughout, and herdr closes the emptied tab.
pub fn move_argvs(nvim: &str, place: &Place, view: &str, layout: Layout) -> [Vec<String>; 2] {
    let (side, _, ratio) = layout.side();
    let argv = |args: &[&str]| std::iter::once(herdr()).chain(args.iter().map(|arg| arg.to_string())).collect::<Vec<String>>();
    [
        argv(&["pane", "move", nvim, "--new-tab", "--workspace", &place.workspace, "--label", "kebbie review", "--no-focus"]),
        argv(&["pane", "move", nvim, "--tab", &place.tab, "--split", side, "--target-pane", view, "--ratio", ratio, "--no-focus"]),
    ]
}

pub fn nvim_command(dir: &Path, lua: &Path) -> String {
    format!("REVIEW_DIR={} nvim --listen {} -S {}; exit", quote(&dir.to_string_lossy()), quote(&nvim_socket(dir).to_string_lossy()), quote(&lua.to_string_lossy()))
}

// The review's width: the view and nvim side by side, or the view alone; zoomed, the view alone fills the tab and nothing moves.
pub fn region(layout: &Value, view: &str, nvim: Option<&str>) -> Option<Place> {
    let panes = layout["panes"].as_array()?;
    let rect = |id: &str| panes.iter().find(|pane| pane["pane_id"] == id).map(|pane| {
        let at = |key: &str| pane["rect"][key].as_u64().unwrap_or_default() as u16;
        (at("x"), at("x") + at("width"))
    });
    let (mut left, mut right) = rect(view)?;
    if let Some((nvim_left, nvim_right)) = nvim.and_then(rect) {
        left = left.min(nvim_left);
        right = right.max(nvim_right);
    }
    let text = |key: &str| layout[key].as_str().unwrap_or_default().to_string();
    Some(Place { width: right - left, zoomed: layout["zoomed"] == true, tab: text("tab_id"), workspace: text("workspace_id") })
}

fn result_of(output: &str) -> Value {
    serde_json::from_str::<Value>(output).map(|reply| reply["result"].clone()).unwrap_or_default()
}

impl View {
    pub fn new(dir: &Path, root: &Path, panes: Panes, run: Box<Runner>, marks: Box<Marks>) -> View {
        let mut view = View {
            dir: dir.to_path_buf(), root: root.to_path_buf(), base: String::new(), since: String::new(), branch: String::new(), entries: Entries::default(), uncommitted: false,
            selected: None, files: Vec::new(), file: 0, scroll: 0, focus: Panel::Steps, back_to: Panel::Steps, collapsed: dir.join("collapsed").exists(),
            filter: String::new(), typing: false, help: false, note: None, layout: Layout::Narrow, hits: Vec::new(), list_rows: 0, follow: true, quit: false, panes, sent: None, pending: 0, run,
            marks, upstream: crate::repo::default_branch(root).ok(), watched: data::kebbie_folders(), stamp: None,
        };
        view.load();
        view.selected = view.first_waiting();
        view.reload_files(false);
        view
    }

    // Ok(false) when nothing it reads from moved since the last load, so nothing was read again.
    fn load(&mut self) -> bool {
        let stamp = data::stamp(&self.root, self.upstream.as_deref(), &self.watched);
        if self.stamp.as_ref() == Some(&stamp) {
            return false;
        }
        self.uncommitted = stamp.uncommitted();
        self.stamp = Some(stamp);
        self.base = data::resolve_base(&self.root, &data::base_spec(&self.dir));
        self.since = data::since(&self.dir, &self.root);
        self.branch = data::branch(&self.root);
        self.entries = data::read_entries(&self.root, &self.base, &mut *self.marks);
        true
    }

    // A selected commit that left the branch, by a rebase or an amend, gives way to the whole branch.
    pub fn refresh(&mut self) {
        if !self.load() {
            return;
        }
        if self.selected.as_ref().is_some_and(|sha| self.entries.find(sha).is_none()) {
            self.selected = None;
            self.file = 0;
        }
        self.reload_files(true);
    }

    fn reload_files(&mut self, keep: bool) {
        let path = self.current().map(|file| file.path.clone()).filter(|_| keep);
        self.files = data::files(&self.dir, &self.root, &self.base, self.selected.as_deref());
        self.file = path.and_then(|path| self.visible().iter().position(|file| file.path == path)).unwrap_or(self.file.min(self.visible().len().saturating_sub(1)));
        self.show();
    }

    pub fn visible(&self) -> Vec<&File> {
        let words: Vec<&str> = self.filter.split_whitespace().collect();
        self.files.iter().filter(|file| words.iter().all(|word| match word.strip_prefix('!') {
            Some(hidden) => hidden.is_empty() || !file.path.contains(hidden),
            None => file.path.contains(word),
        })).collect()
    }

    pub fn current(&self) -> Option<&File> {
        self.visible().get(self.file).copied()
    }

    pub fn selected_entry(&self) -> Option<(usize, &data::Entry)> {
        let sha = self.selected.as_deref()?;
        self.entries.find(sha).map(|index| (index, &self.entries.items[index]))
    }

    fn first_waiting(&self) -> Option<String> {
        self.entries.task.as_ref()?;
        self.entries.items.iter().find(|entry| entry.mark == Mark::Committed).and_then(|entry| entry.sha.clone())
    }

    fn waiting_after(&self, index: usize) -> Option<String> {
        let waiting = |entry: &&data::Entry| entry.mark == Mark::Committed;
        self.entries.items.iter().skip(index + 1).find(waiting).or_else(|| self.entries.items.iter().take(index).find(waiting)).and_then(|entry| entry.sha.clone())
    }

    pub fn title(&self) -> String {
        let noun = if self.entries.steps { "step" } else { "commit" };
        match self.selected_entry() {
            Some((_, entry)) => format!("{noun} {}", entry.number),
            None => {
                let numbers: Vec<usize> = self.entries.committed().map(|(_, entry)| entry.number).collect();
                match (numbers.first(), numbers.last()) {
                    (Some(first), Some(last)) if first != last => format!("{noun}s {first}–{last}"),
                    (Some(first), _) => format!("{noun} {first}"),
                    _ => "all".to_string(),
                }
            }
        }
    }

    pub fn select(&mut self, sha: Option<String>) {
        if self.selected != sha {
            self.selected = sha;
            self.file = 0;
            self.scroll = 0;
            self.reload_files(false);
        }
    }

    fn tabs(&self) -> Vec<Option<String>> {
        std::iter::once(None).chain(self.entries.committed().map(|(_, entry)| entry.sha.clone())).collect()
    }

    pub fn step(&mut self, forward: bool) {
        let tabs = self.tabs();
        let at = tabs.iter().position(|tab| *tab == self.selected).unwrap_or(0);
        let to = if forward { (at + 1).min(tabs.len() - 1) } else { at.saturating_sub(1) };
        self.select(tabs[to].clone());
    }

    pub fn move_file(&mut self, down: bool) {
        let count = self.visible().len();
        if count == 0 {
            return;
        }
        self.file = if down { (self.file + 1).min(count - 1) } else { self.file.saturating_sub(1) };
        self.follow = true;
        self.show();
    }

    pub fn pick_file(&mut self, index: usize) {
        if index < self.visible().len() {
            self.file = index;
            self.follow = true;
            self.show();
        }
    }

    // kebbie accept is the user's alone, and this pane runs under their own shell, so kebbie takes it from here.
    pub fn accept(&mut self) {
        let Some(task) = self.entries.task.clone() else {
            self.note = Some(("Accepting is for a task's commits; this review is not a task's".to_string(), YELLOW));
            return;
        };
        let Some((index, entry)) = self.selected_entry().map(|(index, entry)| (index, entry.clone())) else {
            self.note = Some(("Pick a step first: ← →".to_string(), YELLOW));
            return;
        };
        let sha = entry.sha.clone().unwrap_or_default();
        let withdraw = entry.mark == Mark::Accepted;
        let mut argv = vec![data::kebbie_bin(), "accept".into(), task, sha.clone()];
        if withdraw {
            argv.push("--withdraw".into());
        }
        let result = (self.run)(&argv);
        self.stamp = None;
        self.refresh();
        match result {
            Ok(_) if withdraw => self.note = Some((format!("Acceptance withdrawn: {}", data::short(&sha)), YELLOW)),
            Ok(_) => {
                self.note = Some((format!("✓ accepted {}", data::short(&sha)), GREEN));
                if let Some(next) = self.waiting_after(index) {
                    self.select(Some(next));
                }
            }
            Err(error) => self.note = Some((error.lines().find(|line| !line.trim().is_empty()).unwrap_or("kebbie accept failed").to_string(), RED)),
        }
    }

    pub fn toggle_read(&mut self) {
        let Some(file) = self.current().cloned() else { return };
        if let Err(error) = data::set_read(&self.root, &file.path, self.selected.as_deref(), !file.read) {
            self.note = Some((error, RED));
            return;
        }
        self.reload_files(true);
        if !file.read {
            self.move_file(true);
        }
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
        let marker = self.dir.join("collapsed");
        let _ = if self.collapsed { fs::write(&marker, "") } else { fs::remove_file(&marker) };
    }

    // nvim reads shown.json at its start too, so a view faster than nvim's socket loses nothing.
    pub fn show(&mut self) {
        let shown = match self.current() {
            Some(file) => data::shown(&file.path, &self.title(), self.selected.as_deref(), self.file + 1, self.visible().len(), data::diff_argv(&self.root, &self.base, &file.path, self.selected.as_deref())),
            None => data::shown("", &self.title(), self.selected.as_deref(), 0, 0, Vec::new()),
        };
        if self.sent.as_ref() == Some(&shown) {
            return;
        }
        if crate::state::write(&self.dir.join("shown.json"), shown.to_string()).is_ok() {
            self.pending = RETRIES;
            self.push();
        }
        self.sent = Some(shown);
    }

    // A load that does not land, nvim still starting or at a prompt, is tried again from the loop until RETRIES runs out.
    pub fn push(&mut self) {
        for _ in 0..4 {
            let Ok(mode) = (self.run)(&rpc_argv(&self.dir, "nvim_get_mode", None)) else { break };
            let mode: Value = serde_json::from_str(&mode).unwrap_or_default();
            match nvim::next(mode["mode"].as_str().unwrap_or_default(), mode["blocking"] == true) {
                Next::Load => {
                    if (self.run)(&rpc_argv(&self.dir, "nvim_exec_lua", Some(LOAD))).is_ok() {
                        self.pending = 0;
                        return;
                    }
                    break;
                }
                Next::Dismiss(key) => {
                    let _ = (self.run)(&rpc_argv(&self.dir, "nvim_input", Some(key)));
                }
                Next::Wait => break,
            }
        }
        self.pending = self.pending.saturating_sub(1);
    }

    // herdr focuses a pane by its neighbour, so the view and nvim reach each other by the side they share.
    pub fn focus_to(&mut self, panel: Panel) {
        let (toward, back, _) = self.layout.side();
        match (self.focus, panel) {
            (Panel::Diff, Panel::Diff) => {}
            (_, Panel::Diff) => {
                if let Some(view) = self.panes.view.clone() {
                    let _ = (self.run)(&focus_argv(&view, toward));
                }
            }
            (Panel::Diff, _) => {
                if let Some(nvim) = self.panes.nvim.clone() {
                    let _ = (self.run)(&focus_argv(&nvim, back));
                }
            }
            _ => {}
        }
        if panel != Panel::Diff {
            self.back_to = panel;
        }
        self.focus = panel;
    }

    pub fn cycle(&mut self, forward: bool) {
        let order = [Panel::Steps, Panel::Files, Panel::Diff];
        let at = order.iter().position(|panel| *panel == self.focus).unwrap_or(0);
        let to = if forward { (at + 1) % 3 } else { (at + 2) % 3 };
        self.focus_to(order[to]);
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.note = None;
        if self.typing {
            return self.type_filter(key);
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.focus == Panel::Diff && !matches!(key.code, KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('q')) {
            self.focus = self.back_to;
        }
        match key.code {
            KeyCode::Tab => self.cycle(true),
            KeyCode::BackTab => self.cycle(false),
            KeyCode::Left | KeyCode::Char('h') => self.step(false),
            KeyCode::Right | KeyCode::Char('l') => self.step(true),
            KeyCode::Up | KeyCode::Char('k') => self.move_file(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_file(true),
            KeyCode::Enter => self.focus_to(Panel::Diff),
            KeyCode::Char('a') => self.accept(),
            KeyCode::Char('r') => self.toggle_read(),
            KeyCode::Char('m') => self.toggle_collapsed(),
            KeyCode::Char('/') => {
                self.typing = true;
                self.focus = Panel::Files;
            }
            KeyCode::Esc => self.filter.clear(),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            _ => {}
        }
    }

    fn type_filter(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.typing = false,
            KeyCode::Esc => {
                self.typing = false;
                self.filter.clear();
            }
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(character) => self.filter.push(character),
            _ => return,
        }
        self.file = 0;
        self.scroll = 0;
        self.show();
    }

    pub fn hit_at(&self, x: u16, y: u16) -> Option<Hit> {
        let inside = |rect: &Rect| x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height;
        self.hits.iter().find(|(rect, hit)| inside(rect) && !matches!(hit, Hit::Panel(_))).or_else(|| self.hits.iter().find(|(rect, _)| inside(rect))).map(|(_, hit)| *hit)
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => match self.hit_at(mouse.column, mouse.row) {
                Some(Hit::All) => {
                    self.focus = Panel::Steps;
                    self.select(None);
                }
                Some(Hit::Entry(index)) => {
                    self.focus = Panel::Steps;
                    if let Some(sha) = self.entries.items.get(index).and_then(|entry| entry.sha.clone()) {
                        self.select(Some(sha));
                    }
                }
                Some(Hit::File(index)) => {
                    self.focus = Panel::Files;
                    self.pick_file(index);
                }
                Some(Hit::Panel(panel)) => self.focus = panel,
                None => {}
            },
            MouseEventKind::ScrollDown => {
                self.follow = false;
                self.scroll = (self.scroll + 3).min(self.visible().len().saturating_sub(self.list_rows.max(1)));
            }
            MouseEventKind::ScrollUp => {
                self.follow = false;
                self.scroll = self.scroll.saturating_sub(3);
            }
            _ => {}
        }
        if self.focus != Panel::Diff {
            self.back_to = self.focus;
        }
    }

    // The nvim pane moves beside the view on a wide review and under it on a narrow one; the review's width holds through the move.
    pub fn relayout(&mut self, own_width: u16) {
        let Some(view) = self.panes.view.clone() else {
            self.layout = Layout::of(own_width);
            return;
        };
        let reply = (self.run)(&[herdr(), "pane".into(), "layout".into(), "--pane".into(), view.clone()]).map(|output| result_of(&output)["layout"].clone());
        let Some(place) = reply.ok().and_then(|layout| region(&layout, &view, self.panes.nvim.as_deref())) else {
            self.layout = Layout::of(own_width);
            return;
        };
        let wanted = Layout::of(place.width);
        if wanted == self.layout || place.zoomed {
            return;
        }
        if let Some(nvim) = self.panes.nvim.clone() {
            let [out, back] = move_argvs(&nvim, &place, &view, wanted);
            let moved = (self.run)(&out).and_then(|_| (self.run)(&back));
            let Ok(output) = moved else { return };
            if let Some(pane) = result_of(&output)["move_result"]["pane"]["pane_id"].as_str() {
                self.panes.nvim = Some(pane.to_string());
                let _ = fs::write(self.dir.join("nvim"), pane);
            }
        }
        self.layout = wanted;
    }

    pub fn open_nvim(&mut self, lua: &Path) {
        let Some(view) = self.panes.view.clone() else { return };
        let (side, _, ratio) = self.layout.side();
        let split = [herdr().as_str(), "pane", "split", &view, "--direction", side, "--ratio", ratio, "--cwd", &self.root.to_string_lossy(), "--no-focus"].map(str::to_string).to_vec();
        let Some(pane) = (self.run)(&split).ok().and_then(|output| result_of(&output)["pane"]["pane_id"].as_str().map(str::to_string)) else {
            self.note = Some(("herdr did not open the diff pane".to_string(), RED));
            return;
        };
        let _ = fs::write(self.dir.join("nvim"), &pane);
        let _ = (self.run)(&[herdr(), "pane".into(), "run".into(), pane.clone(), nvim_command(&self.dir, lua)]);
        self.panes.nvim = Some(pane);
    }

    pub fn close(&mut self) {
        if let Some(nvim) = self.panes.nvim.take() {
            let _ = (self.run)(&[herdr(), "pane".into(), "close".into(), nvim]);
        }
        for file in ["live", "nvim", "shown.json"] {
            let _ = fs::remove_file(self.dir.join(file));
        }
    }
}

fn execute(argv: &[String]) -> std::result::Result<String, String> {
    let (program, args) = argv.split_first().ok_or("nothing to run")?;
    if program == RPC && args.len() >= 2 {
        return nvim::call(&args[0], &args[1], args.get(2).map(String::as_str));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::sh::attempt(program, &args, None)?
}

pub fn run(dir: &Path) -> Result<String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let root = data::toplevel(&cwd).ok_or("kebbie review --view runs in the repo under review")?;
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    fs::write(dir.join("live"), std::process::id().to_string()).map_err(|error| error.to_string())?;
    let panes = Panes { view: std::env::var("HERDR_PANE_ID").ok().filter(|pane| !pane.is_empty()), nvim: None };
    let mut view = View::new(dir, &root, panes, Box::new(execute), Box::new(data::kebbie_marks));
    let mut terminal = init_terminal();
    let _ = execute!(std::io::stdout(), EnableFocusChange);
    let width = terminal.size().map(|size| size.width).unwrap_or(80);
    view.relayout(width);
    view.open_nvim(&crate::lead::install_root()?.join("scripts/review.lua"));
    let (mut refreshed, mut pushed) = (Instant::now(), Instant::now());
    while !view.quit {
        terminal.draw(|frame| draw::draw(frame, &mut view)).map_err(|error| error.to_string())?;
        if event::poll(Duration::from_millis(250)).map_err(|error| error.to_string())? {
            match event::read().map_err(|error| error.to_string())? {
                Event::Key(key) if key.kind == KeyEventKind::Press => view.handle_key(key),
                Event::Mouse(mouse) => view.handle_mouse(mouse),
                Event::Resize(width, _) => view.relayout(width),
                Event::FocusLost => view.focus = Panel::Diff,
                Event::FocusGained if view.focus == Panel::Diff => view.focus = view.back_to,
                _ => {}
            }
        }
        if view.pending > 0 && pushed.elapsed() >= RETRY {
            view.push();
            pushed = Instant::now();
        }
        if refreshed.elapsed() >= REFRESH {
            view.refresh();
            refreshed = Instant::now();
        }
    }
    let _ = execute!(std::io::stdout(), DisableFocusChange);
    restore_terminal();
    view.close();
    Ok(String::new())
}

