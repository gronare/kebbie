use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Mode, Page, Tab, home_relative};
use super::chooser::Typed;
use crate::agent::Agent;
use crate::board::widgets::{self, BLUE, FAINT, GREEN, MANTLE, MAUVE, MUTED, PEACH, RED, SUBTEXT, SURFACE, Spot, TEAL, TEXT, YELLOW, bold, cut, fit, group_line, modal, modal_spots, options_lines, style, text_width, wrap};
use crate::config::{Kind, Layer};
use crate::profiles::{self, Profile};

const POPUP: usize = 64;

impl Page {
    pub(super) fn summary(&self, profile: &Profile) -> String {
        let model = self.model_said(profile);
        let mut said = vec![format!("{}: {}, {model}", profile.name, profile.agent.key())];
        said.extend(profile.effort.clone());
        let mut text = said.join(", ");
        if let Some(role) = profiles::role_name(&self.paths.profiles, profile) {
            text.push_str(&format!(" · role {role}"));
        }
        if profiles::built_in(&profile.name).is_some() && let Some(file) = &profile.file {
            text.push_str(&format!(" · your settings in {}", self.shown_file(file)));
        }
        text
    }

    pub(super) fn layer_word(&self, layer: Layer) -> (String, Color) {
        match layer {
            Layer::Default => ("default".to_string(), MUTED),
            Layer::Machine => ("machine".to_string(), BLUE),
            Layer::Repo if self.linked() => ("team link".to_string(), PEACH),
            Layer::Repo => (format!("repo {}", self.repo_name()), TEAL),
        }
    }

    pub fn hint_lines(&self) -> (Line<'static>, Line<'static>) {
        let line = |hints: &[&str]| Line::from(Span::styled(hints.join(" · "), style(MUTED)));
        if self.on_profiles() {
            return (
                line(&["↑ ↓ profile", "← → section", "m model", "e effort", "r role", "n new", "⏎ open", "d default", "x delete", "tab machine / repo", "/ search", "esc board"]),
                line(&["↑ ↓ profile", "m model", "e effort", "r role", "n new", "tab repo", "esc board"]),
            );
        }
        let change = match self.current().map(|entry| entry.kind) {
            Some(Kind::Bool) => "space toggle",
            Some(Kind::Choice(_) | Kind::Skill | Kind::Profile | Kind::ReviewProfile | Kind::SecondOpinion) => "⏎ pick",
            _ => "⏎ edit",
        };
        (
            line(&["↑ ↓ key", "← → section", change, "d back to default", "tab machine / repo", "/ search", "esc board"]),
            line(&["↑ ↓ key", change, "d default", "tab repo", "esc board"]),
        )
    }

