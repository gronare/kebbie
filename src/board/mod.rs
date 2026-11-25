mod asks;
mod background;
mod beside;
mod bug_report;
mod card;
mod clock;
mod columns;
mod comments;
mod confirm;
mod detail;
pub(crate) use detail::{link_label, task_urls};
mod draw;
mod editor;
mod header;
mod hub_window;
pub(crate) mod images;
mod inbox;
mod input;
mod item_card;
mod jobs;
pub(crate) mod keys_window;
mod link;
mod message_log;
mod mouse;
mod next_window;
mod note_card;
mod note_window;
mod notices;
mod options;
mod park;
mod paste;
mod pulse;
mod push;
pub(crate) mod review;
pub(crate) mod review_data;
mod review_draw;
mod review_nvim;
#[cfg(test)]
mod review_tests;
mod send_review;
mod space;
mod status_bar;
mod steps;
mod subtasks;
mod new_task;
pub(crate) mod terminal;
#[cfg(test)]
mod test_support;
mod threads;
mod agent_updates;
mod update_window;
pub(crate) mod widgets;

pub(crate) use input::{StartWith, start_in_background};

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, Sender};
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
use crate::report_wire::BugReport;
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
pub(crate) use columns::suggested_name;
pub(crate) use new_task::{start_described, task_name};
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
    Backlog,
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
    SubtaskOptions(usize),
    ReviewMenu(Box<send_review::ReviewMenu>),
    Input { purpose: Purpose, label: String, buffer: String },
    Message(String),
    ConfirmDrop { path: PathBuf, name: String, force: bool, nudged: bool },
    ConfirmReset { path: PathBuf, name: String, nudged: bool },
    ConfirmDone { path: PathBuf, name: String, nudged: bool },
    ConfirmDismiss { path: PathBuf, name: String, nudged: bool },
    ConfirmQueued { name: String, about: String, now: bool, nudged: bool },
    ConfirmClaimed { entry: Entry, about: String, machine: String, nudged: bool },
    RestartLead(usize),
    FixedOptions(usize),
    Park { address: String, name: String, index: usize, date: Option<String> },
    Filter { buffer: String },
    Card { path: PathBuf, name: String, scroll: u16, image: usize },
    Fullscreen { path: PathBuf, name: String, scroll: u16, index: usize },
    ConfirmPush { path: PathBuf, commits: Vec<tasks::Commit>, selected: usize, card: Option<(report::Card, bool)>, tests: Option<evidence::Shown>, marks: push::Marks, scroll: push::Scroll },
    EditMessage(Box<MessageEditor>),
    PushOpening { path: PathBuf, label: String, read: Receiver<push::Opening> },
    CheckingLead(Receiver<Result<()>>),
    Questions(Box<QuestionWindow>),
    Report { id: String, shown: Box<BugReport>, scroll: u16, read: bool, nudged: bool },
    ReportAll { shown: Vec<bug_report::Offer>, scroll: u16, read: bool, nudged: bool },
    NoteCard { key: String, scroll: u16, nudged: bool },
    TaskNote { key: String, name: String, scroll: u16 },
    ItemCard(Box<item_card::ItemCard>),
    Comment { id: u64, scroll: u16, nudged: bool },
    Help(keys_window::Scroll),
    Hub(Box<hub_window::Window>),
    Next(next_window::Window),
    PushAll { order: Vec<next_window::Push>, choice: usize, back: next_window::Window },
    Device { id: String, back: next_window::Window },
    Update,
    AgentUpdates,
    SubtaskCard(Box<subtasks::View>),
    NewTask(Box<new_task::NewTask>),
    Subtasks,
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
    Fixed,
    Comment(usize),
    Ask(usize),
    Beside,
    Tasks,
    Key(KeyCode),
}

struct App {
    column: Column,
    tasks: Vec<Item>,
    checkouts: Vec<Checkout>,
    backlog: Backlog,
    found: Vec<crate::found::Item>,
    work_selected: usize,
    backlog_selected: usize,
    mode: Mode,
    overlay: bool,
    loaded: bool,
    error: Option<String>,
    screens: crate::behind::Reader<String>,
    refresh: Sender<()>,
    quit: bool,
    leaving: bool,
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
    after_message: Option<(String, Mode)>,
    questions: Vec<Message>,
    answering: Option<(String, Receiver<Result<String>>)>,
    sending: Option<(Vec<String>, Receiver<Result<String>>)>,
    offers: Vec<bug_report::Offer>,
    offer_panel: bug_report::OfferPanel,
    reporter: bug_report::Reporter,
    fixed: Option<crate::issue_recheck::Fixed>,
    fixed_seen: u64,
    usage: Option<Usage>,
    ongoing: Vec<Ongoing>,
    groups: Vec<Group>,
    ongoing_at: OngoingAt,
    next_question: bool,
    parked_open: bool,
    details: HashMap<PathBuf, std::rc::Rc<Detail>>,
    detail_requests: Option<Sender<DetailRequest>>,
    detail_results: Option<Receiver<(PathBuf, Detail)>>,
    asked_details: HashMap<PathBuf, Instant>,
    graphics: Graphics,
    images: HashMap<(PathBuf, u16, u16), Option<Protocol>>,
    cell_guessed: bool,
    viewer: Option<&'static str>,
    focus_tint: ratatui::style::Color,
    decoded: HashMap<PathBuf, Option<image::DynamicImage>>,
    image_requests: Option<Sender<PathBuf>>,
    image_results: Option<Receiver<Decoded>>,
    plans: HashMap<String, Text<'static>>,
    plan_reads: crate::behind::Reader<Option<String>>,
    card_read: Option<Receiver<Option<item_card::ItemCard>>>,
    card_reader: bool,
    updates: Vec<(String, String)>,
    updater: Updater,
    update: Option<update_window::Run>,
    progress_file: Option<PathBuf>,
    recheck: Option<Sender<()>>,
    stale: Vec<String>,
    stale_at: u64,
    restart_after_update: bool,
    agent_offer: Option<agent_updates::Offer>,
    agent_mode: std::sync::Arc<dyn Fn() -> config::AgentUpdates + Send + Sync>,
    installer: agent_updates::Install,
    filter: Option<(Column, String)>,
    ctrl_c: Option<Instant>,
    go_again: Option<(String, Instant)>,
    notice: Option<(String, Instant)>,
    lead_chat: Option<bool>,
    restart_auto: bool,
    disk_line: Option<String>,
    lead_restarter: Updater,
    lead_fresh_starter: Updater,
    lead_checker: std::sync::Arc<dyn Fn() -> Result<()> + Send + Sync>,
    lead_restarted: std::sync::Arc<std::sync::atomic::AtomicU64>,
    lead_context: Option<f64>,
    lead_context_limit: u64,
    restart_why: String,
    devices: Vec<crate::hub::devices::Request>,
    subject: Option<notices::Subject>,
    shown: Option<notices::Subject>,
    toast: Option<notices::Toast>,
    hub_paths: Option<hub_window::Paths>,
    hub_banner: Option<String>,
    comments: Vec<crate::comments::Comment>,
    comment_panel: comments::CommentPanel,
    ask_focus: Option<usize>,
    comment_dir: Option<PathBuf>,
    copier: comments::Copier,
    reopener: comments::Reopener,
    pusher: jobs::Pusher,
    jobs: Vec<jobs::Job>,
    outcomes: Vec<(Mode, Option<notices::Subject>)>,
    dropper: jobs::Dropper,
    lead_teller: LeadTeller,
    telling_drafts: std::sync::Arc<std::sync::atomic::AtomicBool>,
    subtask_row: Option<(String, String)>,
    subtask_io: subtasks::Io,
    reads: subtasks::Reads,
    loads: subtasks::Loads,
    space_repos: crate::behind::Reader<Vec<PathBuf>>,
    new_io: new_task::Io,
    beside: beside::Beside,
}

type Updater = std::sync::Arc<dyn Fn() -> Result<String> + Send + Sync>;

type LeadTeller = std::sync::Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Debug)]
enum BacklogRow {
    Ongoing(usize),
    Found(usize),
    Entry(usize),
}

#[derive(Clone, Default)]
struct Ongoing {
    key: String,
    title: String,
    area: String,
    task: Option<String>,
    also: Vec<String>,
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

