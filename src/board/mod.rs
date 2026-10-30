mod bug_report;
mod card;
mod columns;
mod confirm;
mod detail;
mod draw;
mod editor;
mod header;
mod hub_window;
pub(crate) mod images;
mod inbox;
mod input;
pub(crate) mod keys_window;
mod link;
mod message_log;
mod mouse;
mod note_card;
mod notices;
mod options;
mod park;
mod paste;
mod push;
pub(crate) mod review;
pub(crate) mod review_data;
mod review_draw;
mod review_nvim;
#[cfg(test)]
mod review_tests;
mod space;
mod status_bar;
mod steps;
pub(crate) mod terminal;
#[cfg(test)]
mod test_support;
mod threads;
mod update_window;
pub(crate) mod widgets;

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::Text;
use ratatui::DefaultTerminal;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;

use crate::area_page::{self, Effect};
use crate::routine_page;
use crate::settings_page;
use crate::backlog::{self, Backlog, Entry};
use crate::config::{self, OngoingAt, Source};
use crate::fetch::Fetcher;
use crate::herdr;
use crate::keys::{self, Act, Ctx};
use crate::messages::Message;
use crate::notice::{Notice, Tone};
use crate::permit;
use crate::peers;
use crate::repo::name_of;
use crate::evidence;
use crate::report;
use crate::sh::Result;
use crate::status::{Checkout, Group, Item};
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
    Settings,
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
    ConfirmDismiss { path: PathBuf, name: String, nudged: bool },
    ConfirmQueued { name: String, about: String, now: bool, nudged: bool },
    ConfirmClaimed { entry: Entry, about: String, machine: String, nudged: bool },
    RestartLead(usize),
    Park { address: String, name: String, index: usize, date: Option<String> },
    Filter { buffer: String },
    Card { path: PathBuf, name: String, scroll: u16, image: usize },
    Fullscreen { path: PathBuf, name: String, scroll: u16, index: usize },
    ConfirmPush { path: PathBuf, commits: Vec<tasks::Commit>, selected: usize, card: Option<(report::Card, bool)>, tests: Option<evidence::Shown>, marks: push::Marks, scroll: push::Scroll },
    EditMessage(Box<MessageEditor>),
    Working { label: String, result: Receiver<Result<String>> },
    Questions(Box<QuestionWindow>),
    Report { id: String, scroll: u16, read: bool, nudged: bool },
    NoteCard { key: String, scroll: u16, nudged: bool },
    Help(keys_window::Scroll),
    Hub(Box<hub_window::Window>),
    Update,
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
    Offer(usize),
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
    settings: settings_page::Page,
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
    answering: Option<(String, Receiver<Result<String>>)>,
    offers: Vec<bug_report::Offer>,
    offer_panel: bug_report::OfferPanel,
    reporter: bug_report::Reporter,
    fixed: Option<crate::issue_recheck::Fixed>,
    fixed_seen: u64,
    usage: Option<Usage>,
    ongoing: Vec<Ongoing>,
    groups: Vec<Group>,
    ongoing_at: OngoingAt,
    parked_open: bool,
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
    update: Option<update_window::Run>,
    progress_file: Option<PathBuf>,
    recheck: Option<Sender<()>>,
    stale: Vec<String>,
    stale_at: u64,
    restart_after_update: bool,
    filter: Option<(Column, String)>,
    ctrl_c: Option<Instant>,
    notice: Option<(String, Instant)>,
    lead_chat: Option<bool>,
    restart_auto: bool,
    lead_restarter: Updater,
    lead_fresh_starter: Updater,
    lead_checker: std::sync::Arc<dyn Fn() -> Result<()> + Send + Sync>,
    lead_context: Option<f64>,
    subject: Option<notices::Subject>,
    shown: Option<notices::Subject>,
    toast: Option<notices::Toast>,
    hub_paths: Option<hub_window::Paths>,
    hub_banner: Option<String>,
}

type Updater = std::sync::Arc<dyn Fn() -> Result<String> + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Debug)]
enum BacklogRow {
    Ongoing(usize),
    Entry(usize),
}

#[derive(Clone, Default)]
struct Ongoing {
    key: String,
    title: String,
    area: String,
    task: Option<String>,
    claim: Option<crate::vault::Claim>,
    status: String,
    summary: String,
    opening: String,
}

impl App {
    // Another machine's claim on the note: the start asks first, and true says it did.
    fn ask_if_claimed(&mut self, entry: &Entry) -> bool {
        let Some(claim) = self.ongoing.iter().find(|note| note.key == entry.reference).and_then(|note| note.claim.clone()) else { return false };
        let about = format!("kebbie {} since {}. Started here, the note's claim moves to this machine.", claim.shown(crate::backlog::today()), claim.since.get(..10).unwrap_or(&claim.since));
        self.mode = Mode::ConfirmClaimed { entry: entry.clone(), about, machine: claim.machine, nudged: false };
        true
    }

    fn selected_task(&self) -> Option<Item> {
        self.tasks.get(self.work_selected).filter(|_| self.shown(Column::Work, self.work_selected)).cloned()
    }

    // The Backlog column's rows top to bottom; backlog_selected and Hit::Entry index into this.
    fn backlog_rows(&self) -> Vec<BacklogRow> {
        let ongoing = (0..self.ongoing.len()).map(BacklogRow::Ongoing);
        let entries = (0..self.backlog.entries.len()).map(BacklogRow::Entry);
        match self.ongoing_at {
            OngoingAt::Top => ongoing.chain(entries).collect(),
            OngoingAt::Bottom => entries.chain(ongoing).collect(),
            OngoingAt::Hidden => entries.collect(),
        }
    }

    fn backlog_row(&self, index: usize) -> Option<BacklogRow> {
        self.backlog_rows().get(index).copied()
    }

    fn selected_entry(&self) -> Option<Entry> {
        let Some(BacklogRow::Entry(entry)) = self.backlog_row(self.backlog_selected) else { return None };
        self.backlog.entries.get(entry).filter(|_| self.shown(Column::Backlog, self.backlog_selected)).cloned()
    }

