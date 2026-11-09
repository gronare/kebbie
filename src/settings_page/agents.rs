use std::path::{Path, PathBuf};

use super::{Page, Paths, edit_text, home_relative};
use super::chooser::{Choice, Field, Then, choice, keep_current};
use crate::agent::Agent;
use crate::agent::codex::CachedModel;
use crate::config::{Entry, Layer};
use crate::profiles::{self, Profile};

const CLAUDE_MODELS: [(&str, &str); 4] = [("opus", "the newest Opus"), ("sonnet", "the newest Sonnet"), ("haiku", "the newest Haiku"), ("fable", "the newest Fable")];

#[derive(Default)]
pub(super) struct Agents {
    pub(super) profiles: Vec<Profile>,
    pub(super) broken: Vec<(String, PathBuf, String)>,
    pub(super) codex_models: Option<Vec<CachedModel>>,
    pub(super) codex_default: Option<String>,
    pub(super) codex_effort: Option<String>,
    pub(super) claude_default: Option<String>,
    pub(super) review_model: Option<(String, Layer)>,
    pub(super) review_profile: String,
    pub(super) version: String,
    pub(super) chosen: Vec<Agent>,
}

impl Agents {
    pub(super) fn load(paths: &Paths, entries: &[Entry]) -> Self {
        let (profiles, broken) = profiles::load_all_in(&paths.profiles);
        Agents {
            profiles, broken,
            review_model: entries.iter().find(|entry| entry.key == "review.model" && entry.layer != Layer::Default).map(|entry| (edit_text(&entry.value), entry.layer)),
            review_profile: entries.iter().find(|entry| entry.key == "review.profile").map_or_else(|| profiles::REVIEWER.to_string(), |entry| edit_text(&entry.value)),
            codex_models: crate::agent::codex::cached_models(&paths.codex_home),
            codex_default: crate::agent::codex::configured_model(&paths.codex_home),
            codex_effort: codex_effort(&paths.codex_home),
            claude_default: crate::agent::claude::configured_model(&paths.claude_home),
            version: crate::issue_recheck::version_at(&paths.kebbie).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
            chosen: crate::agent::chosen_in(Some(&paths.codex_home)),
        }
    }

    pub(super) fn default_model(&self, agent: Agent) -> Option<&str> {
        match agent {
            Agent::Codex => self.codex_default.as_deref(),
            Agent::Claude => self.claude_default.as_deref(),
        }
    }

    // review.model is the model of the default reviewer, whichever review profile review.profile names.
    pub(super) fn default_reviewer(&self, profile: &Profile) -> bool {
        profile.review && profile.name == self.review_profile
    }

    pub(super) fn review_model(&self, profile: &Profile) -> Option<&str> {
        self.review_model.as_ref().filter(|_| self.default_reviewer(profile)).map(|(model, _)| model.as_str())
    }

    // A reviewer's pick offers the review profiles of the agents setup chose.
    pub(super) fn reviewers(&self) -> impl Iterator<Item = &Profile> {
        self.profiles.iter().filter(|profile| profile.review && self.set_up(profile))
    }

    pub(super) fn own_model(&self, profile: &Profile) -> Option<String> {
        self.review_model(profile).map(str::to_string).or_else(|| profile.model.clone())
    }

    pub(super) fn model(&self, profile: &Profile) -> Option<String> {
        self.own_model(profile).or_else(|| self.default_model(profile.agent).map(str::to_string))
    }

    pub(super) fn offered(&self, model: &str) -> Option<bool> {
        self.codex_models.as_ref().map(|models| models.iter().any(|cached| cached.listed && cached.slug == model))
    }

    pub(super) fn cached(&self, model: &str) -> Option<&CachedModel> {
        self.codex_models.iter().flatten().find(|cached| cached.slug == model)
    }

    pub(super) fn set_up(&self, profile: &Profile) -> bool {
        self.chosen.contains(&profile.agent)
    }

    pub(super) fn warning(&self, profile: &Profile) -> Option<String> {
        if !self.set_up(profile) {
            return Some(format!("⚠ {}", profile.agent.not_set_up()));
        }
        let model = self.model(profile).filter(|_| profile.agent == Agent::Codex)?;
        if self.offered(&model) != Some(false) {
            return None;
        }
        let from = if self.own_model(profile).is_none() { " from Codex's config.toml" } else { "" };
        Some(format!("⚠ {} runs {model}{from}, which Codex does not offer to your account; pick a model for it with m.", profile.name))
    }
}

// The effort Codex runs a profile without one at: its config.toml's model_reasoning_effort, ahead of the model's own default.
fn codex_effort(home: &Path) -> Option<String> {
    let config: toml::Table = std::fs::read_to_string(home.join("config.toml")).ok()?.parse().ok()?;
    config.get("model_reasoning_effort").and_then(toml::Value::as_str).map(str::trim).filter(|effort| !effort.is_empty()).map(str::to_string)
}

