use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::repo::main_checkout;
use crate::sh::Result;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    Vault,
    Linear,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Vault, Source::Linear];

    pub fn key(self) -> &'static str {
        match self {
            Source::Vault => "vault",
            Source::Linear => "linear",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Source::Vault => "vault",
            Source::Linear => "Linear",
        }
    }
}

pub const DEFAULT_SETUP_SKILL: &str = "kelpie:set-up-worktree";
pub const REPO_FILE: &str = ".kelpie.toml";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub setup_skill: Option<String>,
    pub dev_server: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub sources: Option<Vec<Source>>,
    pub worktree: Worktree,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layer {
    Repo,
    Machine,
    Default,
}

impl Layer {
    pub fn label(self) -> &'static str {
        match self {
            Layer::Repo => REPO_FILE,
            Layer::Machine => "machine config",
            Layer::Default => "default",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Setting<T> {
    pub value: T,
    pub layer: Layer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub sources: Setting<Vec<Source>>,
    pub setup_skill: Setting<String>,
    pub dev_server: Setting<Option<String>>,
}

pub struct Loaded {
    pub machine: PathBuf,
    pub repo: Option<PathBuf>,
    pub settings: Settings,
}

pub fn dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("kelpie")
}

pub fn path() -> PathBuf {
    std::env::var("KELPIE_CONFIG").map(PathBuf::from).unwrap_or_else(|_| dir().join("config.toml"))
}

pub fn repo_file(repo: &Path) -> Option<PathBuf> {
    main_checkout(repo).ok().map(|main| main.join(REPO_FILE))
}

fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub fn load_machine(path: &Path) -> Result<Machine> {
    read(path)?.map_or(Ok(Machine::default()), |text| parse_machine(&text, &path.display().to_string()))
}

pub fn load_repo(path: &Path) -> Result<Worktree> {
    read(path)?.map_or(Ok(Worktree::default()), |text| parse_repo(&text, &path.display().to_string()))
}

pub fn unknown_keys(table: &Table, prefix: &str, known: &[&str]) -> Result<()> {
    match table.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(format!("{prefix}{key}: unknown key; known keys here: {}", known.join(", "))),
        None => Ok(()),
    }
}

fn subtable<'a>(table: &'a Table, key: &str, name: &str) -> Result<Option<&'a Table>> {
    table.get(key).map(|value| value.as_table().ok_or(format!("{name}: must be a table"))).transpose()
}

fn strings(value: &Value, key: &str) -> Result<Vec<String>> {
    let items = value.as_array().ok_or(format!("{key}: must be a list of strings"))?;
    items.iter().map(|item| item.as_str().map(str::to_string).ok_or(format!("{key}: must be a list of strings"))).collect()
}

fn text(table: &Table, field: &str, key: &str) -> Result<Option<String>> {
    let Some(value) = table.get(field) else { return Ok(None) };
    match value.as_str().map(str::trim) {
        Some("") => Err(format!("{key}: must not be empty")),
        Some(text) => Ok(Some(text.to_string())),
        None => Err(format!("{key}: must be a string")),
    }
}

fn backlog_sources(backlog: &Table) -> Result<Option<Vec<Source>>> {
    let Some(value) = backlog.get("sources") else { return Ok(None) };
    let mut found = Vec::new();
    for name in strings(value, "board.backlog.sources")? {
        let source = Source::ALL.into_iter().find(|source| source.key() == name)
            .ok_or(format!("board.backlog.sources: \"{name}\" is not a source; use \"vault\" or \"linear\""))?;
        if found.contains(&source) {
            return Err(format!("board.backlog.sources: \"{name}\" is listed twice"));
        }
        found.push(source);
    }
    Ok(Some(found))
}

fn board(table: &Table) -> Result<Option<Vec<Source>>> {
    let Some(board) = subtable(table, "board", "board")? else { return Ok(None) };
    unknown_keys(board, "board.", &["backlog"])?;
    let Some(backlog) = subtable(board, "backlog", "board.backlog")? else { return Ok(None) };
    unknown_keys(backlog, "board.backlog.", &["sources"])?;
    backlog_sources(backlog)
}

fn worktree(table: &Table) -> Result<Worktree> {
    let Some(worktree) = subtable(table, "worktree", "worktree")? else { return Ok(Worktree::default()) };
    unknown_keys(worktree, "worktree.", &["setup_skill", "dev_server"])?;
    Ok(Worktree {
        setup_skill: text(worktree, "setup_skill", "worktree.setup_skill")?,
        dev_server: text(worktree, "dev_server", "worktree.dev_server")?,
    })
}

