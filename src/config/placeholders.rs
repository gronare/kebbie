use std::path::Path;

use toml::Table;

use crate::repo::{name_of, slot_number, worktree_port};
use crate::sh::Result;

const PLACEHOLDERS: [&str; 4] = ["worktree", "repo", "number", "port"];

pub(super) enum Piece<'a> {
    Text(&'a str),
    Field(&'a str),
}

pub(super) fn pieces(template: &str) -> Result<Vec<Piece<'_>>> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        pieces.push(Piece::Text(&rest[..open]));
        let close = rest[open..].find('}').ok_or(format!("{template:?} opens a {{ it never closes"))? + open;
        let name = &rest[open + 1..close];
        if !PLACEHOLDERS.contains(&name) {
            return Err(format!("{{{name}}} is not a placeholder; use {{worktree}}, {{repo}}, {{number}} or {{port}}"));
        }
        pieces.push(Piece::Field(name));
        rest = &rest[close + 1..];
    }
    pieces.push(Piece::Text(rest));
    Ok(pieces)
}

// An empty template is kept, since it is how dev_url turns the link off.
pub(super) fn template(table: &Table, field: &str, key: &str) -> Result<Option<String>> {
    let Some(value) = table.get(field) else { return Ok(None) };
    let text = value.as_str().ok_or(format!("{key}: must be a string"))?.trim();
    pieces(text).map_err(|error| format!("{key}: {error}"))?;
    Ok(Some(text.to_string()))
}

pub struct Place {
    pub worktree: String,
    pub repo: String,
    pub number: Option<usize>,
    pub port: Option<u16>,
}

pub fn expand(template: &str, place: &Place) -> Option<String> {
    let parts: Option<Vec<String>> = pieces(template).ok()?.into_iter().map(|piece| match piece {
        Piece::Text(text) => Some(text.to_string()),
        Piece::Field("worktree") => Some(place.worktree.clone()),
        Piece::Field("repo") => Some(place.repo.clone()),
        Piece::Field("number") => place.number.map(|number| number.to_string()),
        Piece::Field(_) => place.port.map(|port| port.to_string()),
    }).collect();
    Some(parts?.concat()).filter(|url| !url.is_empty())
}

pub fn dev_url(template: Option<&str>, repo: &Path, worktree: &Path) -> Option<String> {
    let template = template?;
    let port = if template.contains("{port}") { worktree_port(worktree) } else { None };
    expand(template, &Place { worktree: name_of(worktree), repo: name_of(repo), number: slot_number(repo, worktree), port })
}
