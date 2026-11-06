use unicode_width::UnicodeWidthStr;

use crate::doctor::{Check, Level};

const TASKS: &str = "Tasks";
const GROUP_WIDTH: usize = 14;

pub const GROUPS: [(&str, &[&str]); 9] = [
    ("kebbie", &["updates", "plugin binary", "keeper", "board", "kebbie on PATH", "current link", "herdr link"]),
    ("Claude Code", &["claude", "claude login", "claude plugin", "claude plugin source", "status line", "claude model"]),
    ("Codex", &["codex", "codex login", "codex plugin", "codex old plugin", "codex hooks", "codex approvals", "codex model", "codex profile", "codex update", "codex vault"]),
    ("Vault", &["vault", "machine", "backlog vault"]),
    ("herdr", &["herdr", "herdr plugin"]),
    ("Tools", &["git", "jq", "curl", "nvim", "tig", "cargo"]),
    ("Files", &["config", "repo config", "state dir", "task hooks", "setup skill"]),
    (TASKS, &["sandbox", "push block", "setup command", "task profile", "profile", "reviewer", "docs", "comments", "backlog", "backlog local", "backlog linear"]),
    ("Signals", &["maintainers", "reports", "hub", "notify"]),
];

fn place(name: &str) -> (&'static str, usize) {
    GROUPS.iter().find_map(|(group, names)| names.iter().position(|known| *known == name).map(|at| (*group, at))).unwrap_or((TASKS, usize::MAX))
}

fn members<'a>(checks: &'a [Check], group: &str) -> Vec<&'a Check> {
    let mut members: Vec<&Check> = checks.iter().filter(|check| place(&check.name).0 == group).collect();
    members.sort_by_key(|check| place(&check.name).1);
    members
}

pub struct Screen {
    pub width: usize,
    pub colour: bool,
    pub all: bool,
}

impl Screen {
    pub fn current(all: bool) -> Self {
        let width = ratatui::crossterm::terminal::size().map_or(80, |(width, _)| width as usize).max(40);
        Screen { width, colour: true, all }
    }
}

pub fn wanted(plain: bool) -> bool {
    !plain && crate::hub::card::colour_for(std::io::IsTerminal::is_terminal(&std::io::stdout())) != crate::hub::card::Colour::None
}

#[derive(Clone, Copy)]
enum Tone {
    Good,
    Wait,
    Bad,
    Dim,
    Bold,
}

