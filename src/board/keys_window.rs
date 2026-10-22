use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::keys::{Act, Ctx, Keys, LEAD_ONLY, TABLE};
use super::widgets::{MAUVE, MUTED, PEACH, TEXT, bold, centered, modal_block, style, take_width, text_width};

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Scroll {
    at: u16,
    max: u16,
}

impl Scroll {
    // None closes the window: every key but the scroll keys, and those too when it all fits.
    pub fn key(self, key: &KeyEvent) -> Option<Scroll> {
        if self.max == 0 {
            return None;
        }
        let at = match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.at.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.at.saturating_add(1),
            KeyCode::PageUp => self.at.saturating_sub(10),
            KeyCode::PageDown | KeyCode::Char(' ') => self.at.saturating_add(10),
            _ => return None,
        };
        Some(Scroll { at: at.min(self.max), max: self.max })
    }
}

enum On {
    Acts(&'static [Act]),
    Own(&'static str),
}

struct Item {
    on: On,
    wide: &'static str,
    narrow: &'static str,
}

struct Group {
    name: &'static str,
    note: (&'static str, &'static str),
    items: &'static [Item],
}

struct Sheet {
    title: &'static str,
    ctxs: &'static [Ctx],
    groups: &'static [Group],
    left: &'static [usize],
    right: &'static [usize],
    narrow: &'static [usize],
    card: Option<usize>,
    others: [(Act, &'static str); 2],
}

const fn bind(acts: &'static [Act], wide: &'static str, narrow: &'static str) -> Item {
    Item { on: On::Acts(acts), wide, narrow }
}

const fn group(name: &'static str, items: &'static [Item]) -> Group {
    Group { name, note: ("", ""), items }
}

const PAGES: &[Act] = &[Act::WorkPage, Act::AreasPage, Act::RoutinesPage];

const WINDOWS: Group = group("WINDOWS", &[
    bind(&[Act::Back], "one step back", "back"),
    bind(&[Act::Close], "close the view", "close"),
    bind(&[Act::Quit], "twice: quit the board", "quit ×2"),
    bind(&[Act::Help], "these keys", "keys"),
]);

const WAITING: Group = group("WAITING ON YOU", &[bind(&[Act::Inbox], "inbox: questions, one at a time", "inbox")]);

const WORK: Sheet = Sheet {
    title: "keys · work",
    ctxs: &[Ctx::Work, Ctx::Backlog, Ctx::Card],
    groups: &[
        group("MOVE AROUND", &[
            bind(&[Act::Move], "move", "move"),
            bind(&[Act::Switch], "work · backlog", "work · backlog"),
            bind(PAGES, "work · areas · routines", "pages"),
            bind(&[Act::Filter], "filter the column", "filter"),
            bind(&[Act::Lead], "the lead's chat", "lead's chat"),
            bind(&[Act::HideChat], "hide or show the lead's chat", "hide chat"),
        ]),
        group("WAITING ON YOU", &[
            bind(&[Act::Inbox], "inbox: questions, one at a time", "inbox"),
            bind(&[Act::Messages], "messages", "messages"),
            bind(&[Act::Away], "away mode", "away mode"),
        ]),
        group("THE BOARD", &[
            bind(&[Act::New], "new task", "new task"),
            bind(&[Act::Refresh], "refresh the backlog", "refresh"),
            bind(&[Act::Update], "update kelpie and its plugins", "update"),
        ]),
        WINDOWS,
        group("THE SELECTED TASK", &[
            bind(&[Act::Options], "options, each one explained", "options"),
            bind(&[Act::Go], "go to its Space", "its Space"),
            bind(&[Act::Chat], "message its Claude", "message it"),
            bind(&[Act::Card], "its question, or its card", "question or card"),
            bind(&[Act::StartAt], "start it later", "start later"),
        ]),
        group("PUSH", &[
            bind(&[Act::Push], "push, or take it out of the line", "push"),
            bind(&[Act::SendReview], "send to review, or withdraw it", "to review"),
            bind(&[Act::Review], "open its review", "open review"),
            bind(&[Act::Permit], "let the lead push it", "lead may push"),
            bind(&[Act::Rebase], "rebase onto main now", "rebase now"),
            bind(&[Act::Follow], "keep it rebased on main, or stop", "follow main"),
            bind(&[Act::Tidy], "tidy its commits", "tidy commits"),
            bind(&[Act::DismissCi], "dismiss its red CI (asks first)", "dismiss red CI"),
        ]),
        Group { name: "END IT", note: ("each asks first", "asks first"), items: &[
            bind(&[Act::Done], "done", "done"),
            bind(&[Act::Drop], "drop", "drop"),
            bind(&[Act::Reset], "reset: its work goes", "reset"),
        ] },
        group("DEV SERVERS", &[
            bind(&[Act::Server], "dev server on or off", "dev server"),
            bind(&[Act::Worker], "worker on or off", "worker"),
        ]),
        group("IN THE CARD WINDOW", &[
            bind(&[Act::Switch], "image", "image"),
            bind(&[Act::Move], "scroll", "scroll"),
            bind(&[Act::Fullscreen], "fullscreen", "fullscreen"),
            bind(&[Act::OpenImage], "open the image", "open image"),
            bind(&[Act::Push], "push", "push"),
        ]),
    ],
    left: &[0, 1, 2, 3],
    right: &[4, 5, 6, 7],
    narrow: &[0, 1, 4, 5, 6, 2, 7, 3, 8],
    card: Some(8),
    others: [(Act::AreasPage, "areas"), (Act::RoutinesPage, "routines")],
};

const AREAS: Sheet = Sheet {
    title: "keys · areas",
    ctxs: &[Ctx::Areas],
    groups: &[
        group("MOVE AROUND", &[
            bind(&[Act::Move], "move", "move"),
            bind(&[Act::Switch], "list · detail", "list · detail"),
            bind(PAGES, "work · areas · routines", "pages"),
            bind(&[Act::Lead], "the lead's chat", "lead's chat"),
        ]),
        WAITING,
        WINDOWS,
        group("THE SELECTED AREA", &[
            bind(&[Act::Options], "its actions; in the detail, run one", "actions"),
            bind(&[Act::Go], "open its Space", "its Space"),
            bind(&[Act::New], "new task in its repo", "new task"),
            bind(&[Act::AreaRefresh], "refresh its backlog", "refresh"),
            bind(&[Act::Log], "its vault log", "vault log"),
            Item { on: On::Own("A–Z"), wide: "the repo's own actions", narrow: "repo's own" },
        ]),
        group("PUSH", &[
            bind(&[Act::Permit], "let the lead push its reviewed tasks", "lead may push"),
            bind(&[Act::Push], "push main when it is ahead", "push main"),
        ]),
    ],
    left: &[0, 1, 2],
    right: &[3, 4],
    narrow: &[0, 1, 3, 4, 2],
    card: None,
    others: [(Act::WorkPage, "work"), (Act::RoutinesPage, "routines")],
};

const ROUTINES: Sheet = Sheet {
    title: "keys · routines",
    ctxs: &[Ctx::Routines],
    groups: &[
        group("MOVE AROUND", &[
            bind(&[Act::Move], "move", "move"),
            bind(PAGES, "work · areas · routines", "pages"),
            bind(&[Act::Lead], "the lead's chat", "lead's chat"),
        ]),
        WAITING,
        group("THE PAGE", &[bind(&[Act::New], "new routine", "new routine")]),
        WINDOWS,
        group("THE SELECTED ROUTINE", &[
            bind(&[Act::Options], "options, each one explained", "options"),
            bind(&[Act::Go], "run it now", "run now"),
            bind(&[Act::Edit], "edit it", "edit"),
            bind(&[Act::Pause], "pause or resume it", "pause"),
            bind(&[Act::Card], "its last report", "last report"),
        ]),
        Group { name: "END IT", note: ("asks first", "asks first"), items: &[bind(&[Act::Delete], "delete it", "delete")] },
    ],
    left: &[0, 1, 2, 3],
    right: &[4, 5],
    narrow: &[0, 1, 4, 5, 2, 3],
    card: None,
    others: [(Act::WorkPage, "work"), (Act::AreasPage, "areas")],
};

fn sheet(page: Ctx) -> &'static Sheet {
    match page {
        Ctx::Areas => &AREAS,
        Ctx::Routines => &ROUTINES,
        _ => &WORK,
    }
}

const KEY: usize = 10;
const COLUMN: usize = 54;
const HINT_AT: usize = 40;
const SLOTS: [(usize, usize); 3] = [(0, 10), (30, 6), (50, 5)];

struct Shown {
    key: String,
    wide: &'static str,
    narrow: &'static str,
    extra: Option<String>,
}

fn acts_on(act: Act, sheet: &Sheet, lead: bool) -> bool {
    (lead || !LEAD_ONLY.contains(&act)) && TABLE.iter().any(|row| row.act == act && row.on.iter().any(|ctx| sheet.ctxs.contains(ctx)))
}

// The Backlog column's own meaning of a key that acts on both columns.
fn in_backlog(act: Act) -> Option<(&'static str, &'static str)> {
    match act {
        Act::New => Some(("new backlog item", "new item")),
        Act::Go => Some(("start it as a task", "start it")),
        _ => None,
    }
}

fn shown_items(keys: &Keys, sheet: &Sheet, group: &Group, lead: bool, update: Option<&str>, backlog: bool) -> Vec<Shown> {
    group.items.iter().filter_map(|item| {
        let (key, extra) = match item.on {
            On::Own(key) => (key.to_string(), None),
            On::Acts(acts) => {
                if !acts.iter().all(|act| acts_on(*act, sheet, lead)) {
                    return None;
                }
                let extra = update.filter(|_| acts == [Act::Update]).map(|version| format!("↑ {version}"));
                (acts.iter().map(|act| keys.label(*act)).collect::<Vec<_>>().join(" "), extra)
            }
        };
        let (wide, narrow) = match item.on {
            On::Acts([act]) if backlog => in_backlog(*act).unwrap_or((item.wide, item.narrow)),
            _ => (item.wide, item.narrow),
        };
        Some(Shown { key, wide, narrow, extra })
    }).collect()
}

fn padded(text: &str, width: usize) -> String {
    let text = take_width(text, width);
    let gap = width.saturating_sub(text_width(&text));
    format!("{text}{}", " ".repeat(gap))
}

fn header(group: &Group, wide: bool) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(group.name, bold(PEACH))];
    let note = if wide { group.note.0 } else { group.note.1 };
    if !note.is_empty() {
        let gap = if wide { (KEY + 12).saturating_sub(group.name.len()).max(2) } else { 2 };
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(note, style(MUTED)));
    }
    spans
}

