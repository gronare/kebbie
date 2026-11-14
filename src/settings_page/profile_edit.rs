use std::path::{Path, PathBuf};

use super::{Effect, Mode, Page, home_relative};
use super::chooser::{Change, Choice, Chooser, Field, Then, choice};
use crate::board::widgets::{self, GREEN, MUTED, RED};
use crate::config;
use crate::config_edit::Set;
use crate::profiles::{self, Edited, Profile, Role};
use crate::sh::Result;

impl Page {
    pub(super) fn shown_file(&self, file: &Path) -> String {
        let name = file.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        format!("profiles/{name}")
    }

    pub(super) fn built_in_without_file(&self, profile: &Profile) -> bool {
        profiles::built_in(&profile.name).is_some() && profile.file.is_none()
    }

    pub(super) fn set_field(&mut self, field: Field, value: Option<&str>) {
        if field == Field::Model && self.profile().is_some_and(|profile| self.agents.default_reviewer(&profile)) {
            return self.request("review.model", value.map_or(Change::Remove, |model| Change::Set(Set::Text(model.to_string()))));
        }
        _ = self.write_profile(&[(field.key(), value)]);
    }

    pub(super) fn foot(&self, profile: &Profile, field: Field) -> Vec<String> {
        let key = field.key();
        if field == Field::Model && self.agents.default_reviewer(profile) {
            let file = home_relative(&self.file());
            return vec![format!("This writes review.model in {file}, ahead of {}'s own model, as {} is review.profile.", profile.name, profile.name), "A review already running keeps its model; the next review takes it.".to_string()];
        }
        let running = format!("A task already running keeps its {key}; the next start takes it.");
        if self.built_in_without_file(profile) {
            return vec![format!("{name} is kebbie's: a {key} you set goes in profiles/{name}.toml, which an update never writes, and kebbie's role and rules stay as they ship. d on the row removes that file again.", name = profile.name), running];
        }
        vec![running]
    }

    pub(super) fn choose(&mut self, field: Field) -> Effect {
        let Some(profile) = self.profile() else { return Effect::Handled };
        let reviewer = self.agents.default_reviewer(&profile);
        let (question, (explain, choices)) = match field {
            Field::Model => {
                let current = if reviewer { self.agents.review_model(&profile).map(str::to_string) } else { profile.model.clone() };
                ("Which model?", self.model_choices(profile.agent, current.as_deref(), reviewer.then_some(&profile)))
            }
            Field::Effort => ("How much effort?", self.effort_choices(profile.agent, self.agents.model(&profile).as_deref(), profile.effort.as_deref())),
        };
        let foot = self.foot(&profile, field);
        self.said = None;
        self.mode = Mode::Choose(Box::new(Chooser::new(format!("{} · {}", field.key(), profile.name), question, explain, choices, foot)));
        Effect::Handled
    }

    pub(super) fn broken(&self) -> Option<&(String, PathBuf, String)> {
        if !self.on_profiles() {
            return None;
        }
        self.agents.broken.get(self.row.checked_sub(self.agents.profiles.len())?)
    }

    pub(super) fn open_profile(&mut self) -> Effect {
        if let Some((_, file, _)) = self.broken() {
            return Effect::Open(file.clone());
        }
        let Some(profile) = self.profile() else { return Effect::Handled };
        match profile.file {
            Some(file) => Effect::Open(file),
            None => {
                self.say(format!("{} is kebbie's and has no file of yours until you set something: m, e or r writes profiles/{}.toml", profile.name, profile.name), MUTED);
                Effect::Handled
            }
        }
    }

    pub fn opened(&mut self, file: &Path, result: Result<bool>) {
        self.load();
        let shown = self.shown_file(file);
        match result {
            Ok(_) if file.extension().is_some_and(|extension| extension == "md") => {
                let users: Vec<String> = self.agents.profiles.iter().filter(|profile| profile.role.as_deref() == Some(file)).map(|profile| profile.name.clone()).collect();
                let read = if users.is_empty() { "no profile reads it".to_string() } else { format!("read after kebbie's role by {}", users.join(", ")) };
                self.say(format!("{shown}: {read}"), GREEN);
            }
            Ok(_) => match self.agents.broken.iter().find(|(_, broken, _)| broken == file).map(|(_, _, error)| error) {
                Some(error) => self.say(format!("✗ {}", error.replace(&file.display().to_string(), &shown)), RED),
                None => self.say(format!("{shown} loads"), GREEN),
            },
            Err(error) => self.say(format!("✗ the editor: {error}"), RED),
        }
    }

    pub fn shown(&mut self, result: Result<bool>) {
        if let Err(error) = result {
            self.say(format!("✗ the pager: {error}"), RED);
        }
    }

