use std::path::Path;

use toml_edit::{Array, DocumentMut, Item, Table, TableLike, Value};

use crate::sh::Result;

#[derive(Clone, Debug, PartialEq)]
pub enum Set {
    Bool(bool),
    Number(i64),
    Text(String),
    List(Vec<String>),
}

impl Set {
    pub fn shown(&self) -> String {
        self.value().to_string().trim().to_string()
    }

    fn value(&self) -> Value {
        match self {
            Set::Bool(on) => Value::from(*on),
            Set::Number(number) => Value::from(*number),
            Set::Text(text) => Value::from(text.as_str()),
            Set::List(items) => Value::Array(items.iter().map(String::as_str).collect::<Array>()),
        }
    }
}

fn document(text: &str) -> Result<DocumentMut> {
    text.parse::<DocumentMut>().map_err(|error| error.to_string().trim().to_string())
}

fn split(key: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = key.split('.').collect();
    let leaf = parts.pop().unwrap_or_default();
    (parts, leaf)
}

fn table_for<'a>(root: &'a mut dyn TableLike, path: &[&str], key: &str) -> Result<&'a mut dyn TableLike> {
    let Some((name, rest)) = path.split_first() else { return Ok(root) };
    let item = root.entry(name).or_insert_with(|| {
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    });
    let table = item.as_table_like_mut().ok_or_else(|| format!("{key}: {name} is not a table in this file, so kebbie leaves it alone"))?;
    table_for(table, rest, key)
}

// Only the key's value changes, keeping its trailing comment: every other line, comment and the order stay as they were.
pub fn set(text: &str, key: &str, value: &Set) -> Result<String> {
    let mut document = document(text)?;
    let (path, leaf) = split(key);
    let table = table_for(document.as_table_mut(), &path, key)?;
    let mut new = value.value();
    match table.get_mut(leaf) {
        Some(item) if item.is_value() => {
            if let Some(old) = item.as_value() {
                *new.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(new);
        }
        _ => {
            table.insert(leaf, Item::Value(new));
        }
    }
    Ok(document.to_string())
}

pub fn remove(text: &str, key: &str) -> Result<String> {
    let mut document = document(text)?;
    let (path, leaf) = split(key);
    let mut table: &mut dyn TableLike = document.as_table_mut();
    for name in path {
        match table.get_mut(name).and_then(Item::as_table_like_mut) {
            Some(inner) => table = inner,
            None => return Ok(text.to_string()),
        }
    }
    table.remove(leaf);
    Ok(document.to_string())
}

// Writes through a link to the file it points at; with replace_link, a link becomes a file of this machine's own.
pub fn write(file: &Path, machine: bool, replace_link: bool, change: impl FnOnce(&str) -> Result<String>) -> Result<()> {
    if replace_link && std::fs::symlink_metadata(file).is_ok_and(|meta| meta.file_type().is_symlink()) {
        let text = std::fs::read_to_string(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let changed = change(&text)?;
        crate::routine_file::check(file, machine, &changed)?;
        return crate::state::write(file, changed);
    }
    crate::routine_file::rewrite(file, machine, change)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "# my kebbie\n\n[task]\nsandbox = true # off on the build box\nrules_skill = \"kebbie:work-rules\"\n\n[restart]\n# ten is plenty\nidle_minutes = 10\n";

    #[test]
    fn a_value_changes_in_place_and_every_comment_and_line_around_it_stays() {
        let changed = set(FILE, "task.sandbox", &Set::Bool(false)).unwrap();
        assert_eq!(changed, FILE.replace("sandbox = true # off", "sandbox = false # off"));
        let changed = set(&changed, "restart.idle_minutes", &Set::Number(25)).unwrap();
        assert!(changed.ends_with("[restart]\n# ten is plenty\nidle_minutes = 25\n"), "{changed}");
        let changed = set(&changed, "task.rules_skill", &Set::Text("~/code/team/RULES.md".to_string())).unwrap();
        assert!(changed.contains("rules_skill = \"~/code/team/RULES.md\"\n"), "{changed}");
        assert!(changed.starts_with("# my kebbie\n\n[task]\n"), "{changed}");
    }

    #[test]
    fn a_new_key_joins_its_table_or_a_new_one_at_the_end() {
        let changed = set(FILE, "task.test_commands", &Set::List(vec!["bin/test".to_string(), "make check".to_string()])).unwrap();
        assert!(changed.contains("rules_skill = \"kebbie:work-rules\"\ntest_commands = [\"bin/test\", \"make check\"]\n\n[restart]"), "{changed}");
        let changed = set(FILE, "board.backlog.sources", &Set::List(vec!["vault".to_string()])).unwrap();
        assert!(changed.starts_with(FILE) && changed.ends_with("\n[board.backlog]\nsources = [\"vault\"]\n"), "{changed}");
        assert_eq!(changed.parse::<toml::Table>().unwrap()["board"]["backlog"]["sources"][0].as_str(), Some("vault"));
        assert_eq!(set("", "machine.name", &Set::Text("mac".to_string())).unwrap(), "[machine]\nname = \"mac\"\n");
        assert!(set("task = 3\n", "task.sandbox", &Set::Bool(true)).unwrap_err().contains("task is not a table"));
    }

    #[test]
    fn removing_a_key_leaves_the_rest_and_a_missing_one_changes_nothing() {
        let removed = remove(FILE, "task.sandbox").unwrap();
        assert_eq!(removed, FILE.replace("sandbox = true # off on the build box\n", ""));
        assert_eq!(remove(FILE, "hub.url").unwrap(), FILE);
        assert_eq!(remove(FILE, "task.profile").unwrap(), FILE);
    }

    #[test]
    fn a_write_checks_the_whole_file_and_a_link_is_written_through_or_replaced() {
        let dir = crate::sh::test_dir("config-edit");
        let (team, own) = (dir.join("team.toml"), dir.join("app.toml"));
        std::fs::write(&team, "[ci]\nwatch = true\n").unwrap();
        std::os::unix::fs::symlink(&team, &own).unwrap();
        let refused = write(&own, false, false, |text| set(text, "ci.watch", &Set::Text("yes".to_string()))).unwrap_err();
        assert!(refused.contains("ci.watch: must be true or false"), "{refused}");
        assert_eq!(std::fs::read_to_string(&team).unwrap(), "[ci]\nwatch = true\n", "a change that does not parse is never written");
        write(&own, false, false, |text| set(text, "ci.watch", &Set::Bool(false))).unwrap();
        assert_eq!(std::fs::read_to_string(&team).unwrap(), "[ci]\nwatch = false\n");
        assert!(std::fs::symlink_metadata(&own).unwrap().file_type().is_symlink());
        write(&own, false, true, |text| set(text, "ci.watch", &Set::Bool(true))).unwrap();
        assert!(!std::fs::symlink_metadata(&own).unwrap().file_type().is_symlink(), "the link became a file of its own");
        assert_eq!((std::fs::read_to_string(&own).unwrap(), std::fs::read_to_string(&team).unwrap()), ("[ci]\nwatch = true\n".to_string(), "[ci]\nwatch = false\n".to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
