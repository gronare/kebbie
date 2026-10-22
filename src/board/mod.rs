mod card;
mod columns;
mod confirm;
mod detail;
mod draw;
mod editor;
mod header;
pub(crate) mod images;
mod inbox;
mod input;
mod link;
mod message_log;
mod mouse;
mod options;
mod paste;
mod push;
mod space;
mod status_bar;
pub(crate) mod terminal;
#[cfg(test)]
mod test_support;
mod threads;
pub(crate) mod widgets;

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::text::Text;
use ratatui::DefaultTerminal;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;

use crate::area_page::{self, Effect};
use crate::routine_page;
use crate::backlog::{self, Backlog, Entry};
use crate::config::{self, Source};
use crate::fetch::Fetcher;
use crate::herdr;
use crate::keys::{self, Act, Ctx};
use crate::messages::Message;
use crate::permit;
use crate::peers;
use crate::repo::name_of;
use crate::evidence;
use crate::report;
use crate::sh::Result;
use crate::status::{Checkout, Item};
use crate::tasks;
use crate::usage::Usage;
use crate::{lead, update};
use columns::suggested_name;
use detail::Detail;
use draw::draw;
use editor::MessageEditor;
use images::{Decoded, Graphics, GraphicsEnv, image_viewer, spawn_decoder};
use inbox::QuestionWindow;
use input::Purpose;
use link::Link;
use message_log::{Fold, fold_file, load_fold, save_fold};
use options::{busy, ongoing_entry};
use space::Process;
use terminal::{init_terminal, restore_terminal};
use threads::{DetailRequest, spawn_ci_watcher, spawn_detailer, spawn_refresher, spawn_restarter, spawn_routine_runner, spawn_update_checker};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Page {
    Work,
    Areas,
    Routines,
}

#[derive(Clone, Copy, PartialEq)]
enum Column {
    Work,
    Backlog,
}

enum Mode {
    Normal,
    Options(usize),
    Input { purpose: Purpose, label: String, buffer: String },
    Message(String),
    ConfirmDrop { path: PathBuf, name: String, force: bool, nudged: bool },
    ConfirmReset { path: PathBuf, name: String, nudged: bool },
    ConfirmDone { path: PathBuf, name: String, nudged: bool },
    ConfirmQueued { name: String, about: String, now: bool, nudged: bool },
    Filter { buffer: String },
    Card { path: PathBuf, name: String, scroll: u16, image: usize },
    Fullscreen { path: PathBuf, name: String, scroll: u16, index: usize },
    ConfirmPush { path: PathBuf, commits: Vec<tasks::Commit>, selected: usize, card: Option<(report::Card, bool)>, tests: Option<evidence::Shown>, scroll: push::Scroll },
    EditMessage(Box<MessageEditor>),
    Working { label: String, result: Receiver<Result<String>> },
    Questions(Box<QuestionWindow>),
    Help,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Task(usize),
    Entry(usize),
    Choice(usize),
    Commit(usize),
    CardImage(usize),
    Tab(Page),
    Question(usize),
    Answer(usize),
    Messages(Option<usize>),
    Key(KeyCode),
}

struct App {
    column: Column,
    tasks: Vec<Item>,
    checkouts: Vec<Checkout>,
    backlog: Backlog,
    work_selected: usize,
    backlog_selected: usize,
    mode: Mode,
    overlay: bool,
    loaded: bool,
    error: Option<String>,
    screens: HashMap<String, (Instant, String)>,
    refresh: Sender<()>,
    quit: bool,
    wanted_task: Option<String>,
    hits: Vec<(Rect, Hit)>,
    links: Vec<Link>,
    page: Page,
    areas: area_page::Page,
    routines: routine_page::Page,
    sources: Vec<Source>,
    fetch_failures: Vec<String>,
    away: bool,
    messages: Vec<Message>,
    fold: Fold,
    seen: u64,
    fold_file: Option<PathBuf>,
    in_messages: bool,
    message_back: usize,
    questions: Vec<Message>,
    usage: Option<Usage>,
    ongoing: Vec<Ongoing>,
    details: HashMap<PathBuf, std::rc::Rc<Detail>>,
    detail_requests: Option<Sender<DetailRequest>>,
    detail_results: Option<Receiver<(PathBuf, Detail)>>,
    asked_details: HashMap<PathBuf, Instant>,
    graphics: Graphics,
    images: HashMap<(PathBuf, u16, u16), Option<Protocol>>,
    cell_guessed: bool,
    viewer: Option<&'static str>,
    decoded: HashMap<PathBuf, Option<image::DynamicImage>>,
    image_requests: Option<Sender<PathBuf>>,
    image_results: Option<Receiver<Decoded>>,
    plans: HashMap<String, Text<'static>>,
    updates: Vec<(String, String)>,
    updater: Updater,
    update_running: bool,
    update_shown: bool,
    recheck: Option<Sender<()>>,
    stale: Vec<String>,
    filter: Option<(Column, String)>,
    ctrl_c: Option<Instant>,
    notice: Option<(String, Instant)>,
    lead_chat: Option<bool>,
}

type Updater = std::sync::Arc<dyn Fn() -> Result<String> + Send + Sync>;

#[derive(Clone)]
struct Ongoing {
    key: String,
    title: String,
    area: String,
    task: Option<String>,
}

impl App {
    fn selected_task(&self) -> Option<Item> {
        self.tasks.get(self.work_selected).filter(|_| self.shown(Column::Work, self.work_selected)).cloned()
    }