fn column(keys: &Keys, sheet: &Sheet, groups: &[usize], lead: bool, update: Option<&str>, backlog: bool) -> Vec<Vec<Span<'static>>> {
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    for &index in groups {
        let group = &sheet.groups[index];
        let items = shown_items(keys, sheet, group, lead, update, backlog);
        if items.is_empty() {
            continue;
        }
        if !rows.is_empty() {
            rows.push(Vec::new());
        }
        rows.push(header(group, true));
        for item in items {
            let mut spans = vec![Span::styled(padded(&item.key, KEY), style(MAUVE)), Span::styled(item.wide, style(TEXT))];
            if let Some(extra) = item.extra {
                let gap = HINT_AT.saturating_sub(KEY + text_width(item.wide)).max(2);
                spans.push(Span::raw(" ".repeat(gap)));
                spans.push(Span::styled(take_width(&extra, COLUMN.saturating_sub(KEY + text_width(item.wide) + gap + 1)), style(MUTED)));
            }
            rows.push(spans);
        }
    }
    rows
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|span| text_width(&span.content)).sum()
}

fn pages_line(keys: &Keys, sheet: &Sheet) -> String {
    let [(first, first_name), (second, second_name)] = sheet.others;
    format!("{first_name} ({}) and {second_name} ({}) show their own keys here when you are on them", keys.label(first), keys.label(second))
}

