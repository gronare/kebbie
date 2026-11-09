use ratatui::crossterm::event::KeyCode;
use ratatui::text::{Line, Span};

use super::{Page, default_skill, edit_text, home_relative};
use crate::board::widgets::{Answer, MAUVE, MUTED, Spot, answer_lines, bold, hint_line, style, wrap};
use crate::config::{self, Entry, Kind, Layer};
use crate::config_edit::Set;
use crate::profiles::{self, Profile, Role};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Change {
    Set(Set),
    Remove,
    Copy(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Typed {
    Value,
    SkillName,
    SkillFile,
    Model,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Field {
    Model,
    Effort,
}

impl Field {
    pub(super) fn key(self) -> &'static str {
        match self {
            Field::Model => "model",
            Field::Effort => "effort",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum Then {
    Write(Change),
    Type(Typed),
    Field(Field, Option<String>),
    TypeModel,
    Role(Role),
}

pub(super) type Choice = Answer<Then>;

pub(super) fn choice(label: &str, about: &str, then: Then, now: bool) -> Choice {
    Answer::new(label, about, then).now(now)
}

// A value in effect that no answer offers becomes one, before the last `behind` answers, so ⏎ keeps it rather than writing the first.
pub(super) fn keep_current(choices: &mut Vec<Choice>, behind: usize, current: Option<&str>, make: impl FnOnce(&str) -> Choice) {
    if let Some(current) = current && !choices.iter().any(|choice| choice.now) {
        choices.insert(choices.len().saturating_sub(behind), make(current).now(true));
    }
}

pub(super) struct Chooser {
    pub(super) title: String,
    pub(super) question: String,
    pub(super) explain: String,
    pub(super) choices: Vec<Choice>,
    pub(super) index: usize,
    pub(super) foot: Vec<String>,
    pub(super) opens: bool,
}

impl Chooser {
    pub(super) fn new(title: String, question: &str, explain: String, choices: Vec<Choice>, foot: Vec<String>) -> Self {
        let index = choices.iter().position(|choice| choice.now).unwrap_or(0);
        Chooser { title, question: question.to_string(), explain, choices, index, foot, opens: false }
    }

    pub(super) fn view(&self, width: usize) -> (Vec<Line<'static>>, Vec<(usize, Spot)>) {
        let mut spots = Vec::new();
        let mut lines = vec![Line::from(Span::styled(self.question.clone(), bold(MAUVE)))];
        lines.extend(wrap(&self.explain, width, 6).into_iter().map(|text| Line::from(Span::styled(text, style(MUTED)))));
        lines.push(Line::default());
        answer_lines(&mut lines, &mut spots, &self.choices, self.index, width);
        lines.push(Line::default());
        if !self.foot.is_empty() {
            for text in &self.foot {
                lines.extend(wrap(text, width, 4).into_iter().map(|text| Line::from(Span::styled(text, style(MUTED)))));
            }
            lines.push(Line::default());
        }
        let mut keys = vec![("↑ ↓ choose", None), ("⏎ set", Some(KeyCode::Enter.into()))];
        keys.extend(self.opens.then(|| ("o open the file", Some(KeyCode::Char('o').into()))));
        keys.push(("esc close", Some(KeyCode::Esc.into())));
        hint_line(&mut lines, &mut spots, &keys);
        (lines, spots)
    }
}

impl Page {
    pub(super) fn chooser(&self, entry: &Entry) -> Chooser {
        let now = edit_text(&entry.value);
        let (question, choices) = match &entry.kind {
            Kind::Choice(options) => ("Which value?", options.iter().map(|option| {
                let none = *option == "none";
                let label = if none { "none".to_string() } else { format!("{option:?}") };
                choice(&label, "", Then::Write(if none { Change::Remove } else { Change::Set(Set::Text(option.to_string())) }), *option == now || (none && entry.value == "none"))
            }).collect()),
            Kind::Profile => {
                let set = (entry.layer != Layer::Default).then_some(now);
                let mut choices = vec![choice(&format!("none ({})", profiles::TASK), "the built-in task", Then::Write(Change::Remove), set.is_none())];
                choices.extend(self.agents.profiles.iter().filter(|profile| !profile.review && self.agents.set_up(profile)).map(|profile| {
                    let about = if profile.fits.is_empty() { "(for not said)" } else { profile.fits.as_str() };
                    choice(&format!("{:?}", profile.name), about, Then::Write(Change::Set(Set::Text(profile.name.clone()))), set.as_deref() == Some(profile.name.as_str()))
                }));
                let unset = |name: &str| self.agents.profiles.iter().find(|profile| profile.name == name).map(|profile| profile.agent.not_set_up());
                keep_current(&mut choices, 0, set.as_deref(), |name| choice(&format!("{name:?}"), &unset(name).unwrap_or_else(|| "no profile loads by this name".to_string()), Then::Write(Change::Set(Set::Text(name.to_string()))), true));
                ("Which profile?", choices)
            }
            Kind::ReviewProfile => {
                let runs = |profile: &Profile| self.reviewer_runs(profile);
                let mut choices: Vec<Choice> = self.agents.reviewers().map(|profile| choice(&profile.name, &runs(profile), Then::Write(Change::Set(Set::Text(profile.name.clone()))), profile.name == now)).collect();
                keep_current(&mut choices, 0, Some(now.as_str()), |name| choice(name, "no review profile loads by this name, or its agent is not set up", Then::Write(Change::Set(Set::Text(name.to_string()))), true));
                return Chooser::new(entry.key.to_string(), "Who reviews by default?", "Its verdict holds the push. Review profiles only.".to_string(), choices, Vec::new());
            }
            Kind::SecondOpinion => {
                let current = config::Second::parse(&now).unwrap_or_default();
                let others: Vec<&Profile> = self.agents.reviewers().filter(|profile| profile.name != self.agents.review_profile).collect();
                let mut choices = vec![choice("None", "s is not offered", Then::Write(Change::Remove), current == config::Second::None)];
                choices.extend(others.iter().map(|profile| choice(&profile.name, &self.reviewer_runs(profile), Then::Write(Change::Set(Set::Text(profile.name.clone()))), current == config::Second::Offered(profile.name.clone()))));
                let single = others.len() == 1;
                choices.extend(others.iter().map(|profile| {
                    let label = if single { "Always".to_string() } else { format!("Always: {}", profile.name) };
                    choice(&label, "run the second opinion with every review", Then::Write(Change::Set(Set::Text(config::Second::Always(profile.name.clone()).key()))), current == config::Second::Always(profile.name.clone()))
                }));
                keep_current(&mut choices, 0, current.profile(), |_| choice(&current.key(), "no review profile loads by this name, or its agent is not set up", Then::Write(Change::Set(Set::Text(current.key()))), true));
                let explain = "Offered with s when you send a task to review; it never holds the push. The lead relays its points to the task.".to_string();
                return Chooser::new(entry.key.to_string(), "Second opinion?", explain, choices, Vec::new());
            }
            Kind::Skill => {
                let default = default_skill(entry.key);
                let skill = default.split_once(':').map_or(default, |(_, skill)| skill);
                ("Which skill?", vec![
                    choice(&format!("{default:?}"), "built in", Then::Write(Change::Set(Set::Text(default.to_string()))), false),
                    choice("Your skill, by name", "<skill> or <plugin>:<skill>", Then::Type(Typed::SkillName), false),
                    choice("A Markdown file, by path", "an absolute or ~ path to a .md file", Then::Type(Typed::SkillFile), false),
                    choice(&format!("Copy kebbie's to {}/{skill} and edit it", home_relative(&self.paths.claude_home.join("skills"))), &format!("then the key names \"{skill}\""), Then::Write(Change::Copy(skill)), false),
                ])
            }
            _ => ("", Vec::new()),
        };
        Chooser::new(entry.key.to_string(), question, entry.about.clone(), choices, Vec::new())
    }
}