    pub(super) fn role(&mut self) -> Effect {
        let Some(profile) = self.profile() else { return Effect::Handled };
        self.said = None;
        if profiles::built_in(&profile.name).is_some() {
            self.mode = Mode::Menu { name: profile.name, index: 0 };
            return Effect::Handled;
        }
        let dir = &self.paths.profiles;
        let current = profiles::role_name(dir, &profile);
        let new = profiles::free_role(dir, &profile.name);
        let (mine, others): (Vec<_>, Vec<_>) = profiles::role_choices(dir, &self.agents.profiles, &profile.name).into_iter().partition(|(file, _)| Some(file) == current.as_ref());
        let mut choices: Vec<Choice> = mine.into_iter().chain(others).map(|(file, about)| choice(&file, &about, Then::Role(Role::File(file.clone())), Some(&file) == current.as_ref())).collect();
        choices.push(choice("New role", &format!("{new}, an empty file"), Then::Role(Role::New(new.clone())), false));
        if let Some(current) = &current {
            choices.push(choice(&format!("Copy of {current}"), "to change it for this profile alone", Then::Role(Role::Copy { from: current.clone(), to: new }), false));
        }
        choices.push(choice("None", "kebbie's role alone", Then::Role(Role::None), current.is_none()));
        let explain = format!("Your lines, read after {}.", profiles::kebbie_role_for(&profile).0);
        let mut chooser = Chooser::new(format!("role · {}", profile.name), "Which role?", explain, choices, vec!["A shared file changes for every profile that uses it.".to_string()]);
        chooser.opens = true;
        self.mode = Mode::Choose(Box::new(chooser));
        Effect::Handled
    }

    // A role file made here takes a name no file has, so it never replaces one the user has.
    pub(super) fn set_role(&mut self, to: Role) -> Effect {
        let dir = self.paths.profiles.clone();
        let made = match &to {
            Role::New(file) => Some((file.clone(), Ok(String::new()))),
            Role::Copy { from, to } => Some((to.clone(), std::fs::read_to_string(dir.join(from)).map_err(|error| format!("✗ {from}: {error}")))),
            _ => None,
        };
        if let Some((file, text)) = &made {
            let written = text.clone().and_then(|text| if dir.join(file).exists() { Err(format!("✗ {file} is in the folder already")) } else { crate::state::write(&dir.join(file), text).map_err(|error| format!("✗ {file}: {error}")) });
            if let Err(error) = written {
                _ = self.fail(error);
                return Effect::Handled;
            }
        }
        let written = self.write_profile(&[("role", to.file())]);
        match made {
            Some((file, _)) if written.is_err() => {
                let _ = std::fs::remove_file(dir.join(file));
                Effect::Handled
            }
            Some((file, _)) => Effect::Open(dir.join(file)),
            None => Effect::Handled,
        }
    }

    pub(super) fn menu_choices(&self, name: &str) -> Vec<widgets::Choice> {
        let row = |key: char, label: String, about: String| widgets::Choice { key, act: None, label, about, link: None };
        let review = self.agents.profiles.iter().find(|profile| profile.name == name).is_some_and(|profile| profile.review);
        let file = profiles::kebbie_role(name).1;
        let every = if review { "every review" } else { "every task" };
        let own = self.agents.profiles.iter().find(|profile| profile.name == name).and_then(|profile| profiles::role_name(&self.paths.profiles, profile)).unwrap_or_else(|| format!("{name}.md"));
        let others: Vec<String> = profiles::role_readers(&self.paths.profiles, &own).into_iter().filter(|reader| reader != name).collect();
        let shared = if others.is_empty() { String::new() } else { format!(", shared with {}", others.join(", ")) };
        let mut choices = vec![
            row('s', "Show kebbie's role".to_string(), format!("{file}, read-only")),
            row('a', "Add your lines".to_string(), format!("profiles/{own}{shared}, read after kebbie's role by {every}")),
        ];
        choices.push(row('n', format!("New profile from {name}"), format!("the new-profile steps, starting from {name}")));
        choices
    }

    pub(super) fn sessions(&self) -> Option<PathBuf> {
        self.paths.sessions.clone().or_else(|| crate::lead::state_location().ok().map(|state| state.join("sessions")))
    }

    pub(super) fn kept(&self, profile: &Profile) -> Option<String> {
        if profiles::built_in(&profile.name).is_some() {
            return Some(format!("✗ {} is kebbie's and stays; d removes your settings on it", profile.name));
        }
        let open = self.sessions().map(|sessions| crate::tasks::on_profile(&sessions, &profile.name)).unwrap_or_default();
        if !open.is_empty() {
            return Some(format!("✗ {} is the profile of {}, still open; it can go once {} closes", profile.name, open.join(", "), if open.len() == 1 { "that task" } else { "they" }));
        }
        let machine = config::check_machine(&self.paths.machine).ok().map(|(machine, _)| machine);
        if let Some((file, _)) = crate::doctor::named_profiles(machine.as_ref(), &self.paths.machine, &self.paths.repos_dir).into_iter().find(|(_, named)| *named == profile.name) {
            return Some(format!("✗ task.profile names {} in {file}, so every new task there starts with it; pick another first", profile.name));
        }
        if machine.as_ref().and_then(|machine| machine.lead_profile.as_deref()) == Some(profile.name.as_str()) {
            return Some(format!("✗ lead.profile names {}, so the lead starts with it; pick another first", profile.name));
        }
        let routines: Vec<String> = crate::routines::load_from(&self.paths.machine, &self.paths.repos_dir).0.into_iter().filter(|routine| routine.profile.as_deref() == Some(profile.name.as_str())).map(|routine| format!("\"{}\"", routine.title)).collect();
        if !routines.is_empty() {
            return Some(format!("✗ {} is the profile the routine {} starts its tasks with; give {} another first", profile.name, routines.join(", "), if routines.len() == 1 { "it" } else { "them" }));
        }
        None
    }