fn table(text: &str, source: &str) -> Result<Table> {
    text.parse().map_err(|error: toml::de::Error| format!("{source}: {}", error.to_string().trim()))
}

pub fn parse_machine(text: &str, source: &str) -> Result<Machine> {
    let table = table(text, source)?;
    let fail = |error: String| format!("{source}: {error}");
    unknown_keys(&table, "", &["board", "worktree"]).map_err(fail)?;
    Ok(Machine { sources: board(&table).map_err(fail)?, worktree: worktree(&table).map_err(fail)? })
}

// The [[actions]] are actions.rs's to check.
pub fn parse_repo(text: &str, source: &str) -> Result<Worktree> {
    let table = table(text, source)?;
    let fail = |error: String| format!("{source}: {error}");
    unknown_keys(&table, "", &["actions", "worktree"]).map_err(fail)?;
    worktree(&table).map_err(fail)
}

fn pick<T: Clone>(repo: Option<&T>, machine: Option<&T>) -> Option<Setting<T>> {
    match (repo, machine) {
        (Some(value), _) => Some(Setting { value: value.clone(), layer: Layer::Repo }),
        (None, Some(value)) => Some(Setting { value: value.clone(), layer: Layer::Machine }),
        (None, None) => None,
    }
}

fn or_default<T>(setting: Option<Setting<T>>, value: T) -> Setting<T> {
    setting.unwrap_or(Setting { value, layer: Layer::Default })
}

pub fn settings(repo: &Worktree, machine: &Machine) -> Settings {
    let dev_server = pick(repo.dev_server.as_ref(), machine.worktree.dev_server.as_ref());
    Settings {
        sources: or_default(pick(None, machine.sources.as_ref()), Vec::new()),
        setup_skill: or_default(pick(repo.setup_skill.as_ref(), machine.worktree.setup_skill.as_ref()), DEFAULT_SETUP_SKILL.to_string()),
        dev_server: or_default(dev_server.map(|setting| Setting { value: Some(setting.value), layer: setting.layer }), None),
    }
}

pub fn resolve(repo: Option<&Path>) -> Result<Loaded> {
    let machine = path();
    let repo = repo.and_then(repo_file);
    let own = repo.as_deref().map(load_repo).transpose()?.unwrap_or_default();
    Ok(Loaded { settings: settings(&own, &load_machine(&machine)?), machine, repo })
}

pub fn sources() -> Result<Vec<Source>> {
    Ok(resolve(None)?.settings.sources.value)
}

fn quoted(items: &[&str]) -> String {
    format!("[{}]", items.iter().map(|item| format!("{item:?}")).collect::<Vec<_>>().join(", "))
}

fn found(path: &Path) -> &'static str {
    if path.exists() { "found" } else { "not found" }
}

pub fn describe(loaded: &Loaded) -> String {
    let Settings { sources, setup_skill, dev_server } = &loaded.settings;
    let repo = match &loaded.repo {
        Some(path) => format!("{} ({})", path.display(), found(path)),
        None => "none: not in a git repo (--repo <path> names one)".to_string(),
    };
    let mut lines: Vec<String> = [
        (REPO_FILE, repo),
        ("machine config", format!("{} ({})", loaded.machine.display(), found(&loaded.machine))),
        ("defaults", "built in".to_string()),
    ].into_iter().map(|(layer, file)| format!("{:<16}{file}", format!("{layer}:"))).collect();
    lines.push(String::new());
    let names: Vec<&str> = sources.value.iter().map(|source| source.key()).collect();
    let mut key = |name: &str, value: String, layer: Layer, about: &str| {
        lines.push(format!("{name} = {value}  ({})", layer.label()));
        lines.push(format!("  {about}"));
    };
    key("board.backlog.sources", quoted(&names), sources.layer,
        "Where the lead finds work for the board's Backlog tab: \"vault\", \"linear\" or both; machine config only.");
    key("worktree.setup_skill", format!("{:?}", setup_skill.value), setup_skill.layer,
        "The skill a task's Claude runs first to make its worktree runnable.");
    key("worktree.dev_server", dev_server.value.as_ref().map_or("none".to_string(), |command| format!("{command:?}")), dev_server.layer,
        "The shell command the board's w key runs in a tab of the task's Space; without one there is no w.");
    lines.join("\n")
}