    fn selected_entry(&self) -> Option<Entry> {
        self.backlog.entries.get(self.backlog_selected.checked_sub(self.ongoing.len())?).filter(|_| self.shown(Column::Backlog, self.backlog_selected)).cloned()
    }

    fn filter_of(&self, column: Column) -> Option<&str> {
        self.filter.as_ref().filter(|(on, text)| *on == column && !text.is_empty()).map(|(_, text)| text.as_str())
    }

    fn shown(&self, column: Column, index: usize) -> bool {
        let Some(filter) = self.filter_of(column) else { return true };
        let wanted = filter.to_lowercase();
        let has = |text: &str| text.to_lowercase().contains(&wanted);
        match column {
            Column::Work => self.tasks.get(index).is_some_and(|item| has(&item.name) || has(&item.place)),
            Column::Backlog => match index.checked_sub(self.ongoing.len()) {
                None => self.ongoing.get(index).is_some_and(|note| has(&note.title) || has(&note.area)),
                Some(entry) => self.backlog.entries.get(entry).is_some_and(|entry| has(&entry.title) || has(&entry.area)),
            },
        }
    }

    fn ctx(&self) -> Ctx {
        match self.column {
            Column::Work => Ctx::Work,
            Column::Backlog => Ctx::Backlog,
        }
    }

    fn say(&mut self, text: String) {
        self.notice = Some((text, Instant::now()));
    }

    fn toggle_chat(&mut self) {
        let Some(hidden) = self.lead_chat else {
            return self.say(format!("{} hides the lead's chat only on the board in the lead Space", keys::map().label(Act::HideChat)));
        };
        match lead::toggle_chat() {
            Ok(_) => {
                self.lead_chat = Some(!hidden);
                let _ = self.refresh.send(());
            }
            Err(error) => self.mode = Mode::Message(error),
        }
    }

    fn backlog_len(&self) -> usize {
        self.ongoing.len() + self.backlog.entries.len()
    }

    fn repo_named(&self, name: &str) -> Option<PathBuf> {
        self.tasks.iter().filter_map(|item| item.repo.clone()).find(|repo| name_of(repo) == name)
            .or_else(|| herdr::workspaces().ok()?.iter().filter_map(|workspace| workspace["worktree"]["repo_root"].as_str())
                .map(PathBuf::from).find(|repo| name_of(repo) == name))
    }

    fn any_repo(&self) -> PathBuf {
        self.selected_task().and_then(|item| item.repo)
            .or_else(|| self.checkouts.first().and_then(|checkout| self.repo_named(&checkout.name)))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    }

    fn lead_pane(&self) -> Option<String> {
        herdr::agents().ok()?.into_iter().find(|agent| agent["name"] == "lead")
            .and_then(|agent| agent["pane_id"].as_str().map(str::to_string))
    }

    // Pages switch only when nothing is open on either.
    fn at_rest(&self) -> bool {
        matches!(self.mode, Mode::Normal) && self.areas.is_normal() && self.routines.is_normal()
    }