fn wide_lines(keys: &Keys, sheet: &Sheet, lead: bool, update: Option<&str>, backlog: bool) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let left = column(keys, sheet, sheet.left, lead, update, backlog);
    let right = column(keys, sheet, sheet.right, lead, update, backlog);
    let mut body = vec![Line::default()];
    for row in 0..left.len().max(right.len()) {
        let mut spans = left.get(row).cloned().unwrap_or_default();
        let gap = COLUMN.saturating_sub(spans_width(&spans));
        spans.push(Span::raw(" ".repeat(gap)));
        spans.extend(right.get(row).cloned().unwrap_or_default());
        body.push(Line::from(spans));
    }
    body.push(Line::default());
    let mut footer = Vec::new();
    if let Some(card) = sheet.card {
        let mut spans = vec![Span::styled("in the card window", style(MUTED))];
        for (position, item) in shown_items(keys, sheet, &sheet.groups[card], lead, None, false).into_iter().enumerate() {
            spans.push(Span::styled(if position == 0 { "  " } else { " · " }, style(MUTED)));
            spans.push(Span::styled(item.key, style(MAUVE)));
            spans.push(Span::styled(format!(" {}", item.wide), style(MUTED)));
        }
        footer.push(Line::from(spans));
    }
    footer.push(Line::from(Span::styled(format!("{} · any key closes", pages_line(keys, sheet)), style(MUTED))));
    (body, footer)
}