    pub(super) fn deletion(&self, profile: &Profile) -> (String, String, bool) {
        let file = profile.file.as_deref().map(|file| self.shown_file(file)).unwrap_or_default();
        let Some(role) = profiles::role_name(&self.paths.profiles, profile) else { return (format!("{file} goes."), format!("✓ removed {file}"), false) };
        let others: Vec<String> = profiles::role_readers(&self.paths.profiles, &role).into_iter().filter(|reader| *reader != profile.name).collect();
        match others.as_slice() {
            [] => (format!("{file} goes, and its role {role}, which no other profile reads."), format!("✓ removed {file} and its role {role}"), true),
            others => (format!("{file} goes; its role {role} stays, as {} reads it too.", others.join(", ")), format!("✓ removed {file}; its role {role} stays for {}", others.join(", ")), false),
        }
    }

    pub(super) fn ask_delete(&mut self) {
        let Some(profile) = self.profile() else { return };
        match self.kept(&profile) {
            Some(why) => self.say(why, RED),
            None => self.mode = Mode::Delete { name: profile.name, nudged: false },
        }
    }

    pub(super) fn delete(&mut self, name: &str) {
        let Some(profile) = self.agents.profiles.iter().find(|profile| profile.name == name).cloned() else { return };
        if let Some(why) = self.kept(&profile) {
            return self.say(why, RED);
        }
        let (_, said, role_goes) = self.deletion(&profile);
        let removed = profile.file.as_deref().map(std::fs::remove_file).transpose().and_then(|_| match &profile.role {
            Some(role) if role_goes && role.starts_with(&self.paths.profiles) => std::fs::remove_file(role),
            _ => Ok(()),
        });
        self.load();
        match removed {
            Ok(()) => self.say(said, GREEN),
            Err(error) => self.say(format!("✗ {name}: {error}"), RED),
        }
    }

    pub(super) fn menu(&mut self, key: char) -> Effect {
        let Some(profile) = self.profile() else { return Effect::Handled };
        match key {
            's' => {
                let file = self.paths.kebbie.join(profiles::kebbie_role_for(&profile).1);
                if file.is_file() {
                    return Effect::Show(file);
                }
                self.say(format!("✗ {} is not in kebbie's install", home_relative(&file)), RED);
            }
            'n' => self.wizard(&profile.name),
            'a' => {
                if let Some(role) = profile.role {
                    return Effect::Open(role);
                }
                let file = format!("{}.md", profile.name);
                let path = self.paths.profiles.join(&file);
                let made = !path.exists();
                if made && let Err(error) = crate::state::write(&path, String::new()) {
                    _ = self.fail(format!("✗ {file}: {error}"));
                    return Effect::Handled;
                }
                if self.write_profile(&[("role", Some(&file))]).is_err() {
                    if made {
                        let _ = std::fs::remove_file(&path);
                    }
                    return Effect::Handled;
                }
                return Effect::Open(path);
            }
            _ => {}
        }
        Effect::Handled
    }

    pub(super) fn write_profile(&mut self, changes: &[(&str, Option<&str>)]) -> Result<()> {
        let Some(profile) = self.profile() else { return Ok(()) };
        let name = profile.name.clone();
        let edited = profiles::edit_in(&self.paths.profiles, &name, changes);
        self.load();
        let now = self.agents.profiles.iter().find(|profile| profile.name == name).map(|profile| self.runs(profile)).unwrap_or_default();
        let keys = changes.iter().map(|(key, _)| *key).collect::<Vec<_>>().join(if edited.as_ref().is_ok_and(|edited| *edited == Edited::Unchanged) { " or " } else { " and " });
        match edited {
            Ok(Edited::Wrote(file)) => {
                let said = match changes {
                    [(key, Some(value))] => format!("✓ wrote {key} = {} to {}", Set::Text(value.to_string()).shown(), self.shown_file(&file)),
                    _ => format!("✓ removed {keys} from {}", self.shown_file(&file)),
                };
                self.say(format!("{said} · {now}; a running task keeps what it started with"), GREEN);
            }
            Ok(Edited::Removed(file)) => self.say(format!("✓ removed {}, which held nothing else · {now}", self.shown_file(&file)), GREEN),
            Ok(Edited::Unchanged) if changes.len() > 1 => self.say(format!("{name} has no {keys} of its own · {now}"), MUTED),
            Ok(Edited::Unchanged) => self.say(format!("{name} has that {keys} already · {now}"), MUTED),
            Err(error) => return self.fail(format!("✗ {}", error.lines().next().unwrap_or_default().replace(&self.paths.profiles.display().to_string(), "profiles"))),
        }
        Ok(())
    }
}