    // The backlog items whose questions block their start and wait on the user, newest first, as the hub lists them under Needs you.
    pub(super) fn backlog_asks(&self) -> Vec<usize> {
        let days = |entry: &Entry| entry.age.split_whitespace().next().and_then(|days| days.parse::<u64>().ok()).unwrap_or(u64::MAX);
        let mut asks: Vec<usize> = (0..self.backlog.entries.len()).filter(|index| self.backlog.entries[*index].shape.waiting > 0).collect();
        asks.sort_by_key(|index| days(&self.backlog.entries[*index]));
        asks
    }

    // The Backlog column's rows top to bottom; backlog_selected and Hit::Entry index into this.
    fn backlog_rows(&self) -> Vec<BacklogRow> {
        let asks = self.backlog_asks();
        let needs = asks.iter().copied().map(BacklogRow::Entry);
        let ongoing = (0..self.ongoing.len()).map(BacklogRow::Ongoing);
        let entries = (0..self.found.len()).map(BacklogRow::Found).chain((0..self.backlog.entries.len()).filter(|index| !asks.contains(index)).map(BacklogRow::Entry));
        match self.ongoing_at {
            OngoingAt::Top => needs.chain(ongoing).chain(entries).collect(),
            OngoingAt::Bottom => needs.chain(entries).chain(ongoing).collect(),
            OngoingAt::Hidden => needs.chain(entries).collect(),
        }
    }

    fn backlog_row(&self, index: usize) -> Option<BacklogRow> {
        self.backlog_rows().get(index).copied()
    }

    fn selected_entry(&self) -> Option<Entry> {
        let Some(BacklogRow::Entry(entry)) = self.backlog_row(self.backlog_selected) else { return None };
        self.backlog.entries.get(entry).filter(|_| self.shown(Column::Backlog, self.backlog_selected)).cloned()
    }

    fn selected_found(&self) -> Option<crate::found::Item> {
        let Some(BacklogRow::Found(found)) = self.backlog_row(self.backlog_selected) else { return None };
        self.found.get(found).filter(|_| self.shown(Column::Backlog, self.backlog_selected)).cloned()
    }

    // A running task the item suits: one that found it, else one in its repo.
    fn found_target(&self, item: &crate::found::Item) -> Option<String> {
        let running = |item_task: &Item| item_task.agent.is_some() && item_task.queued.is_none();
        self.tasks.iter().filter(|task| running(task)).map(|task| peers::address(&task.name, &task.place)).find(|address| item.sources.contains(address))
            .or_else(|| self.tasks.iter().filter(|task| running(task) && task.place == item.repo).map(|task| peers::address(&task.name, &task.place)).next())
    }

    fn act_on_found(&mut self, act: Act, item: crate::found::Item) {
        match act {
            Act::Go => self.mode = Mode::Input { purpose: Purpose::FoundName(item.id.clone()), label: "Start as task: name".to_string(), buffer: item.name() },
            Act::Chat => {
                let buffer = self.found_target(&item).unwrap_or_default();
                self.mode = Mode::Input { purpose: Purpose::FoundAdd(item.id), label: "Add to running task: name".to_string(), buffer };
            }
            Act::Park => {
                let result = crate::lead::state_dir().and_then(|state| crate::found::keep_in(&state, &crate::permit::Caller::User, &item.id));
                self.subject = Some(notices::Subject::plain("backlog"));
                if result.is_ok() && let Some(kept) = self.found.iter_mut().find(|found| found.id == item.id) {
                    kept.state = crate::found::State::Kept;
                }
                self.show(result);
            }
            Act::Drop => {
                let result = crate::lead::state_dir().and_then(|state| crate::found::dismiss_in(&state, &crate::permit::Caller::User, &item.id));
                self.subject = Some(notices::Subject::plain("backlog"));
                if result.is_ok() {
                    self.found.retain(|found| found.id != item.id);
                    self.backlog_selected = self.backlog_selected.min(self.backlog_len().saturating_sub(1));
                }
                self.show(result);
            }
            Act::Refresh => self.refresh_backlog(),
            _ => {}
        }
    }

    // A refresh that could not read the vault leaves the notes as they were, or every task's note line and the Ongoing section blink out for it.
    fn take_ongoing(&mut self, read: Option<Vec<Ongoing>>) {
        if let Some(ongoing) = read {
            self.ongoing = ongoing;
        }
    }

    fn selected_ongoing(&self) -> Option<Ongoing> {
        let Some(BacklogRow::Ongoing(note)) = self.backlog_row(self.backlog_selected) else { return None };
        self.ongoing.get(note).filter(|_| self.shown(Column::Backlog, self.backlog_selected)).cloned()
    }