fn paint(text: &str, tone: Tone, colour: bool) -> String {
    let code = match tone {
        Tone::Good => "32",
        Tone::Wait => "33",
        Tone::Bad => "31",
        Tone::Dim => "2",
        Tone::Bold => "1",
    };
    if colour && !text.is_empty() { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
}

fn mark(level: Level) -> (&'static str, Tone) {
    match level {
        Level::Ok => ("✓", Tone::Good),
        Level::Skip => ("?", Tone::Dim),
        Level::Warn => ("!", Tone::Wait),
        Level::Fail => ("✗", Tone::Bad),
    }
}

fn rank(level: Level) -> u8 {
    match level {
        Level::Ok => 0,
        Level::Skip => 1,
        Level::Warn => 2,
        Level::Fail => 3,
    }
}

fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn pad(text: &str, to: usize) -> String {
    format!("{text}{}", " ".repeat(to.saturating_sub(width(text))))
}

pub fn wrap(text: &str, room: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && width(&line) + 1 + width(word) > room {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);
    lines
}

pub fn cut(text: &str, room: usize) -> String {
    if width(text) <= room {
        return text.to_string();
    }
    let mut shown = String::new();
    for character in text.chars() {
        if width(&shown) + width(character.encode_utf8(&mut [0; 4])) + 1 > room {
            break;
        }
        shown.push(character);
    }
    format!("{}…", shown.trim_end())
}

fn fact(check: &Check) -> String {
    if check.level == Level::Skip && !check.fact.contains("not checked") { format!("not checked: {}", check.fact) } else { check.fact.clone() }
}

fn row(check: &Check, indent: usize, name_width: usize, screen: &Screen) -> Vec<String> {
    let (sign, tone) = mark(check.level);
    let column = indent + 2 + name_width;
    let room = screen.width.saturating_sub(column).max(20);
    let mut lines = Vec::new();
    for (at, text) in wrap(&fact(check), room).into_iter().enumerate() {
        if at == 0 {
            lines.push(format!("{}{} {}{text}", " ".repeat(indent), paint(sign, tone, screen.colour), pad(&check.name, name_width)));
        } else {
            lines.push(format!("{}{text}", " ".repeat(column)));
        }
    }
    for fix in &check.fix {
        for (at, text) in wrap(fix, room.saturating_sub(2).max(18)).into_iter().enumerate() {
            let lead = if at == 0 { "→ " } else { "  " };
            lines.push(format!("{}{}", " ".repeat(column), paint(&format!("{lead}{text}"), Tone::Dim, screen.colour)));
        }
    }
    lines
}

pub fn summary(checks: &[&Check]) -> String {
    let mut briefs: Vec<(String, usize)> = Vec::new();
    for check in checks.iter().filter(|check| check.level == Level::Ok) {
        let brief = if check.brief.is_empty() { check.name.clone() } else { check.brief.clone() };
        match briefs.iter_mut().find(|(seen, _)| *seen == brief) {
            Some((_, count)) => *count += 1,
            None => briefs.push((brief, 1)),
        }
    }
    briefs.into_iter().map(|(brief, count)| if count > 1 { format!("{count} {brief}s") } else { brief }).collect::<Vec<_>>().join(" · ")
}

fn totals(checks: &[Check]) -> String {
    let count = |wanted: &[Level]| checks.iter().filter(|check| wanted.contains(&check.level)).count();
    let mut parts = vec![format!("{} ok", count(&[Level::Ok]))];
    let look = count(&[Level::Warn, Level::Fail]);
    if look > 0 {
        parts.push(format!("{look} to look at"));
    }
    let skipped = count(&[Level::Skip]);
    if skipped > 0 {
        parts.push(format!("{skipped} not checked"));
    }
    parts.join(" · ")
}

pub fn render(checks: &[Check], fixed: &[String], screen: &Screen) -> String {
    let title = "kebbie doctor";
    let totals = totals(checks);
    let gap = screen.width.saturating_sub(width(title) + width(&totals)).max(2);
    let mut lines = vec![format!("{}{}{totals}", paint(title, Tone::Bold, screen.colour), " ".repeat(gap)), String::new()];
    if !fixed.is_empty() {
        lines.push(paint(" Fixed", Tone::Bold, screen.colour));
        for line in fixed {
            lines.extend(wrap(line, screen.width.saturating_sub(4).max(20)).into_iter().enumerate().map(|(at, text)| format!("{}{text}", if at == 0 { "  · " } else { "    " })));
        }
        lines.push(String::new());
    }
    if screen.all {
        let name_width = checks.iter().map(|check| width(&check.name)).max().unwrap_or(0) + 2;
        for (name, _) in GROUPS {
            let members = members(checks, name);
            if members.is_empty() {
                continue;
            }
            lines.push(paint(&format!(" {name}"), Tone::Bold, screen.colour));
            for check in members {
                lines.extend(row(check, 2, name_width, screen));
            }
            lines.push(String::new());
        }
        lines.pop();
        return lines.join("\n");
    }
    let waiting: Vec<&Check> = checks.iter().filter(|check| check.level != Level::Ok).collect();
    if !waiting.is_empty() {
        let name_width = waiting.iter().map(|check| width(&check.name)).max().unwrap_or(0) + 2;
        lines.push(paint(" Needs you", Tone::Bold, screen.colour));
        for check in waiting {
            lines.extend(row(check, 2, name_width, screen));
        }
        lines.push(String::new());
    }
    for (name, _) in GROUPS {
        let members = members(checks, name);
        let Some(worst) = members.iter().map(|check| check.level).max_by_key(|level| rank(*level)) else { continue };
        let (sign, tone) = mark(worst);
        let room = screen.width.saturating_sub(1 + GROUP_WIDTH + 2);
        let text = cut(&summary(&members), room);
        lines.push(format!(" {}{} {text}", pad(name, GROUP_WIDTH), paint(sign, tone, screen.colour)).trim_end().to_string());
    }
    lines.push(String::new());
    lines.push(paint(" kebbie doctor --all shows every check", Tone::Dim, screen.colour));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(level: Level, name: &str, fact: &str, fix: &[&str], brief: &str) -> Check {
        Check::new(level, name, fact, fix.iter().map(|fix| fix.to_string()).collect()).brief(brief)
    }

    fn screen(width: usize, all: bool) -> Screen {
        Screen { width, colour: false, all }
    }

    fn machine() -> Vec<Check> {
        vec![
            check(Level::Ok, "git", "/usr/bin/git", &[], "git"),
            check(Level::Ok, "herdr", "0.9.5 at /opt/homebrew/bin/herdr", &[], "0.9.5"),
            check(Level::Ok, "plugin binary", "/k/target/release/kebbie, built from source", &[], "built from source"),
            check(Level::Ok, "keeper", "running, pid 1", &[], "keeper running"),
            check(Level::Ok, "state dir", "/s (backlog)", &[], "state dir"),
            check(Level::Ok, "state dir", "/s/roles (roles)", &[], "state dir"),
            check(Level::Ok, "config", "/c/config.toml parses", &[], "config"),
            check(Level::Warn, "reports", "11 reports wait in quarantine", &["allow or block their senders on the issues endpoint"], ""),
            check(Level::Warn, "sandbox", "off (task.sandbox = false): a task's shell commands run unsandboxed and can pass as you to kebbie's user-only commands", &["set task.sandbox = true (now from the machine config)"], ""),
            check(Level::Skip, "claude login", "login not checked: no answer to its status command", &[], ""),
        ]
    }

    #[test]
    fn the_compact_view_puts_what_needs_you_first_then_one_line_per_group() {
        let shown = render(&machine(), &[], &screen(80, false));
        assert_eq!(shown, [
            "kebbie doctor                                7 ok · 2 to look at · 1 not checked",
            "",
            " Needs you",
            "  ! reports       11 reports wait in quarantine",
            "                  → allow or block their senders on the issues endpoint",
            "  ! sandbox       off (task.sandbox = false): a task's shell commands run",
            "                  unsandboxed and can pass as you to kebbie's user-only commands",
            "                  → set task.sandbox = true (now from the machine config)",
            "  ? claude login  login not checked: no answer to its status command",
            "",
            " kebbie        ✓ built from source · keeper running",
            " Claude Code   ?",
            " herdr         ✓ 0.9.5",
            " Tools         ✓ git",
            " Files         ✓ config · 2 state dirs",
            " Tasks         !",
            " Signals       !",
            "",
            " kebbie doctor --all shows every check",
        ].join("\n"));
    }

    #[test]
    fn nothing_to_look_at_leaves_out_the_needs_you_section() {
        let fine: Vec<Check> = machine().into_iter().filter(|check| check.level == Level::Ok).collect();
        let shown = render(&fine, &[], &screen(80, false));
        assert!(!shown.contains("Needs you"), "{shown}");
        assert!(shown.lines().next().unwrap().ends_with("  7 ok"), "{shown}");
    }

    #[test]
    fn wrapping_breaks_between_words_and_a_group_line_is_cut_to_the_width() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
        assert_eq!(wrap("unbreakable-word here", 5), ["unbreakable-word", "here"], "a word longer than the room is never split");
        assert_eq!(cut("main @ f8dfadf · built from source", 20), "main @ f8dfadf · bu…");
        let many: Vec<Check> = (0..20).map(|at| check(Level::Ok, "git", "", &[], &format!("tool{at}"))).collect();
        let shown = render(&many, &[], &screen(50, false));
        let tools = shown.lines().find(|line| line.starts_with(" Tools")).unwrap();
        assert!(width(tools) <= 50 && tools.ends_with('…'), "{tools}");
    }

    #[test]
    fn all_lists_every_check_under_its_group_with_fixes_and_fixed_lines_on_top() {
        let shown = render(&machine(), &["linked kebbie in herdr again".to_string()], &screen(80, true));
        assert!(shown.contains(" Fixed\n  · linked kebbie in herdr again\n"), "{shown}");
        assert!(shown.contains(" Files\n  ✓ config         /c/config.toml parses\n  ✓ state dir      /s (backlog)\n  ✓ state dir      /s/roles (roles)\n"), "{shown}");
        assert!(shown.contains(" Signals\n  ! reports        11 reports wait in quarantine\n                   → allow or block"), "{shown}");
        assert!(!shown.contains("--all shows every check"), "{shown}");
    }

    #[test]
    fn colour_marks_the_verdicts_and_dims_the_fixes() {
        let shown = render(&machine(), &[], &Screen { width: 80, colour: true, all: false });
        assert!(shown.contains("\x1b[33m!\x1b[0m reports") && shown.contains("\x1b[2m→ allow or block") && shown.contains("\x1b[32m✓\x1b[0m built from source"), "{shown}");
    }

    #[test]
    fn every_check_doctor_makes_belongs_to_one_group() {
        let names = [
            ("plugin binary", "kebbie"), ("current link", "kebbie"), ("kebbie on PATH", "kebbie"), ("updates", "kebbie"), ("keeper", "kebbie"), ("board", "kebbie"), ("herdr link", "kebbie"),
            ("claude", "Claude Code"), ("claude login", "Claude Code"), ("claude plugin", "Claude Code"), ("claude plugin source", "Claude Code"), ("status line", "Claude Code"), ("claude model", "Claude Code"),
            ("unknown check", "Tasks"),
            ("codex", "Codex"), ("codex plugin", "Codex"), ("codex old plugin", "Codex"), ("codex hooks", "Codex"), ("codex model", "Codex"), ("codex login", "Codex"), ("codex update", "Codex"), ("codex profile", "Codex"), ("codex approvals", "Codex"), ("codex vault", "Codex"),
            ("vault", "Vault"), ("backlog vault", "Vault"), ("machine", "Vault"),
            ("herdr", "herdr"), ("herdr plugin", "herdr"),
            ("git", "Tools"), ("jq", "Tools"), ("curl", "Tools"), ("nvim", "Tools"), ("tig", "Tools"), ("cargo", "Tools"),
            ("state dir", "Files"), ("config", "Files"), ("repo config", "Files"), ("task hooks", "Files"), ("setup skill", "Files"),
            ("sandbox", "Tasks"), ("push block", "Tasks"), ("setup command", "Tasks"), ("docs", "Tasks"), ("comments", "Tasks"), ("task profile", "Tasks"), ("profile", "Tasks"), ("reviewer", "Tasks"), ("backlog", "Tasks"), ("backlog local", "Tasks"), ("backlog linear", "Tasks"),
            ("maintainers", "Signals"), ("reports", "Signals"), ("hub", "Signals"), ("notify", "Signals"),
        ];
        for (name, wanted) in names {
            assert_eq!(place(name).0, wanted, "{name}");
        }
    }
}