fn flow(items: Vec<Shown>, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cursor = 0;
    let mut next = 0;
    for item in items {
        let word = text_width(item.narrow) + item.extra.as_ref().map_or(0, |extra| 1 + text_width(extra));
        loop {
            let slot = SLOTS.iter().enumerate().skip(next)
                .find(|(_, (start, key))| (spans.is_empty() || *start > cursor) && text_width(&item.key) + 2 <= *key && start + key + word <= width);
            if let Some((index, (start, key))) = slot {
                spans.push(Span::raw(" ".repeat(start - cursor)));
                spans.push(Span::styled(padded(&item.key, *key), style(MAUVE)));
                spans.push(Span::styled(item.narrow, style(TEXT)));
                if let Some(extra) = &item.extra {
                    spans.push(Span::styled(format!(" {extra}"), style(MUTED)));
                }
                cursor = start + key + word;
                next = index + 1;
                break;
            }
            if spans.is_empty() {
                spans.push(Span::styled(format!("{} ", item.key), style(MAUVE)));
                spans.push(Span::styled(item.narrow, style(TEXT)));
                cursor = width;
                next = SLOTS.len();
                break;
            }
            lines.push(Line::from(std::mem::take(&mut spans)));
            (cursor, next) = (0, 0);
        }
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

fn narrow_lines(keys: &Keys, sheet: &Sheet, lead: bool, update: Option<&str>, backlog: bool, width: usize) -> Vec<Line<'static>> {
    let mut body = Vec::new();
    for &index in sheet.narrow {
        let group = &sheet.groups[index];
        let items = shown_items(keys, sheet, group, lead, update, backlog);
        if items.is_empty() {
            continue;
        }
        body.push(Line::from(header(group, false)));
        body.extend(flow(items, width));
    }
    body
}

// The keys window over a page, Backlog for the Work page with that column selected: two columns from 114 screen columns, else rows of up to three keys that scroll.
pub(crate) fn draw(frame: &mut Frame, keys: &Keys, page: Ctx, lead: bool, update: Option<&str>, scroll: &mut Scroll) {
    let sheet = sheet(page);
    let backlog = page == Ctx::Backlog;
    let screen = frame.area();
    let available = screen.width.saturating_sub(4);
    let wide = available >= 110;
    let width = available.min(if wide { 116 } else { 76 });
    let inner = width.saturating_sub(4) as usize;
    let (body, mut footer) = if wide {
        wide_lines(keys, sheet, lead, update, backlog)
    } else {
        (narrow_lines(keys, sheet, lead, update, backlog, inner), vec![Line::from(Span::styled("any key closes", style(MUTED)))])
    };
    let room = (screen.height.saturating_sub(4) as usize).saturating_sub(footer.len()).max(1);
    let visible = body.len().min(room);
    let max = body.len() - visible;
    scroll.max = max as u16;
    scroll.at = scroll.at.min(scroll.max);
    let at = scroll.at as usize;
    if at < max
        && let Some(last) = footer.last_mut()
    {
        last.spans.push(Span::styled(" · ↓ more", style(MUTED)));
    }
    let mut lines: Vec<Line<'static>> = body.into_iter().skip(at).take(visible).collect();
    lines.append(&mut footer);
    let area = centered(screen, width, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(modal_block(sheet.title.to_string())), area);
}

// The update hint beside U: kelpie's own version only, never another component's or an override.
pub(crate) fn update_hint(updates: &[(String, String)]) -> Option<String> {
    updates.iter().find(|(name, _)| name == "kelpie").map(|(_, version)| version.clone())
}

#[cfg(test)]
pub(crate) fn shown(keys: &Keys, page: Ctx, lead: bool) -> Vec<(String, &'static str)> {
    let sheet = sheet(page);
    sheet.groups.iter().flat_map(|group| shown_items(keys, sheet, group, lead, None, page == Ctx::Backlog)).map(|item| (item.key, item.wide)).collect()
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;

    use super::*;
    use crate::board::Mode;
    use crate::board::test_support::*;
    use crate::board::widgets::{MAUVE, PEACH};

    fn rows_of(keys: &Keys, page: Ctx, lead: bool, width: u16, height: u16, scroll: &mut Scroll) -> Vec<String> {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, keys, page, lead, None, scroll)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol().to_string()).collect()).collect()
    }

    fn row_with<'a>(rows: &'a [String], text: &str) -> &'a str {
        rows.iter().find(|row| row.contains(text)).unwrap_or_else(|| panic!("{text} is not on screen:\n{}", rows.join("\n")))
    }

    #[test]
    fn at_150_columns_the_keys_window_has_two_columns_of_groups_with_a_short_line_per_key() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.lead_chat = Some(false);
        app.updates = vec![("kelpie".to_string(), "0.26.0".to_string())];
        press(&mut app, key(KeyCode::Char('?')));
        let (rows, buffer) = render(&mut app, 150, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("╭ keys · work ─"), "{screen}");
        for (left, right) in [("MOVE AROUND", "THE SELECTED TASK"), ("WAITING ON YOU", "p         push, or take it out of the line"), ("i         inbox: questions, one at a time", "v         send to review"), ("↑ ↓ j k   move", "⏎         options, each one explained")] {
            let row = row_with(&rows, left);
            assert!(row.contains(right) && row.find(left) < row.find(right), "{left} sits left of {right}: {row}");
        }
        let (move_x, _) = find(&rows, "MOVE AROUND")[0];
        for group in ["WAITING ON YOU", "THE BOARD", "WINDOWS"] {
            assert_eq!(find(&rows, group)[0].0, move_x, "{group} is in the left column: {screen}");
        }
        let (task_x, _) = find(&rows, "THE SELECTED TASK")[0];
        for group in ["PUSH", "END IT", "DEV SERVERS"] {
            assert_eq!(find(&rows, group)[0].0, task_x, "{group} is in the right column: {screen}");
        }
        assert!(row_with(&rows, "END IT").contains("each asks first"));
        assert!(row_with(&rows, "U         update kelpie").contains("↑ 0.26.0"), "{screen}");
        assert!(screen.contains("h         hide or show the lead's chat") && screen.contains("ctrl+c    twice: quit the board"), "{screen}");
        assert!(screen.contains("in the card window  ← → image · ↑ ↓ j k scroll · f fullscreen · o open the image · p push"), "{screen}");
        assert!(screen.contains("areas (2) and routines (3) show their own keys here when you are on them · any key closes") && !screen.contains("↓ more"), "{screen}");
        assert!(!screen.contains("one at a time with tab") && !screen.contains("worktree.dev_server"), "the long texts stay out: {screen}");
        let (x, y) = find(&rows, "THE SELECTED TASK")[0];
        assert_eq!((buffer[(x as u16, y as u16)].fg, buffer[(x as u16 - 54 , y as u16)].fg), (PEACH, PEACH));
        let (x, y) = find(&rows, "⏎         options")[0];
        assert_eq!(buffer[(x as u16, y as u16)].fg, MAUVE);
        let (x, y) = find(&rows, "↑ 0.26.0")[0];
        assert_eq!(buffer[(x as u16, y as u16)].fg, MUTED);
        press(&mut app, key(KeyCode::Down));
        assert!(matches!(app.mode, Mode::Normal), "everything fits, so any key closes");
    }

    #[test]
    fn at_80_columns_the_keys_window_packs_up_to_three_keys_a_row_and_scrolls() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        press(&mut app, key(KeyCode::Char('?')));
        let (rows, _) = render(&mut app, 80, 20);
        let screen = rows.join("\n");
        assert!(screen.contains("╭ keys · work ─"), "{screen}");
        let row = row_with(&rows, "↑ ↓ j k   move");
        assert!(row.contains("← →   work · backlog"), "{row}");
        let row = row_with(&rows, "1 2 3     pages");
        assert!(row.contains("/     filter") && row.contains("tab  lead's chat"), "{row}");
        let row = row_with(&rows, "p         push");
        assert!(row.contains("v     to review") && row.contains("r    open review"), "{row}");
        assert!(row_with(&rows, "END IT").contains("asks first"));
        assert!(screen.contains("any key closes · ↓ more") && !screen.contains("DEV SERVERS"), "{screen}");
        for _ in 0..20 {
            press(&mut app, key(KeyCode::Down));
        }
        assert!(matches!(app.mode, Mode::Help(_)), "↓ scrolls instead of closing");
        let (rows, _) = render(&mut app, 80, 20);
        let screen = rows.join("\n");
        assert!(screen.contains("DEV SERVERS") && screen.contains("IN THE CARD WINDOW") && !screen.contains("↓ more") && !screen.contains("MOVE AROUND"), "{screen}");
        press(&mut app, key(KeyCode::PageUp));
        let (rows, _) = render(&mut app, 80, 20);
        assert!(rows.join("\n").contains("MOVE AROUND"));
        press(&mut app, key(KeyCode::Char('x')));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn on_the_areas_page_the_window_shows_the_areas_keys_alone() {
        let rows = rows_of(crate::keys::map(), Ctx::Areas, false, 150, 40, &mut Scroll::default());
        let screen = rows.join("\n");
        assert!(screen.contains("╭ keys · areas ─") && screen.contains("THE SELECTED AREA") && screen.contains("l         its vault log") && screen.contains("A–Z       the repo's own actions"), "{screen}");
        assert!(screen.contains("work (1) and routines (3) show their own keys here"), "{screen}");
        for gone in ["THE SELECTED TASK", "DEV SERVERS", "send to review", "away mode", "in the card window", "filter the column"] {
            assert!(!screen.contains(gone), "{gone}: {screen}");
        }
        let mut scroll = Scroll::default();
        let rows = rows_of(crate::keys::map(), Ctx::Routines, false, 80, 30, &mut scroll);
        let screen = rows.join("\n");
        assert!(screen.contains("keys · routines") && screen.contains("THE SELECTED ROUTINE") && screen.contains("D         delete") && !screen.contains("PUSH"), "{screen}");
    }

    #[test]
    fn a_moved_key_shows_and_the_chat_key_only_with_the_lead() {
        let (keys, errors) = Keys::with(&[("inbox".to_string(), "I".to_string())]);
        assert!(errors.is_empty());
        let rows = rows_of(&keys, Ctx::Work, false, 150, 40, &mut Scroll::default());
        let screen = rows.join("\n");
        assert!(screen.contains("I         inbox: questions") && !screen.contains("i         inbox") && !screen.contains("hide or show the lead's chat"), "{screen}");
        let rows = rows_of(&keys, Ctx::Work, true, 150, 40, &mut Scroll::default());
        assert!(rows.join("\n").contains("h         hide or show the lead's chat"));
        let rows = rows_of(&keys, Ctx::Work, false, 80, 40, &mut Scroll::default());
        assert!(!rows.join("\n").contains("hide chat"));
    }

    #[test]
    fn beside_u_only_kelpies_own_version_shows_and_never_moves_the_right_column() {
        let updates = |pairs: &[(&str, &str)]| pairs.iter().map(|(name, version)| (name.to_string(), version.to_string())).collect::<Vec<_>>();
        assert_eq!(update_hint(&updates(&[("running", "override /tmp/kelpie"), ("vault", "0.2.17")])), None);
        assert_eq!(update_hint(&updates(&[("running", "override /tmp/kelpie"), ("kelpie", "0.26.0")])).as_deref(), Some("0.26.0"));
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        app.updates = updates(&[("running", "override /tmp/kelpie"), ("vault", "0.2.17")]);
        press(&mut app, key(KeyCode::Char('?')));
        let (rows, _) = render(&mut app, 150, 40);
        let row = row_with(&rows, "U         update kelpie and its plugins");
        assert!(!row.contains('↑') && !row.contains("override") && !row.contains("vault"), "{row}");
        app.updates = updates(&[("kelpie", "0.26.0-a-very-long-prerelease-name")]);
        let (rows, _) = render(&mut app, 150, 40);
        let (task_x, _) = find(&rows, "THE SELECTED TASK")[0];
        let (end_x, _) = find(&rows, "END IT")[0];
        assert!(row_with(&rows, "U         update kelpie").contains("↑ 0.26.0") && end_x == task_x, "{}", rows.join("\n"));
        let row = row_with(&rows, "U         update kelpie");
        assert!(!row.contains("prerelease-name"), "the hint is clipped to the left column: {row}");
    }

    #[test]
    fn with_the_backlog_column_selected_n_and_g_say_what_they_do_there() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        press(&mut app, key(KeyCode::Char('?')));
        let (rows, _) = render(&mut app, 150, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("n         new task") && screen.contains("g         go to its Space"), "{screen}");
        press(&mut app, key(KeyCode::Char('x')));
        app.column = crate::board::Column::Backlog;
        press(&mut app, key(KeyCode::Char('?')));
        let (rows, _) = render(&mut app, 150, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("keys · work") && screen.contains("n         new backlog item") && screen.contains("g         start it as a task") && !screen.contains("go to its Space"), "{screen}");
        let (rows, _) = render(&mut app, 80, 40);
        let screen = rows.join("\n");
        assert!(screen.contains("g     start it") && screen.contains("n         new item"), "{screen}");
    }

    #[test]
    fn every_key_the_page_acts_on_is_in_its_window() {
        let keys = Keys::defaults();
        for page in [Ctx::Work, Ctx::Areas, Ctx::Routines] {
            let shown = shown(&keys, page, true);
            for row in TABLE.iter().filter(|row| row.on.iter().any(|ctx| sheet(page).ctxs.contains(ctx))) {
                let label = keys.label(row.act);
                assert!(shown.iter().any(|(key, _)| key.split(' ').any(|part| part == label) || *key == label), "{} ({label}) is missing from the {page:?} keys window", row.act.id());
            }
            for group in sheet(page).groups {
                for item in group.items {
                    assert!(text_width(item.wide) <= 40 && text_width(item.narrow) <= 16, "{} is too long", item.wide);
                }
            }
        }
    }
}