    // In the Backlog column ⏎ opens a card: an ongoing note's, or the item's; an item to write, or a row with no card, has its options.
    fn open_options(&mut self) {
        if self.column != Column::Backlog || self.open_note_card() {
            if self.column != Column::Backlog {
                self.mode = Mode::Options(0);
            }
            return;
        }
        let entry = self.selected_entry().map(|entry| if entry.shape.into.is_empty() { entry } else { self.merged_note(&entry) });
        if entry.as_ref().is_some_and(|entry| crate::backlog_file::writable(entry, crate::messages::now())) {
            self.mode = Mode::Options(0);
            return;
        }
        if !entry.is_some_and(|entry| self.open_item_card(entry)) {
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
                Some(BacklogRow::Found(found)) => self.found.get(found).is_some_and(|item| has(&item.line) || has(&item.repo)),
                None => false,
            },
        }
    }

    // The Backlog page is the board's Backlog column on its own across the width, so self.page stays Work and the column says which of the two shows.
    fn go_page(&mut self, page: Page) {
        self.beside.focused = false;
        match page {
            Page::Work | Page::Backlog => {
                (self.page, self.column) = (Page::Work, if page == Page::Backlog { Column::Backlog } else { Column::Work });
                if page == Page::Backlog {
                    (self.in_messages, self.offer_panel.focused, self.comment_panel.focused, self.ask_focus) = (false, false, false, None);
                }
            }
            other => self.page = other,
        }
    }

    // The tab that is lit.
    fn shown_page(&self) -> Page {
        if self.page == Page::Work && self.column == Column::Backlog { Page::Backlog } else { self.page }
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
        let label = format!("{} the lead's chat", if hidden { "Showing" } else { "Hiding" });
        self.in_background(label, || lead::toggle_chat().map(|_| String::new())).then(move |app| app.lead_chat = Some(!hidden));
    }

    fn backlog_len(&self) -> usize {
        self.backlog_rows().len()
    }

    fn repo_named(&mut self, name: &str) -> Option<PathBuf> {
        self.tasks.iter().filter_map(|item| item.repo.clone()).find(|repo| name_of(repo) == name)
            .or_else(|| self.space_repos(Duration::from_secs(60)).into_iter().find(|repo| name_of(repo) == name))
    }

    // The repos of herdr's Spaces, read on a thread; until the first read answers there are none.
    fn space_repos(&mut self, wait: Duration) -> Vec<PathBuf> {
        self.space_repos.get("spaces", wait, || herdr::workspaces().map(|workspaces| workspaces.iter().filter_map(|workspace| workspace["worktree"]["repo_root"].as_str()).map(PathBuf::from).collect()).unwrap_or_default()).unwrap_or_default()
    }

    fn any_repo(&mut self) -> PathBuf {
        let first = self.checkouts.first().map(|checkout| checkout.name.clone());
        self.selected_task().and_then(|item| item.repo)
            .or_else(|| first.and_then(|name| self.repo_named(&name)))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    }

    // Pages switch only when nothing is open on either.
    fn at_rest(&self) -> bool {
        matches!(self.mode, Mode::Normal) && self.areas.is_normal() && self.routines.is_normal() && self.settings.is_normal()
    }

    pub(super) fn after_settings(&mut self, effect: settings_page::Effect, terminal: Option<&mut DefaultTerminal>) {
        match effect {
            settings_page::Effect::Left => self.page = Page::Work,
            settings_page::Effect::Open(file) => match terminal {
                Some(terminal) => {
                    let status = crate::board::terminal::outside(terminal, || settings_page::open_in_editor(&file));
                    self.settings.opened(&file, status);
                }
                None => self.settings.opened(&file, Err("no terminal to open it in".to_string())),
            },
            settings_page::Effect::Show(file) => match terminal {
                Some(terminal) => {
                    let status = crate::board::terminal::outside(terminal, || settings_page::show_in_pager(&file));
                    self.settings.shown(status);
                }
                None => self.settings.shown(Err("no terminal to show it in".to_string())),
            },
            settings_page::Effect::Run(command) => match terminal {
                Some(terminal) => {
                    let result = crate::board::terminal::outside(terminal, || settings_page::run(&command));
                    self.settings.ran(&command, result);
                }
                None => self.settings.ran(&command, Err("no terminal to run it in".to_string())),
            },
            settings_page::Effect::Handled => {}
        }
    }

    fn open_settings(&mut self) {
        let first = self.checkouts.first().map(|checkout| checkout.name.clone());
        let repo = self.selected_task().and_then(|item| item.repo).or_else(|| first.and_then(|name| self.repo_named(&name)));
        self.settings.open(repo);
        self.page = Page::Settings;
    }

    // A restart the user asks for always runs; only the lead's own state, read on a thread, or a restart already under way, refuses it before the question opens.
    fn restart_lead(&mut self) {
        if let Some(doing) = self.doing(LEAD_JOB, None) {
            self.subject = Some(notices::Subject::plain("lead"));
            return self.already(doing);
        }
        let (checker, (sender, read)) = (self.lead_checker.clone(), std::sync::mpsc::channel());
        std::thread::spawn(move || {
            let _ = sender.send(checker());
        });
        self.mode = Mode::CheckingLead(read);
    }

    pub(super) fn poll_lead_check(&mut self) {
        let Mode::CheckingLead(read) = &self.mode else { return };
        let checked = match read.try_recv() {
            Ok(checked) => checked,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the check stopped without an answer".to_string()),
        };
        self.mode = Mode::Normal;
        match checked {
            Ok(()) => {
                let (index, why) = crate::lead_restart::preset(&self.tasks, &self.messages);
                self.restart_why = why;
                self.mode = Mode::RestartLead(index);
            }
            Err(why) => {
                self.subject = Some(notices::Subject::plain("lead"));
                self.show(crate::lead_restart::outcome(Err(why)));
            }
        }
    }

    fn resume_lead(&mut self) {
        let restarter = self.lead_restarter.clone();
        let restarted = self.lead_restarted.clone();
        self.subject = Some(notices::Subject::plain("lead"));
        let label = if self.lead_line().is_some() { "Restarting the lead on the new kebbie…" } else { "Restarting the lead…" };
        self.task_in_background(LEAD_JOB, None, "restarting", label.to_string(), move || noted_restart(&restarted, restarter()));
    }

    fn start_lead_fresh(&mut self) {
        let starter = self.lead_fresh_starter.clone();
        let restarted = self.lead_restarted.clone();
        self.subject = Some(notices::Subject::plain("lead"));
        self.task_in_background(LEAD_JOB, None, "restarting", "Starting the lead fresh…".to_string(), move || noted_restart(&restarted, starter()));
    }

    pub(super) fn take_stale(&mut self, at: u64, stale: Vec<String>) {
        (self.stale_at, self.stale) = (at, stale);
        self.drop_restarted_lead();
    }

    // A lead restarted from here runs the installed kebbie, so a stale reading from a round that began before that restart no longer holds for it.
    fn drop_restarted_lead(&mut self) {
        let restarted = self.lead_restarted.load(std::sync::atomic::Ordering::SeqCst);
        if restarted > 0 && self.stale_at <= restarted {
            self.stale.retain(|line| !line.strip_prefix(crate::status::LEAD_LABEL).is_some_and(|rest| rest.starts_with(": ")));
        }
    }

    fn context_line_for(&self, width: usize) -> Option<String> {
        crate::lead_restart::context_line(self.lead_context, self.lead_context_limit, width)
    }

    fn lead_line(&self) -> Option<String> {
        self.lead_line_for(usize::MAX)
    }

    fn lead_line_for(&self, width: usize) -> Option<String> {
        crate::lead_restart::line(&self.stale, self.restart_auto, width)
    }

    // A rebuilt binary takes over only once nothing is open, and never while an update this board runs is going or its summary is unread, nor while a job's outcome waits to be shown.
    fn may_restart(&self) -> bool {
        matches!(self.mode, Mode::Normal | Mode::Help(_) | Mode::Message(_)) && self.jobs.is_empty() && self.outcomes.is_empty() && self.update.is_none() && self.agent_offer.is_none() && self.areas.is_idle() && self.routines.is_idle() && self.settings.is_normal()
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

    // A window that opened over another goes back to it once read.
    fn back_from_message(&mut self, text: &str) {
        if let Some((shown, back)) = self.after_message.take()
            && shown == text
        {
            self.mode = back;
        }
    }

    fn about_path(&mut self, kind: &str, path: &Path) {
        let name = self.name_at(path);
        self.about(kind, &name);
    }

    fn name_at(&self, path: &Path) -> String {
        self.tasks.iter().find(|item| item.path.as_deref() == Some(path)).map_or_else(|| name_of(path), |item| item.name.clone())
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
                self.say(format!("{} again to quit{}", map.label(Act::Quit), self.jobs_stop_line()));
            }
            return;
        }
        self.notice = None;
        if self.page == Page::Settings {
            if self.at_rest() {
                match map.act(Ctx::Settings, &key) {
                    Some(Act::WorkPage) => return self.go_page(Page::Work),
                    Some(Act::BacklogPage) => return self.go_page(Page::Backlog),
                    Some(Act::AreasPage) => return self.go_page(Page::Areas),
                    Some(Act::RoutinesPage) => return self.go_page(Page::Routines),
                    _ => {}
                }
            }
            let effect = self.settings.handle(key);
            return self.after_settings(effect, terminal);
        }
        if self.at_rest() {
            let ctx = match self.page {
                Page::Areas => Ctx::Areas,
                Page::Routines => Ctx::Routines,
                Page::Settings => Ctx::Settings,
                Page::Work | Page::Backlog => self.ctx(),
            };
            match map.act(ctx, &key) {
                Some(Act::Lead) => {
                    self.in_background("Switching to the lead".to_string(), || lead::focus_toggle().map(|_| String::new())).then(|app| {
                        if app.lead_chat == Some(true) {
                            app.lead_chat = Some(false);
                        }
                    });
                    return;
                }
                Some(Act::HideChat) => return self.toggle_chat(),
                Some(Act::WorkPage) => return self.go_page(Page::Work),
                Some(Act::BacklogPage) => return self.go_page(Page::Backlog),
                Some(Act::AreasPage) => return self.go_page(Page::Areas),
                Some(Act::RoutinesPage) => return self.go_page(Page::Routines),
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
            match self.areas.handle(key) {
                Effect::Left => self.close_overlay(false),
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
            Mode::ReviewMenu(menu) => self.handle_review_menu(menu, act, key, terminal),
            Mode::Help(scroll) => if let Some(scroll) = scroll.key(&key) {
                self.mode = Mode::Help(scroll);
            },
            Mode::Message(text) => {
                let shown = self.shown.take();
                let offered = Notice::parse(&text).map(|notice| notices::offers(&notice, shown.as_ref())).unwrap_or_default();
                if act.is_some_and(|act| offered.contains(&act)) {
                    self.after_message = None;
                    self.handle_normal(key);
                } else {
                    self.back_from_message(&text);
                }
            }
            Mode::Questions(window) => self.handle_questions(window, key),
            Mode::Hub(window) => self.handle_hub(window, key),
            Mode::Next(window) => self.handle_next(window, key, act),
            Mode::PushAll { order, choice, back } => self.handle_push_all(order, choice, back, key, act),
            Mode::Device { id, back } => self.handle_device(id, back, key, act),
            Mode::Update => self.handle_update(key),
            Mode::AgentUpdates => self.handle_agent_updates(key),
            Mode::SubtaskCard(view) => self.handle_subtask_card(view, key, act),
            Mode::NewTask(new) => self.handle_new_task(new, key),
            Mode::Subtasks => self.handle_subtasks(act, &key),
            Mode::SubtaskOptions(index) => self.handle_subtask_options(index, act, key),
            Mode::Report { id, shown, scroll, read, .. } => self.confirm_report(id, shown, scroll, read, key, cancels),
            Mode::ReportAll { shown, scroll, read, .. } => self.confirm_report_all(shown, scroll, read, key, cancels),
            Mode::NoteCard { key: note, scroll, .. } => self.handle_note_card(note, scroll, key, cancels),
            Mode::TaskNote { key: note, name, scroll } => self.handle_task_note(note, name, scroll, key, cancels),
            Mode::ItemCard(card) => self.handle_item_card(card, key, terminal),
            Mode::Comment { id, scroll, .. } => self.handle_comment_window(id, scroll, key, cancels),
            Mode::ConfirmReset { path, name, .. } => self.confirm_reset(path, name, key, cancels),
            Mode::ConfirmDone { path, name, .. } => self.confirm_done(path, name, key, cancels),
            Mode::ConfirmDismiss { path, name, .. } => self.confirm_dismiss(path, name, key, cancels),
            Mode::ConfirmQueued { name, about, now, .. } => self.confirm_queued(name, about, now, key, cancels),
            Mode::ConfirmClaimed { entry, about, machine, .. } => self.confirm_claimed(entry, about, machine, key, cancels),
            Mode::ConfirmDrop { path, name, force, .. } => self.confirm_drop(path, name, force, key, cancels),
            Mode::RestartLead(index) => self.choose_restart(index, act, key),
            Mode::FixedOptions(index) => self.choose_fixed(index, act, key),
            Mode::Park { address, name, index, date } => self.choose_park(address, name, index, date, act, key),
            Mode::Card { path, name, scroll, image } => self.handle_card(path, name, scroll, image, key),
            Mode::Fullscreen { path, name, scroll, index } => self.handle_fullscreen(path, name, scroll, index, key),
            Mode::Filter { buffer } => self.handle_filter(buffer, key),
            Mode::ConfirmPush { path, commits, selected, card, tests, marks, scroll } => self.handle_push(path, commits, selected, (card, tests, marks), scroll, key),
            Mode::EditMessage(editor) => self.handle_editor(editor, key, terminal),
            opening @ Mode::PushOpening { .. } => if !cancels {
                self.mode = opening;
            },
            checking @ Mode::CheckingLead(_) => if !cancels {
                self.mode = checking;
            },
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
            Some(Act::Back | Act::Close) if self.overlay => self.close_overlay(true),
            Some(Act::Close) => {
                let quit = keys::map().label(Act::Quit);
                self.say(format!("{quit} twice to quit"));
            }
            _ => {}
        }
    }

    // An overlay closes at once when nothing runs behind it; otherwise once what runs is done, or at a second esc that stops it where it is.
    fn close_overlay(&mut self, confirmed: bool) {
        if !self.overlay {
            return;
        }
        if self.under_way().is_empty() || (confirmed && self.leaving) {
            self.quit = true;
        } else {
            self.leaving = true;
        }
    }

    fn leave_once_done(&mut self) {
        if self.leaving && self.under_way().is_empty() {
            self.quit = true;
        }
    }

    fn under_way(&self) -> Vec<String> {
        self.jobs.iter().map(|job| self.job_line(job)).chain(self.areas.running()).chain(self.routines.running()).collect()
    }

    fn leaving_line(&self) -> Option<String> {
        if !self.leaving {
            return None;
        }
        let esc = keys::map().label(Act::Back);
        Some(match self.under_way().as_slice() {
            [] => "Closing…".to_string(),
            [one] => format!("Closes once {} is done · {esc} again closes now and stops it where it is", one.split('…').next().unwrap_or(one)),
            many => format!("Closes once the {} jobs under way are done · {esc} again closes now and stops them where they are", many.len()),
        })
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        if self.in_messages {
            return self.handle_messages(key);
        }
        if self.beside_key(&key) {
            return;
        }
        if self.asks_focused().is_some() {
            return self.handle_asks(key);
        }
        if self.comment_panel.focused {
            return self.handle_comments(key);
        }
        if self.offer_panel.focused {
            return self.handle_offers(key);
        }
        let ctx = self.ctx();
        let Some(act) = keys::map().act(ctx, &key) else { return };
        match act {
            Act::Messages => self.press_m(),
            Act::Hub => self.open_hub(),
            Act::Next => self.open_next(),
            Act::Move => {
                let (down, before) = (!matches!(key.code, KeyCode::Up | KeyCode::Char('k')), self.work_selected);
                if ctx == Ctx::Work && (self.enter_asks(down) || self.enter_comments(down)) {
                    return;
                }
                self.move_selection(if down { 1 } else { -1 });
                if down && ctx == Ctx::Work && self.work_selected == before {
                    if self.offers.is_empty() { self.focus_fixed() } else { self.focus_offers(0) }
                }
            }
            Act::Switch if key.code == KeyCode::Right => self.enter_task(),
            Act::Subtasks => self.enter_subtasks(),
            Act::Switch => {}
            Act::Back | Act::Close => self.at_top(Some(act)),
            Act::Help => self.mode = Mode::Help(Default::default()),
            Act::Inbox => self.open_questions(None),
            Act::Options if self.on_parked_line() => self.show_parked(true),
            Act::Options if self.column == Column::Work && self.selected_task().is_some_and(|item| item.unsent_start) => {
                let Some(item) = self.selected_task() else { return };
                let address = peers::address(&item.name, &item.place);
                self.about("prompt", &item.name);
                self.task_in_background(&item.name, item.path.as_deref(), "sending its prompt", format!("Sending {}'s start prompt", item.name), move || crate::typing::send_unsent(&address));
            }
            Act::Options => {
                let has_selection = match self.column {
                    Column::Work => self.selected_task().is_some(),
                    Column::Backlog => self.backlog_len() > 0 && self.shown(Column::Backlog, self.backlog_selected),
                };
                if has_selection {
                    self.open_options();
                }
            }
            Act::ItemOptions if ctx == Ctx::Backlog && self.backlog_len() > 0 && self.shown(Column::Backlog, self.backlog_selected) => self.mode = Mode::Options(0),
            Act::Refresh => self.refresh_backlog(),
            Act::Away => {
                self.subject = Some(notices::Subject::plain("away"));
                let turn = if permit::away_since().is_some() { "off" } else { "on" };
                self.in_background(format!("Telling the lead you are {}", if turn == "on" { "away" } else { "back" }), move || permit::away(Some(turn)));
            }
            Act::Update => self.start_update(),
            Act::RestartLead => self.restart_lead(),
            Act::Filter => {
                let buffer = self.filter_of(self.column).unwrap_or_default().to_string();
                self.mode = Mode::Filter { buffer };
            }
            Act::New if ctx == Ctx::Backlog && crate::config::filing(&self.sources).is_some() => self.act(Act::New),
            Act::Card if ctx == Ctx::Work => self.act(Act::Card),
            Act::New => self.open_new_task(),
            other if self.choices().iter().any(|choice| choice.act == Some(other)) => self.act(other),
            _ => {}
        }
    }

    fn refresh_backlog(&mut self) {
        self.subject = Some(notices::Subject::plain("backlog"));
        let prompt = match backlog::refresh_prompt(&self.sources) {
            Ok(prompt) => prompt,
            Err(why) => return self.show(Notice::new(Tone::NotNow, "Nothing for the lead to refresh").detail("why", why).err()),
        };
        self.in_background("Asking the lead to refresh the backlog".to_string(), move || match lead_pane() {
            Some(pane) => crate::typing::prompt(&pane, &prompt, "the lead").map(|_| Notice::new(Tone::Moving, "The lead refreshes the backlog").next("It updates here when it is saved.").to_string())
                .map_err(|refused| Notice::new(Tone::NotNow, format!("Nothing was typed: the lead {}", refused.reason())).next("Ask again once it is clear.").to_string()),
            None => Notice::new(Tone::NotNow, "No lead is running").next("prefix+space starts it.").err(),
        });
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
        if self.column == Column::Backlog && let Some(item) = self.selected_found() {
            return self.act_on_found(act, item);
        }
        match self.column {
            Column::Backlog => match act {
                Act::Go => {
                    if let Some(entry) = self.selected_entry() {
                        self.start_entry(entry);
                    }
                }
                Act::StartAt => {
                    if self.selected_entry().is_some_and(|entry| entry.shape.kept) {
                        return self.say("Not started: the lead has yet to write it out and file it in the vault".to_string());
                    }
                    if let Some(entry) = self.selected_entry() {
                        let buffer = suggested_name(&entry.title);
                        self.mode = Mode::Input { purpose: Purpose::BacklogLaterName(entry), label: "Start later as task: name".to_string(), buffer };
                    }
                }
                Act::Refresh => self.refresh_backlog(),
                Act::New => {
                    let label = "Add to the backlog: a line, the lead writes it out".to_string();
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
                        let name = item.name.clone();
                        self.task_in_background(&item.name, item.path.as_deref(), "restarting", format!("Restarting {}", item.name), move || tasks::start(&path, &name, &tasks::Start { prompt: None, focus: true, from: None, mode: None, profile: None, backlog: None, note: true, anyway: false }));
                    }
                    Act::Go => {
                        if let Some(workspace) = item.workspace.clone() {
                            self.in_background(format!("Going to {}", item.name), move || {
                                let _ = herdr::focus_workspace(&workspace);
                                Ok(String::new())
                            }).then(|app| app.close_overlay(false));
                        }
                    }
                    Act::Chat => {
                        if let Some(agent) = item.agent {
                            let label = format!("Message to {}", item.name);
                            self.mode = Mode::Input { purpose: Purpose::Message(agent.pane, item.name), label, buffer: String::new() };
                        }
                    }
                    Act::Review => self.open_review(&item),
                    Act::Note => self.open_task_note(&item),
                    Act::Card => match (item.path.clone(), self.card_of(&item), self.s_question(&item)) {
                        (_, _, Some(at)) => self.open_questions(Some(at)),
                        (Some(path), Some(_), None) => self.mode = Mode::Card { path, name: item.name.clone(), scroll: 0, image: 0 },
                        _ => {
                            self.about("card", &item.name);
                            self.show(Notice::new(Tone::NotNow, "No sign-off card yet").next("Its Claude writes one when it reports ready.").ok());
                        }
                    },
                    Act::Push | Act::Rebase | Act::Tidy | Act::Stage | Act::Done | Act::Reset | Act::Drop | Act::DismissCi | Act::RerunCi | Act::SendReview if self.doing(&item.name, item.path.as_deref()).is_some() => {
                        self.about(act_kind(act), &item.name);
                        let doing = self.doing(&item.name, item.path.as_deref()).unwrap_or_default();
                        self.already(doing);
                    }
                    Act::Stage if item.staged => {
                        if let Some(path) = item.path.clone() {
                            self.about("stage", &item.name);
                            self.task_in_background(&item.name, item.path.as_deref(), "unstaging", format!("Taking {} off local main", item.name), move || tasks::unstage(&path, None));
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
                            self.task_in_background(&item.name, item.path.as_deref(), "leaving the push line", format!("Taking {} out of the push line", item.name), move || tasks::cancel_push(&path, None));
                        }
                    }
                    Act::Push if item.awaiting_review() => {
                        self.about("push", &item.name);
                        self.show(Notice::new(Tone::NotNow, "Not yet: it is in review").next("Push opens once its reviewer passes its commits.").ok());
                    }
                    Act::Stage => {
                        if let Some(path) = item.path.clone() {
                            self.about("stage", &item.name);
                            self.task_in_background(&item.name, item.path.as_deref(), "staging", format!("Putting {} on local main", item.name), move || tasks::stage(&path, None, false));
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
                            self.task_in_background(&item.name, item.path.as_deref(), "rebasing", format!("Rebasing {}", item.name), move || tasks::rebase(&path));
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
                        let address = peers::address(&item.name, &item.place);
                        self.task_in_background(&item.name, item.path.as_deref(), "changing its permission", format!("Changing the lead's permission for {}", item.name), move || permit::toggle_task(&address));
                    }
                    Act::SendReview if item.path.is_some() => {
                        self.about("review", &item.name);
                        self.open_review_menu(&item);
                    }
                    Act::RerunCi if crate::board::options::reruns_ci(&item) => {
                        let address = peers::address(&item.name, &item.place);
                        self.about("ci", &item.name);
                        self.task_in_background(&item.name, item.path.as_deref(), "re-running CI", format!("Re-running CI for {}", item.name), move || crate::ci::rerun(&address));
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
                            Act::Push => self.open_push(&item, path),
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

// The name a job on the lead holds; a task's name never has a space.
const LEAD_JOB: &str = "the lead";

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

const IDLE_REDRAW: Duration = Duration::from_secs(1);

fn noted_restart(at: &std::sync::atomic::AtomicU64, said: Result<String>) -> Result<String> {
    if said.as_ref().is_ok_and(|said| said.starts_with(crate::lead::RESTARTED)) {
        at.store(crate::messages::now(), std::sync::atomic::Ordering::SeqCst);
    }
    said
}

// The plain board redraws at once on what changes it and once a second for its ages and countdowns; a window or another page redraws on every pass.
fn redraw_due(changed: bool, plain: bool, drawn_at: Option<Instant>, now: Instant) -> bool {
    changed || !plain || drawn_at.is_none_or(|at| now.duration_since(at) >= IDLE_REDRAW)
}

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
        let path = peers::installed_command().ok().map(PathBuf::from);
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
            column: Column::Work, tasks: Vec::new(), checkouts: Vec::new(), backlog, found: Vec::new(), work_selected: 0, backlog_selected: 0,
            mode: Mode::Normal, overlay, loaded: false, error: None, screens: Default::default(), refresh, quit: false, leaving: false, wanted_task: None, hits: Vec::new(), links: Vec::new(),
            page: Page::Work, areas: area_page::Page::new(sources.clone()), routines: routine_page::Page::new(config::path(), config::repos_dir()), settings: settings_page::Page::new(settings_page::Paths::current(), None), sources, fetch_failures: Vec::new(), away: false,
            messages: Vec::new(), fold: Fold::Line, seen: 0, fold_file: None, in_messages: false, message_back: 0, after_message: None, questions: Vec::new(), answering: None, sending: None, offers: Vec::new(), offer_panel: Default::default(), reporter: Default::default(), fixed: None, fixed_seen: 0, usage: None, ongoing: Vec::new(), groups: Group::ALL.to_vec(), ongoing_at: OngoingAt::default(), next_question: true, parked_open: false,
            details: HashMap::new(), detail_requests: None, detail_results: None, asked_details: HashMap::new(),
            graphics: Graphics::Off, images: HashMap::new(), cell_guessed: false, viewer: None, focus_tint: background::focus_tint(None), decoded: HashMap::new(), image_requests: None, image_results: None, plans: HashMap::new(), plan_reads: Default::default(), card_read: None, card_reader: false,
            updates: Vec::new(), updater: std::sync::Arc::new(|| update::run(&crate::doctor::Env::current().claude_dir, false, &crate::release::Request::Channel).map(|outcome| outcome.details.join("\n"))), update: None, progress_file: None, recheck: None, stale: Vec::new(), stale_at: 0, restart_after_update: false,
            agent_offer: None, agent_mode: std::sync::Arc::new(|| config::agent_updates_in(&config::path())), installer: agent_updates::default_install(),
            filter: None, ctrl_c: None, go_again: None, notice: None, lead_chat: None, restart_auto: true, disk_line: None, lead_restarter: std::sync::Arc::new(crate::lead_restart::restart_now), lead_fresh_starter: std::sync::Arc::new(crate::lead_restart::fresh_now), lead_checker: std::sync::Arc::new(crate::lead::restart_refusal), lead_restarted: std::sync::Arc::default(), lead_context: None, lead_context_limit: crate::config::DEFAULT_LEAD_CONTEXT, restart_why: String::new(), devices: Vec::new(), subject: None, shown: None, toast: None, hub_paths: None, hub_banner: None,
            comments: Vec::new(), comment_panel: Default::default(), ask_focus: None, comment_dir: None, copier: comments::default_copier(), reopener: comments::default_reopener(), pusher: jobs::live_pusher(), jobs: Vec::new(), outcomes: Vec::new(), dropper: jobs::live_dropper(), lead_teller: std::sync::Arc::new(crate::backlog_file::tell_lead), telling_drafts: Default::default(),
            subtask_row: None, subtask_io: Default::default(), reads: Default::default(), loads: Default::default(), space_repos: Default::default(), new_io: Default::default(), beside: Default::default(),
        }
    }
}

pub fn run(overlay: bool) -> Result<String> {
    if std::env::var_os(RESTART_STATE).is_some() {
        crate::sh::adopt_children();
    }
    let sources = config::sources()?;
    // A warning printed to stderr while the board draws lands inside its screen; kebbie doctor shows unknown keys.
    config::report_unknown_keys_quietly();
    let (snapshots, refresh) = spawn_refresher(std::env::var("HERDR_PANE_ID").ok().filter(|_| !overlay));
    spawn_ci_watcher(refresh.clone());
    let mut app = App::new(sources, backlog::load(), overlay, refresh);
    app.away = permit::away_since().is_some();
    let (update_notices, recheck) = spawn_update_checker();
    app.recheck = Some(recheck);
    let restarter = spawn_restarter(overlay);
    spawn_routine_runner(overlay);
    if !overlay {
        crate::vault_snapshot::spawn_board_refresher();
    }
    let (requests, results) = spawn_detailer();
    app.detail_requests = Some(requests);
    app.reads.spawn();
    app.loads.spawn();
    app.screens.spawn();
    app.space_repos.spawn();
    app.settings.spawn_reads();
    app.card_reader = true;
    app.space_repos(Duration::ZERO);
    app.plan_reads.spawn();
    app.detail_results = Some(results);
    (app.image_requests, app.image_results) = spawn_decoder().map_or((None, None), |(requests, results)| (Some(requests), Some(results)));
    let height = ratatui::crossterm::terminal::size().map_or(50, |(_, height)| height);
    app.fold_file = fold_file();
    (app.fold, app.seen) = load_fold(app.fold_file.as_deref(), height);
    app.offer_panel.file = bug_report::panel_file();
    app.offer_panel.shown = bug_report::load_shown(app.offer_panel.file.as_deref());
    app.comment_panel.file = comments::panel_file();
    app.comment_panel.folded_at = comments::load_folded(app.comment_panel.file.as_deref());
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
    let mut shift_enter = false;
    app.focus_tint = background::focus_tint(background::ask());
    let graphics = GraphicsEnv::current();
    (app.graphics, app.cell_guessed) = graphics.startup();
    app.viewer = image_viewer(std::env::consts::OS, &|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    let (mut changed, mut drawn_at) = (true, None);
    let mut pulse = pulse::Pulse::started(Instant::now());
    let result = (|| -> Result<()> {
        while !app.quit {
            if let Mode::EditMessage(editor) = &mut app.mode {
                editor.poll_draft();
            }
            app.poll_answer();
            app.poll_sending();
            app.poll_jobs();
            app.poll_push_opening();
            app.poll_lead_check();
            app.poll_hub();
            app.poll_update();
            app.poll_agent_install();
            while let Ok(updates) = update_notices.try_recv() {
                app.updates = updates;
                changed = true;
            }
            while let Ok((at, stale)) = restarter.try_recv() {
                app.take_stale(at, stale);
                changed = true;
            }
            while let Ok(update) = snapshots.try_recv() {
                changed = true;
                log_pulse(pulse.snapshot(Instant::now()).into_iter().collect());
                match update {
                    Ok(snapshot) => {
                        let previous = app.selected_task().map(|item| (item.name, item.place));
                        if let Ok((groups, ongoing_at, next_question)) = config::board_layout() {
                            (app.groups, app.ongoing_at, app.next_question) = (groups, ongoing_at, next_question);
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
                        app.settle_subtask_cursor();
                        if let Ok(sources) = config::sources() {
                            app.sources = sources;
                        }
                        app.backlog = snapshot.backlog;
                        app.found = crate::lead::state_dir().map(|state| crate::found::open_in(&state)).unwrap_or_default();
                        app.reread_item_card();
                        if app.sources.contains(&Source::Local) {
                            app.tell_new_drafts();
                        }
                        let kept = app.selected_message().filter(|_| app.message_back > 0).map(|message| message.id.clone());
                        app.messages = snapshot.messages;
                        if let Some(index) = kept.and_then(|id| app.messages.iter().position(|message| message.id == id)) {
                            app.message_back = app.messages.len() - 1 - index;
                        }
                        app.message_back = app.message_back.min(app.messages.len().saturating_sub(1));
                        app.questions = snapshot.questions;
                        app.offers = snapshot.offers;
                        app.offers_changed();
                        app.comments = snapshot.comments;
                        app.comments_changed();
                        app.fixed = snapshot.fixed.filter(|fixed| fixed.last > app.fixed_seen);
                        app.fixed_changed();
                        app.usage = snapshot.usage;
                        app.take_ongoing(snapshot.ongoing);
                        app.lead_chat = snapshot.lead_chat;
                        app.restart_auto = snapshot.restart_auto;
                        app.disk_line = snapshot.disk;
                        app.lead_context = snapshot.lead_context;
                        app.lead_context_limit = snapshot.lead_context_limit;
                        app.devices = snapshot.devices;
                        app.backlog_selected = app.backlog_selected.min(app.backlog_len().saturating_sub(1));
                        let newest = app.messages.last().map_or(0, |message| message.at);
                        if app.fold == Fold::Open && newest > app.seen {
                            app.seen = newest;
                            save_fold(app.fold_file.as_deref(), app.fold, app.seen);
                        }
                        app.follow_questions();
                        app.hub_banner = crate::lead::state_location().ok().and_then(|state| crate::hub::relay::load(&state).not_enabled_error().map(str::to_string));
                        app.error = None;
                    }
                    Err(error) => app.error = Some(error),
                }
                app.loaded = true;
            }
            if let Some(results) = &app.detail_results {
                while let Ok((path, detail)) = results.try_recv() {
                    app.details.insert(path, std::rc::Rc::new(detail));
                    changed = true;
                }
            }
            changed |= app.take_images();
            changed |= app.reads.take();
            changed |= app.loads.take();
            changed |= app.screens.take();
            changed |= app.space_repos.take();
            changed |= app.plan_reads.take();
            changed |= app.settings.take_reads();
            changed |= app.take_item_card_read();
            if let Some(Effect::Left) = app.areas.tick(app.page == Page::Areas) {
                app.close_overlay(false);
            }
            app.leave_once_done();
            if let Some(effect) = app.routines.tick(app.page == Page::Routines) {
                app.after_routines(effect, None);
            }
            let mut repos: Vec<PathBuf> = app.tasks.iter().filter_map(|item| item.repo.clone()).chain(app.areas.repos()).collect();
            repos.sort();
            repos.dedup();
            if fetcher.tick(repos) {
                let _ = app.refresh.send(());
                app.areas.refresh();
            }
            app.fetch_failures = fetcher.failures();
            if changed {
                app.away = permit::away_since().is_some();
            }
            if matches!(app.graphics, Graphics::Pending { wanted: true }) {
                (app.graphics, app.cell_guessed) = graphics.resolve(Picker::from_query_stdio().ok());
                app.images.clear();
                changed = true;
            }
            let size = ratatui::crossterm::terminal::size().unwrap_or_default();
            repaint |= seen.replace(size).is_some_and(|before| before != size);
            if hold.is_some_and(|until| app.loaded || Instant::now() >= until) {
                hold = None;
            }
            let now = Instant::now();
            if hold.is_none() && redraw_due(changed || repaint, app.page == Page::Work && matches!(app.mode, Mode::Normal), drawn_at, now) && resize.draw_now(size, now) {
                let every_cell = std::mem::take(&mut repaint);
                terminal.draw(|frame| {
                    draw(frame, &mut app);
                    if every_cell {
                        terminal::write_every_cell(frame.buffer_mut());
                    }
                }).map_err(|error| error.to_string())?;
                (changed, drawn_at) = (false, Some(now));
                log_pulse(pulse.drawn(now, app.tasks.iter().map(|item| format!("{}@{} {:?}", item.name, item.place, item.group())).collect()));
            } else if changed || repaint {
                pulse.waits(now);
            }
            log_pulse(pulse.check(now, || format!("hold {hold:?}, size {size:?}, drawn at {:?}, pending {:?}", resize.drawn, resize.pending)));
            if app.may_restart() && ((std::mem::take(&mut app.restart_after_update) && rebuild.changed()) || rebuild.ready()) {
                let error = rebuild.restart(&app);
                app.show_as(Err(error), true);
            }
            let typing = matches!(&app.mode, Mode::NewTask(new) if new.choice.is_none());
            if typing != shift_enter {
                shift_enter = typing;
                terminal::shift_enter(&mut std::io::stdout(), typing);
            }
            let wait = if app.page == Page::Areas && app.areas.waiting() { 20 } else { 250 };
            if event::poll(Duration::from_millis(wait)).map_err(|error| error.to_string())? {
                changed = true;
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

fn log_pulse(lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    if let Ok(state) = crate::lead::state_dir() {
        for line in lines {
            crate::keeper::log_line(&state, &line);
        }
    }
}

fn lead_pane() -> Option<String> {
    herdr::agents().ok()?.into_iter().find(|agent| agent["name"] == "lead")
        .and_then(|agent| agent["pane_id"].as_str().map(str::to_string))
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
    fn a_board_sees_the_kebbie_an_update_installed_in_a_new_folder_as_rebuilt() {
        let dir = std::env::temp_dir().join(format!("kebbie-board-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = &dir;
        let binary = |folder: &str, at: u64| {
            let file = home.join(folder).join("target/release/kebbie");
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, folder).unwrap();
            std::fs::File::options().write(true).open(&file).unwrap().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(at)).unwrap();
        };
        binary("old", 1_000);
        binary("new", 2_000);
        let link = home.join(".local/share/kebbie/current");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(home.join("old"), &link).unwrap();
        let installed = crate::peers::installed_in(home).expect("the link names an installed kebbie");
        let started = built(&installed);
        assert!(crate::update::relink_current(home, &home.join("new")).unwrap());
        assert!(built(&installed).is_some() && built(&installed) != started, "the watched path follows the link to the new folder's binary");
        assert_eq!(crate::peers::installed_in(&home.join("nowhere")), None, "without an install the board watches its own binary");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lead_board_alone_offers_h_and_says_chat_hidden_while_it_is() {
        let mut app = test_app(Vec::new());
        app.handle_key(KeyEvent::from(KeyCode::Char('h')), None);
        assert_eq!(app.notice.as_ref().map(|(text, _)| text.as_str()), Some("h hides the lead's chat only on the board in the lead Space"));
        assert!(!hint_lines(Ctx::Work, None, false, false, &Vec::new()).0.to_string().contains("h chat") && hint_lines(Ctx::Work, None, true, false, &Vec::new()).0.to_string().contains("h chat"));
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
        let menus = [task_choices(Some(&item), Some("bin/dev"), Some("bin/jobs")), backlog_choices(&[Source::Local], None, 0), ongoing_choices(&Ongoing { key: String::new(), title: String::new(), area: String::new(), task: None, claim: None, ..Ongoing::default() })];
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
    fn an_overlay_closed_while_a_job_runs_closes_once_it_is_done_or_at_a_second_esc() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.overlay = true;
        let (go, wait) = std::sync::mpsc::channel::<()>();
        app.in_background("Rebasing billing".to_string(), move || {
            let _ = wait.recv();
            Ok(String::new())
        });
        press(&mut app, key(KeyCode::Esc));
        assert!(!app.quit && app.leaving, "esc waits for the job");
        let (rows, _) = render(&mut app, 140, 30);
        assert!(rows.join("\n").contains("Closes once Rebasing billing is done · esc again closes now"), "a line says what still runs");
        app.in_background("Going to billing".to_string(), || Ok(String::new())).then(|app| app.close_overlay(false));
        app.poll_jobs();
        let start = Instant::now();
        while app.jobs.len() > 1 && start.elapsed() < Duration::from_secs(10) {
            app.poll_jobs();
        }
        app.leave_once_done();
        assert!(!app.quit, "a job's own close waits for the one still running");
        go.send(()).unwrap();
        settle_jobs(&mut app);
        app.leave_once_done();
        assert!(app.quit, "it closes once the job is done");
        let mut app = test_app(Vec::new());
        app.overlay = true;
        let (_go, wait) = std::sync::mpsc::channel::<()>();
        app.in_background("Pushing billing".to_string(), move || {
            let _ = wait.recv();
            Ok(String::new())
        });
        press(&mut app, key(KeyCode::Char('q')));
        assert!(!app.quit);
        press(&mut app, key(KeyCode::Esc));
        assert!(app.quit, "esc again closes at once");
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
        assert!(matches!(&app.mode, Mode::NewTask(_)), "n starts a new task");
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
    fn four_opens_the_routines_page_its_tab_counts_the_runs_waiting_on_you_and_enter_goes_to_the_run() {
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
        assert!(header(&app, 200).0.to_string().ends_with(" 1 Work  2 Backlog  3 Areas  4 Routines ⚠1  , Settings "), "{}", header(&app, 200).0);
        press(&mut app, key(KeyCode::Char('4')));
        assert_eq!(app.page, Page::Routines);
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("▌⚠ Cluster health") && screen.contains("⏎ options · n new · g run now · e edit · p pause · s last report · ? keys"), "{screen}");
        assert!(header(&app, 70).0.to_string().ends_with(" 1  2  3  4 Routines ⚠1  , "), "a narrow header names only the page it is on: {}", header(&app, 70).0);
        let (x, y) = find(&rows, "4 Routines")[0];
        assert_eq!(app.hit_at(x as u16, y as u16), Some(Hit::Tab(Page::Routines)));
        press(&mut app, key(KeyCode::Enter));
        press(&mut app, key(KeyCode::Enter));
        assert!(app.page == Page::Work && app.column == Column::Work, "Go to its task opens the Work page");
        assert_eq!(app.selected_task().map(|item| item.name).as_deref(), Some("cluster-health-20261019-0600"), "with the run selected");
        app.routines.tasks.clear();
        assert!(!header(&app, 200).0.to_string().contains('⚠'), "a closed run's findings wait for no one");
    }

    #[test]
    fn two_opens_the_backlog_page_across_the_width_with_whole_titles_and_its_own_keys() {
        let mut app = test_app(vec![served("search-filters")]);
        let title = "Retry a failed CI push once before telling anyone, with the run's own log";
        app.backlog.entries = vec![crate::backlog::Entry { source: "vault".to_string(), title: title.to_string(), priority: "high".to_string(), area: "web".to_string(), reference: "Projects/retry.md".to_string(), summary: String::new(), age: "3 d".to_string(), shape: Default::default() },
            crate::backlog::Entry { source: "vault".to_string(), title: "Session drops on reload".to_string(), priority: "low".to_string(), area: "shop".to_string(), reference: "Projects/session.md".to_string(), summary: String::new(), age: "6 d".to_string(), shape: Default::default() }];
        let work = render(&mut app, 140, 40).0;
        assert!(!work.join("\n").contains("BACKLOG") && !header(&app, 200).0.to_string().contains("backlog:"), "Work has no backlog now: {}", work.join("\n"));
        press(&mut app, key(KeyCode::Char('2')));
        assert!(app.page == Page::Work && app.column == Column::Backlog && app.shown_page() == Page::Backlog);
        let (rows, buffer) = render(&mut app, 140, 40);
        let screen = rows.join("\n");
        assert!(rows[1].ends_with(" 1 Work  2 Backlog  3 Areas  4 Routines  , Settings   "), "{}", rows[1]);
        assert!(header(&app, 200).0.to_string().contains("backlog: "), "the Backlog page dates its items: {}", header(&app, 200).0);
        let (x, y) = find(&rows, "2 Backlog")[0];
        assert_eq!(buffer[(x as u16, y as u16)].fg, crate::board::widgets::MAUVE, "its tab is lit");
        assert!(screen.contains(title) && screen.contains("Session drops on reload") && !screen.contains("WORK") && !screen.contains("CARD"), "whole titles across the width: {screen}");
        assert_eq!(app.hit_at(x as u16, y as u16), Some(Hit::Tab(Page::Backlog)));
        press(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.backlog_selected, 1, "j moves through the items");
        press(&mut app, key(KeyCode::Char('l')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogLaterName(entry), .. } if entry.title == "Session drops on reload"), "l starts it later");
        app.mode = Mode::Normal;
        press(&mut app, key(KeyCode::Char('n')));
        assert!(matches!(&app.mode, Mode::Input { purpose: Purpose::BacklogItem, .. }), "n adds an item");
        app.mode = Mode::Normal;
        press(&mut app, key(KeyCode::Char('3')));
        assert_eq!(app.page, Page::Areas);
        press(&mut app, key(KeyCode::Char('1')));
        assert!(app.page == Page::Work && app.column == Column::Work && app.shown_page() == Page::Work, "1 is the Work page again");
        press(&mut app, key(KeyCode::Char('2')));
        press(&mut app, key(KeyCode::Char('4')));
        assert_eq!(app.page, Page::Routines);
    }

    #[test]
    fn a_comma_opens_the_settings_page_on_the_selected_tasks_repo_and_esc_goes_back() {
        let dir = crate::sh::test_dir("board-settings");
        std::fs::write(dir.join("config.toml"), "[task]\nsandbox = false\n").unwrap();
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.settings = settings_page::Page::new(settings_page::Paths { machine: dir.join("config.toml"), repos_dir: dir.join("repos"), claude_home: dir.join("claude"), kebbie: dir.join("kebbie"), profiles: dir.join("profiles"), codex_home: dir.join("codex"), path: Vec::new(), kebbie_command: "kebbie".to_string(), sessions: Some(dir.join("sessions")) }, None);
        press(&mut app, key(KeyCode::Char(',')));
        assert_eq!(app.page, Page::Settings);
        let (rows, _) = render(&mut app, 140, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("Settings") && screen.contains("task.sandbox") && screen.contains("○ off"), "{screen}");
        assert!(screen.contains("↑ ↓ key · ← → section · ⏎ pick · d back to default · tab machine / repo · / search · esc board"), "the hint line names the page's keys: {screen}");
        assert!(header(&app, 200).0.to_string().ends_with(" , Settings "), "{}", header(&app, 200).0);
        press(&mut app, key(KeyCode::Down));
        press(&mut app, key(KeyCode::Char('3')));
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
    fn the_plain_board_redraws_at_once_on_a_change_and_otherwise_once_a_second_and_a_window_on_every_pass() {
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        assert!(redraw_due(false, true, None, at(0)), "the first frame draws");
        assert!(!redraw_due(false, true, Some(at(0)), at(250)), "a quiet board is not drawn four times a second");
        assert!(!redraw_due(false, true, Some(at(0)), at(750)));
        assert!(redraw_due(false, true, Some(at(0)), at(1000)), "ages and notices still move once a second");
        assert!(redraw_due(true, true, Some(at(0)), at(10)), "a key, a snapshot or a resize draws at once");
        assert!(redraw_due(false, false, Some(at(0)), at(10)), "a window or another page draws on every pass, as its countdowns and waits need");
    }

    fn ask_restart(app: &mut App) {
        app.handle_normal(key(KeyCode::Char('L')));
        let started = Instant::now();
        while matches!(app.mode, Mode::CheckingLead(_)) {
            assert!(started.elapsed() < Duration::from_secs(10), "the lead check never answered");
            app.poll_lead_check();
        }
    }

    #[test]
    fn l_checks_the_lead_on_a_thread_behind_a_small_window_and_asks_once_it_answers() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let wait = std::sync::Arc::new(std::sync::Mutex::new(wait));
        app.lead_checker = std::sync::Arc::new(move || {
            let _ = wait.lock().unwrap().recv();
            Ok(())
        });
        app.handle_normal(key(KeyCode::Char('L')));
        assert!(matches!(app.mode, Mode::CheckingLead(_)), "L answers at once");
        assert!(render(&mut app, 120, 30).0.join("\n").contains("Checking the lead…"));
        press(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal), "esc closes it");
        go.send(()).unwrap();
        app.handle_normal(key(KeyCode::Char('L')));
        go.send(()).unwrap();
        let started = Instant::now();
        while matches!(app.mode, Mode::CheckingLead(_)) {
            assert!(started.elapsed() < Duration::from_secs(10), "the lead check never answered");
            app.poll_lead_check();
        }
        assert!(matches!(app.mode, Mode::RestartLead(_)), "the question opens once the check answers");
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
        ask_restart(&mut app);
        assert!(matches!(app.mode, Mode::RestartLead(0)), "L asks for a lead kebbie counts as current too");
        press(&mut app, key(KeyCode::Char('2')));
        assert!(matches!(app.mode, Mode::Normal) && app.jobs.len() == 1, "2 resumes it in the background");
        settle_jobs(&mut app);
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

    #[test]
    fn the_lead_line_goes_once_l_restarted_the_lead_and_only_a_round_begun_after_that_restart_brings_it_back() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.lead_checker = std::sync::Arc::new(|| Ok(()));
        app.lead_restarter = std::sync::Arc::new(|| Ok("Restarted the lead with its current role and settings, resuming its conversation.".to_string()));
        let stale = || vec!["billing@app: restarts when idle".to_string(), format!("lead: {}", crate::restart::Verdict::Focused.words(true))];
        app.take_stale(100, stale());
        assert!(app.lead_line().is_some_and(|line| line.contains("when you leave")), "the keeper's round says the lead waits for you to leave it");
        ask_restart(&mut app);
        press(&mut app, key(KeyCode::Char('2')));
        settle_jobs(&mut app);
        let restarted = app.lead_restarted.load(std::sync::atomic::Ordering::SeqCst);
        assert!(restarted > 0, "the restart is noted");
        assert_eq!(app.lead_line(), None, "the line goes as soon as L restarted the lead");
        assert_eq!(app.stale, vec!["billing@app: restarts when idle".to_string()], "another stale session stays");
        app.take_stale(restarted, stale());
        assert_eq!(app.lead_line(), None, "a round that began before the restart finished says nothing about the restarted lead");
        app.take_stale(restarted + 1, stale());
        assert!(app.lead_line().is_some(), "a later round that still finds it stale shows the line again");
        let mut refused = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        refused.lead_checker = std::sync::Arc::new(|| Ok(()));
        refused.lead_restarter = std::sync::Arc::new(|| crate::lead_restart::outcome(Err("Not restarted: the lead is working.".to_string())));
        refused.take_stale(100, stale());
        ask_restart(&mut refused);
        press(&mut refused, key(KeyCode::Char('2')));
        settle_jobs(&mut refused);
        assert!(refused.lead_line().is_some(), "a refused restart leaves the line");
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
        ask_restart(&mut app);
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
        ask_restart(&mut app);
        press(&mut app, key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal) && app.jobs.len() == 1, "enter on the first row starts it fresh in the background");
        settle_jobs(&mut app);
        assert_eq!((count(&calls), count(&resumed)), (1, 0));
        assert!(app.toast.as_ref().is_some_and(|toast| toast.notice.headline.contains("with a new conversation")), "the result goes by as a toast");
        app.mode = Mode::Normal;
        ask_restart(&mut app);
        press(&mut app, key(KeyCode::Char('1')));
        settle_jobs(&mut app);
        assert_eq!((count(&calls), count(&resumed)), (2, 0), "1 starts it fresh");
        app.mode = Mode::Normal;
        ask_restart(&mut app);
        let (rows, _) = render(&mut app, 120, 30);
        let (x, y) = find(&rows, "Resume this conversation")[0];
        click(&mut app, x as u16, y as u16);
        settle_jobs(&mut app);
        assert_eq!((count(&calls), count(&resumed)), (2, 1), "a click on a row picks it");
    }

    #[test]
    fn with_work_going_on_l_preselects_resuming_and_says_why_and_enter_resumes() {
        let mut app = test_app(vec![crate::status::tests::task(Some("working"), None, 0)]);
        app.lead_checker = std::sync::Arc::new(|| Ok(()));
        let resumed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = resumed.clone();
        app.lead_restarter = std::sync::Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("Restarted the lead with its current role and settings, resuming its conversation.".to_string())
        });
        app.lead_fresh_starter = std::sync::Arc::new(|| panic!("enter never throws the conversation away while work is going on"));
        app.lead_context = Some(91.2);
        let (rows, _) = render(&mut app, 170, 30);
        assert!(rows.join("\n").contains("The lead's context is 91% full · L to restart it fresh"), "a full context still points at L");
        ask_restart(&mut app);
        assert!(matches!(app.mode, Mode::RestartLead(1)), "resume is preselected");
        let (rows, _) = render(&mut app, 120, 30);
        let screen = rows.join("\n");
        assert!(screen.contains("▸ Resume this conversation") && screen.contains("  New conversation") && screen.contains("resume: 1 task is working"), "{screen}");
        press(&mut app, key(KeyCode::Enter));
        settle_jobs(&mut app);
        assert_eq!(resumed.load(std::sync::atomic::Ordering::SeqCst), 1, "enter resumes");
    }

    #[test]
    fn on_an_empty_or_idle_board_l_preselects_a_new_conversation_and_says_nothing_is_going_on() {
        for tasks in [Vec::new(), vec![crate::status::tests::task(Some("idle"), None, 0)]] {
            let mut app = test_app(tasks);
            app.lead_checker = std::sync::Arc::new(|| Ok(()));
            ask_restart(&mut app);
            assert!(matches!(app.mode, Mode::RestartLead(0)), "fresh is preselected");
            let (rows, _) = render(&mut app, 120, 30);
            let screen = rows.join("\n");
            assert!(screen.contains("▸ New conversation") && screen.contains("fresh: nothing is going on"), "{screen}");
            press(&mut app, key(KeyCode::Down));
            assert!(matches!(app.mode, Mode::RestartLead(1)), "resume stays one row away");
        }
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
        ask_restart(&mut app);
        assert!(matches!(app.mode, Mode::Normal), "no question opens");
        assert!(app.toast.as_ref().is_some_and(|toast| toast.notice.headline == "Not restarted: the lead's input box holds text you have not sent." && toast.notice.tone == Tone::NotNow), "the plain reason as a toast, not an error");
        ask_restart(&mut app);
        assert_eq!(checked.load(std::sync::atomic::Ordering::SeqCst), 2, "a refused restart can be pressed again");
    }

    #[test]
    fn a_nearly_full_lead_context_shows_one_line_that_offers_l_and_nothing_below_the_mark() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.lead_checker = std::sync::Arc::new(|| Ok(()));
        for used in [None, Some(60.0), Some(74.9)] {
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