    pub(super) fn row_lines(&self, width: usize) -> (Vec<Line<'static>>, usize) {
        let sections = self.sections();
        let Some((_, rows)) = sections.get(self.section) else {
            let empty = if self.filter_text().is_empty() { "no keys" } else { "no key matches" };
            return (vec![Line::from(Span::styled(empty.to_string(), style(MUTED)))], 0);
        };
        let key_width = rows.iter().map(|index| self.entries[*index].key.len()).max().unwrap_or(0).min(width / 2);
        let layer_width = 12usize;
        let value_width = width.saturating_sub(key_width + layer_width + 6);
        let (mut lines, mut selected) = (Vec::new(), 0);
        for (at, index) in rows.iter().enumerate() {
            let entry = &self.entries[*index];
            let chosen = at == self.row;
            let value = match (&self.mode, chosen) {
                (Mode::Edit { buffer, .. }, true) => Span::styled(format!("{buffer}▏"), bold(TEXT)),
                _ if entry.kind == Kind::Bool && entry.value == "true" => Span::styled("● on".to_string(), style(GREEN)),
                _ if entry.kind == Kind::Bool => Span::styled("○ off".to_string(), style(MUTED)),
                _ => Span::styled(cut(&entry.value, value_width), style(TEXT)),
            };
            let (layer, color) = self.layer_word(entry.layer);
            let used = text_width(&value.content);
            let bar = if chosen { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") };
            let key = Span::styled(format!(" {:<key_width$}  ", cut(entry.key, key_width)), if chosen { bold(TEXT) } else { style(TEXT) });
            let gap = Span::raw(" ".repeat(value_width.saturating_sub(used) + 1));
            let line = Line::from(vec![bar, key, value, gap, Span::styled(format!("{layer:>layer_width$}"), style(color))]);
            if chosen {
                selected = lines.len();
                lines.push(line.style(Style::new().bg(SURFACE)));
                lines.extend(wrap(&entry.about, width.saturating_sub(4), 8).into_iter().map(|text| Line::from(Span::styled(format!("    {text}"), style(SUBTEXT)))));
                lines.push(Line::default());
            } else {
                lines.push(line);
            }
        }
        (lines, selected)
    }

    pub(super) fn adapter(&self, profile: &Profile, chosen: bool) -> Vec<Span<'static>> {
        let agents = &self.agents;
        let dot = || Span::styled(" · ", style(MUTED));
        let mut spans = vec![Span::styled(profile.agent.key().to_string(), style(if profile.agent == Agent::Codex { BLUE } else { TEAL }))];
        let warned = agents.warning(profile).is_some();
        match (&self.mode, chosen) {
            (Mode::Edit { typed: Typed::Model, buffer }, true) => spans.extend([dot(), Span::styled(format!("{buffer}▏"), bold(TEXT))]),
            _ => {
                if let Some(model) = agents.model(profile) {
                    spans.extend([dot(), Span::styled(model, style(if warned { RED } else if agents.own_model(profile).is_some() { TEXT } else { MUTED }))]);
                }
                if warned {
                    spans.push(Span::styled(" ⚠", style(RED)));
                }
            }
        }
        if let Some(effort) = &profile.effort {
            spans.extend([dot(), Span::styled(effort.clone(), style(TEXT))]);
        }
        spans
    }

