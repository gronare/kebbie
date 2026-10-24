use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::board::widgets::{FAINT, GREEN, MAUVE, MUTED, RED, SURFACE, TEAL, TEXT, bold, cut, style, wrap};
use crate::routine_file::{self, Draft};
use crate::routine_page::{first_when, schedule_words};
use crate::routines::{Mode, Routine, Schedule, State};

pub const STEPS: usize = 5;

pub const MODES: [(&str, &str); 4] = [("Report", "read and tell me"), ("Plan", "propose a fix, I answer"), ("Fix", "commit a fix, lead reviews"), ("Fix and push", "lead pushes once reviewed")];

pub fn mode_of(index: usize) -> (Mode, bool) {
    match index {
        1 => (Mode::Plan, false),
        2 => (Mode::Fix, false),
        3 => (Mode::Fix, true),
        _ => (Mode::Report, false),
    }
}

pub fn mode_index(mode: Mode, push: bool) -> usize {
    match (mode, push) {
        (Mode::Report, _) => 0,
        (Mode::Plan, _) => 1,
        (Mode::Fix, false) => 2,
        (Mode::Fix, true) => 3,
    }
}

pub fn mode_label(mode: Mode, push: bool) -> &'static str {
    if mode == Mode::Fix && push { "fix and push" } else { mode.key() }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Repo {
    pub value: String,
    pub name: String,
    pub shown: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spot {
    Choice(usize),
    Key { at: usize, width: usize, key: KeyEvent },
}

pub enum Step {
    Stay,
    Cancel,
    Write { run: bool },
    Editor,
}

pub struct Wizard {
    pub step: usize,
    pub prompt: String,
    pub repos: Vec<Repo>,
    pub filter: String,
    pub repo_index: usize,
    pub when: String,
    pub mode: usize,
    pub title: String,
    pub editing: Option<Routine>,
    pub taken: Vec<String>,
    pub file: PathBuf,
    pub default_repo: Option<String>,
}

fn hints(lines: &mut Vec<Line<'static>>, spots: &mut Vec<(usize, Spot)>, parts: &[(&str, Option<KeyEvent>)]) {
    let mut at = 0;
    for (index, (text, key)) in parts.iter().enumerate() {
        let width = Span::raw(*text).width();
        if let Some(key) = key {
            spots.push((lines.len(), Spot::Key { at, width, key: *key }));
        }
        at += width + if index + 1 < parts.len() { 3 } else { 0 };
    }
    let text = parts.iter().map(|(text, _)| *text).collect::<Vec<_>>().join(" · ");
    lines.push(Line::from(Span::styled(text, style(MUTED))));
}

fn suggested_title(prompt: &str) -> String {
    let words: Vec<&str> = prompt.split_whitespace().take(4).collect();
    let text = words.join(" ").trim_end_matches(|character: char| !character.is_alphanumeric()).to_string();
    let mut characters = text.chars();
    characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default()
}

impl Wizard {
    pub fn new(prompt: &str, when: &str, mode: usize, title: &str, repos: Vec<Repo>, taken: Vec<String>, file: PathBuf) -> Self {
        Self {
            step: 0, prompt: prompt.to_string(), repos, filter: String::new(), repo_index: 0, when: when.to_string(), mode, title: title.to_string(),
            editing: None, taken, file, default_repo: None,
        }
    }

    // The same steps with the routine's own values; its repo is offered even when no checkout of it is found.
    pub fn edit(routine: &Routine, mut repos: Vec<Repo>, file: PathBuf, default_repo: Option<String>) -> Self {
        let name = routine.repo.rsplit('/').next().unwrap_or(&routine.repo).to_string();
        let index = match repos.iter().position(|repo| repo.value == routine.repo) {
            Some(index) => index,
            None => {
                repos.insert(0, Repo { value: routine.repo.clone(), name, shown: routine.repo.clone() });
                0
            }
        };
        Self {
            step: 0, prompt: routine.prompt.clone(), repos, filter: String::new(), repo_index: index, when: routine_file::when_text(routine.schedule),
            mode: mode_index(routine.mode, routine.push), title: routine.title.clone(), editing: Some(routine.clone()), taken: Vec::new(), file, default_repo,
        }
    }

    fn shown_repos(&self) -> Vec<&Repo> {
        let wanted = self.filter.to_lowercase();
        self.repos.iter().filter(|repo| repo.name.to_lowercase().contains(&wanted)).collect()
    }

    pub fn repo(&self) -> Option<String> {
        match self.shown_repos().get(self.repo_index) {
            Some(repo) => Some(repo.value.clone()),
            None => Some(self.filter.trim().to_string()).filter(|typed| !typed.is_empty()),
        }
    }

    pub fn id(&self) -> Result<String, String> {
        if let Some(routine) = &self.editing {
            return Ok(routine.id.clone());
        }
        match routine_file::slug(&self.title) {
            base if base.is_empty() => Err("the name needs a letter from a to z, for its id".to_string()),
            base => Ok(routine_file::free_id(&base, &self.taken)),
        }
    }

    pub fn draft(&self) -> Result<Draft, String> {
        let (mode, push) = mode_of(self.mode);
        Ok(Draft {
            id: self.id()?, title: self.title.trim().to_string(), repo: self.repo().ok_or("choose a repo")?, schedule: routine_file::parse_when(&self.when)?,
            prompt: self.prompt.trim().to_string(), mode, push, enabled: self.editing.as_ref().is_none_or(|routine| routine.enabled),
            profile: self.editing.as_ref().and_then(|routine| routine.profile.clone()),
        })
    }

    fn buffer(&mut self) -> Option<&mut String> {
        match self.step {
            0 => Some(&mut self.prompt),
            2 => Some(&mut self.when),
            4 => Some(&mut self.title),
            _ => None,
        }
    }

    fn ready(&self) -> bool {
        match self.step {
            0 => !self.prompt.trim().is_empty(),
            1 => self.repo().is_some(),
            2 => routine_file::parse_when(&self.when).is_ok(),
            4 => !self.title.trim().is_empty() && self.draft().is_ok(),
            _ => true,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Step {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            return Step::Cancel;
        }
        match (self.step, key.code) {
            (0, KeyCode::Char('e')) if control => return Step::Editor,
            (_, KeyCode::Char(_)) if control => {}
            (STEPS, KeyCode::Char('y')) => return Step::Write { run: false },
            (STEPS, KeyCode::Char('g')) => return Step::Write { run: true },
            (STEPS, KeyCode::Left) => self.step -= 1,
            (STEPS, _) => {}
            (_, KeyCode::Enter) if self.ready() => {
                self.step += 1;
                if self.step == 4 && self.title.trim().is_empty() {
                    self.title = suggested_title(&self.prompt);
                }
            }
            (1, KeyCode::Up) => self.repo_index = self.repo_index.saturating_sub(1),
            (1, KeyCode::Down) => self.repo_index = (self.repo_index + 1).min(self.shown_repos().len().saturating_sub(1)),
            (1, KeyCode::Char(character)) => {
                self.filter.push(character);
                self.repo_index = 0;
            }
            (1, KeyCode::Backspace) => {
                self.filter.pop();
                self.repo_index = 0;
            }
            (3, KeyCode::Up | KeyCode::Char('k')) => self.mode = self.mode.saturating_sub(1),
            (3, KeyCode::Down | KeyCode::Char('j')) => self.mode = (self.mode + 1).min(MODES.len() - 1),
            (_, KeyCode::Char(character)) => {
                if let Some(buffer) = self.buffer() {
                    buffer.push(character);
                }
            }
            (_, KeyCode::Backspace) => {
                if let Some(buffer) = self.buffer() {
                    buffer.pop();
                }
            }
            _ => {}
        }
        Step::Stay
    }

    // True when the step has a text field to take the pasted text.
    pub fn paste(&mut self, text: &str) -> bool {
        if self.step == 1 {
            self.filter.push_str(text);
            self.repo_index = 0;
            return true;
        }
        if self.step == STEPS {
            return false;
        }
        self.buffer().map(|buffer| buffer.push_str(text)).is_some()
    }

    // True when the choice was already the selected one, which a click takes as ⏎.
    pub fn choose(&mut self, index: usize) -> bool {
        let chosen = match self.step {
            1 => &mut self.repo_index,
            3 => &mut self.mode,
            _ => return false,
        };
        let again = *chosen == index;
        *chosen = index;
        again
    }

    pub fn title(&self) -> String {
        if self.editing.is_some() { "edit routine".to_string() } else { "new routine".to_string() }
    }

    fn input(text: &str, width: usize) -> Vec<Line<'static>> {
        let rows: Vec<String> = text.split('\n').flat_map(|line| { let wrapped = wrap(line, width.saturating_sub(3), 6); if wrapped.is_empty() { vec![String::new()] } else { wrapped } }).collect();
        let count = rows.len();
        rows.into_iter().enumerate().map(|(index, row)| {
            let mut spans = vec![Span::raw(" "), Span::styled(row, style(TEXT))];
            if index + 1 == count {
                spans.push(Span::styled("▏", style(MAUVE)));
            }
            Line::from(spans).style(Style::new().bg(SURFACE))
        }).collect()
    }

    fn choice(selected: bool, label: &str, width: usize, about: &str, about_color: ratatui::style::Color) -> Line<'static> {
        let bar = if selected { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") };
        let label_style = if selected { bold(TEXT) } else { style(TEXT) };
        let line = Line::from(vec![bar, Span::styled(format!("{label:<width$}"), label_style), Span::styled(about.to_string(), style(about_color))]);
        if selected { line.style(Style::new().bg(SURFACE)) } else { line }
    }

    // The second value is each clickable spot with the line it is drawn on.
    pub fn view(&self, now: u64, offset: i64, width: usize) -> (Vec<Line<'static>>, Vec<(usize, Spot)>) {
        let mut spots = Vec::new();
        let heading = |text: &str| Line::from(Span::styled(text.to_string(), bold(MAUVE)));
        let muted = |text: String| Line::from(Span::styled(text, style(MUTED)));
        let mut lines = vec![Line::from(Span::styled(if self.step < STEPS { format!("step {} of {STEPS}", self.step + 1) } else { "check".to_string() }, style(FAINT)))];
        match self.step {
            0 => {
                lines.push(heading("What should it do?"));
                lines.push(muted("In your words; this is its prompt.".to_string()));
                lines.push(Line::default());
                lines.extend(Self::input(&self.prompt, width));
                lines.push(Line::default());
                hints(&mut lines, &mut spots, &[("⏎ next", Some(KeyCode::Enter.into())), ("ctrl-e open in editor", Some(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL))), ("esc cancel", Some(KeyCode::Esc.into()))]);
            }
            1 => {
                lines.push(heading("Which repo?"));
                lines.push(muted("The task runs in a worktree of it.".to_string()));
                lines.push(Line::default());
                if !self.filter.is_empty() {
                    lines.extend(Self::input(&self.filter, width));
                }
                let shown = self.shown_repos();
                let name_width = shown.iter().map(|repo| repo.name.chars().count()).max().unwrap_or(0) + 3;
                let first = self.repo_index.saturating_sub(7);
                for (index, repo) in shown.iter().enumerate().skip(first).take(8) {
                    spots.push((lines.len(), Spot::Choice(index)));
                    lines.push(Self::choice(index == self.repo_index, &repo.name, name_width, &repo.shown, MUTED));
                }
                if shown.is_empty() {
                    lines.push(muted(if self.filter.trim().is_empty() { "No checkouts found under your repo folders; type the repo's path.".to_string() } else { format!("⏎ uses \"{}\" as the repo", self.filter.trim()) }));
                }
                lines.push(Line::default());
                hints(&mut lines, &mut spots, &[("↑ ↓ choose", None), ("type to filter", None), ("⏎ next", Some(KeyCode::Enter.into()))]);
            }
            2 => {
                lines.push(heading("When?"));
                lines.push(muted("07:00 · mon 08:00 · every 6h".to_string()));
                lines.push(Line::default());
                lines.extend(Self::input(&self.when, width));
                lines.push(Line::default());
                lines.push(match routine_file::parse_when(&self.when) {
                    Ok(schedule) => Line::from(Span::styled(format!("  {}", self.when_said(schedule, now, offset)), style(GREEN))),
                    Err(error) => Line::from(Span::styled(format!("  {error}"), style(RED))),
                });
                lines.push(Line::default());
                hints(&mut lines, &mut spots, &[("⏎ next", Some(KeyCode::Enter.into()))]);
            }
            3 => {
                lines.push(heading("What may it do?"));
                lines.push(Line::default());
                for (index, (label, about)) in MODES.iter().enumerate() {
                    spots.push((lines.len(), Spot::Choice(index)));
                    lines.push(Self::choice(index == self.mode, label, 14, about, MUTED));
                }
                lines.push(Line::default());
                hints(&mut lines, &mut spots, &[("↑ ↓ choose", None), ("⏎ next", Some(KeyCode::Enter.into()))]);
            }
            4 => {
                lines.push(heading("Name"));
                lines.push(muted("Shown on the page and in notifications.".to_string()));
                lines.push(Line::default());
                lines.extend(Self::input(&self.title, width));
                lines.push(Line::default());
                lines.push(match self.id() {
                    Ok(id) => muted(format!("id {id}")),
                    Err(error) => Line::from(Span::styled(error, style(RED))),
                });
                lines.push(Line::default());
                hints(&mut lines, &mut spots, &[("⏎ next", Some(KeyCode::Enter.into()))]);
            }
            _ => {
                let file = crate::area_page::home_relative(&self.file);
                lines.push(heading(&if self.editing.is_some() { format!("Save in {file}?") } else { format!("Add to {file}?") }));
                lines.push(Line::default());
                match self.draft() {
                    Ok(draft) => lines.extend(self.preview(&draft, width)),
                    Err(error) => lines.push(Line::from(Span::styled(error, style(RED)))),
                }
                lines.push(Line::default());
                let verb = if self.editing.is_some() { "save" } else { "add" };
                let (add, run) = (format!("y {verb}"), format!("g {verb} and run now"));
                hints(&mut lines, &mut spots, &[(&add, Some(KeyCode::Char('y').into())), (&run, Some(KeyCode::Char('g').into())), ("← back", Some(KeyCode::Left.into())), ("esc cancel", Some(KeyCode::Esc.into()))]);
            }
        }
        (lines, spots)
    }

    fn when_said(&self, schedule: Schedule, now: u64, offset: i64) -> String {
        let first = if self.editing.is_some() { "next run" } else { "first run" };
        match schedule {
            Schedule::Every(_) => format!("{}, {first} within a minute", schedule.text()),
            _ => {
                let state = State { seen: Some(now), ..State::default() };
                format!("{}, {first} {}", schedule_words(schedule), first_when(crate::routines::next_run(schedule, &state, now, offset), now, offset))
            }
        }
    }

    fn preview(&self, draft: &Draft, width: usize) -> Vec<Line<'static>> {
        let table = routine_file::block(draft, self.default_repo.as_deref());
        let prompt_at = table.find("prompt = ").unwrap_or(table.len());
        let mut lines: Vec<Line> = table[..prompt_at].lines().map(|line| Line::from(Span::styled(line.to_string(), style(if line.starts_with('[') { TEAL } else { TEXT })))).collect();
        let first = draft.prompt.lines().next().unwrap_or_default();
        let room = width.saturating_sub(16).max(10);
        let mut shown = cut(first, room);
        if draft.prompt.lines().count() > 1 && !shown.ends_with('…') {
            shown.push('…');
        }
        lines.push(Line::from(Span::styled(format!("prompt = \"\"\"{shown}\"\"\""), style(TEXT))));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repos() -> Vec<Repo> {
        ["homelab-gitops", "greenhouse", "kebbie"].iter().map(|name| Repo { value: format!("~/{name}"), name: name.to_string(), shown: format!("~/{name}") }).collect()
    }

    fn typed(wizard: &mut Wizard, text: &str) {
        for character in text.chars() {
            wizard.key(KeyEvent::from(KeyCode::Char(character)));
        }
    }

    fn enter(wizard: &mut Wizard) -> Step {
        wizard.key(KeyEvent::from(KeyCode::Enter))
    }

    fn screen(wizard: &Wizard) -> String {
        wizard.view(1_792_391_400, 0, 60).0.iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_paste_lands_in_the_steps_text_field_and_the_steps_without_one_turn_it_down() {
        let mut wizard = Wizard::new("", "07:00", 0, "", repos(), Vec::new(), PathBuf::from("/c/config.toml"));
        assert!(wizard.paste("Check CI"));
        assert_eq!(wizard.prompt, "Check CI");
        enter(&mut wizard);
        assert!(wizard.paste("green") && wizard.filter == "green", "the repo step's filter takes it");
        wizard.step = 3;
        assert!(!wizard.paste("x"), "the mode step has no text field");
        wizard.step = STEPS;
        assert!(!wizard.paste("y"), "the check has no text field, and a pasted y must not add the routine");
    }

    #[test]
    fn five_steps_with_enter_all_the_way_make_the_table_the_check_shows() {
        let mut wizard = Wizard::new("", "07:00", 0, "", repos(), vec!["backups".to_string()], PathBuf::from("/home/u/.config/kebbie/config.toml"));
        enter(&mut wizard);
        assert_eq!(wizard.step, 0, "a routine needs a prompt");
        typed(&mut wizard, "Check every node, Longhorn volume and certificate.");
        assert!(screen(&wizard).contains("step 1 of 5\nWhat should it do?\nIn your words; this is its prompt.") && screen(&wizard).contains("⏎ next · ctrl-e open in editor · esc cancel"), "{}", screen(&wizard));
        enter(&mut wizard);
        assert!(screen(&wizard).contains("▌homelab-gitops   ~/homelab-gitops\n greenhouse       ~/greenhouse"), "{}", screen(&wizard));
        enter(&mut wizard);
        assert!(screen(&wizard).contains("every day at 07:00, first run today 07:00"), "06:30 now, so today: {}", screen(&wizard));
        enter(&mut wizard);
        assert!(screen(&wizard).contains("▌Report        read and tell me\n Plan          propose a fix, I answer"), "{}", screen(&wizard));
        enter(&mut wizard);
        assert_eq!(wizard.title, "Check every node, Longhorn", "a blank start suggests a name from the prompt");
        for _ in 0..wizard.title.len() {
            wizard.key(KeyEvent::from(KeyCode::Backspace));
        }
        typed(&mut wizard, "Cluster health");
        assert!(screen(&wizard).contains("id cluster-health"), "{}", screen(&wizard));
        enter(&mut wizard);
        assert_eq!(screen(&wizard), [
            "check", "Add to /home/u/.config/kebbie/config.toml?", "", "[[routines]]", "id = \"cluster-health\"", "title = \"Cluster health\"", "repo = \"~/homelab-gitops\"",
            "at = \"07:00\"", "mode = \"report\"", "prompt = \"\"\"Check every node, Longhorn volume and certi…\"\"\"", "", "y add · g add and run now · ← back · esc cancel",
        ].join("\n"));
        assert!(matches!(wizard.key(KeyEvent::from(KeyCode::Char('g'))), Step::Write { run: true }));
        assert!(matches!(wizard.key(KeyEvent::from(KeyCode::Char('y'))), Step::Write { run: false }));
        wizard.key(KeyEvent::from(KeyCode::Left));
        assert_eq!(wizard.step, 4, "← goes back to the name");
        assert!(matches!(wizard.key(KeyEvent::from(KeyCode::Esc)), Step::Cancel));
    }

    #[test]
    fn each_step_takes_only_an_answer_it_can_use() {
        let mut wizard = Wizard::new("Check CI", "", 0, "", repos(), vec!["ci".to_string()], PathBuf::from("/c/config.toml"));
        enter(&mut wizard);
        typed(&mut wizard, "KEB");
        assert_eq!(wizard.repo().as_deref(), Some("~/kebbie"), "typing filters the repos");
        assert!(screen(&wizard).contains("▌kebbie"), "{}", screen(&wizard));
        typed(&mut wizard, "x");
        assert_eq!(wizard.repo().as_deref(), Some("KEBx"), "a name no checkout has is taken as typed");
        for _ in 0..4 {
            wizard.key(KeyEvent::from(KeyCode::Backspace));
        }
        wizard.key(KeyEvent::from(KeyCode::Down));
        assert_eq!(wizard.repo().as_deref(), Some("~/greenhouse"));
        enter(&mut wizard);
        typed(&mut wizard, "soon");
        enter(&mut wizard);
        assert_eq!(wizard.step, 2, "a time kebbie cannot read keeps the step");
        assert!(screen(&wizard).contains("\"soon\" is not a time"), "{}", screen(&wizard));
        for _ in 0..4 {
            wizard.key(KeyEvent::from(KeyCode::Backspace));
        }
        typed(&mut wizard, "every 6h");
        assert!(screen(&wizard).contains("every 6h, first run within a minute"));
        enter(&mut wizard);
        wizard.key(KeyEvent::from(KeyCode::Down));
        wizard.key(KeyEvent::from(KeyCode::Char('j')));
        wizard.key(KeyEvent::from(KeyCode::Char('j')));
        wizard.key(KeyEvent::from(KeyCode::Char('j')));
        enter(&mut wizard);
        for _ in 0..wizard.title.len() {
            wizard.key(KeyEvent::from(KeyCode::Backspace));
        }
        typed(&mut wizard, "!!");
        enter(&mut wizard);
        assert_eq!(wizard.step, 4, "a name with no letter makes no id");
        assert!(screen(&wizard).contains("the name needs a letter"));
        wizard.key(KeyEvent::from(KeyCode::Backspace));
        wizard.key(KeyEvent::from(KeyCode::Backspace));
        typed(&mut wizard, "CI");
        assert!(screen(&wizard).contains("id ci-2"), "a taken id steps aside: {}", screen(&wizard));
        enter(&mut wizard);
        let draft = wizard.draft().unwrap();
        assert_eq!((draft.id.as_str(), draft.repo.as_str(), draft.schedule, draft.mode, draft.push), ("ci-2", "~/greenhouse", Schedule::Every(6 * 3600), Mode::Fix, true), "the fourth answer is fix and push");
        assert!(screen(&wizard).contains("every = \"6h\"\nmode = \"fix\"\npush = true"), "{}", screen(&wizard));
    }

    #[test]
    fn edit_opens_the_steps_with_the_routines_values_and_keeps_its_id() {
        let routine = Routine {
            id: "deps".to_string(), title: "Deps".to_string(), repo: "infra".to_string(), schedule: Schedule::Weekly(0, 480), prompt: "p".to_string(),
            mode: Mode::Plan, push: false, enabled: false, profile: None, source: "/c/repos/infra.toml".to_string(),
        };
        let mut wizard = Wizard::edit(&routine, repos(), PathBuf::from("/c/repos/infra.toml"), Some("infra".to_string()));
        assert_eq!((wizard.when.as_str(), wizard.mode, wizard.repo().as_deref(), wizard.title()), ("mon 08:00", 1, Some("infra"), "edit routine".to_string()));
        wizard.title = "Dependencies".to_string();
        wizard.step = STEPS;
        let draft = wizard.draft().unwrap();
        assert_eq!((draft.id.as_str(), draft.enabled), ("deps", false), "the id and the pause stay");
        let shown = screen(&wizard);
        assert!(shown.contains("Save in /c/repos/infra.toml?") && !shown.contains("repo =") && shown.contains("y save · g save and run now"), "{shown}");
        assert_eq!(mode_label(Mode::Fix, true), "fix and push");
        for index in 0..MODES.len() {
            let (mode, push) = mode_of(index);
            assert_eq!(mode_index(mode, push), index);
        }
    }
}
