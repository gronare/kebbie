use super::{Effect, Mode, Page, home_relative};
use super::chooser::{Choice, Then};
use crate::agent::Agent;
use crate::board::widgets::{Answer, GREEN};
use crate::profile_wizard::{self, Pick, Wizard};
use crate::profiles::{self, Profile, Role};

impl Page {
    pub(super) fn wizard(&mut self, from: &str) {
        let starts: Vec<Profile> = self.agents.profiles.iter().filter(|profile| self.agents.set_up(profile)).cloned().collect();
        let start = starts.iter().position(|profile| profile.name == from).or_else(|| starts.iter().position(|profile| profile.name == profiles::TASK)).unwrap_or(0);
        let dir = &self.paths.profiles;
        let mut taken: Vec<String> = self.agents.profiles.iter().map(|profile| profile.name.clone()).collect();
        taken.extend(std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".toml").map(str::to_string)));
        let agents = self.agents.chosen.iter().map(|agent| (*agent, match agent {
            Agent::Claude => "kebbie's own default".to_string(),
            Agent::Codex => crate::agent::codex::availability(&self.paths.path, Some(&self.paths.codex_home), &self.paths.kebbie_command).short_line(),
        })).collect();
        let files = profiles::role_choices(dir, &self.agents.profiles, "");
        self.said = None;
        self.mode = Mode::Wizard(Box::new(Wizard::new(starts, start, files, dir.clone(), home_relative(dir), taken, agents)));
    }

    pub(super) fn answers(choices: Vec<Choice>) -> (Vec<Answer<Pick>>, usize) {
        let selected = choices.iter().position(|choice| choice.now).unwrap_or(0);
        let answers = choices.into_iter().map(|choice| Answer::new(&choice.label, &choice.about, match choice.then { Then::TypeModel => Pick::Type, Then::Field(_, value) => Pick::Value(value), _ => Pick::Value(None) })).collect();
        (answers, selected)
    }

    pub(super) fn offer(&self, wizard: &mut Wizard) {
        let start = wizard.from().filter(|profile| profile.agent == wizard.agent()).cloned();
        match wizard.step {
            profile_wizard::MODEL => {
                let (explain, choices) = self.model_choices(wizard.agent(), start.as_ref().and_then(|profile| profile.model.as_deref()), None);
                let (answers, selected) = Self::answers(choices);
                wizard.offer_models(explain, answers, selected);
            }
            profile_wizard::EFFORT => {
                let model = wizard.model().or_else(|| self.agents.default_model(wizard.agent()).map(str::to_string));
                let (explain, choices) = self.effort_choices(wizard.agent(), model.as_deref(), start.as_ref().and_then(|profile| profile.effort.as_deref()));
                let (answers, selected) = Self::answers(choices);
                wizard.offer_efforts(explain, answers, selected);
            }
            _ => {}
        }
    }

    // The role file goes first, as the profile parses only with its role there; a role file this call wrote goes again when the profile is not written.
    pub(super) fn add(&mut self, mut wizard: Box<Wizard>, open: bool) -> Effect {
        let name = wizard.name.trim().to_string();
        let dir = self.paths.profiles.clone();
        let file = dir.join(format!("{name}.toml"));
        let Some(made) = wizard.made() else { return Effect::Handled };
        let role = wizard.chosen_role();
        let made_role = match &role {
            Role::New(to) => Ok(Some((to.clone(), String::new()))),
            Role::Copy { from, to } => std::fs::read_to_string(dir.join(from)).map(|text| Some((to.clone(), text))).map_err(|error| format!("{from}: {error}")),
            _ => Ok(None),
        };
        let mut wrote = None;
        let written = made_role.and_then(|made_role| {
            if file.exists() {
                return Err(format!("{name}.toml is in the folder already"));
            }
            if let Some((to, text)) = made_role {
                if dir.join(&to).exists() {
                    return Err(format!("{to} is in the folder already"));
                }
                crate::state::write(&dir.join(&to), text)?;
                wrote = Some(to);
            }
            profiles::parse(&made.text, &name, &file, None)?;
            crate::state::write(&file, &made.text)
        });
        if let Err(error) = written {
            if let Some(to) = wrote {
                let _ = std::fs::remove_file(dir.join(to));
            }
            wizard.error = Some(error.replace(&dir.display().to_string(), "profiles"));
            self.mode = Mode::Wizard(wizard);
            return Effect::Handled;
        }
        self.load();
        if let Some(row) = self.agents.profiles.iter().position(|profile| profile.name == name) {
            self.row = row;
        }
        let role_said = role.file().map_or_else(String::new, |file| format!(", its role {file}"));
        let used = if wizard.review { "review.profile or review.second on this page" } else { "kebbie start --profile {name}, or task.profile" }.replace("{name}", &name);
        self.say(format!("✓ added profiles/{name}.toml{role_said} · {used}"), GREEN);
        match (open, role.file()) {
            (false, _) => Effect::Handled,
            (true, Some(role)) => Effect::Open(dir.join(role)),
            (true, None) => Effect::Open(file),
        }
    }
}