impl Page {
    pub(super) fn model_choices(&self, agent: Agent, current: Option<&str>, reviewer: Option<&Profile>) -> (String, Vec<Choice>) {
        let agents = &self.agents;
        match agent {
            Agent::Codex => {
                let cache = home_relative(&self.paths.codex_home.join("models_cache.json"));
                let explain = if agents.codex_models.is_some() { "Codex's own list for your account.".to_string() } else { format!("No list from Codex at {cache}: Codex writes it when it starts.") };
                let mut choices: Vec<Choice> = agents.codex_models.iter().flatten().filter(|model| model.listed).map(|model| choice(&model.name, &model.slug, Then::Field(Field::Model, Some(model.slug.clone())), current == Some(model.slug.as_str()))).collect();
                let default = match agents.codex_default.as_deref() {
                    Some(model) if agents.offered(model) == Some(false) => format!("{model}, not offered to you ⚠"),
                    Some(model) => model.to_string(),
                    None => "none in Codex's config.toml: Codex picks".to_string(),
                };
                choices.push(choice("Codex's default", &default, Then::Field(Field::Model, None), current.is_none()));
                choices.push(choice("Type a model…", "any id Codex accepts", Then::TypeModel, false));
                keep_current(&mut choices, 2, current, |model| choice(model, if agents.offered(model) == Some(false) { "not offered to you ⚠" } else { "not in Codex's list" }, Then::Field(Field::Model, Some(model.to_string())), true));
                (explain, choices)
            }
            Agent::Claude => {
                let settings = home_relative(&self.paths.claude_home.join("settings.json"));
                let default = match agents.claude_default.as_deref() {
                    Some(model) => format!("{model}, from {settings}"),
                    None => format!("none in {settings}: Claude Code picks"),
                };
                let (label, default) = match reviewer.and_then(|profile| profile.model.as_ref().map(|model| (model, &profile.name))) {
                    Some((model, name)) => ("The profile's".to_string(), format!("{model}, from {name}.toml")),
                    None => ("Claude's default".to_string(), default),
                };
                let mut choices = vec![choice(&label, &default, Then::Field(Field::Model, None), current.is_none())];
                choices.extend(CLAUDE_MODELS.iter().map(|(name, about)| choice(name, about, Then::Field(Field::Model, Some(name.to_string())), current == Some(*name))));
                choices.push(choice("Type a model…", "a full id, as claude-opus-5-5", Then::TypeModel, false));
                keep_current(&mut choices, 1, current, |model| choice(model, "a model id of its own", Then::Field(Field::Model, Some(model.to_string())), true));
                let explain = if reviewer.is_some() { "Claude Code's model names; this sets review.model, which kebbie review runs.".to_string() } else { "Claude Code's model names.".to_string() };
                (explain, choices)
            }
        }
    }

    pub(super) fn effort_choices(&self, agent: Agent, model: Option<&str>, current: Option<&str>) -> (String, Vec<Choice>) {
        let agents = &self.agents;
        let cached = model.and_then(|model| agents.cached(model)).filter(|_| agent == Agent::Codex);
        let shown = model.map_or_else(|| format!("{}'s default", agent.name()), str::to_string);
        let (levels, default, explain): (Vec<String>, Option<String>, String) = match (agent, cached) {
            (Agent::Codex, Some(cached)) => (cached.efforts.iter().filter(|level| profiles::CODEX_EFFORTS.contains(&level.as_str())).cloned().collect(), cached.default_effort.clone(), format!("The levels {} takes, from Codex's list.", cached.name)),
            (Agent::Codex, None) => (profiles::CODEX_EFFORTS.map(str::to_string).to_vec(), None, format!("{shown} is not in Codex's list, so these are every level Codex takes.")),
            (Agent::Claude, _) => (profiles::EFFORTS.map(str::to_string).to_vec(), None, format!("Claude Code's effort levels, for {shown}.")),
        };
        let configured = agents.codex_effort.clone().filter(|_| agent == Agent::Codex);
        let (default, said) = match configured {
            Some(level) => (Some(level), "Codex's default, from its config.toml"),
            None => (default, "Codex's default for this model"),
        };
        let mut choices: Vec<Choice> = levels.iter().map(|level| {
            let about = if default.as_deref() == Some(level.as_str()) { said } else if level == "ultra" { "only some Codex models" } else { "" };
            choice(level, about, Then::Field(Field::Effort, Some(level.clone())), current == Some(level.as_str()))
        }).collect();
        let leave = match (agent, &default) {
            (Agent::Codex, Some(level)) => format!("leave it to Codex: {level}"),
            _ => format!("leave it to {}", agent.name()),
        };
        choices.push(choice("Default", &leave, Then::Field(Field::Effort, None), current.is_none()));
        keep_current(&mut choices, 1, current, |level| choice(level, "not a level this model lists", Then::Field(Field::Effort, Some(level.to_string())), true));
        (explain, choices)
    }

    pub(super) fn model_said(&self, profile: &Profile) -> String {
        match (self.agents.review_model.as_ref().filter(|_| self.agents.default_reviewer(profile)), &profile.model, self.agents.default_model(profile.agent)) {
            (Some((model, layer)), _, _) => format!("{model} from review.model ({})", self.layer_word(*layer).0),
            (None, Some(model), _) => model.clone(),
            (None, None, Some(model)) => format!("{model} by {}'s default", profile.agent.name()),
            (None, None, None) => format!("{}'s default model", profile.agent.name()),
        }
    }

    pub(super) fn reviewer_runs(&self, profile: &Profile) -> String {
        let mut parts = vec![profile.agent.key().to_string()];
        parts.extend(self.agents.own_model(profile));
        parts.extend(profile.effort.clone());
        parts.join(" · ")
    }

    pub(super) fn runs(&self, profile: &Profile) -> String {
        let model = self.model_said(profile);
        let effort = profile.effort.as_deref().map_or_else(String::new, |effort| format!(", effort {effort}"));
        format!("{} runs {} on {model}{effort}", profile.name, profile.agent.key())
    }
}