pub fn show(cwd: &Path, repo: Option<&str>) -> Result<String> {
    let repo = match repo {
        Some(path) => {
            let path = PathBuf::from(path);
            main_checkout(&path).map_err(|_| format!("--repo {}: not a git repository", path.display()))?;
            Some(path)
        }
        None => main_checkout(cwd).ok().map(|_| cwd.to_path_buf()),
    };
    Ok(describe(&resolve(repo.as_deref())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE: &str = "[board.backlog]\nsources = [\"vault\", \"linear\"]\n\n[worktree]\nsetup_skill = \"my-tools:set-up-worktree\"\ndev_server = \"bin/dev\"\n";
    const REPO: &str = "[worktree]\nsetup_skill = \"app-tools:set-up-worktree\"\ndev_server = \"npm run dev\"\n\n[[actions]]\nkey = \"B\"\ntitle = \"Build\"\nrun = \"cargo build\"\n";

    fn machine(text: &str) -> Machine {
        parse_machine(text, "config.toml").unwrap()
    }

    fn repo(text: &str) -> Worktree {
        parse_repo(text, ".kelpie.toml").unwrap()
    }

    #[test]
    fn an_empty_file_is_every_default() {
        assert_eq!(machine(""), Machine::default());
        assert_eq!(repo(""), Worktree::default());
        let missing = std::env::temp_dir().join(format!("kelpie-config-missing-{}.toml", std::process::id()));
        assert_eq!(load_machine(&missing).unwrap(), Machine::default());
        assert_eq!(load_repo(&missing).unwrap(), Worktree::default());
        let defaults = settings(&Worktree::default(), &Machine::default());
        assert_eq!(defaults.sources, Setting { value: Vec::new(), layer: Layer::Default });
        assert_eq!(defaults.setup_skill, Setting { value: DEFAULT_SETUP_SKILL.to_string(), layer: Layer::Default });
        assert_eq!(defaults.dev_server, Setting { value: None, layer: Layer::Default });
    }

    #[test]
    fn reads_the_machine_keys() {
        let config = machine(MACHINE);
        assert_eq!(config.sources, Some(vec![Source::Vault, Source::Linear]));
        assert_eq!(config.worktree.setup_skill.as_deref(), Some("my-tools:set-up-worktree"));
        assert_eq!(config.worktree.dev_server.as_deref(), Some("bin/dev"));
        assert_eq!(machine("[board.backlog]\nsources = []\n").sources, Some(Vec::new()));
    }

    #[test]
    fn a_repo_file_holds_worktree_and_actions_together() {
        let own = repo(REPO);
        assert_eq!(own.setup_skill.as_deref(), Some("app-tools:set-up-worktree"));
        assert_eq!(own.dev_server.as_deref(), Some("npm run dev"));
        let (actions, errors) = crate::actions::parse(REPO, ".kelpie.toml");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(actions[0].title, "Build");
    }

    #[test]
    fn each_key_comes_from_the_highest_layer_that_sets_it() {
        let both = settings(&repo(REPO), &machine(MACHINE));
        assert_eq!(both.setup_skill, Setting { value: "app-tools:set-up-worktree".to_string(), layer: Layer::Repo });
        assert_eq!(both.dev_server, Setting { value: Some("npm run dev".to_string()), layer: Layer::Repo });
        assert_eq!(both.sources.layer, Layer::Machine);
        let skill_only = settings(&repo("[worktree]\nsetup_skill = \"app-tools:set-up-worktree\"\n"), &machine(MACHINE));
        assert_eq!(skill_only.setup_skill.layer, Layer::Repo);
        assert_eq!(skill_only.dev_server, Setting { value: Some("bin/dev".to_string()), layer: Layer::Machine });
        let server_only = settings(&repo("[worktree]\ndev_server = \"npm run dev\"\n"), &machine(MACHINE));
        assert_eq!(server_only.setup_skill, Setting { value: "my-tools:set-up-worktree".to_string(), layer: Layer::Machine });
        assert_eq!(server_only.dev_server.layer, Layer::Repo);
        let machine_only = settings(&Worktree::default(), &machine(MACHINE));
        assert_eq!((machine_only.setup_skill.layer, machine_only.dev_server.layer), (Layer::Machine, Layer::Machine));
        let repo_only = settings(&repo(REPO), &Machine::default());
        assert_eq!((repo_only.setup_skill.layer, repo_only.dev_server.layer, repo_only.sources.layer), (Layer::Repo, Layer::Repo, Layer::Default));
    }

    #[test]
    fn the_repo_file_is_read_from_the_main_checkout() {
        let dir = std::env::temp_dir().join(format!("kelpie-config-repo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::repo::git(&dir, &["init", "--quiet"]).unwrap();
        crate::repo::git(&dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--allow-empty", "-m", "first"]).unwrap();
        let worktree = dir.with_file_name(format!("wt1.{}", crate::repo::name_of(&dir)));
        crate::repo::git(&dir, &["worktree", "add", "--quiet", "--detach", &worktree.to_string_lossy()]).unwrap();
        std::fs::write(worktree.join(REPO_FILE), "[worktree]\ndev_server = \"from the worktree\"\n").unwrap();
        std::fs::write(dir.join(REPO_FILE), "[worktree]\ndev_server = \"npm run dev\"\n").unwrap();
        assert_eq!(repo_file(&worktree).unwrap().canonicalize().unwrap(), dir.join(REPO_FILE).canonicalize().unwrap());
        assert_eq!(load_repo(&repo_file(&worktree).unwrap()).unwrap().dev_server.as_deref(), Some("npm run dev"));
        crate::repo::git(&dir, &["worktree", "remove", "--force", &worktree.to_string_lossy()]).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_keys_and_bad_values_name_the_file_and_the_key() {
        let error = |text: &str| parse_machine(text, "/home/u/.config/kelpie/config.toml").unwrap_err();
        assert_eq!(error("colour = 1\n"), "/home/u/.config/kelpie/config.toml: colour: unknown key; known keys here: board, worktree");
        assert_eq!(error("[backlog]\nsources = []\n"), "/home/u/.config/kelpie/config.toml: backlog: unknown key; known keys here: board, worktree");
        assert_eq!(error("[board.backlog]\nsource = []\n"), "/home/u/.config/kelpie/config.toml: board.backlog.source: unknown key; known keys here: sources");
        assert_eq!(error("[board.backlog]\nsources = [\"jira\"]\n"), "/home/u/.config/kelpie/config.toml: board.backlog.sources: \"jira\" is not a source; use \"vault\" or \"linear\"");
        assert!(error("[board.backlog]\nsources = \"vault\"\n").contains("board.backlog.sources: must be a list of strings"));
        assert!(error("[board.backlog]\nsources = [\"vault\", \"vault\"]\n").contains("listed twice"));
        assert!(error("[repos.app]\nsetup = [\"x\"]\n").contains("repos: unknown key"));
        assert!(error("[worktree]\nsetup = \"x\"\n").contains("worktree.setup: unknown key; known keys here: setup_skill, dev_server"));
        assert!(error("[worktree]\ndev_server = [\"bin/dev\"]\n").contains("worktree.dev_server: must be a string"));
        assert!(error("[worktree]\nsetup_skill = \" \"\n").contains("worktree.setup_skill: must not be empty"));
        assert!(error("not toml [").starts_with("/home/u/.config/kelpie/config.toml: TOML parse error"));
        let repo_error = |text: &str| parse_repo(text, "/src/app/.kelpie.toml").unwrap_err();
        assert_eq!(repo_error("[board.backlog]\nsources = []\n"), "/src/app/.kelpie.toml: board: unknown key; known keys here: actions, worktree");
        assert_eq!(repo_error("[worktree]\nport = 3000\n"), "/src/app/.kelpie.toml: worktree.port: unknown key; known keys here: setup_skill, dev_server");
    }

    #[test]
    fn describe_names_each_value_and_its_layer() {
        let loaded = |own: Worktree, machine: Machine| Loaded { machine: PathBuf::from("/c.toml"), repo: Some(PathBuf::from("/src/app/.kelpie.toml")), settings: settings(&own, &machine) };
        let shown = describe(&loaded(Worktree::default(), Machine::default()));
        assert!(shown.contains("/src/app/.kelpie.toml (not found)"), "{shown}");
        assert!(shown.contains("machine config: /c.toml (not found)"));
        assert!(shown.contains("board.backlog.sources = []  (default)\n  Where the lead finds work"));
        assert!(shown.contains("worktree.setup_skill = \"kelpie:set-up-worktree\"  (default)"));
        assert!(shown.contains("worktree.dev_server = none  (default)"));
        let shown = describe(&loaded(repo("[worktree]\ndev_server = \"npm run dev\"\n"), machine(MACHINE)));
        assert!(shown.contains("board.backlog.sources = [\"vault\", \"linear\"]  (machine config)"));
        assert!(shown.contains("worktree.setup_skill = \"my-tools:set-up-worktree\"  (machine config)"));
        assert!(shown.contains("worktree.dev_server = \"npm run dev\"  (.kelpie.toml)"));
        let outside = Loaded { machine: PathBuf::from("/c.toml"), repo: None, settings: settings(&Worktree::default(), &Machine::default()) };
        assert!(describe(&outside).contains("none: not in a git repo"));
    }
}
