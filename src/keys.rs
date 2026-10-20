use std::collections::HashMap;
use std::sync::OnceLock;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctx {
    Work,
    Backlog,
    Areas,
    Card,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Act {
    Options,
    Move,
    Switch,
    WorkPage,
    AreasPage,
    Lead,
    Messages,
    Help,
    Inbox,
    New,
    Go,
    Chat,
    Review,
    Card,
    Server,
    Worker,
    SendReview,
    Permit,
    Tidy,
    Push,
    Done,
    Reset,
    Drop,
    Refresh,
    Away,
    Update,
    Filter,
    AreaRefresh,
    Log,
    Fullscreen,
    OpenImage,
    Back,
    Close,
    Quit,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Default {
    Char(char),
    Fixed(&'static [KeyCode], &'static str),
    CtrlC,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hint {
    No,
    Full,
    Both,
}

pub struct Binding {
    pub act: Act,
    pub on: &'static [Ctx],
    pub help: &'static str,
    pub hint: Hint,
}

use Ctx::{Areas, Backlog, Card, Work};

const BOARD: &[Ctx] = &[Work, Backlog];
const ALL: &[Ctx] = &[Work, Backlog, Areas];

impl Act {
    pub const EVERY: [Act; 34] = [
        Act::Options, Act::Move, Act::Switch, Act::WorkPage, Act::AreasPage, Act::Lead, Act::Messages, Act::Help, Act::Inbox, Act::New, Act::Go, Act::Chat,
        Act::Review, Act::Card, Act::Server, Act::Worker, Act::SendReview, Act::Permit, Act::Tidy, Act::Push, Act::Done, Act::Reset, Act::Drop, Act::Refresh, Act::Away,
        Act::Update, Act::Filter, Act::AreaRefresh, Act::Log, Act::Fullscreen, Act::OpenImage, Act::Back, Act::Close, Act::Quit,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Act::Options => "options",
            Act::Move => "move",
            Act::Switch => "switch",
            Act::WorkPage => "work_page",
            Act::AreasPage => "areas_page",
            Act::Lead => "lead",
            Act::Messages => "messages",
            Act::Help => "help",
            Act::Inbox => "inbox",
            Act::New => "new",
            Act::Go => "go",
            Act::Chat => "chat",
            Act::Review => "review",
            Act::Card => "card",
            Act::Server => "server",
            Act::Worker => "worker",
            Act::SendReview => "send_review",
            Act::Permit => "permit",
            Act::Tidy => "tidy",
            Act::Push => "push",
            Act::Done => "done",
            Act::Reset => "reset",
            Act::Drop => "drop",
            Act::Refresh => "refresh",
            Act::Away => "away",
            Act::Update => "update",
            Act::Filter => "filter",
            Act::AreaRefresh => "area_refresh",
            Act::Log => "log",
            Act::Fullscreen => "fullscreen",
            Act::OpenImage => "open_image",
            Act::Back => "back",
            Act::Close => "close",
            Act::Quit => "quit",
        }
    }

    fn default(self) -> Default {
        match self {
            Act::Options => Default::Fixed(&[KeyCode::Enter], "⏎"),
            Act::Move => Default::Fixed(&[KeyCode::Up, KeyCode::Down, KeyCode::Char('k'), KeyCode::Char('j')], "↑ ↓ j k"),
            Act::Switch => Default::Fixed(&[KeyCode::Left, KeyCode::Right], "← →"),
            Act::Lead => Default::Fixed(&[KeyCode::Tab, KeyCode::BackTab], "tab"),
            Act::Back => Default::Fixed(&[KeyCode::Esc], "esc"),
            Act::Fullscreen => Default::Fixed(&[KeyCode::Char('f')], "f"),
            Act::OpenImage => Default::Fixed(&[KeyCode::Char('o')], "o"),
            Act::Quit => Default::CtrlC,
            Act::WorkPage => Default::Char('1'),
            Act::AreasPage => Default::Char('2'),
            Act::Messages => Default::Char('m'),
            Act::Help => Default::Char('?'),
            Act::Inbox => Default::Char('i'),
            Act::New => Default::Char('n'),
            Act::Go => Default::Char('g'),
            Act::Chat => Default::Char('c'),
            Act::Review => Default::Char('r'),
            Act::Card => Default::Char('s'),
            Act::Server => Default::Char('w'),
            Act::Worker => Default::Char('b'),
            Act::SendReview => Default::Char('v'),
            Act::Permit => Default::Char('a'),
            Act::Tidy => Default::Char('t'),
            Act::Push => Default::Char('p'),
            Act::Done => Default::Char('d'),
            Act::Reset => Default::Char('z'),
            Act::Drop => Default::Char('x'),
            Act::Refresh => Default::Char('R'),
            Act::Away => Default::Char('A'),
            Act::Update => Default::Char('U'),
            Act::Filter => Default::Char('/'),
            Act::AreaRefresh => Default::Char('b'),
            Act::Log => Default::Char('l'),
            Act::Close => Default::Char('q'),
        }
    }

    pub fn remappable(self) -> bool {
        matches!(self.default(), Default::Char(_))
    }
}

// One table for every key on the board: the handlers, the option menus, ? help, the hint line and the mouse all read it.
pub const TABLE: &[Binding] = &[
    Binding { act: Act::Options, on: BOARD, help: "options for the selected task or backlog item", hint: Hint::Both },
    Binding { act: Act::Options, on: &[Areas], help: "actions for the selected area (in the detail: run the selected action)", hint: Hint::Both },
    Binding { act: Act::Switch, on: BOARD, help: "switch between work and backlog", hint: Hint::No },
    Binding { act: Act::Switch, on: &[Areas], help: "list · detail", hint: Hint::No },
    Binding { act: Act::Move, on: ALL, help: "move", hint: Hint::No },
    Binding { act: Act::Move, on: &[Card], help: "in the card window: scroll", hint: Hint::No },
    Binding { act: Act::WorkPage, on: ALL, help: "the work page", hint: Hint::No },
    Binding { act: Act::AreasPage, on: ALL, help: "the areas page", hint: Hint::No },
    Binding { act: Act::Lead, on: ALL, help: "move focus to the lead's chat", hint: Hint::Full },
    Binding { act: Act::Inbox, on: ALL, help: "inbox: the questions waiting on you; read the plan, answer with a comment", hint: Hint::Both },
    Binding { act: Act::Messages, on: BOARD, help: "messages at the bottom: one line, then open with ↑ ↓ moving through them and ⏎ reading one (esc back to the tasks), then hidden", hint: Hint::Full },
    Binding { act: Act::Filter, on: BOARD, help: "filter the selected column: type part of a name, ⏎ keeps it, esc clears it", hint: Hint::No },
    Binding { act: Act::New, on: &[Work], help: "new task", hint: Hint::No },
    Binding { act: Act::New, on: &[Backlog], help: "in the Backlog column: a new item in the local backlog (a new task without the local source)", hint: Hint::No },
    Binding { act: Act::Go, on: BOARD, help: "go to the task's Space, or start the backlog item", hint: Hint::No },
    Binding { act: Act::Chat, on: &[Work], help: "chat: message the task's Claude", hint: Hint::No },
    Binding { act: Act::Review, on: &[Work], help: "open the task's review next to its Claude, or go to the one already open", hint: Hint::No },
    Binding { act: Act::Card, on: &[Work], help: "the task's sign-off card with its images, in a window: ↑ ↓ scroll, p goes on to push", hint: Hint::No },
    Binding { act: Act::Server, on: &[Work], help: "start or stop the task's dev server (worktree.dev_server); its address shows in the task's details (worktree.dev_url)", hint: Hint::No },
    Binding { act: Act::Worker, on: &[Work], help: "start or stop the task's worker (worktree.dev_worker)", hint: Hint::No },
    Binding { act: Act::SendReview, on: &[Work], help: "send the task to review: a read-only Claude reads its commits; on a task in review, withdraw it", hint: Hint::No },
    Binding { act: Act::Permit, on: &[Work], help: "let the lead push the task itself, or take that back", hint: Hint::No },
    Binding { act: Act::Tidy, on: &[Work], help: "tidy commits: its Claude reshapes them by the rules skill", hint: Hint::No },
    Binding { act: Act::Push, on: &[Work, Card], help: "push: rebase, then its card and commits, all or the first few", hint: Hint::No },
    Binding { act: Act::Done, on: &[Work], help: "done: after you pushed elsewhere (asks first)", hint: Hint::No },
    Binding { act: Act::Reset, on: &[Work], help: "reset the task: its work goes, its backlog item returns (asks first)", hint: Hint::No },
    Binding { act: Act::Drop, on: &[Work], help: "drop the task (asks first)", hint: Hint::No },
    Binding { act: Act::Refresh, on: BOARD, help: "ask the lead to refresh the backlog", hint: Hint::No },
    Binding { act: Act::Away, on: BOARD, help: "away mode on or off: the lead settles how-questions and keeps what-questions for you", hint: Hint::No },
    Binding { act: Act::Update, on: BOARD, help: "update everything to the latest (kelpie update); ↑ at the bottom shows when there is a newer version", hint: Hint::No },
    Binding { act: Act::Go, on: &[Areas], help: "open the area's Space", hint: Hint::No },
    Binding { act: Act::New, on: &[Areas], help: "new task in its repo, with its notes", hint: Hint::No },
    Binding { act: Act::AreaRefresh, on: &[Areas], help: "ask the lead to refresh its backlog", hint: Hint::No },
    Binding { act: Act::Log, on: &[Areas], help: "its vault log", hint: Hint::No },
    Binding { act: Act::Permit, on: &[Areas], help: "let the lead push the repo's reviewed tasks, or take that back", hint: Hint::No },
    Binding { act: Act::Push, on: &[Areas], help: "push main when it is ahead (asks first)", hint: Hint::No },
    Binding { act: Act::Fullscreen, on: &[Card], help: "in the card window: the image alone over the whole board, as large as it fits (← → between images; esc, q or f back)", hint: Hint::No },
    Binding { act: Act::Switch, on: &[Card], help: "in the fullscreen image: the previous or next image", hint: Hint::No },
    Binding { act: Act::OpenImage, on: &[Card], help: "in the card window: open the image in the system viewer (not over SSH, and on Linux only with a display)", hint: Hint::No },
    Binding { act: Act::Help, on: ALL, help: "these keys", hint: Hint::Both },
    Binding { act: Act::Back, on: &[Work, Backlog, Areas, Card], help: "one step back: close a window, a menu or the filter; at the top, close the board when it is an overlay", hint: Hint::No },
    Binding { act: Act::Close, on: &[Work, Backlog, Areas, Card], help: "close the current view like esc; at the top, close an overlay board", hint: Hint::No },
    Binding { act: Act::Quit, on: ALL, help: "twice within 2 s: quit the board", hint: Hint::No },
];

pub type Hints = Vec<(String, &'static str, KeyCode)>;

pub struct Keys {
    chars: HashMap<Act, char>,
}

pub fn ctrl_c(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

impl Keys {
    pub fn defaults() -> Keys {
        Keys { chars: Act::EVERY.iter().filter_map(|act| match act.default() { Default::Char(key) => Some((*act, key)), _ => None }).collect() }
    }

    pub fn with(entries: &[(String, String)]) -> (Keys, Vec<String>) {
        let mut keys = Keys::defaults();
        let mut errors = Vec::new();
        let mut wanted: Vec<(Act, char)> = Vec::new();
        for (name, value) in entries {
            let Some(act) = Act::EVERY.into_iter().find(|act| act.id() == name) else {
                errors.push(format!("keys.{name}: not an action; the actions are {}", Act::EVERY.iter().filter(|act| act.remappable()).map(|act| act.id()).collect::<Vec<_>>().join(", ")));
                continue;
            };
            if !act.remappable() {
                errors.push(format!("keys.{name}: {} stays on {}; only actions on a plain character move", act.id(), keys.label(act)));
                continue;
            }
            let mut characters = value.chars();
            match (characters.next(), characters.next()) {
                (Some(key), None) if !key.is_control() && !key.is_whitespace() => wanted.push((act, key)),
                _ => errors.push(format!("keys.{name}: \"{value}\" is not one plain character")),
            }
        }
        for (act, key) in &wanted {
            keys.chars.insert(*act, *key);
        }
        for (act, key) in &wanted {
            if let Some(other) = keys.taken_by(*act, *key) {
                errors.push(format!("keys.{}: {key} is taken by {}", act.id(), other.id()));
                let default = match act.default() { Default::Char(default) => default, _ => continue };
                keys.chars.insert(*act, default);
            }
        }
        let clashing: Vec<Act> = wanted.iter().map(|(act, _)| *act).filter(|act| keys.taken_by(*act, keys.char(*act)).is_some()).collect();
        for act in clashing {
            let default = match act.default() { Default::Char(default) => default, _ => continue };
            keys.chars.insert(act, default);
        }
        (keys, errors)
    }

    fn taken_by(&self, act: Act, key: char) -> Option<Act> {
        let places: Vec<Ctx> = TABLE.iter().filter(|row| row.act == act).flat_map(|row| row.on.iter().copied()).collect();
        Act::EVERY.into_iter().filter(|other| *other != act && self.codes(*other).contains(&KeyCode::Char(key)))
            .find(|other| TABLE.iter().any(|row| row.act == *other && row.on.iter().any(|ctx| places.contains(ctx))))
    }

    pub fn char(&self, act: Act) -> char {
        self.chars.get(&act).copied().unwrap_or(' ')
    }

    pub fn codes(&self, act: Act) -> Vec<KeyCode> {
        match act.default() {
            Default::Char(_) => vec![KeyCode::Char(self.char(act))],
            Default::Fixed(codes, _) => codes.to_vec(),
            Default::CtrlC => Vec::new(),
        }
    }

    pub fn label(&self, act: Act) -> String {
        match act.default() {
            Default::Char(_) => self.char(act).to_string(),
            Default::Fixed(_, label) => label.to_string(),
            Default::CtrlC => "ctrl+c".to_string(),
        }
    }

    pub fn code(&self, act: Act) -> KeyCode {
        self.codes(act).first().copied().unwrap_or(KeyCode::Null)
    }

    // The action a key press means here; ctrl+c is the only key read with its modifier.
    pub fn act(&self, ctx: Ctx, key: &KeyEvent) -> Option<Act> {
        if ctrl_c(key) {
            return Some(Act::Quit);
        }
        TABLE.iter().filter(|row| row.on.contains(&ctx) && row.act != Act::Quit).map(|row| row.act).find(|act| self.codes(*act).contains(&key.code))
    }


    pub fn help(&self, page: &[Ctx]) -> Vec<(String, &'static str)> {
        TABLE.iter().filter(|row| row.on.iter().any(|ctx| page.contains(ctx))).map(|row| (self.label(row.act), row.help)).collect()
    }

    pub fn hints(&self, page: Ctx) -> (Hints, Hints) {
        let entry = |row: &Binding| {
            let key = if row.act == Act::Lead { format!("prefix+{}", self.label(row.act)) } else { self.label(row.act) };
            (key, hint_word(row.act), self.code(row.act))
        };
        let rows: Vec<&Binding> = TABLE.iter().filter(|row| row.on.contains(&page) && row.hint != Hint::No).collect();
        (rows.iter().map(|row| entry(row)).collect(), rows.iter().filter(|row| row.hint == Hint::Both).map(|row| entry(row)).collect())
    }

    pub fn reserved(&self) -> Vec<char> {
        let mut keys: Vec<char> = TABLE.iter().filter(|row| row.on.contains(&Areas) && row.act != Act::Quit).flat_map(|row| self.codes(row.act))
            .filter_map(|code| match code { KeyCode::Char(key) => Some(key), _ => None }).collect();
        keys.sort();
        keys.dedup();
        keys
    }

    pub fn describe(&self, errors: &[String]) -> Vec<String> {
        let mut lines: Vec<String> = Act::EVERY.iter().filter(|act| act.remappable()).map(|act| {
            let default = match act.default() { Default::Char(key) => key, _ => ' ' };
            let layer = if self.char(*act) == default { "default" } else { "machine config" };
            format!("keys.{} = {:?}  ({layer})", act.id(), self.char(*act).to_string())
        }).collect();
        lines.extend(errors.iter().map(|error| format!("  ignored: {error}")));
        lines
    }
}

pub fn hint_word(act: Act) -> &'static str {
    match act {
        Act::Options => "options",
        Act::Inbox => "inbox",
        Act::Messages => "messages",
        Act::Lead => "lead",
        Act::Help => "keys",
        _ => act.id(),
    }
}

static MAP: OnceLock<Keys> = OnceLock::new();

// The keys in effect: the defaults with config.toml's [keys]; tests always see the defaults.
pub fn map() -> &'static Keys {
    MAP.get_or_init(|| {
        if cfg!(test) {
            return Keys::defaults();
        }
        let entries = crate::config::load_machine(&crate::config::path()).map(|machine| machine.keys).unwrap_or_default();
        Keys::with(&entries).0
    })
}

pub fn key(act: Act) -> char {
    map().char(act)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
    }

    #[test]
    fn every_action_has_a_row_and_every_row_one_action_with_one_key_per_place() {
        for act in Act::EVERY {
            assert!(TABLE.iter().any(|row| row.act == act), "{} has no row", act.id());
        }
        let keys = Keys::defaults();
        for ctx in [Work, Backlog, Areas, Card] {
            let mut seen: Vec<(KeyCode, Act)> = Vec::new();
            for row in TABLE.iter().filter(|row| row.on.contains(&ctx)) {
                for code in keys.codes(row.act) {
                    let clash = seen.iter().find(|(other, act)| *other == code && *act != row.act && row.act != Act::Quit && *act != Act::Quit);
                    assert!(clash.is_none(), "{:?} is both {} and {} in {ctx:?}", code, row.act.id(), clash.unwrap().1.id());
                    seen.push((code, row.act));
                }
            }
        }
        let ids: Vec<&str> = Act::EVERY.iter().map(|act| act.id()).collect();
        assert_eq!(ids.len(), ids.iter().collect::<std::collections::HashSet<_>>().len());
    }

    #[test]
    fn the_new_map_puts_inbox_new_chat_push_and_filter_on_their_letters() {
        let keys = Keys::defaults();
        let at = |ctx, code| keys.act(ctx, &KeyEvent::from(code));
        assert_eq!(at(Work, KeyCode::Char('i')), Some(Act::Inbox));
        assert_eq!(at(Work, KeyCode::Char('n')), Some(Act::New));
        assert_eq!(at(Backlog, KeyCode::Char('n')), Some(Act::New));
        assert_eq!(at(Work, KeyCode::Char('c')), Some(Act::Chat));
        assert_eq!(at(Work, KeyCode::Char('p')), Some(Act::Push));
        assert_eq!(at(Work, KeyCode::Char('/')), Some(Act::Filter));
        assert_eq!(at(Work, KeyCode::Char('q')), Some(Act::Close));
        assert_eq!(at(Work, KeyCode::Esc), Some(Act::Back));
        assert_eq!(at(Work, KeyCode::Char('s')), Some(Act::Card), "s shows the card");
        assert_eq!(at(Work, KeyCode::Char('l')), None, "l is free on the board");
        assert_eq!(at(Areas, KeyCode::Char('b')), Some(Act::AreaRefresh));
        assert_eq!(at(Work, KeyCode::Char('b')), Some(Act::Worker));
        assert_eq!(keys.act(Work, &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(Act::Quit));
    }

    #[test]
    fn f_and_o_act_in_the_card_window_only_next_to_its_scroll_push_and_close() {
        let keys = Keys::defaults();
        let at = |ctx, code| keys.act(ctx, &KeyEvent::from(code));
        for ctx in [Work, Backlog, Areas] {
            assert_eq!((at(ctx, KeyCode::Char('f')), at(ctx, KeyCode::Char('o'))), (None, None), "{ctx:?}");
        }
        assert_eq!(at(Card, KeyCode::Char('f')), Some(Act::Fullscreen));
        assert_eq!(at(Card, KeyCode::Char('o')), Some(Act::OpenImage));
        assert_eq!(at(Card, KeyCode::Char('p')), Some(Act::Push));
        assert_eq!(at(Card, KeyCode::Down), Some(Act::Move));
        assert_eq!(at(Card, KeyCode::Right), Some(Act::Switch));
        assert_eq!((at(Card, KeyCode::Esc), at(Card, KeyCode::Char('q'))), (Some(Act::Back), Some(Act::Close)));
        assert!(!Act::Fullscreen.remappable() && !Act::OpenImage.remappable(), "keys inside a window stay where they are");
    }

    #[test]
    fn a_key_moves_by_its_action_id_and_one_already_taken_is_refused_naming_the_other() {
        let (keys, errors) = Keys::with(&entries(&[("inbox", "I"), ("new", "N")]));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!((keys.char(Act::Inbox), keys.char(Act::New)), ('I', 'N'));
        assert_eq!(keys.act(Work, &KeyEvent::from(KeyCode::Char('I'))), Some(Act::Inbox));
        assert_eq!(keys.act(Work, &KeyEvent::from(KeyCode::Char('i'))), None);
        let (keys, errors) = Keys::with(&entries(&[("chat", "p")]));
        assert_eq!(errors, ["keys.chat: p is taken by push"]);
        assert_eq!(keys.char(Act::Chat), 'c', "a refused entry leaves the default");
        let (keys, errors) = Keys::with(&entries(&[("chat", "p"), ("push", "c")]));
        assert!(errors.is_empty(), "moving both in one table works: {errors:?}");
        assert_eq!((keys.char(Act::Chat), keys.char(Act::Push)), ('p', 'c'));
        let (_, errors) = Keys::with(&entries(&[("go", "j")]));
        assert_eq!(errors, ["keys.go: j is taken by move"]);
        let (keys, errors) = Keys::with(&entries(&[("log", "w")]));
        assert!(errors.is_empty(), "w is the worker on Work, but log acts on Areas only: {errors:?}");
        assert_eq!(keys.char(Act::Log), 'w');
    }

    #[test]
    fn only_plain_characters_move_and_fixed_keys_stay() {
        let (keys, errors) = Keys::with(&entries(&[("back", "x"), ("quit", "Q"), ("options", "o"), ("move", "h"), ("inbox", "ii"), ("new", " "), ("nothing", "z")]));
        assert_eq!(errors.len(), 7, "{errors:?}");
        assert!(errors[0].starts_with("keys.back: back stays on esc"), "{errors:?}");
        assert!(errors[1].contains("stays on ctrl+c") && errors[2].contains("stays on ⏎") && errors[3].contains("stays on ↑ ↓ j k"), "{errors:?}");
        assert!(errors[4].contains("\"ii\" is not one plain character") && errors[5].contains("not one plain character"), "{errors:?}");
        assert!(errors[6].starts_with("keys.nothing: not an action"), "{errors:?}");
        assert_eq!(keys.char(Act::Inbox), 'i');
        assert_eq!(keys.label(Act::Back), "esc");
    }

    #[test]
    fn help_and_hints_name_the_key_in_effect() {
        let (keys, _) = Keys::with(&entries(&[("inbox", "I")]));
        let help = keys.help(&[Work, Backlog]);
        assert!(help.iter().any(|(key, text)| key == "I" && text.starts_with("inbox")), "{help:?}");
        assert!(!help.iter().any(|(key, _)| key == "i"));
        let (full, short) = keys.hints(Work);
        let shown: Vec<String> = full.iter().map(|(key, word, _)| format!("{key} {word}")).collect();
        assert_eq!(shown, ["⏎ options", "prefix+tab lead", "I inbox", "m messages", "? keys"]);
        assert_eq!(short.iter().map(|(key, word, _)| format!("{key} {word}")).collect::<Vec<_>>(), ["⏎ options", "I inbox", "? keys"]);
        assert!(full.iter().all(|(_, _, code)| keys.act(Work, &KeyEvent::from(*code)).is_some()), "every hint is a key the board acts on");
    }

    #[test]
    fn a_repo_action_cannot_take_an_areas_key_and_follows_a_moved_one() {
        let keys = Keys::defaults();
        let reserved = keys.reserved();
        for key in ['g', 'n', 'a', 'b', 'l', 'p', 'i', 'q', '?', 'j', 'k', '1', '2'] {
            assert!(reserved.contains(&key), "{key}: {reserved:?}");
        }
        assert!(!reserved.contains(&'s') && !reserved.contains(&'u') && !reserved.contains(&'R'), "{reserved:?}");
        let (moved, _) = Keys::with(&entries(&[("log", "L")]));
        assert!(moved.reserved().contains(&'L') && !moved.reserved().contains(&'l'));
    }
}
