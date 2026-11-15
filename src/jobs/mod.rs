use std::collections::BTreeMap;
use std::path::Path;

use crate::sh::Result;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq)]
pub struct Def {
    pub run: String,
    pub on: Option<String>,
}

pub fn parse(table: &toml::Table) -> Result<BTreeMap<String, Def>> {
    let Some(jobs) = table.get("jobs") else { return Ok(BTreeMap::new()) };
    let jobs = jobs.as_table().ok_or("jobs: must be a table of jobs, as [jobs] with build = \"xcodebuild -scheme App build\"")?;
    jobs.iter().map(|(name, value)| {
        let text = |value: &toml::Value, key: &str| value.as_str().map(str::trim).filter(|text| !text.is_empty()).map(str::to_string).ok_or_else(|| format!("jobs.{key}: must be a non-empty string"));
        let def = match value {
            toml::Value::String(_) => Def { run: text(value, name)?, on: None },
            toml::Value::Table(job) => {
                if let Some(unknown) = job.keys().find(|key| !["run", "on"].contains(&key.as_str())) {
                    return Err(format!("jobs.{name}.{unknown}: unknown key; known keys here: run, on"));
                }
                let run = job.get("run").ok_or_else(|| format!("jobs.{name}: needs run, the command the worker runs"))?;
                Def { run: text(run, &format!("{name}.run"))?, on: job.get("on").map(|on| text(on, &format!("{name}.on"))).transpose()? }
            }
            _ => return Err(format!("jobs.{name}: must be a command, or a table with run and on")),
        };
        Ok((name.clone(), def))
    }).collect()
}

pub fn defined(file: &Path) -> Result<BTreeMap<String, Def>> {
    let Ok(text) = std::fs::read_to_string(file) else { return Ok(BTreeMap::new()) };
    let table: toml::Table = text.parse().map_err(|error| format!("{}: {error}", file.display()))?;
    parse(&table).map_err(|error| format!("{}: {error}", file.display()))
}