    pub(super) fn role_spans(&self, profile: &Profile) -> Vec<Span<'static>> {
        let own = profiles::role_name(&self.paths.profiles, profile).map(|role| Span::styled(format!("+ {role}"), style(BLUE)));
        match (profiles::built_in(&profile.name).is_some(), own) {
            (true, Some(own)) => vec![Span::styled(format!("{} ", profiles::kebbie_role_for(profile).0), style(MUTED)), own],
            (_, Some(own)) => vec![own],
            (_, None) => vec![Span::styled(profiles::kebbie_role_for(profile).0.to_string(), style(MUTED))],
        }
    }

    pub(super) fn profile_lines(&self, width: usize) -> (Vec<Line<'static>>, usize) {
        let agents = &self.agents;
        let name_width = agents.profiles.iter().map(|profile| text_width(&profile.name)).max().unwrap_or(0).min(width / 4) + 2;
        let adapters: Vec<Vec<Span<'static>>> = agents.profiles.iter().enumerate().map(|(at, profile)| self.adapter(profile, at == self.row)).collect();
        let adapter_width = adapters.iter().map(|spans| spans.iter().map(Span::width).sum::<usize>()).max().unwrap_or(0) + 3;
        let role_width = agents.profiles.iter().map(|profile| self.role_spans(profile).iter().map(Span::width).sum::<usize>()).max().unwrap_or(0);
        let roles = 2 + name_width + adapter_width + role_width <= width;
        let muted = |text: &str, color: Color| wrap(text, width.saturating_sub(2), 4).into_iter().map(|text| Line::from(Span::styled(format!("  {}", cut(&text, width.saturating_sub(2))), style(color)))).collect::<Vec<_>>();
        let (mut lines, mut selected) = (Vec::new(), 0);
        let kebbies = agents.profiles.iter().filter(|profile| profiles::built_in(&profile.name).is_some()).count();
        let heading = |lines: &mut Vec<Line<'static>>, title: &str, about: String| {
            let mut line = group_line(title, if title == "YOURS" { agents.profiles.len() - kebbies } else { kebbies }, TEAL);
            line.spans.insert(0, Span::raw(" "));
            lines.push(line);
            lines.extend(muted(&about, MUTED));
        };
        heading(&mut lines, "FROM KEBBIE", "follow kebbie's updates; your settings on them are kept".to_string());
        let mut yours = false;
        for (at, profile) in agents.profiles.iter().enumerate() {
            if !yours && profiles::built_in(&profile.name).is_none() {
                yours = true;
                lines.push(Line::default());
                heading(&mut lines, "YOURS", format!("{} · an update never writes here", home_relative(&self.paths.profiles)));
            }
            let chosen = at == self.row;
            let used: usize = adapters[at].iter().map(Span::width).sum();
            let mut spans = vec![
                if chosen { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") },
                Span::styled(format!(" {:<name_width$}", cut(&profile.name, name_width - 2)), if chosen { bold(TEXT) } else { style(TEXT) }),
            ];
            spans.extend(adapters[at].clone());
            if roles {
                spans.push(Span::raw(" ".repeat(adapter_width.saturating_sub(used))));
                spans.extend(self.role_spans(profile));
            }
            let line = fit(spans, width);
            if !chosen {
                lines.push(line);
                continue;
            }
            selected = lines.len();
            lines.push(line.style(Style::new().bg(SURFACE)));
            let kebbies = profiles::kebbie_role_for(profile).0;
            for (number, text) in wrap(&format!("{kebbies}, from kebbie {}, updates with kebbie", agents.version), width.saturating_sub(11), 4).into_iter().enumerate() {
                lines.push(Line::from(vec![Span::styled(if number == 0 { "    role:  " } else { "           " }, style(SUBTEXT)), Span::styled(text, style(SUBTEXT))]));
            }
            if let Some(role) = profiles::role_name(&self.paths.profiles, profile) {
                lines.push(fit(vec![Span::raw("         "), Span::styled(format!("+ {role}"), style(BLUE)), Span::styled(", yours: read after kebbie's role", style(SUBTEXT))], width));
            }
            let last = if profiles::built_in(&profile.name).is_some() { ("d", "default") } else { ("x", "delete") };
            let mut keys = vec![Span::raw("    ")];
            for (key, word) in [("m", "model"), ("e", "effort"), ("r", "role"), ("n", "new profile"), ("⏎", "open file"), last] {
                let shown = format!(" {word}");
                let fits = keys.iter().map(Span::width).sum::<usize>() + 2 + text_width(key) + text_width(&shown) <= width;
                if keys.len() > 1 && !fits {
                    lines.push(Line::from(std::mem::replace(&mut keys, vec![Span::raw("    ")])));
                }
                keys.extend([Span::raw(if keys.len() == 1 { "" } else { "  " }), Span::styled(key, style(MAUVE)), Span::styled(shown, style(SUBTEXT))]);
            }
            lines.push(Line::from(keys));
            lines.push(Line::default());
        }
        if !yours {
            lines.push(Line::default());
            heading(&mut lines, "YOURS", format!("{} · an update never writes here", home_relative(&self.paths.profiles)));
            lines.extend(muted("none yet: n makes one", MUTED));
        }
        if !agents.broken.is_empty() {
            lines.push(Line::default());
            let mut line = group_line("DOES NOT LOAD", agents.broken.len(), RED);
            line.spans.insert(0, Span::raw(" "));
            lines.push(line);
            lines.extend(muted("kebbie leaves these out until they load; ⏎ opens one", MUTED));
        }
        for (at, (name, file, error)) in agents.broken.iter().enumerate() {
            let chosen = agents.profiles.len() + at == self.row;
            let shown = self.shown_file(file);
            let row = fit(vec![
                if chosen { Span::styled("▌", style(MAUVE)) } else { Span::raw(" ") },
                Span::styled(format!(" {:<name_width$}", cut(name, name_width - 2)), if chosen { bold(RED) } else { style(RED) }),
                Span::styled(shown.clone(), style(MUTED)),
            ], width);
            if !chosen {
                lines.push(row);
                continue;
            }
            selected = lines.len();
            lines.push(row.style(Style::new().bg(SURFACE)));
            let error = error.replace(&file.display().to_string(), &shown).replace(&self.paths.profiles.display().to_string(), "profiles");
            lines.extend(wrap(&error, width.saturating_sub(4), 4).into_iter().map(|text| Line::from(Span::styled(format!("    {text}"), style(RED)))));
            lines.push(Line::from(vec![Span::raw("    "), Span::styled("⏎", style(MAUVE)), Span::styled(" open file", style(SUBTEXT))]));
            lines.push(Line::default());
        }
        lines.push(Line::default());
        lines.extend(muted("dim = the agent's own default:", MUTED));
        let (claude, codex) = (home_relative(&self.paths.claude_home.join("settings.json")), home_relative(&self.paths.codex_home.join("config.toml")));
        lines.extend(muted(&format!("Claude's from {claude}, Codex's from {codex}"), MUTED));
        (lines, selected)
    }

    pub fn draw(&mut self, frame: &mut Frame, body: Rect) {
        let [title, tabs, _, main, said] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(2)]).areas(body);
        let right = "from the board: , · or kebbie config edit";
        let gap = (title.width as usize).saturating_sub(8 + text_width(right)).max(1);
        frame.render_widget(Paragraph::new(Line::from(vec![Span::styled("Settings", bold(MAUVE)), Span::raw(" ".repeat(gap)), Span::styled(right, style(MUTED))])), title);
        let lit = |on: bool| if on { bold(MANTLE).bg(MAUVE) } else { style(MUTED) };
        let mut spans = vec![Span::styled(" This machine ", lit(self.tab == Tab::Machine)), Span::raw("  ")];
        if self.repo.is_some() {
            spans.push(Span::styled(format!(" Repo: {} ", self.repo_name()), lit(self.tab == Tab::Repo)));
            spans.push(Span::raw("  "));
        }
        let file = home_relative(&self.file());
        let profiles_file = home_relative(&self.paths.profiles.join("<name>.toml"));
        let writes = match (self.tab, self.linked()) {
            (Tab::Machine, _) if self.on_profiles() => format!("writes {profiles_file}"),
            (Tab::Machine, _) => format!("writes {file}"),
            (Tab::Repo, false) => format!("writes {file}, which wins over the machine"),
            (Tab::Repo, true) => format!("{file} links to a team file"),
        };
        spans.push(Span::styled(writes, style(MUTED)));
        frame.render_widget(Paragraph::new(Line::from(spans)), tabs);
        if let Some(error) = &self.error {
            let lines: Vec<Line> = wrap(error, main.width as usize, 6).into_iter().map(|text| Line::from(Span::styled(text, style(RED)))).collect();
            frame.render_widget(Paragraph::new(lines), main);
        } else {
            let [rail, _, rows] = Layout::horizontal([Constraint::Length(20), Constraint::Length(2), Constraint::Min(20)]).areas(main);
            let sections = self.sections();
            let rail_lines: Vec<Line> = sections.iter().enumerate().map(|(at, (name, keys))| {
                let chosen = at == self.section;
                let mark = if chosen { Span::styled("▸ ", style(MAUVE)) } else { Span::raw("  ") };
                let line = Line::from(vec![mark, Span::styled(cut(name, 14), if chosen { bold(TEXT) } else { style(TEXT) }), Span::styled(format!(" {}", keys.len()), style(FAINT))]);
                if chosen { line.style(Style::new().bg(SURFACE)) } else { line }
            }).collect();
            frame.render_widget(Paragraph::new(rail_lines), rail);
            let (lines, selected) = if self.on_profiles() { self.profile_lines(rows.width as usize) } else { self.row_lines(rows.width as usize) };
            let scroll = selected.saturating_sub(rows.height.saturating_sub(10) as usize);
            frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), rows);
        }
        let under = match &self.mode {
            Mode::Filter { buffer } => Some((format!("/ {buffer}▏  · ⏎ keep · esc clear"), TEXT)),
            Mode::Edit { typed, .. } => Some((match typed { Typed::SkillName => "type a skill: <skill> or <plugin>:<skill>", Typed::SkillFile => "type the path of a .md file, starting with / or ~/", Typed::Model => "type the model's id, as the agent takes it", Typed::Value => "editing in place" }.to_string() + " · ⏎ save · esc cancel", SUBTEXT)),
            Mode::Normal if self.said.is_none() && self.on_profiles() => self.profile().map(|profile| match self.agents.warning(&profile) {
                Some(warning) => (warning, YELLOW),
                None => (self.summary(&profile), SUBTEXT),
            }).or_else(|| self.broken().map(|(name, _, _)| (format!("{name} does not load, so no task can start with it; ⏎ opens its file"), RED))),
            _ => self.said.clone(),
        };
        if let Some((text, color)) = under {
            let lines: Vec<Line> = wrap(&text, said.width as usize, 2).into_iter().map(|text| Line::from(Span::styled(text, style(color)))).collect();
            frame.render_widget(Paragraph::new(lines), said);
        }
        self.hits.clear();
        match &self.mode {
            Mode::Choose(chooser) => {
                let width = POPUP.min(frame.area().width.saturating_sub(8) as usize);
                let (lines, spots) = chooser.view(width);
                self.hits = modal_spots(frame, chooser.title.clone(), lines, spots, width).1;
            }
            Mode::Wizard(wizard) => {
                let width = POPUP.min(frame.area().width.saturating_sub(8) as usize);
                let (lines, spots) = wizard.view(width);
                self.hits = modal_spots(frame, "new profile".to_string(), lines, spots, width).1;
            }
            Mode::Delete { name, nudged } => {
                let about = self.agents.profiles.iter().find(|profile| &profile.name == name).map(|profile| self.deletion(profile).0).unwrap_or_default();
                modal(frame, "delete".to_string(), widgets::confirm_lines(name, Some(&about), "Delete this profile?", *nudged), 72);
            }
            Mode::Menu { name, index } => {
                let intro = wrap(&format!("{name} is kebbie's: its role comes with kebbie and an update replaces it, so it is not edited here."), 56, 3);
                let mut lines: Vec<Line> = intro.into_iter().map(|text| Line::from(Span::styled(text, style(TEXT)))).collect();
                lines.push(Line::default());
                let choices = self.menu_choices(name);
                let spots = (0..choices.len()).flat_map(|at| [(lines.len() + 2 * at, Spot::Choice(at)), (lines.len() + 2 * at + 1, Spot::Choice(at))]).collect();
                lines.extend(options_lines(choices, *index, 56));
                self.hits = modal_spots(frame, format!("role · {name}"), lines, spots, 58).1;
            }
            Mode::Team { key, .. } => {
                let target = std::fs::canonicalize(self.file()).map(|path| home_relative(&path)).unwrap_or_default();
                let lines = vec![
                    Line::from(Span::styled(format!("{} links to the team's file {target}.", home_relative(&self.file())), style(TEXT))),
                    Line::default(),
                    Line::from(vec![Span::styled("t", style(MAUVE)), Span::styled("  change the team's file, for everyone who links it", style(TEXT))]),
                    Line::from(vec![Span::styled("o", style(MAUVE)), Span::styled("  override it here: the link becomes a copy of this machine's own", style(TEXT))]),
                    Line::default(),
                    Line::from(Span::styled("esc cancel", style(FAINT))),
                ];
                modal(frame, format!("Change {key}"), lines, 76);
            }
            _ => {}
        }
    }

    pub(super) fn draw_alone(&mut self, frame: &mut Frame) {
        let area = frame.area().inner(Margin { horizontal: 2, vertical: 1 });
        let [body, hint] = Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).areas(area);
        self.draw(frame, body);
        let (full, short) = self.hint_lines();
        let line = if full.width() <= hint.width as usize { full } else { short };
        let spans: Vec<Span> = line.spans.into_iter().map(|span| Span::styled(span.content.replace("esc board", "esc quit"), span.style)).collect();
        frame.render_widget(Paragraph::new(Line::from(spans)), hint);
    }
}