    fn in_background(&mut self, label: String, step: impl FnOnce() -> Result<String> + Send + 'static) {
        let (sender, result) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(step());
        });
        self.mode = Mode::Working { label, result };
    }

    fn start_update(&mut self) {
        let updater = self.updater.clone();
        self.update_running = true;
        self.in_background("Updating kelpie, its plugins and the vault plugin…".to_string(), move || updater());
    }

    fn take_working_result(&mut self) {
        let Mode::Working { result, .. } = &self.mode else { return };
        let Ok(result) = result.try_recv() else { return };
        if std::mem::take(&mut self.update_running) {
            self.update_shown = true;
            self.updates.clear();
            if let Some(recheck) = &self.recheck {
                let _ = recheck.send(());
            }
        }
        self.show(result);
    }

    fn show(&mut self, result: Result<String>) {
        self.mode = Mode::Message(result.unwrap_or_else(|error| error));
        let _ = self.refresh.send(());
    }

    fn move_selection(&mut self, step: isize) {
        let column = self.column;
        let (selected, length) = match column {
            Column::Work => (self.work_selected, self.tasks.len()),
            Column::Backlog => (self.backlog_selected, self.ongoing.len() + self.backlog.entries.len()),
        };
        let mut at = selected;
        let next = loop {
            let Some(moved) = at.checked_add_signed(step).filter(|moved| *moved < length) else { break selected };
            if self.shown(column, moved) {
                break moved;
            }
            at = moved;
        };
        match column {
            Column::Work => self.work_selected = next,
            Column::Backlog => self.backlog_selected = next,
        }
    }

    fn handle(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) {
        self.handle_key(key, Some(terminal));
    }

    // Without a terminal (in tests) the steps that leave the board for an editor or a long start are skipped.
    fn handle_key(&mut self, key: KeyEvent, terminal: Option<&mut DefaultTerminal>) {
        let map = keys::map();
        if keys::ctrl_c(&key) {
            if self.ctrl_c.is_some_and(|at| at.elapsed() < Duration::from_secs(2)) {
                self.quit = true;
            } else {
                self.ctrl_c = Some(Instant::now());
                self.say(format!("{} again to quit", map.label(Act::Quit)));
            }
            return;
        }
        self.notice = None;
        if self.at_rest() {
            let ctx = match self.page {
                Page::Areas => Ctx::Areas,
                Page::Routines => Ctx::Routines,
                Page::Work => self.ctx(),
            };
            match map.act(ctx, &key) {
                Some(Act::Lead) => {
                    if let Err(error) = lead::focus_toggle() {
                        self.mode = Mode::Message(error);
                    } else if self.lead_chat == Some(true) {
                        self.lead_chat = Some(false);
                    }
                    return;
                }
                Some(Act::HideChat) => return self.toggle_chat(),
                Some(Act::WorkPage) => return self.page = Page::Work,
                Some(Act::AreasPage) => return self.page = Page::Areas,
                Some(Act::RoutinesPage) => return self.page = Page::Routines,
                Some(Act::Inbox) => {
                    self.page = Page::Work;
                    return self.open_questions(None);
                }
                _ => {}
            }
        }
        if self.page == Page::Routines {
            let top = self.routines.is_normal();
            let effect = self.routines.handle(key, terminal);
            return self.after_routines(effect, top.then(|| map.act(Ctx::Routines, &key)).flatten());
        }
        if self.page == Page::Areas {
            let top = self.areas.is_normal();
            let Some(terminal) = terminal else { return };
            match self.areas.handle(key, terminal) {
                Effect::Left => self.quit = self.overlay,
                Effect::Unhandled if top => self.at_top(map.act(Ctx::Areas, &key)),
                _ => {}
            }
            return;
        }
        let act = map.act(self.ctx(), &key);
        let cancels = matches!(act, Some(Act::Back | Act::Close)) || key.code == KeyCode::Char('n');
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.handle_normal(key),
            Mode::Options(index) => self.handle_options(index, act, key),
            Mode::Help | Mode::Message(_) => {}
            working @ Mode::Working { .. } => self.mode = working,
            Mode::Questions(window) => self.handle_questions(window, key),
            Mode::ConfirmReset { path, name, .. } => self.confirm_reset(path, name, key, cancels),
            Mode::ConfirmDone { path, name, .. } => self.confirm_done(path, name, key, cancels),
            Mode::ConfirmQueued { name, about, now, .. } => self.confirm_queued(name, about, now, key, cancels),
            Mode::ConfirmDrop { path, name, force, .. } => self.confirm_drop(path, name, force, key, cancels),
            Mode::Card { path, name, scroll, image } => self.handle_card(path, name, scroll, image, key),
            Mode::Fullscreen { path, name, scroll, index } => self.handle_fullscreen(path, name, scroll, index, key),
            Mode::Filter { buffer } => self.handle_filter(buffer, key),
            Mode::ConfirmPush { path, commits, selected, card, tests, scroll } => self.handle_push(path, commits, selected, card, tests, scroll, key),
            Mode::EditMessage(editor) => self.handle_editor(editor, key, terminal),
            Mode::Input { purpose, label, buffer } => self.handle_input(purpose, label, buffer, key, terminal),
        }
    }

    fn after_routines(&mut self, effect: routine_page::Effect, top: Option<Act>) {
        let mut select = |address: &str| -> Option<Item> {
            let index = self.tasks.iter().position(|item| peers::address(&item.name, &item.place) == address)?;
            (self.page, self.column, self.work_selected, self.filter, self.in_messages) = (Page::Work, Column::Work, index, None, false);
            self.tasks.get(index).cloned()
        };
        match effect {
            routine_page::Effect::Unhandled if top.is_some() => self.at_top(top),
            routine_page::Effect::Task(address) => {
                select(&address);
            }
            routine_page::Effect::Card(address) => {
                if let Some(item) = select(&address) {
                    match (item.path.clone(), self.card_of(&item)) {
                        (Some(path), Some(_)) => self.mode = Mode::Card { path, name: item.name, scroll: 0, image: 0 },
                        _ => self.show(Ok(format!("{}'s card is still loading; s shows it here in a moment.", item.name))),
                    }
                }
            }
            _ => {}
        }
    }

    // esc and q at the top: an overlay board closes; the full board stays, as it is quit with ctrl+c twice.
    fn at_top(&mut self, act: Option<Act>) {
        match act {
            Some(Act::Back) if self.filter.is_some() && self.page == Page::Work => self.filter = None,
            Some(Act::Back | Act::Close) if self.overlay => self.quit = true,
            Some(Act::Close) => {
                let quit = keys::map().label(Act::Quit);
                self.say(format!("{quit} twice to quit"));
            }
            _ => {}
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        if self.in_messages {
            return self.handle_messages(key);
        }
        let ctx = self.ctx();
        let Some(act) = keys::map().act(ctx, &key) else { return };
        match act {
            Act::Messages => self.press_m(),
            Act::Move => self.move_selection(if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) { -1 } else { 1 }),
            Act::Switch => self.column = if self.column == Column::Work { Column::Backlog } else { Column::Work },
            Act::Back | Act::Close => self.at_top(Some(act)),
            Act::Help => self.mode = Mode::Help,
            Act::Inbox => self.open_questions(None),
            Act::Options => {
                let has_selection = match self.column {
                    Column::Work => self.selected_task().is_some(),
                    Column::Backlog => self.backlog_len() > 0 && self.shown(Column::Backlog, self.backlog_selected),
                };
                if has_selection {
                    self.mode = Mode::Options(0);
                }
            }
            Act::Refresh => self.refresh_backlog(),
            Act::Away => self.show(permit::away(Some(if permit::away_since().is_some() { "off" } else { "on" }))),
            Act::Update => self.start_update(),
            Act::Filter => {
                let buffer = self.filter_of(self.column).unwrap_or_default().to_string();
                self.mode = Mode::Filter { buffer };
            }
            Act::New if ctx == Ctx::Backlog && self.sources.contains(&Source::Local) => self.act(Act::New),
            Act::Card if ctx == Ctx::Work => self.act(Act::Card),
            Act::New => self.mode = Mode::Input { purpose: Purpose::TaskName, label: "New task: name".to_string(), buffer: String::new() },
            other if self.choices().iter().any(|choice| choice.act == Some(other)) => self.act(other),
            _ => {}
        }
    }

    fn refresh_backlog(&mut self) {
        let result = backlog::refresh_prompt(&self.sources).and_then(|prompt| match self.lead_pane() {
            Some(pane) => crate::typing::prompt(&pane, &prompt, "the lead").map(|_| "Asked the lead to refresh the backlog; it updates here when it is saved.".to_string()).map_err(|refused| format!("Nothing was typed: the lead {}; ask again once it is clear.", refused.reason())),
            None => Err("No lead is running; prefix+space starts it.".to_string()),
        });
        self.show(result);
    }

    fn act(&mut self, act: Act) {
        if !self.shown(self.column, if self.column == Column::Work { self.work_selected } else { self.backlog_selected }) {
            return;
        }
        if self.column == Column::Backlog && let Some(note) = self.ongoing.get(self.backlog_selected).cloned().filter(|_| act == Act::Go) {
            match &note.task {
                Some(task) => {
                    if let Some(index) = self.tasks.iter().position(|item| &peers::address(&item.name, &item.place) == task) {
                        self.column = Column::Work;
                        self.work_selected = index;
                    }
                }
                None => {
                    let entry = ongoing_entry(&note);
                    let buffer = suggested_name(&entry.title);
                    self.mode = Mode::Input { purpose: Purpose::BacklogName(entry), label: "Resume as task: name".to_string(), buffer };
                }
            }
            return;
        }
        match self.column {
            Column::Backlog => match act {
                Act::Go => {
                    if let Some(entry) = self.selected_entry() {
                        let buffer = suggested_name(&entry.title);
                        self.mode = Mode::Input { purpose: Purpose::BacklogName(entry), label: "Start as task: name".to_string(), buffer };
                    }
                }
                Act::Refresh => self.refresh_backlog(),
                Act::New => {
                    let label = "Add to the backlog: title (— summary and (area) optional)".to_string();
                    self.mode = Mode::Input { purpose: Purpose::BacklogItem, label, buffer: String::new() };
                }
                _ => {}
            },
            Column::Work => {
                let Some(item) = self.selected_task() else { return };
                if let Some(note) = item.queued_note() {
                    let (go, drop) = (keys::key(Act::Go), keys::key(Act::Drop));
                    let note = if note.ends_with('.') { note } else { format!("{note}.") };
                    match act {
                        Act::Go => self.mode = Mode::ConfirmQueued { about: format!("{note} Started now, it begins {}.", item.start_now_means().unwrap_or_default()), name: item.name, now: true, nudged: false },
                        Act::Drop => self.mode = Mode::ConfirmQueued { about: format!("{note} Dropped, {}.", item.drop_means().unwrap_or_default()), name: item.name, now: false, nudged: false },
                        _ => self.show(Err(format!("{} is queued and has no worktree yet: {go} starts it now, {drop} drops the wait.", item.name))),
                    }
                    return;
                }
                match act {
                    Act::Go if item.agent.is_none() => {
                        let Some(path) = item.path.clone() else { return };
                        let (sender, result) = mpsc::channel();
                        let name = item.name.clone();
                        thread::spawn(move || {
                            let options = tasks::Start { prompt: None, focus: true, from: None, mode: None, profile: None, backlog: None, note: true };
                            let _ = sender.send(tasks::start(&path, &name, &options));
                        });
                        self.mode = Mode::Working { label: format!("Restarting {}…", item.name), result };
                    }
                    Act::Go => {
                        if let Some(workspace) = &item.workspace {
                            let _ = herdr::focus_workspace(workspace);
                            if self.overlay {
                                self.quit = true;
                            }
                        }
                    }
                    Act::Chat => {
                        if let Some(agent) = item.agent {
                            let label = format!("Message to {}", item.name);
                            self.mode = Mode::Input { purpose: Purpose::Message(agent.pane, item.name), label, buffer: String::new() };
                        }
                    }
                    Act::Review => self.open_review(&item),
                    Act::Card => match (item.path.clone(), self.card_of(&item), self.question_of(&item)) {
                        (_, _, Some(at)) => self.open_questions(Some(at)),
                        (Some(path), Some(_), None) => self.mode = Mode::Card { path, name: item.name.clone(), scroll: 0, image: 0 },
                        _ => self.show(Ok(format!("{} has no sign-off card yet; it writes one when it reports ready.", item.name))),
                    },
                    Act::Tidy | Act::Push | Act::Done | Act::SendReview | Act::Rebase if busy(&item) => self.show(Err(format!("{} is still working; wait until it reports.", item.name))),
                    Act::Push if item.awaiting_review() => self.show(Err(format!("{} is in review; push opens once its reviewer passes its commits.", item.name))),
                    Act::Tidy => {
                        if let Some(path) = item.path.clone() {
                            let label = "How should the commits look? (optional, enter to let Claude decide)".to_string();
                            self.mode = Mode::Input { purpose: Purpose::Tidy(path), label, buffer: String::new() };
                        }
                    }
                    Act::Server => self.toggle(&item, Process::Server),
                    Act::Worker => self.toggle(&item, Process::Worker),
                    Act::Rebase => {
                        if let Some(path) = item.path.clone() {
                            self.in_background(format!("Rebasing {}…", item.name), move || tasks::rebase(&path));
                        }
                    }
                    Act::Follow => {
                        if let Some(repo) = &item.repo {
                            self.show(tasks::toggle_follow(repo, &item.name));
                        }
                    }
                    Act::Permit if item.is_task() => self.show(permit::toggle_task(&peers::address(&item.name, &item.place))),
                    Act::SendReview => {
                        if let Some(path) = &item.path {
                            self.show(if item.awaiting_review() { crate::review::withdraw(path, None) } else { crate::review::request(path, None) });
                        }
                    }
                    Act::Reset => {
                        if let Some(path) = item.path.clone() {
                            self.mode = Mode::ConfirmReset { path, name: item.name, nudged: false };
                        }
                    }
                    Act::Push | Act::Done | Act::Drop => {
                        let Some(path) = item.path.clone() else { return };
                        match act {
                            Act::Push => match tasks::commits(&path) {
                                Ok(commits) if commits.is_empty() => self.show(Err(format!("{} has no commits to push.", item.name))),
                                Ok(commits) => {
                                    let (card, tests) = (report::card_for(&path), evidence::for_path(&path));
                                    self.mode = Mode::ConfirmPush { path, commits, selected: 0, card, tests, scroll: push::Scroll::default() };
                                }
                                Err(error) => self.show(Err(error)),
                            },
                            Act::Done => self.mode = Mode::ConfirmDone { path, name: item.name, nudged: false },
                            _ => self.mode = Mode::ConfirmDrop { path, name: item.name, force: false, nudged: false },
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

const RESIZE_SETTLE: Duration = Duration::from_secs(1);

// A new terminal size must hold for a second before the board lays out for it, so dragging a pane's edge redraws once, at the end.
#[derive(Default)]
struct Settle {
    drawn: Option<(u16, u16)>,
    pending: Option<((u16, u16), Instant)>,
}

impl Settle {
    fn draw_now(&mut self, size: (u16, u16), now: Instant) -> bool {
        match (self.drawn, self.pending) {
            (None, _) => {
                self.drawn = Some(size);
                true
            }
            (Some(drawn), _) if drawn == size => {
                self.pending = None;
                true
            }
            (Some(_), Some((pending, since))) if pending == size => {
                let settled = now.duration_since(since) >= RESIZE_SETTLE;
                if settled {
                    (self.drawn, self.pending) = (Some(size), None);
                }
                settled
            }
            (Some(_), _) => {
                self.pending = Some((size, now));
                false
            }
        }
    }
}

const RESTART_STATE: &str = "KELPIE_BOARD_STATE";

fn built(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().filter(|metadata| metadata.len() > 0).and_then(|metadata| metadata.modified().ok())
}

// A rebuilt binary takes over the open board, keeping the column and selection; it must look the same on two checks so a half-written one never runs.
struct Rebuild {
    path: Option<PathBuf>,
    started: Option<SystemTime>,
    last: Option<SystemTime>,
    checked: Instant,
}

impl Rebuild {
    fn new() -> Self {
        let path = peers::own_exe().ok();
        let started = path.as_deref().and_then(built);
        Self { path, started, last: started, checked: Instant::now() }
    }

    fn ready(&mut self) -> bool {
        if self.checked.elapsed() < Duration::from_secs(2) {
            return false;
        }
        self.checked = Instant::now();
        let now = self.path.as_deref().and_then(built);
        let ready = now.is_some() && now != self.started && now == self.last;
        self.last = now;
        ready
    }

    fn restart(&self, app: &App) -> String {
        let Some(path) = &self.path else { return "Cannot tell where kelpie's binary is".to_string() };
        let column = match (app.page, app.column) {
            (Page::Areas, _) => "areas",
            (Page::Routines, _) => "routines",
            (_, Column::Backlog) => "backlog",
            _ => "work",
        };
        let task = app.selected_task().map(|item| item.name).unwrap_or_default();
        let state = format!("{column}\t{}\t{task}", app.backlog_selected);
        restore_terminal();
        let error = Command::new(path).args(std::env::args_os().skip(1)).env(RESTART_STATE, state).exec();
        format!("Restarting on the rebuilt kelpie failed: {error}")
    }
}

impl App {
    fn new(sources: Vec<Source>, backlog: Backlog, overlay: bool, refresh: Sender<()>) -> Self {
        App {
            column: Column::Work, tasks: Vec::new(), checkouts: Vec::new(), backlog, work_selected: 0, backlog_selected: 0,
            mode: Mode::Normal, overlay, loaded: false, error: None, screens: HashMap::new(), refresh, quit: false, wanted_task: None, hits: Vec::new(), links: Vec::new(),
            page: Page::Work, areas: area_page::Page::new(sources.clone()), routines: routine_page::Page::new(config::path(), config::repos_dir()), sources, fetch_failures: Vec::new(), away: false,
            messages: Vec::new(), fold: Fold::Line, seen: 0, fold_file: None, in_messages: false, message_back: 0, questions: Vec::new(), usage: None, ongoing: Vec::new(),
            details: HashMap::new(), detail_requests: None, detail_results: None, asked_details: HashMap::new(),
            graphics: Graphics::Off, images: HashMap::new(), cell_guessed: false, viewer: None, decoded: HashMap::new(), image_requests: None, image_results: None, plans: HashMap::new(),
            updates: Vec::new(), updater: std::sync::Arc::new(|| update::run(&crate::doctor::Env::current().claude_dir, false, &crate::release::Request::Channel)), update_running: false, update_shown: false, recheck: None, stale: Vec::new(),
            filter: None, ctrl_c: None, notice: None, lead_chat: None,
        }
    }
}

pub fn run(overlay: bool) -> Result<String> {
    let sources = config::sources()?;
    let (snapshots, refresh) = spawn_refresher(std::env::var("HERDR_PANE_ID").ok().filter(|_| !overlay));
    spawn_ci_watcher(refresh.clone());
    let mut app = App::new(sources, backlog::load(), overlay, refresh);
    app.away = permit::away_since().is_some();
    let (update_notices, recheck) = spawn_update_checker();
    app.recheck = Some(recheck);
    let restarter = spawn_restarter(overlay);
    spawn_routine_runner(overlay);
    let (requests, results) = spawn_detailer();
    app.detail_requests = Some(requests);
    app.detail_results = Some(results);
    (app.image_requests, app.image_results) = spawn_decoder().map_or((None, None), |(requests, results)| (Some(requests), Some(results)));
    let height = ratatui::crossterm::terminal::size().map_or(50, |(_, height)| height);
    app.fold_file = fold_file();
    (app.fold, app.seen) = load_fold(app.fold_file.as_deref(), height);
    if let Ok(state) = std::env::var(RESTART_STATE) {
        let mut parts = state.splitn(3, '\t');
        match parts.next() {
            Some("backlog") => app.column = Column::Backlog,
            Some("areas") => app.page = Page::Areas,
            Some("routines") => app.page = Page::Routines,
            _ => {}
        }
        app.backlog_selected = parts.next().and_then(|index| index.parse().ok()).unwrap_or(0);
        app.wanted_task = parts.next().filter(|name| !name.is_empty()).map(str::to_string);
    }
    let mut rebuild = Rebuild::new();
    let mut fetcher = Fetcher::new();
    let mut resize = Settle::default();
    let mut terminal = init_terminal();
    let graphics = GraphicsEnv::current();
    (app.graphics, app.cell_guessed) = graphics.startup();
    app.viewer = image_viewer(std::env::consts::OS, &|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    let _ = terminal.clear();
    let result = (|| -> Result<()> {
        while !app.quit {
            if let Mode::EditMessage(editor) = &mut app.mode {
                editor.poll_draft();
            }
            app.poll_answer();
            app.take_working_result();
            if !matches!(app.mode, Mode::Message(_)) {
                app.update_shown = false;
            }
            while let Ok(updates) = update_notices.try_recv() {
                app.updates = updates;
            }
            while let Ok(stale) = restarter.try_recv() {
                app.stale = stale;
            }
            while let Ok(update) = snapshots.try_recv() {
                match update {
                    Ok(snapshot) => {
                        let previous = app.selected_task().map(|item| (item.name, item.place));
                        app.tasks = snapshot.items.into_iter().filter(Item::is_task).collect();
                        app.areas.tasks = app.tasks.clone();
                        app.routines.tasks = app.tasks.clone();
                        app.checkouts = snapshot.checkouts;
                        let wanted = app.wanted_task.take();
                        let position = previous.and_then(|(name, place)| app.tasks.iter().position(|item| item.name == name && item.place == place))
                            .or_else(|| wanted.and_then(|name| app.tasks.iter().position(|item| item.name == name)));
                        app.work_selected = position.unwrap_or(app.work_selected.min(app.tasks.len().saturating_sub(1)));
                        if let Ok(sources) = config::sources() {
                            app.sources = sources;
                        }
                        app.backlog = backlog::load();
                        let kept = app.selected_message().filter(|_| app.message_back > 0).map(|message| message.id.clone());
                        app.messages = snapshot.messages;
                        if let Some(index) = kept.and_then(|id| app.messages.iter().position(|message| message.id == id)) {
                            app.message_back = app.messages.len() - 1 - index;
                        }
                        app.message_back = app.message_back.min(app.messages.len().saturating_sub(1));
                        app.questions = snapshot.questions;
                        app.usage = snapshot.usage;
                        app.ongoing = snapshot.ongoing;
                        app.lead_chat = snapshot.lead_chat;
                        app.backlog_selected = app.backlog_selected.min(app.backlog_len().saturating_sub(1));
                        let newest = app.messages.last().map_or(0, |message| message.at);
                        if app.fold == Fold::Open && newest > app.seen {
                            app.seen = newest;
                            save_fold(app.fold_file.as_deref(), app.fold, app.seen);
                        }
                        if let Mode::Questions(window) = &mut app.mode {
                            window.follow(&app.questions);
                        }
                        app.error = None;
                    }
                    Err(error) => app.error = Some(error),
                }
                app.loaded = true;
            }
            if let Some(results) = &app.detail_results {
                while let Ok((path, detail)) = results.try_recv() {
                    app.details.insert(path, std::rc::Rc::new(detail));
                }
            }
            app.take_images();
            if app.page == Page::Areas {
                app.areas.tick();
            }
            app.routines.tick();
            let mut repos: Vec<PathBuf> = app.tasks.iter().filter_map(|item| item.repo.clone()).chain(app.areas.repos()).collect();
            repos.sort();
            repos.dedup();
            if fetcher.tick(repos) {
                let _ = app.refresh.send(());
                app.areas.refresh();
            }
            app.fetch_failures = fetcher.failures();
            app.away = permit::away_since().is_some();
            if (matches!(app.mode, Mode::Normal | Mode::Help) || matches!(app.mode, Mode::Message(_)) && !app.update_shown) && app.areas.is_idle() && app.routines.is_idle() && rebuild.ready() {
                let error = rebuild.restart(&app);
                terminal = init_terminal();
                app.mode = Mode::Message(error);
            }
            if matches!(app.graphics, Graphics::Pending { wanted: true }) {
                (app.graphics, app.cell_guessed) = graphics.resolve(Picker::from_query_stdio().ok());
                app.images.clear();
            }
            let size = ratatui::crossterm::terminal::size().unwrap_or_default();
            if resize.draw_now(size, Instant::now()) {
                terminal.draw(|frame| draw(frame, &mut app)).map_err(|error| error.to_string())?;
            }
            if event::poll(Duration::from_millis(250)).map_err(|error| error.to_string())? {
                match event::read().map_err(|error| error.to_string())? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.handle(key, &mut terminal),
                    Event::Mouse(mouse) => app.mouse(mouse, &mut terminal),
                    Event::Paste(text) => app.paste(text),
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    restore_terminal();
    result.map(|_| String::new())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::board::test_support::*;
    use crate::config::Source;
    use crate::keys::{self, Ctx};
    use crate::board::header::header;
    use crate::board::input::Purpose;
    use crate::board::options::{backlog_choices, ongoing_choices, task_choices};
    use crate::board::status_bar::{help_lines, hint_lines};

    #[test]
    fn the_lead_board_alone_offers_h_and_says_chat_hidden_while_it_is() {
        let mut app = test_app(Vec::new());
        app.handle_key(KeyEvent::from(KeyCode::Char('h')), None);
        assert_eq!(app.notice.as_ref().map(|(text, _)| text.as_str()), Some("h hides the lead's chat only on the board in the lead Space"));
        assert!(!hint_lines(None, false).0.to_string().contains("h chat") && hint_lines(None, true).0.to_string().contains("h chat"));
        assert!(!help_lines(false).iter().any(|line| line.to_string().starts_with("h ")) && help_lines(true).iter().any(|line| line.to_string().starts_with("h ")));
        assert!(!header(&app, 200).0.to_string().contains("chat hidden"));
        app.lead_chat = Some(true);
        assert!(header(&app, 200).0.to_string().contains("◧ chat hidden · h"));
        app.lead_chat = Some(false);
        assert!(!header(&app, 200).0.to_string().contains("chat hidden"));
    }

    #[test]
    fn every_key_in_the_help_the_hints_and_the_menus_is_one_the_table_acts_on_and_every_row_shows_in_help() {
        let map = keys::map();
        let help: Vec<String> = help_lines(true).iter().map(|line| line.spans[0].content.trim().to_string()).collect();
        for row in keys::TABLE.iter().filter(|row| row.on.contains(&Ctx::Work) || row.on.contains(&Ctx::Backlog)) {
            assert!(help.contains(&map.label(row.act)), "{} is in the table but not in ? help: {help:?}", row.act.id());
        }
        for label in &help {
            assert!(keys::TABLE.iter().any(|row| map.label(row.act) == *label), "{label} is in ? help but not in the table");
        }
        let (full, short) = map.hints(Ctx::Work, true);
        for (key, _, code) in full.iter().chain(&short) {
            assert!(map.act(Ctx::Work, &KeyEvent::from(*code)).is_some(), "the hint {key} is not a key the board acts on");
        }
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        item.server = true;
        let menus = [task_choices(Some(&item), Some("bin/dev"), Some("bin/jobs")), backlog_choices(&[Source::Local]), ongoing_choices(&Ongoing { key: String::new(), title: String::new(), area: String::new(), task: None })];
        for choice in menus.iter().flatten() {
            let act = choice.act.expect("a built-in choice names its action");
            assert_eq!(choice.key, map.char(act), "{}'s letter is the table's", choice.label);
            assert!(keys::TABLE.iter().any(|row| row.act == act), "{} is offered but not in the table", choice.label);
        }
    }

    #[test]
    fn esc_and_q_step_back_and_close_only_an_overlay_at_the_top() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.mode = Mode::Help;
        app.handle_normal(key(KeyCode::Esc));
        app.handle_normal(key(KeyCode::Esc));
        assert!(!app.quit, "esc on the full board at the top does nothing");
        app.handle_normal(key(KeyCode::Char('q')));
        assert!(!app.quit && app.notice.as_ref().is_some_and(|(text, _)| text == "ctrl+c twice to quit"), "{:?}", app.notice);
        app.overlay = true;
        app.handle_normal(key(KeyCode::Char('q')));
        assert!(app.quit, "q at the top closes an overlay");
        app.quit = false;
        app.filter = Some((Column::Work, "zzz".to_string()));
        app.handle_normal(key(KeyCode::Esc));
        assert!(app.filter.is_none() && !app.quit, "esc clears the filter first");
        app.handle_normal(key(KeyCode::Esc));
        assert!(app.quit, "then closes the overlay");
    }

    #[test]
    fn ctrl_c_quits_only_when_pressed_twice_within_two_seconds() {
        let mut app = test_app(Vec::new());
        let control_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        press(&mut app, control_c);
        assert!(!app.quit && app.notice.as_ref().is_some_and(|(text, _)| text == "ctrl+c again to quit"));
        app.ctrl_c = Some(Instant::now() - Duration::from_secs(3));
        press(&mut app, control_c);
        assert!(!app.quit, "a press three seconds later starts again");
        press(&mut app, control_c);
        assert!(app.quit);
    }

    #[test]
    fn the_task_menu_and_the_letters_use_the_new_map() {
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 1);
        item.path = Some(PathBuf::from("/nowhere/wt1.app"));
        let mut app = test_app(vec![item]);
        app.handle_normal(key(KeyCode::Char('c')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::Message(..), .. }), "c messages the task's Claude");
        app.mode = Mode::Normal;
        app.handle_normal(key(KeyCode::Char('n')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::TaskName, .. }), "n starts a new task");
        app.mode = Mode::Normal;
        app.handle_normal(key(KeyCode::Char('s')));
        assert!(matches!(&app.mode, Mode::Message(text) if text.contains("no sign-off card")), "s is the card, not a new task");
        app.mode = Mode::Normal;
        app.column = Column::Backlog;
        app.handle_normal(key(KeyCode::Char('n')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogItem, .. }), "n in the Backlog column adds an item");
        app.mode = Mode::Normal;
        app.handle_normal(key(KeyCode::Char('i')));
        assert!(matches!(&app.mode, Mode::Message(text) if text.contains("No questions")), "i is the inbox");
    }

    #[test]
    fn three_opens_the_routines_page_its_tab_counts_the_runs_waiting_on_you_and_enter_goes_to_the_run() {
        let mut run = crate::status::tests::task(Some("idle"), Some("needs-you"), 0);
        (run.name, run.place) = ("cluster-health-20261019-0600".to_string(), "infra".to_string());
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0), run]);
        app.routines.tasks = app.tasks.clone();
        let routine = crate::routines::Routine {
            id: "cluster-health".to_string(), title: "Cluster health".to_string(), repo: "infra".to_string(), schedule: crate::routines::Schedule::Daily(420),
            prompt: "Check every node.".to_string(), mode: crate::routines::Mode::Report, push: false, enabled: true, profile: None, source: "/c/config.toml".to_string(),
        };
        let state = crate::routines::State { last_start: Some(1), last_task: Some("cluster-health-20261019-0600@infra".to_string()), last_outcome: Some(crate::routines::Outcome::Findings), ..Default::default() };
        app.routines.shown(vec![(routine, state)]);
        assert!(header(&app, 200).0.to_string().ends_with(" 1 Work  2 Areas  3 Routines ⚠1 "), "{}", header(&app, 200).0);
        press(&mut app, key(KeyCode::Char('3')));
        assert_eq!(app.page, Page::Routines);
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("▌⚠ Cluster health") && screen.contains("⏎ options · n new · g run now · e edit · p pause · s last report · ? keys"), "{screen}");
        assert!(header(&app, 70).0.to_string().ends_with(" 1  2  3 Routines ⚠1 "), "a narrow header names only the page it is on: {}", header(&app, 70).0);
        let (x, y) = find(&rows, "3 Routines")[0];
        assert_eq!(app.hit_at(x as u16, y as u16), Some(Hit::Tab(Page::Routines)));
        press(&mut app, key(KeyCode::Enter));
        press(&mut app, key(KeyCode::Enter));
        assert!(app.page == Page::Work && app.column == Column::Work, "Go to its task opens the Work page");
        assert_eq!(app.selected_task().map(|item| item.name).as_deref(), Some("cluster-health-20261019-0600"), "with the run selected");
        app.routines.tasks.clear();
        assert!(!header(&app, 200).0.to_string().contains('⚠'), "a closed run's findings wait for no one");
    }

    #[test]
    fn a_new_terminal_size_is_drawn_only_once_it_held_for_a_second() {
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut resize = Settle::default();
        assert!(resize.draw_now((100, 40), at(0)), "the first frame draws at once");
        assert!(resize.draw_now((100, 40), at(100)));
        assert!(!resize.draw_now((90, 40), at(200)), "a new size waits");
        assert!(!resize.draw_now((80, 40), at(700)), "a size still moving starts the wait again");
        assert!(!resize.draw_now((80, 40), at(1600)));
        assert!(resize.draw_now((80, 40), at(1700)), "a second at the same size draws it");
        assert!(resize.draw_now((80, 40), at(1800)));
        assert!(!resize.draw_now((120, 50), at(1900)));
        assert!(resize.draw_now((80, 40), at(2000)), "back to the drawn size before it settled draws at once, nothing moved");
        assert!(!resize.draw_now((120, 50), at(2100)), "and the next change waits its own second");
    }

    #[test]
    fn u_runs_the_update_in_the_background_and_shows_its_result() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        let (release, gate) = mpsc::channel::<()>();
        let gate = std::sync::Mutex::new(gate);
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = ran.clone();
        app.updater = std::sync::Arc::new(move || {
            gate.lock().unwrap().recv().map_err(|error| error.to_string())?;
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok("kelpie 0.18.1 → 0.18.2".to_string())
        });
        let (recheck, rechecked) = mpsc::channel();
        app.recheck = Some(recheck);
        app.updates = vec![("kelpie".to_string(), "0.18.2".to_string())];
        app.handle_normal(key(KeyCode::Char('U')));
        assert!(matches!(app.mode, Mode::Working { .. }), "the key returns while the update is still running");
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
        release.send(()).unwrap();
        for _ in 0..200 {
            app.take_working_result();
            if matches!(app.mode, Mode::Message(_)) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let Mode::Message(text) = &app.mode else { panic!("the result is shown") };
        assert_eq!(text, "kelpie 0.18.1 → 0.18.2");
        assert!(app.update_shown && app.updates.is_empty() && rechecked.try_recv().is_ok(), "the notice goes and a fresh check is asked for");
        assert!(help_lines(false).iter().any(|line| line.to_string().starts_with("U ")), "U is in the keys help");
    }
}