    fn selected_ongoing(&self) -> Option<Ongoing> {
        let Some(BacklogRow::Ongoing(note)) = self.backlog_row(self.backlog_selected) else { return None };
        self.ongoing.get(note).filter(|_| self.shown(Column::Backlog, self.backlog_selected)).cloned()
    }

    fn open_options(&mut self) {
        if self.column != Column::Backlog || !self.open_note_card() {
            self.mode = Mode::Options(0);
        }
    }

    fn set_tasks(&mut self, items: Vec<Item>) {
        let rank = |item: &Item| self.groups.iter().position(|group| *group == item.group()).unwrap_or(usize::MAX);
        let mut tasks: Vec<Item> = items.into_iter().filter(Item::is_task).collect();
        tasks.sort_by_key(rank);
        self.tasks = tasks;
    }

    fn filter_of(&self, column: Column) -> Option<&str> {
        self.filter.as_ref().filter(|(on, text)| *on == column && !text.is_empty()).map(|(_, text)| text.as_str())
    }

    fn shown(&self, column: Column, index: usize) -> bool {
        self.filtered(column, index) && !(column == Column::Work && self.parked_hidden(index))
    }

    fn filtered(&self, column: Column, index: usize) -> bool {
        let Some(filter) = self.filter_of(column) else { return true };
        let wanted = filter.to_lowercase();
        let has = |text: &str| text.to_lowercase().contains(&wanted);
        match column {
            Column::Work => self.tasks.get(index).is_some_and(|item| has(&item.name) || has(&item.place)),
            Column::Backlog => match self.backlog_row(index) {
                Some(BacklogRow::Ongoing(note)) => self.ongoing.get(note).is_some_and(|note| has(&note.title) || has(&note.area)),
                Some(BacklogRow::Entry(entry)) => self.backlog.entries.get(entry).is_some_and(|entry| has(&entry.title) || has(&entry.area)),
                None => false,
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
            Err(error) => self.show(Err(error)),
        }
    }

    fn backlog_len(&self) -> usize {
        self.backlog_rows().len()
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
        matches!(self.mode, Mode::Normal) && self.areas.is_normal() && self.routines.is_normal() && self.settings.is_normal()
    }

    fn open_settings(&mut self) {
        let repo = self.selected_task().and_then(|item| item.repo).or_else(|| self.checkouts.first().and_then(|checkout| self.repo_named(&checkout.name)));
        self.settings.open(repo);
        self.page = Page::Settings;
    }

    fn in_background(&mut self, label: String, step: impl FnOnce() -> Result<String> + Send + 'static) {
        let (sender, result) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(step());
        });
        self.mode = Mode::Working { label, result };
    }

    // A restart the user asks for always runs; only the lead's own state refuses it, before the question opens.
    fn restart_lead(&mut self) {
        match (self.lead_checker)() {
            Ok(()) => self.mode = Mode::RestartLead(0),
            Err(why) => {
                self.subject = Some(notices::Subject::plain("lead"));
                self.show(crate::lead_restart::outcome(Err(why)));
            }
        }
    }

    fn resume_lead(&mut self) {
        let restarter = self.lead_restarter.clone();
        self.subject = Some(notices::Subject::plain("lead"));
        let label = if self.lead_line().is_some() { "Restarting the lead on the new kebbie…" } else { "Restarting the lead…" };
        self.in_background(label.to_string(), move || restarter());
    }

    fn start_lead_fresh(&mut self) {
        let starter = self.lead_fresh_starter.clone();
        self.subject = Some(notices::Subject::plain("lead"));
        self.in_background("Starting the lead fresh…".to_string(), move || starter());
    }

    fn context_line_for(&self, width: usize) -> Option<String> {
        crate::lead_restart::context_line(self.lead_context, width)
    }

    fn lead_line(&self) -> Option<String> {
        self.lead_line_for(usize::MAX)
    }

    fn lead_line_for(&self, width: usize) -> Option<String> {
        crate::lead_restart::line(&self.stale, self.restart_auto, width)
    }

    fn take_working_result(&mut self) {
        let Mode::Working { result, .. } = &self.mode else { return };
        let Ok(result) = result.try_recv() else { return };
        self.show(result);
    }

    // A rebuilt binary takes over only once nothing is open, and never while an update this board runs is going or its summary is unread.
    fn may_restart(&self) -> bool {
        matches!(self.mode, Mode::Normal | Mode::Help(_) | Mode::Message(_)) && self.update.is_none() && self.areas.is_idle() && self.routines.is_idle() && self.settings.is_normal()
    }

    fn show(&mut self, result: Result<String>) {
        self.show_as(result, false);
    }

    // A notice that asks nothing of the user goes by as a toast unless it is kept; the rest open a window that stays until a key.
    fn show_as(&mut self, result: Result<String>, keep: bool) {
        let failed = result.is_err();
        let notice = Notice::read(&result.unwrap_or_else(|error| error), failed);
        let subject = self.subject.take();
        if notice.tone.passes() && notice.you.is_none() && !keep {
            self.toast = Some(notices::Toast { notice, subject, at: Instant::now() });
            self.mode = Mode::Normal;
        } else {
            self.shown = subject;
            self.mode = Mode::Message(notice.to_string());
        }
        let _ = self.refresh.send(());
    }

    fn about_path(&mut self, kind: &str, path: &Path) {
        let name = self.tasks.iter().find(|item| item.path.as_deref() == Some(path)).map(|item| item.name.clone());
        self.about(kind, &name.unwrap_or_else(|| name_of(path)));
    }

    // The task a step is about, by the name the step was started for, so its notice can name the task by its title.
    fn about(&mut self, kind: &str, name: &str) {
        self.subject = Some(match self.tasks.iter().find(|item| item.name == name) {
            Some(item) => notices::Subject::of(kind, item),
            None => notices::Subject { kind: kind.to_string(), title: name.to_string(), task: true },
        });
    }

    fn move_selection(&mut self, step: isize) {
        let column = self.column;
        let (selected, length) = match column {
            Column::Work => (self.work_selected, self.tasks.len()),
            Column::Backlog => (self.backlog_selected, self.backlog_len()),
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
        if self.page == Page::Settings {
            if self.at_rest() {
                match map.act(Ctx::Settings, &key) {
                    Some(Act::WorkPage) => return self.page = Page::Work,
                    Some(Act::AreasPage) => return self.page = Page::Areas,
                    Some(Act::RoutinesPage) => return self.page = Page::Routines,
                    _ => {}
                }
            }
            if let settings_page::Effect::Left = self.settings.handle(key) {
                self.page = Page::Work;
            }
            return;
        }
        if self.at_rest() {
            let ctx = match self.page {
                Page::Areas => Ctx::Areas,
                Page::Routines => Ctx::Routines,
                Page::Settings => Ctx::Settings,
                Page::Work => self.ctx(),
            };
            match map.act(ctx, &key) {
                Some(Act::Lead) => {
                    if let Err(error) = lead::focus_toggle() {
                        self.show(Err(error));
                    } else if self.lead_chat == Some(true) {
                        self.lead_chat = Some(false);
                    }
                    return;
                }
                Some(Act::HideChat) => return self.toggle_chat(),
                Some(Act::WorkPage) => return self.page = Page::Work,
                Some(Act::AreasPage) => return self.page = Page::Areas,
                Some(Act::RoutinesPage) => return self.page = Page::Routines,
                Some(Act::SettingsPage) => return self.open_settings(),
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
            Mode::Help(scroll) => if let Some(scroll) = scroll.key(&key) {
                self.mode = Mode::Help(scroll);
            },
            Mode::Message(text) => {
                let shown = self.shown.take();
                let offered = Notice::parse(&text).map(|notice| notices::offers(&notice, shown.as_ref())).unwrap_or_default();
                if act.is_some_and(|act| offered.contains(&act)) {
                    self.handle_normal(key);
                }
            }
            working @ Mode::Working { .. } => self.mode = working,
            Mode::Questions(window) => self.handle_questions(window, key),
            Mode::Hub(window) => self.handle_hub(window, key),
            Mode::Update => self.handle_update(key),
            Mode::Report { id, scroll, read, .. } => self.confirm_report(id, scroll, read, key, cancels),
            Mode::NoteCard { key: note, scroll, .. } => self.handle_note_card(note, scroll, key, cancels),
            Mode::ConfirmReset { path, name, .. } => self.confirm_reset(path, name, key, cancels),
            Mode::ConfirmDone { path, name, .. } => self.confirm_done(path, name, key, cancels),
            Mode::ConfirmDismiss { path, name, .. } => self.confirm_dismiss(path, name, key, cancels),
            Mode::ConfirmQueued { name, about, now, .. } => self.confirm_queued(name, about, now, key, cancels),
            Mode::ConfirmClaimed { entry, about, machine, .. } => self.confirm_claimed(entry, about, machine, key, cancels),
            Mode::ConfirmDrop { path, name, force, .. } => self.confirm_drop(path, name, force, key, cancels),
            Mode::RestartLead(index) => self.choose_restart(index, act, key),
            Mode::Park { address, name, index, date } => self.choose_park(address, name, index, date, act, key),
            Mode::Card { path, name, scroll, image } => self.handle_card(path, name, scroll, image, key),
            Mode::Fullscreen { path, name, scroll, index } => self.handle_fullscreen(path, name, scroll, index, key),
            Mode::Filter { buffer } => self.handle_filter(buffer, key),
            Mode::ConfirmPush { path, commits, selected, card, tests, marks, scroll } => self.handle_push(path, commits, selected, (card, tests, marks), scroll, key),
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
            Some(Act::Back) if self.page == Page::Work && self.column == Column::Work && self.parked_open && self.selected_task().is_some_and(|item| item.parked.is_some()) => self.show_parked(false),
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
        if self.offer_panel.focused {
            return self.handle_offers(key);
        }
        let ctx = self.ctx();
        let Some(act) = keys::map().act(ctx, &key) else { return };
        match act {
            Act::Messages => self.press_m(),
            Act::Hub => self.open_hub(),
            Act::Move => {
                let (down, before) = (!matches!(key.code, KeyCode::Up | KeyCode::Char('k')), self.work_selected);
                self.move_selection(if down { 1 } else { -1 });
                if down && ctx == Ctx::Work && self.work_selected == before && !self.offers.is_empty() {
                    self.focus_offers(0);
                }
            }
            Act::Switch => self.column = if self.column == Column::Work { Column::Backlog } else { Column::Work },
            Act::Back | Act::Close => self.at_top(Some(act)),
            Act::Help => self.mode = Mode::Help(Default::default()),
            Act::Inbox => self.open_questions(None),
            Act::Options if self.on_parked_line() => self.show_parked(true),
            Act::Options => {
                let has_selection = match self.column {
                    Column::Work => self.selected_task().is_some(),
                    Column::Backlog => self.backlog_len() > 0 && self.shown(Column::Backlog, self.backlog_selected),
                };
                if has_selection {
                    self.open_options();
                }
            }
            Act::Refresh => self.refresh_backlog(),
            Act::Away => {
                self.subject = Some(notices::Subject::plain("away"));
                self.show(permit::away(Some(if permit::away_since().is_some() { "off" } else { "on" })));
            }
            Act::Update => self.start_update(),
            Act::RestartLead => self.restart_lead(),
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
        let prompt = backlog::refresh_prompt(&self.sources).map_err(|why| Notice::new(Tone::NotNow, "Nothing for the lead to refresh").detail("why", why).to_string());
        let result = prompt.and_then(|prompt| match self.lead_pane() {
            Some(pane) => crate::typing::prompt(&pane, &prompt, "the lead").map(|_| Notice::new(Tone::Moving, "The lead refreshes the backlog").next("It updates here when it is saved.").to_string())
                .map_err(|refused| Notice::new(Tone::NotNow, format!("Nothing was typed: the lead {}", refused.reason())).next("Ask again once it is clear.").to_string()),
            None => Notice::new(Tone::NotNow, "No lead is running").next("prefix+space starts it.").err(),
        });
        self.subject = Some(notices::Subject::plain("backlog"));
        self.show(result);
    }

    fn act(&mut self, act: Act) {
        if !self.shown(self.column, if self.column == Column::Work { self.work_selected } else { self.backlog_selected }) {
            return;
        }
        if self.column == Column::Backlog && let Some(note) = self.selected_ongoing().filter(|_| act == Act::Go) {
            match &note.task {
                Some(task) => {
                    if let Some(index) = self.tasks.iter().position(|item| &peers::address(&item.name, &item.place) == task) {
                        self.column = Column::Work;
                        self.work_selected = index;
                    }
                }
                None => {
                    let entry = ongoing_entry(&note);
                    if !self.ask_if_claimed(&entry) {
                        let buffer = suggested_name(&entry.title);
                        self.mode = Mode::Input { purpose: Purpose::BacklogName(entry), label: "Resume as task: name".to_string(), buffer };
                    }
                }
            }
            return;
        }
        match self.column {
            Column::Backlog => match act {
                Act::Go => {
                    if let Some(entry) = self.selected_entry().filter(|entry| !self.ask_if_claimed(entry)) {
                        let buffer = suggested_name(&entry.title);
                        self.mode = Mode::Input { purpose: Purpose::BacklogName(entry), label: "Start as task: name".to_string(), buffer };
                    }
                }
                Act::StartAt => {
                    if let Some(entry) = self.selected_entry() {
                        let buffer = suggested_name(&entry.title);
                        self.mode = Mode::Input { purpose: Purpose::BacklogLaterName(entry), label: "Start later as task: name".to_string(), buffer };
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
                if self.on_parked_line() {
                    self.subject = Some(notices::Subject::plain("park"));
                    return self.show(park::not_on_the_line());
                }
                if act == Act::Park {
                    return self.press_park(item);
                }
                if let Some(note) = item.queued_note() {
                    let (go, drop) = (keys::key(Act::Go), keys::key(Act::Drop));
                    let note = if note.ends_with('.') { note } else { format!("{note}.") };
                    match act {
                        Act::Go => self.mode = Mode::ConfirmQueued { about: format!("{note} Started now, it begins {}.", item.start_now_means().unwrap_or_default()), name: item.name, now: true, nudged: false },
                        Act::StartAt => self.mode = Mode::Input { purpose: Purpose::StartAt(item.name), label: input::START_AT_LABEL.to_string(), buffer: String::new() },
                        Act::Drop => self.mode = Mode::ConfirmQueued { about: format!("{note} Dropped, {}.", item.drop_means().unwrap_or_default()), name: item.name, now: false, nudged: false },
                        _ => {
                            self.about("queued", &item.name);
                            self.show(Notice::new(Tone::NotNow, "It is queued and has no worktree yet").next(format!("{go} starts it now, {} sets when it starts, {drop} drops the wait.", keys::key(Act::StartAt))).ok());
                        }
                    }
                    return;
                }
                match act {
                    Act::Go if item.agent.is_none() => {
                        let Some(path) = item.path.clone() else { return };
                        self.about("restart", &item.name);
                        let (sender, result) = mpsc::channel();
                        let name = item.name.clone();
                        thread::spawn(move || {
                            let options = tasks::Start { prompt: None, focus: true, from: None, mode: None, profile: None, backlog: None, note: true, anyway: false };
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
                        _ => {
                            self.about("card", &item.name);
                            self.show(Notice::new(Tone::NotNow, "No sign-off card yet").next("Its Claude writes one when it reports ready.").ok());
                        }
                    },
                    Act::Stage if item.staged => {
                        if let Some(path) = item.path.clone() {
                            self.about("stage", &item.name);
                            self.in_background(format!("Taking {} off local main…", item.name), move || tasks::unstage(&path, None));
                        }
                    }
                    Act::Tidy | Act::Stage | Act::Done | Act::SendReview | Act::Rebase if busy(&item) => {
                        self.about(act_kind(act), &item.name);
                        self.show(Notice::new(Tone::NotNow, "Not yet: its Claude is still working").next("This opens once it reports.").ok());
                    }
                    Act::Rebase if item.push_waits.is_some() => {
                        self.about("rebase", &item.name);
                        self.show(Notice::new(Tone::NotNow, "Not rebased: it is in the push line").next("kebbie rebases it when its turn comes.").ok());
                    }
                    Act::Push if item.in_push_line() => {
                        if let Some(path) = item.path.clone() {
                            self.about("push", &item.name);
                            self.in_background(format!("Taking {} out of the push line…", item.name), move || tasks::cancel_push(&path, None));
                        }
                    }
                    Act::Push if item.awaiting_review() => {
                        self.about("push", &item.name);
                        self.show(Notice::new(Tone::NotNow, "Not yet: it is in review").next("Push opens once its reviewer passes its commits.").ok());
                    }
                    Act::Stage => {
                        if let Some(path) = item.path.clone() {
                            self.about("stage", &item.name);
                            self.in_background(format!("Putting {} on local main…", item.name), move || tasks::stage(&path, None, false));
                        }
                    }
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
                            self.about("rebase", &item.name);
                            self.in_background(format!("Rebasing {}…", item.name), move || tasks::rebase(&path));
                        }
                    }
                    Act::Follow => {
                        if let Some(repo) = &item.repo {
                            self.about("follow", &item.name);
                            self.show(tasks::toggle_follow(repo, &item.name));
                        }
                    }
                    Act::Permit if item.is_task() => {
                        self.about("permission", &item.name);
                        self.show(permit::toggle_task(&peers::address(&item.name, &item.place)));
                    }
                    Act::SendReview => {
                        if let Some(path) = &item.path {
                            self.about("review", &item.name);
                            self.show(if item.awaiting_review() { crate::review::withdraw(path, None) } else { crate::review::request(path, None) });
                        }
                    }
                    Act::DismissCi if item.ci.as_ref().is_some_and(|ci| !ci.running()) => {
                        if let Some(path) = item.path.clone() {
                            self.mode = Mode::ConfirmDismiss { path, name: item.name, nudged: false };
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
                                Ok(commits) if commits.is_empty() => {
                                    self.about("push", &item.name);
                                    self.show(Notice::new(Tone::NotNow, "Nothing to push: it has no commits yet").ok());
                                }
                                Ok(commits) => {
                                    let marks = push::Marks::read(&path, &commits, item.turn_under_way()).while_working(busy(&item));
                                    if busy(&item) && marks.through().is_none() {
                                        self.about("push", &item.name);
                                        self.show(Notice::new(Tone::NotNow, "Not yet: its Claude is still working").next("This opens once it reports, or once you accept its first step.").ok());
                                        return;
                                    }
                                    let (card, tests) = (report::card_for(&path), evidence::for_path(&path));
                                    self.mode = Mode::ConfirmPush { path, commits, selected: marks.start(), card, tests, marks, scroll: push::Scroll::default() };
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

const RESTART_STATE: &str = "KEBBIE_BOARD_STATE";
// Set only by a board that execs without leaving the alternate screen; a board older than that leaves it, so the new one must enter it again.
const SCREEN_KEPT: &str = "KEBBIE_BOARD_SCREEN_KEPT";
const RENEW_HOLD: Duration = Duration::from_secs(5);

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

    fn changed(&self) -> bool {
        let now = self.path.as_deref().and_then(built);
        now.is_some() && now != self.started
    }

    fn restart(&self, app: &App) -> String {
        let Some(path) = &self.path else { return "Cannot tell where kebbie's binary is".to_string() };
        let column = match (app.page, app.column) {
            (Page::Areas, _) => "areas",
            (Page::Routines, _) => "routines",
            (Page::Settings, _) => "settings",
            (_, Column::Backlog) => "backlog",
            _ => "work",
        };
        let task = app.selected_task().map(|item| item.name).unwrap_or_default();
        let state = format!("{column}\t{}\t{task}", app.backlog_selected);
        let _ = ratatui::crossterm::terminal::disable_raw_mode();
        let error = renewal(path, &state).exec();
        let _ = ratatui::crossterm::terminal::enable_raw_mode();
        format!("Restarting on the rebuilt kebbie failed: {error}")
    }
}

fn renewal(path: &Path, state: &str) -> Command {
    let mut command = Command::new(path);
    command.args(std::env::args_os().skip(1)).env(RESTART_STATE, state).env(SCREEN_KEPT, "1");
    command
}

impl App {
    fn new(sources: Vec<Source>, backlog: Backlog, overlay: bool, refresh: Sender<()>) -> Self {
        App {
            column: Column::Work, tasks: Vec::new(), checkouts: Vec::new(), backlog, work_selected: 0, backlog_selected: 0,
            mode: Mode::Normal, overlay, loaded: false, error: None, screens: HashMap::new(), refresh, quit: false, wanted_task: None, hits: Vec::new(), links: Vec::new(),
            page: Page::Work, areas: area_page::Page::new(sources.clone()), routines: routine_page::Page::new(config::path(), config::repos_dir()), settings: settings_page::Page::new(settings_page::Paths::current(), None), sources, fetch_failures: Vec::new(), away: false,
            messages: Vec::new(), fold: Fold::Line, seen: 0, fold_file: None, in_messages: false, message_back: 0, questions: Vec::new(), answering: None, offers: Vec::new(), offer_panel: Default::default(), reporter: Default::default(), fixed: None, fixed_seen: 0, usage: None, ongoing: Vec::new(), groups: Group::ALL.to_vec(), ongoing_at: OngoingAt::default(), parked_open: false,
            details: HashMap::new(), detail_requests: None, detail_results: None, asked_details: HashMap::new(),
            graphics: Graphics::Off, images: HashMap::new(), cell_guessed: false, viewer: None, decoded: HashMap::new(), image_requests: None, image_results: None, plans: HashMap::new(),
            updates: Vec::new(), updater: std::sync::Arc::new(|| update::run(&crate::doctor::Env::current().claude_dir, false, &crate::release::Request::Channel).map(|outcome| outcome.details.join("\n"))), update: None, progress_file: None, recheck: None, stale: Vec::new(), stale_at: 0, restart_after_update: false,
            filter: None, ctrl_c: None, notice: None, lead_chat: None, restart_auto: true, lead_restarter: std::sync::Arc::new(crate::lead_restart::restart_now), lead_fresh_starter: std::sync::Arc::new(crate::lead_restart::fresh_now), lead_checker: std::sync::Arc::new(crate::lead::restart_refusal), lead_context: None, subject: None, shown: None, toast: None, hub_paths: None, hub_banner: None,
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
    app.offer_panel.file = bug_report::panel_file();
    app.offer_panel.shown = bug_report::load_shown(app.offer_panel.file.as_deref());
    if let Ok(state) = std::env::var(RESTART_STATE) {
        let mut parts = state.splitn(3, '\t');
        match parts.next() {
            Some("backlog") => app.column = Column::Backlog,
            Some("areas") => app.page = Page::Areas,
            Some("routines") => app.page = Page::Routines,
            Some("settings") => app.open_settings(),
            _ => {}
        }
        app.backlog_selected = parts.next().and_then(|index| index.parse().ok()).unwrap_or(0);
        app.wanted_task = parts.next().filter(|name| !name.is_empty()).map(str::to_string);
    }
    let mut rebuild = Rebuild::new();
    let mut fetcher = Fetcher::new();
    let mut resize = Settle::default();
    // A size the board never drew at (a detached ssh client, a pane briefly resized) leaves the screen out of step with ratatui's last frame, so after any change the next frame writes every cell in place; clearing first would blink.
    let (mut seen, mut repaint) = (None, false);
    let renewed = std::env::var_os(RESTART_STATE).is_some() && std::env::var_os(SCREEN_KEPT).is_some();
    let mut terminal = if renewed { terminal::continue_terminal() } else { init_terminal() };
    let mut hold = renewed.then(|| Instant::now() + RENEW_HOLD);
    let graphics = GraphicsEnv::current();
    (app.graphics, app.cell_guessed) = graphics.startup();
    app.viewer = image_viewer(std::env::consts::OS, &|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    let result = (|| -> Result<()> {
        while !app.quit {
            if let Mode::EditMessage(editor) = &mut app.mode {
                editor.poll_draft();
            }
            app.poll_answer();
            app.take_working_result();
            app.poll_hub();
            app.poll_update();
            while let Ok(updates) = update_notices.try_recv() {
                app.updates = updates;
            }
            while let Ok((at, stale)) = restarter.try_recv() {
                (app.stale_at, app.stale) = (at, stale);
            }
            while let Ok(update) = snapshots.try_recv() {
                match update {
                    Ok(snapshot) => {
                        let previous = app.selected_task().map(|item| (item.name, item.place));
                        if let Ok((groups, ongoing_at)) = config::board_layout() {
                            (app.groups, app.ongoing_at) = (groups, ongoing_at);
                        }
                        app.set_tasks(snapshot.items);
                        app.areas.tasks = app.tasks.clone();
                        app.routines.tasks = app.tasks.clone();
                        app.checkouts = snapshot.checkouts;
                        let wanted = app.wanted_task.take();
                        let position = previous.and_then(|(name, place)| app.tasks.iter().position(|item| item.name == name && item.place == place))
                            .or_else(|| wanted.and_then(|name| app.tasks.iter().position(|item| item.name == name)));
                        app.work_selected = position.unwrap_or(app.work_selected.min(app.tasks.len().saturating_sub(1)));
                        app.settle_selection();
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
                        app.offers = snapshot.offers;
                        app.offers_changed();
                        app.fixed = snapshot.fixed.filter(|fixed| fixed.last > app.fixed_seen);
                        app.usage = snapshot.usage;
                        app.ongoing = snapshot.ongoing;
                        app.lead_chat = snapshot.lead_chat;
                        app.restart_auto = snapshot.restart_auto;
                        app.lead_context = snapshot.lead_context;
                        app.backlog_selected = app.backlog_selected.min(app.backlog_len().saturating_sub(1));
                        let newest = app.messages.last().map_or(0, |message| message.at);
                        if app.fold == Fold::Open && newest > app.seen {
                            app.seen = newest;
                            save_fold(app.fold_file.as_deref(), app.fold, app.seen);
                        }
                        app.follow_questions();
                        app.hub_banner = crate::lead::state_location().ok().and_then(|state| crate::hub::relay::load(&state).plan_inactive);
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
            app.areas.tick();
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
            if matches!(app.graphics, Graphics::Pending { wanted: true }) {
                (app.graphics, app.cell_guessed) = graphics.resolve(Picker::from_query_stdio().ok());
                app.images.clear();
            }
            let size = ratatui::crossterm::terminal::size().unwrap_or_default();
            repaint |= seen.replace(size).is_some_and(|before| before != size);
            if hold.is_some_and(|until| app.loaded || Instant::now() >= until) {
                hold = None;
            }
            if hold.is_none() && resize.draw_now(size, Instant::now()) {
                let every_cell = std::mem::take(&mut repaint);
                terminal.draw(|frame| {
                    draw(frame, &mut app);
                    if every_cell {
                        terminal::write_every_cell(frame.buffer_mut());
                    }
                }).map_err(|error| error.to_string())?;
            }
            if app.may_restart() && ((std::mem::take(&mut app.restart_after_update) && rebuild.changed()) || rebuild.ready()) {
                let error = rebuild.restart(&app);
                app.show_as(Err(error), true);
            }
            if event::poll(Duration::from_millis(250)).map_err(|error| error.to_string())? {
                match event::read().map_err(|error| error.to_string())? {
                    Event::Resize(..) => repaint = true,
                    Event::Key(key) if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('l') && key.modifiers.contains(KeyModifiers::CONTROL) => repaint = true,
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

fn act_kind(act: Act) -> &'static str {
    match act {
        Act::Tidy => "tidy",
        Act::Push => "push",
        Act::Stage => "stage",
        Act::Done => "done",
        Act::SendReview => "review",
        Act::Rebase => "rebase",
        _ => "kebbie",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
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
    use crate::board::keys_window::shown;
    use crate::board::status_bar::hint_lines;

    #[test]
    fn the_lead_board_alone_offers_h_and_says_chat_hidden_while_it_is() {
        let mut app = test_app(Vec::new());
        app.handle_key(KeyEvent::from(KeyCode::Char('h')), None);
        assert_eq!(app.notice.as_ref().map(|(text, _)| text.as_str()), Some("h hides the lead's chat only on the board in the lead Space"));
        assert!(!hint_lines(None, false).0.to_string().contains("h chat") && hint_lines(None, true).0.to_string().contains("h chat"));
        assert!(!shown(keys::map(), Ctx::Work, false).iter().any(|(key, _)| key == "h") && shown(keys::map(), Ctx::Work, true).iter().any(|(key, _)| key == "h"));
        assert!(!header(&app, 200).0.to_string().contains("chat hidden"));
        app.lead_chat = Some(true);
        assert!(header(&app, 200).0.to_string().contains("◧ chat hidden · h"));
        app.lead_chat = Some(false);
        assert!(!header(&app, 200).0.to_string().contains("chat hidden"));
    }

    #[test]
    fn every_key_in_the_help_the_hints_and_the_menus_is_one_the_table_acts_on() {
        let map = keys::map();
        let help: Vec<String> = shown(map, Ctx::Work, true).into_iter().map(|(key, _)| key).collect();
        for label in &help {
            assert!(label.split(' ').all(|part| keys::TABLE.iter().any(|row| map.label(row.act).split(' ').any(|own| own == part))), "{label} is in ? help but not in the table");
        }
        let (full, short) = map.hints(Ctx::Work, true);
        for (key, _, code) in full.iter().chain(&short) {
            assert!(map.act(Ctx::Work, &KeyEvent::from(*code)).is_some(), "the hint {key} is not a key the board acts on");
        }
        let mut item = crate::status::tests::task(Some("idle"), Some("ready"), 2);
        item.server = true;
        let menus = [task_choices(Some(&item), Some("bin/dev"), Some("bin/jobs")), backlog_choices(&[Source::Local]), ongoing_choices(&Ongoing { key: String::new(), title: String::new(), area: String::new(), task: None, claim: None, ..Ongoing::default() })];
        for choice in menus.iter().flatten() {
            let act = choice.act.expect("a built-in choice names its action");
            assert_eq!(choice.key, map.char(act), "{}'s letter is the table's", choice.label);
            assert!(keys::TABLE.iter().any(|row| row.act == act), "{} is offered but not in the table", choice.label);
        }
    }

    #[test]
    fn esc_and_q_step_back_and_close_only_an_overlay_at_the_top() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.mode = Mode::Help(Default::default());
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
        assert!(app.toast.as_ref().is_some_and(|toast| toast.notice.headline.contains("No sign-off card")), "s is the card, not a new task");
        app.mode = Mode::Normal;
        app.column = Column::Backlog;
        app.handle_normal(key(KeyCode::Char('n')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogItem, .. }), "n in the Backlog column adds an item");
        app.mode = Mode::Normal;
        app.handle_normal(key(KeyCode::Char('i')));
        assert!(app.toast.as_ref().is_some_and(|toast| toast.notice.headline.contains("No questions")), "i is the inbox");
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
        assert!(header(&app, 200).0.to_string().ends_with(" 1 Work  2 Areas  3 Routines ⚠1  , Settings "), "{}", header(&app, 200).0);
        press(&mut app, key(KeyCode::Char('3')));
        assert_eq!(app.page, Page::Routines);
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("▌⚠ Cluster health") && screen.contains("⏎ options · n new · g run now · e edit · p pause · s last report · ? keys"), "{screen}");
        assert!(header(&app, 70).0.to_string().ends_with(" 1  2  3 Routines ⚠1  , "), "a narrow header names only the page it is on: {}", header(&app, 70).0);
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
    fn a_comma_opens_the_settings_page_on_the_selected_tasks_repo_and_esc_goes_back() {
        let dir = crate::sh::test_dir("board-settings");
        std::fs::write(dir.join("config.toml"), "[task]\nsandbox = false\n").unwrap();
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.settings = settings_page::Page::new(settings_page::Paths { machine: dir.join("config.toml"), repos_dir: dir.join("repos"), claude_home: dir.join("claude"), kebbie: dir.join("kebbie") }, None);
        press(&mut app, key(KeyCode::Char(',')));
        assert_eq!(app.page, Page::Settings);
        let (rows, _) = render(&mut app, 140, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("Settings") && screen.contains("task.sandbox") && screen.contains("○ off"), "{screen}");
        assert!(screen.contains("↑ ↓ key · ← → section · ⏎ pick · d back to default · tab machine / repo · / search · esc board"), "the hint line names the page's keys: {screen}");
        assert!(header(&app, 200).0.to_string().ends_with(" , Settings "), "{}", header(&app, 200).0);
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Char('2')));
        assert_eq!(app.page, Page::Areas, "the page keys work from the settings page");
        press(&mut app, key(KeyCode::Char(',')));
        press(&mut app, key(KeyCode::Esc));
        assert_eq!(app.page, Page::Work, "esc goes back to the board");
        std::fs::remove_dir_all(dir).unwrap();
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
    fn while_the_lead_runs_an_older_kebbie_one_line_says_so_and_l_asks_how_to_restart_it() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.lead_checker = std::sync::Arc::new(|| Ok(()));
        let (rows, _) = render(&mut app, 170, 30);
        assert!(!rows.join("\n").contains("↻ The lead"), "nothing pending, no line");
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = asked.clone();
        app.lead_restarter = std::sync::Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("Restarted the lead with its current role and settings, resuming its conversation.".to_string())
        });
        app.lead_fresh_starter = std::sync::Arc::new(|| panic!("2 never starts fresh"));
        app.handle_normal(key(KeyCode::Char('L')));
        assert!(matches!(app.mode, Mode::RestartLead(0)), "L asks for a lead kebbie counts as current too");
        press(&mut app, key(KeyCode::Char('2')));
        assert!(matches!(app.mode, Mode::Working { .. }), "2 resumes it in the background");
        settle(&mut app);
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(app.notice.is_none() && app.toast.as_ref().is_some_and(|toast| toast.notice.headline.starts_with("Restarted the lead")), "the result goes by as a toast like the board's other results");
        app.toast = None;
        app.stale = vec!["lead: restarts when idle".to_string()];
        for width in [80, 120] {
            let (rows, _) = render(&mut app, width, 30);
            assert!(rows.iter().any(|row| row.trim_end().ends_with("↻ The lead restarts on the new kebbie when it's idle · L restarts it now")), "the whole line at {width} columns: {}", rows.join("\n"));
        }
        let (rows, _) = render(&mut app, 60, 30);
        assert!(rows.iter().any(|row| row.trim_end().ends_with("↻ Lead restarts when idle · L now")), "a narrow board takes the short wording, never a cut one: {}", rows.join("\n"));
        let (rows, _) = render(&mut app, 170, 30);
        let (x, y) = find(&rows, "↻ The lead restarts on the new kebbie")[0];
        assert!(rows[y + 1].trim().is_empty() && rows[y + 2].contains("WORK"), "one line, then the columns: {}", rows.join("\n"));
        assert_eq!(app.hit_at(x as u16, y as u16), Some(Hit::Key(KeyCode::Char('L'))), "a click on the line asks too");
        app.page = Page::Areas;
        let (rows, _) = render(&mut app, 170, 30);
        assert!(!rows.join("\n").contains("↻ The lead"), "the Work page only");
        assert!(shown(keys::map(), Ctx::Work, false).iter().any(|(key, _)| key == "L"), "L is in the keys help");
        assert!(!shown(keys::map(), Ctx::Work, false).iter().any(|(key, _)| key == "F"), "and no F");
    }

    fn settle(app: &mut App) {
        for _ in 0..200 {
            app.take_working_result();
            if !matches!(app.mode, Mode::Working { .. }) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn l_offers_a_new_conversation_first_or_resuming_and_picks_by_arrows_enter_number_or_click() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.lead_checker = std::sync::Arc::new(|| Ok(()));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        app.lead_fresh_starter = std::sync::Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("Restarted the lead with its current role and settings, with a new conversation; its earlier one stays on disk.".to_string())
        });
        let resumed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted_resume = resumed.clone();
        app.lead_restarter = std::sync::Arc::new(move || {
            counted_resume.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("Restarted the lead with its current role and settings, resuming its conversation.".to_string())
        });
        let count = |counter: &std::sync::Arc<std::sync::atomic::AtomicUsize>| counter.load(std::sync::atomic::Ordering::SeqCst);
        app.handle_normal(key(KeyCode::Char('L')));
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("Restart the lead") && screen.contains("▸ New conversation") && screen.contains("Resume this conversation") && screen.contains("from its role and the vault"), "{screen}");
        for (code, at) in [(KeyCode::Down, 1), (KeyCode::Down, 1), (KeyCode::Up, 0), (KeyCode::Char('j'), 1), (KeyCode::Char('k'), 0)] {
            press(&mut app, key(code));
            assert!(matches!(app.mode, Mode::RestartLead(index) if index == at), "{code:?} moves to {at}");
        }
        for other in ['n', 'f', 'r', 'y'] {
            press(&mut app, key(KeyCode::Char(other)));
            assert!(matches!(app.mode, Mode::RestartLead(0)), "{other} is no choice and keeps it open");
        }
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal) && count(&calls) + count(&resumed) == 0, "esc cancels and starts nothing");
        app.handle_normal(key(KeyCode::Char('L')));
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Working { .. }), "enter on the first row starts it fresh in the background");
        settle(&mut app);
        assert_eq!((count(&calls), count(&resumed)), (1, 0));
        assert!(app.toast.as_ref().is_some_and(|toast| toast.notice.headline.contains("with a new conversation")), "the result goes by as a toast");
        app.mode = Mode::Normal;
        app.handle_normal(key(KeyCode::Char('L')));
        press(&mut app, key(KeyCode::Char('1')));
        settle(&mut app);
        assert_eq!((count(&calls), count(&resumed)), (2, 0), "1 starts it fresh");
        app.mode = Mode::Normal;
        app.handle_normal(key(KeyCode::Char('L')));
        let (rows, _) = render(&mut app, 120, 30);
        let (x, y) = find(&rows, "Resume this conversation")[0];
        click(&mut app, x as u16, y as u16);
        settle(&mut app);
        assert_eq!((count(&calls), count(&resumed)), (2, 1), "a click on a row picks it");
    }

    #[test]
    fn a_refused_restart_goes_by_as_a_toast_before_any_question_and_can_be_pressed_again() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        let checked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = checked.clone();
        app.lead_checker = std::sync::Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("Not restarted: the lead's input box holds text you have not sent.".to_string())
        });
        app.lead_restarter = std::sync::Arc::new(|| panic!("a refused restart never runs"));
        app.lead_fresh_starter = std::sync::Arc::new(|| panic!("a refused restart never runs"));
        app.handle_normal(key(KeyCode::Char('L')));
        assert!(matches!(app.mode, Mode::Normal), "no question opens");
        assert!(app.toast.as_ref().is_some_and(|toast| toast.notice.headline == "Not restarted: the lead's input box holds text you have not sent." && toast.notice.tone == Tone::NotNow), "the plain reason as a toast, not an error");
        app.handle_normal(key(KeyCode::Char('L')));
        assert_eq!(checked.load(std::sync::atomic::Ordering::SeqCst), 2, "a refused restart can be pressed again");
    }

    #[test]
    fn a_nearly_full_lead_context_shows_one_line_that_offers_l_and_nothing_below_the_mark() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.lead_checker = std::sync::Arc::new(|| Ok(()));
        for used in [None, Some(60.0), Some(84.9)] {
            app.lead_context = used;
            let (rows, _) = render(&mut app, 170, 30);
            assert!(!rows.join("\n").contains("context is"), "{used:?} shows no line");
        }
        app.lead_context = Some(91.2);
        let (rows, _) = render(&mut app, 170, 30);
        let (x, y) = find(&rows, "The lead's context is 91% full · L to restart it fresh")[0];
        assert!(rows[y + 1].trim().is_empty() && rows[y + 2].contains("WORK"), "one line, then the columns: {}", rows.join("\n"));
        assert_eq!(app.hit_at(x as u16, y as u16), Some(Hit::Key(KeyCode::Char('L'))), "a click on the line asks as L does");
        app.page = Page::Areas;
        let (rows, _) = render(&mut app, 170, 30);
        assert!(!rows.join("\n").contains("context is"), "the Work page only");
    }

    #[test]
    fn a_board_renewing_onto_a_rebuilt_binary_tells_it_the_alternate_screen_is_still_on() {
        let command = renewal(Path::new("/bin/kebbie"), "work\t0\ttask");
        let envs: HashMap<_, _> = command.get_envs().collect();
        assert_eq!(envs.get(std::ffi::OsStr::new(SCREEN_KEPT)), Some(&Some(std::ffi::OsStr::new("1"))), "without it the new board enters the alternate screen again and herdr erases the pane");
        assert_eq!(envs.get(std::ffi::OsStr::new(RESTART_STATE)), Some(&Some(std::ffi::OsStr::new("work\t0\ttask"))));
    }
}
