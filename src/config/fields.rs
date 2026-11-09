use toml::{Table, Value};

use crate::sh::Result;

pub(super) fn subtable<'a>(table: &'a Table, key: &str, name: &str) -> Result<Option<&'a Table>> {
    table.get(key).map(|value| value.as_table().ok_or(format!("{name}: must be a table"))).transpose()
}

pub(super) fn strings(value: &Value, key: &str) -> Result<Vec<String>> {
    let items = value.as_array().ok_or(format!("{key}: must be a list of strings"))?;
    items.iter().map(|item| item.as_str().map(str::to_string).ok_or(format!("{key}: must be a list of strings"))).collect()
}

pub(super) fn text(table: &Table, field: &str, key: &str) -> Result<Option<String>> {
    let Some(value) = table.get(field) else { return Ok(None) };
    match value.as_str().map(str::trim) {
        Some("") => Err(format!("{key}: must not be empty")),
        Some(text) => Ok(Some(text.to_string())),
        None => Err(format!("{key}: must be a string")),
    }
}
