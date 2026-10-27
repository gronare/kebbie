use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::sh::Result;

// Every kebbie subcommand a task's Claude is told to run (roles/task.md, the parts kebbie fills in, and `config` in the set-up-worktree skill) that reaches kebbie's state or herdr's socket, so it runs outside the sandbox; `docs` only reads the install's checkout and stays in.
pub const TASK_COMMANDS: [&str; 10] = ["peers", "tell", "ask", "report", "steps", "accepted", "routine ok", "link", "backlog add", "config"];
pub const DOMAINS: [&str; 8] = ["crates.io", "static.crates.io", "index.crates.io", "registry.npmjs.org", "github.com", "objects.githubusercontent.com", "pypi.org", "files.pythonhosted.org"];
pub const WRITES: [&str; 4] = ["~/.cargo/registry", "~/.cargo/git", "~/.npm", "~/.cache"];
pub const OPS_COMMANDS: [&str; 3] = ["ssh *", "scp *", "rsync *"];
const SHELL_FILES: [&str; 11] = ["~/.zshrc", "~/.zshenv", "~/.zprofile", "~/.zlogin", "~/.zlogout", "~/.bashrc", "~/.bash_profile", "~/.bash_login", "~/.bash_logout", "~/.profile", "~/.config/fish/**"];
const HOSTS: [&str; 5] = ["kebbie", "herdr", "tmux", "screen", "zellij"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Policy {
    pub enabled: Option<bool>,
    pub domains: Option<Vec<String>>,
    pub writes: Option<Vec<String>>,
    pub excluded: Vec<String>,
}

impl Policy {
    pub fn on(&self, configured: bool) -> bool {
        self.enabled.unwrap_or(configured)
    }

    pub fn domains(&self) -> Vec<String> {
        self.domains.clone().unwrap_or_else(|| DOMAINS.map(str::to_string).to_vec())
    }

    pub fn writes(&self) -> Vec<String> {
        self.writes.clone().unwrap_or_else(|| WRITES.map(str::to_string).to_vec())
    }
}

pub fn excluded(kebbie: &str, policy: &Policy) -> Vec<String> {
    TASK_COMMANDS.iter().map(|command| format!("{kebbie} {command} *")).chain(policy.excluded.iter().cloned()).collect()
}

// A task's worktree and its temp folder, which sits in the worktree's own git dir.
pub struct TaskDirs {
    pub worktree: PathBuf,
    pub tmp: PathBuf,
    pub halves: Vec<TaskDirs>,
}

impl TaskDirs {
    pub fn new(worktree: &Path, tmp: &Path) -> TaskDirs {
        TaskDirs { worktree: worktree.to_path_buf(), tmp: tmp.to_path_buf(), halves: Vec::new() }
    }

    // This half and the task's halves in its other repos, each the task's own to write.
    pub fn all(&self) -> Vec<&TaskDirs> {
        std::iter::once(self).chain(&self.halves).collect()
    }

    pub fn git_dir(&self) -> PathBuf {
        self.tmp.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    // A linked worktree's git dir is <common>/worktrees/<name>; the main checkout's is the common dir itself.
    pub fn common_dir(&self) -> PathBuf {
        let git_dir = self.git_dir();
        match git_dir.parent() {
            Some(worktrees) if worktrees.file_name().is_some_and(|name| name == "worktrees") => worktrees.parent().map(Path::to_path_buf).unwrap_or(git_dir),
            _ => git_dir,
        }
    }

    pub fn main_checkout(&self) -> Option<PathBuf> {
        self.common_dir().parent().map(Path::to_path_buf).filter(|main| *main != self.worktree)
    }
}

// Every other pooled worktree's git dir in the shared .git: its config.worktree and kebbie-hooks run as the user when that slot pushes.
pub fn sibling_git_dirs(dirs: &TaskDirs) -> Vec<PathBuf> {
    let own = dirs.git_dir();
    let mut found: Vec<PathBuf> = std::fs::read_dir(dirs.common_dir().join("worktrees")).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|dir| dir.is_dir() && *dir != own).collect();
    found.sort();
    found
}

// What git follows to find a worktree's config: rewriting one points the user's and kebbie's unsandboxed git at a config the task wrote.
fn git_pointers(dirs: &TaskDirs) -> [PathBuf; 4] {
    let git_dir = dirs.git_dir();
    [dirs.worktree.join(".git"), git_dir.join("commondir"), git_dir.join("gitdir"), git_dir.join("config.worktree")]
}

fn other_sessions_temp() -> [String; 2] {
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    [format!("/tmp/claude-{uid}"), format!("/private/tmp/claude-{uid}")]
}

pub fn block(kebbie: &str, policy: &Policy, dirs: Option<&TaskDirs>, mac: bool) -> Value {
    let mut network = json!({ "allowedDomains": policy.domains() });
    if mac {
        network["allowLocalBinding"] = json!(true);
    }
    let mut writes = policy.writes();
    let mut denied: Vec<String> = other_sessions_temp().to_vec();
    if let Some(dirs) = dirs {
        writes.push(dirs.tmp.display().to_string());
        denied.extend(git_pointers(dirs).iter().chain([&dirs.git_dir().join("kebbie-hooks")]).map(|path| path.display().to_string()));
        denied.extend(sibling_git_dirs(dirs).iter().map(|dir| dir.display().to_string()));
    }
    json!({
        "enabled": true, "allowUnsandboxedCommands": false, "failIfUnavailable": true,
        "excludedCommands": excluded(kebbie, policy),
        "network": network,
        "filesystem": { "allowWrite": writes, "denyWrite": denied, "denyRead": SECRETS },
    })
}

// Credentials a sandboxed shell could push or publish with through an allowed host.
pub const SECRETS: [&str; 6] = ["~/.ssh", "~/.config/gh", "~/.git-credentials", "~/.netrc", "~/.cargo/credentials.toml", "~/.cargo/credentials"];

pub fn reader_block() -> Value {
    json!({ "enabled": true, "allowUnsandboxedCommands": false, "failIfUnavailable": true, "excludedCommands": [], "network": { "allowedDomains": [] }, "filesystem": { "allowWrite": [] } })
}

// Files in a home that Claude Code, git, ssh, the shell, the OS or a tool on PATH later loads or runs as the user, outside any sandbox.
const HOME_LOADED: [&str; 42] = [
    ".claude.json", ".claude.json.backup", ".claude/settings*.json", ".claude/CLAUDE.md", ".claude/*.sh", ".claude/*.py", ".claude/*.js", ".claude/*.mjs",
    ".claude/plugins/**", ".claude/hooks/**", ".claude/agents/**", ".claude/skills/**", ".claude/commands/**", ".claude/rules/**", ".claude/workflows/**", ".claude/output-styles/**", ".claude/scheduled_tasks.json",
    ".gitconfig", ".config/git/**", ".ssh/**", ".netrc", ".git-credentials", ".config/gh/**",
    "Library/LaunchAgents/**", ".config/autostart/**", ".config/systemd/**",
    ".local/bin/**", "bin/**", ".cargo/**", ".rustup/**", ".npm/**", ".npmrc", ".cache/**", ".bun/**", "go/bin/**",
    ".config/direnv/**", ".local/share/direnv/**", ".config/mise/**", ".local/state/mise/**",
    ".config/herdr/**", ".local/share/herdr/**", ".codex/**",
];
const SHELL_DIRS: [&str; 2] = [".config/zsh/**", ".oh-my-zsh/**"];

// Inside a worktree, what Claude Code loads as its own configuration and runs outside the sandbox: the same list its sandbox keeps from the shell.
pub const WORKTREE_LOADED: [&str; 9] = [".claude/settings.json", ".claude/settings.local.json", ".claude/hooks", ".claude/skills", ".claude/agents", ".claude/commands", ".claude/workflows", ".claude/scheduled_tasks.json", ".mcp.json"];

pub struct Protected {
    pub state: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub home: PathBuf,
    pub claude_home: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub install: Option<PathBuf>,
    pub path: Vec<PathBuf>,
}

impl Protected {
    pub fn here() -> Protected {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        let data = std::env::var("XDG_DATA_HOME").ok().filter(|dir| !dir.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
        let path = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default();
        Protected {
            state: crate::lead::state_location().unwrap_or_else(|_| home.join(".local/state/kebbie")), config: crate::config::dir(), data: data.join("kebbie"),
            claude_home: std::env::var("CLAUDE_CONFIG_DIR").ok().filter(|dir| !dir.is_empty()).map(PathBuf::from),
            codex_home: std::env::var("CODEX_HOME").ok().filter(|dir| !dir.is_empty()).map(PathBuf::from), install: crate::lead::install_root().ok(), path, home,
        }
    }

    // Only Edit(path) rules guard Claude Code's file tools: a Write(...) or MultiEdit(...) path rule is never consulted. `//` anchors at /, `~/` at the home.
    pub fn deny_rules(&self, kebbie: &str, dirs: Option<&TaskDirs>) -> Vec<String> {
        let absolute = |path: &Path| format!("/{}", path.display());
        let tree = |path: &Path| format!("{}/**", absolute(path));
        let mut paths = vec![tree(&self.state), tree(&self.config), tree(&self.data), absolute(Path::new(kebbie)), "~/.config/kebbie/**".to_string()];
        paths.extend(HOME_LOADED.iter().chain(&SHELL_FILES).chain(&SHELL_DIRS).map(|path| format!("~/{}", path.trim_start_matches("~/"))));
        if let Some(claude) = self.claude_home.as_ref().filter(|claude| **claude != self.home.join(".claude")) {
            paths.extend(HOME_LOADED.iter().filter_map(|path| path.strip_prefix(".claude/")).chain([".claude.json"]).map(|path| format!("{}/{path}", absolute(claude))));
        }
        if let Some(codex) = self.codex_home.as_ref().filter(|codex| **codex != self.home.join(".codex")) {
            paths.push(tree(codex));
        }
        let worktrees: Vec<&Path> = dirs.map(|dirs| dirs.all().into_iter().map(|half| half.worktree.as_path()).collect()).unwrap_or_default();
        let outside = |path: &Path| path.is_absolute() && worktrees.iter().all(|worktree| !path.starts_with(worktree) && !worktree.starts_with(path));
        if let Some(install) = self.install.as_deref().filter(|install| outside(install)) {
            paths.push(tree(install));
        }
        paths.extend(self.path.iter().filter(|dir| outside(dir) && dir.parent().is_some()).map(|dir| tree(dir)));
        paths.extend(other_sessions_temp().iter().map(|dir| tree(Path::new(dir))));
        for dirs in dirs.map(TaskDirs::all).unwrap_or_default() {
            let common = dirs.common_dir();
            paths.extend(WORKTREE_LOADED.iter().map(|path| {
                let path = dirs.worktree.join(path);
                if path.extension().is_some() { absolute(&path) } else { tree(&path) }
            }));
            paths.extend(git_pointers(dirs).iter().map(|path| absolute(path)));
            paths.push(tree(&dirs.worktree.join(".git")));
            paths.extend(["config", "config.worktree"].map(|file| absolute(&common.join(file))));
            paths.extend(["hooks", "modules", "worktrees/*/kebbie-hooks"].map(|dir| tree(&common.join(dir))));
            paths.extend(["commondir", "gitdir", "config.worktree"].map(|file| absolute(&common.join("worktrees/*").join(file))));
            paths.extend(sibling_git_dirs(dirs).iter().map(|dir| tree(dir)));
            if let Some(main) = dirs.main_checkout().filter(|main| !worktrees.contains(&main.as_path())) {
                paths.extend([tree(&main.join(".claude")), absolute(&main.join(".mcp.json"))]);
            }
            let ancestors = dirs.worktree.ancestors().skip(1).filter(|dir| dir.parent().is_some() && *dir != self.home && Some(*dir) != dirs.main_checkout().as_deref() && worktrees.iter().all(|worktree| !worktree.starts_with(dir.join(".claude"))));
            paths.extend(ancestors.flat_map(|dir| [tree(&dir.join(".claude")), absolute(&dir.join(".mcp.json"))]).collect::<Vec<_>>());
        }
        let mut seen = std::collections::HashSet::new();
        paths.into_iter().filter(|path| seen.insert(path.clone())).map(|path| format!("Edit({path})")).collect()
    }
}

// The file tools write only in the task's worktrees, their temp folders and Claude Code's notes for those worktrees and their repos, never what a worktree loads as Claude Code's configuration or git's.
pub fn file_refusal(target: &Path, dirs: &TaskDirs, claude_home: &Path) -> Option<String> {
    let target = resolved(target);
    let halves = dirs.all();
    let projects = halves.iter().flat_map(|half| [Some(half.worktree.clone()), half.main_checkout()]).flatten().map(|folder| crate::agent::claude::transcripts_dir(claude_home, &folder));
    let notes: Vec<PathBuf> = projects.chain([claude_home.join("plans")]).map(|dir| resolved(&dir)).collect();
    if halves.iter().any(|half| target.starts_with(resolved(&half.tmp))) || notes.iter().any(|dir| target.starts_with(dir)) {
        return None;
    }
    let Some(inside) = halves.iter().find_map(|half| target.strip_prefix(resolved(&half.worktree)).ok().map(Path::to_path_buf)) else {
        return Some(format!("{} is outside this task's worktree {}: a task's file tools write only there and in its temp folder {}. Ask the user to make a change elsewhere", target.display(), dirs.worktree.display(), dirs.tmp.display()));
    };
    let loaded = WORKTREE_LOADED.iter().chain(&[".git"]).find(|path| inside.starts_with(path));
    loaded.map(|path| format!("{path} in the worktree is configuration Claude Code or git runs as the user outside the sandbox; a task leaves it alone. Ask the user to make this change"))
}

// The path with every symlink in its existing part resolved and `..` taken lexically in the rest.
fn resolved(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                normal.pop();
            }
            std::path::Component::CurDir => {}
            other => normal.push(other),
        }
    }
    let mut tail = Vec::new();
    let mut head = normal.as_path();
    loop {
        if let Ok(real) = head.canonicalize() {
            return tail.iter().rev().fold(real, |path: PathBuf, part| path.join(part));
        }
        match (head.parent(), head.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                head = parent;
            }
            _ => return normal,
        }
    }
}

pub fn file_hook(task: Option<&str>, worktree: Option<&str>, tmp: Option<&str>, input: &str) -> String {
    let input: Value = serde_json::from_str(input).unwrap_or_default();
    let target = ["file_path", "notebook_path", "path"].iter().find_map(|key| input["tool_input"][key].as_str()).filter(|path| path.starts_with('/'));
    let claude_home = crate::agent::claude::home().unwrap_or_default();
    let reason = match (target, task, worktree, tmp) {
        (Some(target), Some(task), Some(worktree), Some(tmp)) => {
            let mut dirs = TaskDirs::new(Path::new(worktree), Path::new(tmp));
            dirs.halves = crate::lead::state_dir().map(|state| halves(&state, task, &dirs.worktree)).unwrap_or_default();
            file_refusal(Path::new(target), &dirs, &claude_home)
        }
        _ => Some("kebbie could not tell which file this tool writes, so a task's file tool does not run; give it an absolute path".to_string()),
    };
    reason.map(|reason| json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason } }).to_string()).unwrap_or_default()
}

// kebbie's state holds which worktrees run sandboxed tasks, where neither a task's shell nor its file tools reach.
fn mark_file(state: &Path, worktree: &Path) -> PathBuf {
    let real = worktree.canonicalize().unwrap_or_else(|_| worktree.to_path_buf());
    state.join("sandboxed").join(real.to_string_lossy().replace('%', "%25").replace('/', "%2F"))
}

pub fn mark(state: &Path, worktree: &Path, sandboxed: bool) -> Result<()> {
    let file = mark_file(state, worktree);
    if !sandboxed {
        return std::fs::remove_file(&file).or_else(|error| if error.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(error) }).map_err(|error| format!("{}: {error}", file.display()));
    }
    crate::state::write(&file, "")
}

pub fn marked(state: &Path, worktree: &Path) -> bool {
    mark_file(state, worktree).is_file()
}

pub fn marked_worktrees(state: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(state.join("sandboxed")).into_iter().flatten().flatten()
        .filter_map(|entry| entry.file_name().to_str().map(|name| PathBuf::from(name.replace("%2F", "/").replace("%25", "%"))))
        .collect()
}

// A slot made after a sandboxed task started joins the git dirs its shell and file tools may not write, in the settings file Claude Code reads again while it runs.
pub fn deny_new_slot(roles: &Path, repo: &str, git_dir: &Path) -> Result<()> {
    let suffix = format!("@{repo}.settings.json");
    let files = std::fs::read_dir(roles).into_iter().flatten().flatten().map(|entry| entry.path())
        .filter(|file| file.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(&suffix)));
    for file in files {
        let Ok(mut written) = std::fs::read_to_string(&file).map_err(|_| ()).and_then(|text| serde_json::from_str::<Value>(&text).map_err(|_| ())) else { continue };
        if written.get("sandbox").is_none() {
            continue;
        }
        let path = git_dir.display().to_string();
        let rule = format!("Edit(/{path}/**)");
        for (keys, entry) in [(["sandbox", "filesystem", "denyWrite"].as_slice(), &path), (["permissions", "deny"].as_slice(), &rule)] {
            let list = keys.iter().fold(&mut written, |value, key| &mut value[*key]);
            if let Some(entries) = list.as_array_mut().filter(|entries| !entries.contains(&json!(entry))) {
                entries.push(json!(entry));
            }
        }
        crate::state::write(&file, written.to_string())?;
    }
    Ok(())
}

fn halves_file(state: &Path, task: &str) -> PathBuf {
    state.join("halves").join(task.replace('/', "%"))
}

// Every worktree a task has across repos, so each half's file tools take the others' worktrees as the task's own.
pub fn record_half(state: &Path, task: &str, dirs: &TaskDirs) -> Result<()> {
    let file = halves_file(state, task);
    let line = format!("{}\t{}", dirs.worktree.display(), dirs.tmp.display());
    let kept: Vec<String> = std::fs::read_to_string(&file).unwrap_or_default().lines().filter(|known| *known != line).map(str::to_string).chain([line.clone()]).collect();
    crate::state::write(&file, &(kept.join("\n") + "\n"))
}

// A recorded half counts while its worktree is still on the task's branch.
pub fn halves(state: &Path, task: &str, own: &Path) -> Vec<TaskDirs> {
    let on_task = |dirs: &TaskDirs| std::fs::read_to_string(dirs.git_dir().join("HEAD")).is_ok_and(|head| head.trim() == format!("ref: refs/heads/{task}"));
    std::fs::read_to_string(halves_file(state, task)).unwrap_or_default().lines()
        .filter_map(|line| line.split_once('\t')).map(|(worktree, tmp)| TaskDirs::new(Path::new(worktree), Path::new(tmp)))
        .filter(|dirs| dirs.worktree != own && on_task(dirs)).collect()
}

// An entry that would run kebbie, herdr or a terminal multiplexer unsandboxed lets a task pass as the user.
pub fn excluded_refusal(entry: &str) -> Option<String> {
    let program = entry.split_whitespace().next().unwrap_or_default();
    let name = program.rsplit('/').next().unwrap_or(program);
    if program.is_empty() || program.contains('*') || HOSTS.contains(&name) {
        return Some(format!("\"{entry}\" would run {} outside the sandbox, where a task can pass as you; name the command that needs it", if program.is_empty() || program.contains('*') { "any command" } else { name }));
    }
    None
}

pub fn write_refusal(entry: &str, home: &Path) -> Option<String> {
    let path = match entry.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ if entry.starts_with('/') => PathBuf::from(entry),
        _ => return Some(format!("\"{entry}\" is not a path from / or ~/")),
    };
    let path = PathBuf::from(path.to_string_lossy().trim_end_matches(['/', '*']));
    let path = if path.as_os_str().is_empty() { PathBuf::from("/") } else { path };
    let guarded = [".local/state/kebbie", ".local/share/kebbie", ".config/kebbie", ".config/herdr", ".claude", ".codex", ".zshrc", ".bashrc", ".profile"].map(|part| home.join(part));
    guarded.iter().find(|guarded| guarded.starts_with(&path) || path.starts_with(guarded))
        .map(|guarded| format!("\"{entry}\" would let a task's shell write {}", guarded.display()))
}

pub fn missing(os: &str, arch: &str, on_path: impl Fn(&str) -> bool, seccomp: impl Fn(&str) -> bool) -> Vec<String> {
    match os {
        "macos" => Vec::new(),
        "linux" => {
            let mut lacking: Vec<String> = ["bwrap", "socat"].into_iter().filter(|tool| !on_path(tool)).map(str::to_string).collect();
            let filter = match arch {
                "x86_64" => Some("x64"),
                "aarch64" => Some("arm64"),
                _ => None,
            };
            match filter {
                Some(filter) if seccomp(filter) => {}
                Some(_) => lacking.push(SECCOMP.to_string()),
                None => lacking.push(format!("a seccomp filter for {arch}, which only x86_64 and aarch64 have")),
            }
            lacking
        }
        other => vec![format!("a sandbox on {other}: Claude Code sandboxes on macOS and Linux")],
    }
}

const SECCOMP: &str = "the seccomp filter";

pub fn missing_here() -> Vec<String> {
    // Tests never start a real sandbox, so the machine running them counts as able to; the refusal is tested through `missing`.
    if cfg!(test) {
        return Vec::new();
    }
    let path: Vec<PathBuf> = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default();
    let mut lacking = missing(std::env::consts::OS, std::env::consts::ARCH, |tool| crate::doctor::which(tool, &path).is_some(), |arch| seccomp_roots().iter().any(|root| root.join("vendor/seccomp").join(arch).join("apply-seccomp").is_file()));
    let claude = crate::doctor::which("claude", &path).and_then(|claude| crate::sh::run(&claude.to_string_lossy(), &["--version"], None).ok());
    lacking.extend(claude.as_deref().and_then(claude_too_old));
    lacking
}

// Claude Code holds allowUnsandboxedCommands = false from --settings, with no retry outside the sandbox, from this version on.
pub const MIN_CLAUDE: [u64; 3] = [2, 1, 285];

pub fn claude_too_old(version: &str) -> Option<String> {
    let have = crate::doctor::version(version)?;
    let pad = |parts: &[u64]| (0..3).map(|index| parts.get(index).copied().unwrap_or(0)).collect::<Vec<_>>();
    (pad(&have) < MIN_CLAUDE.to_vec()).then(|| format!("Claude Code {} or later (this is {})", MIN_CLAUDE.map(|part| part.to_string()).join("."), have.iter().map(u64::to_string).collect::<Vec<_>>().join(".")))
}

// Where Claude Code looks for @anthropic-ai/sandbox-runtime's seccomp helper: npm's global root, then the usual global folders.
fn seccomp_roots() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let npm = crate::sh::run("npm", &["root", "-g"], None).ok().map(|root| PathBuf::from(root.trim()));
    npm.into_iter().chain([PathBuf::from("/usr/lib/node_modules"), PathBuf::from("/usr/local/lib/node_modules"), PathBuf::from("/opt/homebrew/lib/node_modules"), home.join(".npm/lib/node_modules"), home.join(".npm-global/lib/node_modules")])
        .map(|root| root.join("@anthropic-ai/sandbox-runtime")).collect()
}

fn install_hint(item: &str) -> &'static str {
    match item {
        "bwrap" => "bubblewrap (apt-get install bubblewrap, dnf install bubblewrap)",
        "socat" => "socat (apt-get install socat, dnf install socat)",
        item if item.starts_with("Claude Code ") => "a newer Claude Code (claude update)",
        SECCOMP => "the seccomp filter that blocks Unix sockets (npm install -g @anthropic-ai/sandbox-runtime)",
        _ => "",
    }
}

pub fn fixes(missing: &[String]) -> Vec<String> {
    missing.iter().map(|item| match install_hint(item) {
        "" => item.clone(),
        hint => format!("install {hint}"),
    }).collect()
}

pub fn refusal(missing: &[String]) -> Result<()> {
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "task.sandbox is on, but this machine cannot sandbox a task's shell commands fully: {}. Install what is missing, or set task.sandbox = false in {} to run tasks unsandboxed, where a task's Claude can pass as you to kebbie's user-only commands; kebbie doctor shows the sandbox's state",
        fixes(missing).join("; "), crate::config::path().display()
    ))
}

// A task-side command that failed inside the sandbox was spelled so Claude Code did not take it out: with cd, a pipe, $(…) or a redirect.
pub fn ran_sandboxed(command: &str, error: &str, confined: bool) -> String {
    if !confined || !TASK_COMMANDS.contains(&command) {
        return error.to_string();
    }
    format!("{error}\nkebbie {command} ran inside the sandbox, where it cannot reach kebbie's state: run kebbie {command} on its own, without cd, pipes, redirects or $(…), so it runs outside it")
}

pub const OFF: &str = "off (task.sandbox = false): a task's shell commands run unsandboxed and can pass as you to kebbie's user-only commands";

// A process inside a sandbox cannot create a file in kebbie's real state dir or the folder it would be made in, and on macOS the kernel says so outright.
pub fn confined() -> bool {
    // Test builds probe the sealed state dir, as they read the sealed ps; a release reads neither env nor PATH.
    if cfg!(debug_assertions) {
        return crate::lead::state_location().map_or(true, |dir| writes_nearest(&dir) == Some(false));
    }
    kernel_says_sandboxed() || real_state_dir().is_none_or(|dir| writes_nearest(&dir) == Some(false))
}

// The probe in the state dir, or where it would be made: its nearest existing folder in the home.
pub fn writes_nearest(dir: &Path) -> Option<bool> {
    dir.ancestors().find(|folder| folder.is_dir()).and_then(writes_in)
}

// None when the folder does not exist: the probe never creates it.
pub fn writes_in(dir: &Path) -> Option<bool> {
    if !dir.is_dir() {
        return None;
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
    let probe = dir.join(format!(".capability-{}-{nanos}", std::process::id()));
    Some(match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    })
}

fn real_state_dir() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into static storage or null; its pw_dir is copied out before any other call.
    let home = unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() || (*entry).pw_dir.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*entry).pw_dir).to_string_lossy().to_string()
    };
    Some(PathBuf::from(home).join(".local/state/kebbie"))
}

#[cfg(target_os = "macos")]
fn kernel_says_sandboxed() -> bool {
    // SAFETY: looks sandbox_check up in the loaded libraries; null when this macOS has none.
    let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"sandbox_check".as_ptr()) };
    if symbol.is_null() {
        return false;
    }
    // SAFETY: sandbox_check(pid_t, const char *operation, int type, ...) from libsystem_sandbox; a null operation with no extra arguments asks whether the process is sandboxed at all.
    let check: unsafe extern "C" fn(libc::pid_t, *const libc::c_char, libc::c_int) -> libc::c_int = unsafe { std::mem::transmute(symbol) };
    unsafe { check(libc::getpid(), std::ptr::null(), 0) != 0 }
}

#[cfg(not(target_os = "macos"))]
fn kernel_says_sandboxed() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_without_bubblewrap_socat_or_the_seccomp_filter_cannot_sandbox_and_kebbie_names_what_to_install() {
        let found = |present: &'static [&'static str]| move |tool: &str| present.contains(&tool);
        assert!(missing("macos", "aarch64", found(&[]), |_| false).is_empty(), "macOS sandboxes with what it ships");
        assert!(missing("linux", "x86_64", found(&["bwrap", "socat"]), |arch| arch == "x64").is_empty());
        assert_eq!(missing("linux", "aarch64", found(&["bwrap", "socat"]), |arch| arch == "x64"), vec![SECCOMP.to_string()], "the filter for this CPU");
        assert_eq!(missing("linux", "x86_64", found(&["socat"]), |_| true), vec!["bwrap".to_string()]);
        assert!(missing("linux", "riscv64", found(&["bwrap", "socat"]), |_| true)[0].contains("only x86_64 and aarch64"));
        assert!(missing("freebsd", "x86_64", found(&[]), |_| true)[0].contains("macOS and Linux"));
        assert_eq!(refusal(&[]), Ok(()));
        let refused = refusal(&missing("linux", "x86_64", found(&[]), |_| false)).unwrap_err();
        for said in ["task.sandbox is on", "apt-get install bubblewrap", "apt-get install socat", "npm install -g @anthropic-ai/sandbox-runtime", "set task.sandbox = false"] {
            assert!(refused.contains(said), "{said}: {refused}");
        }
    }

    #[test]
    fn a_tasks_halves_are_the_worktrees_it_recorded_that_are_still_on_its_branch() {
        let base = crate::sh::test_dir("halves");
        let state = base.join("state");
        let half = |name: &str, head: &str| {
            let dirs = TaskDirs::new(&base.join(format!("wt.{name}")), &base.join(name).join(".git/worktrees/1/kebbie-tmp"));
            std::fs::create_dir_all(&dirs.tmp).unwrap();
            std::fs::write(dirs.git_dir().join("HEAD"), format!("ref: refs/heads/{head}\n")).unwrap();
            dirs
        };
        let (app, api, reused) = (half("app", "fix/billing"), half("api", "fix/billing"), half("web", "fix/billing"));
        for dirs in [&app, &api, &reused, &app] {
            record_half(&state, "fix/billing", dirs).unwrap();
        }
        std::fs::write(reused.git_dir().join("HEAD"), "ref: refs/heads/other\n").unwrap();
        let found: Vec<PathBuf> = halves(&state, "fix/billing", &app.worktree).into_iter().map(|dirs| dirs.worktree).collect();
        assert_eq!(found, std::slice::from_ref(&api.worktree), "not itself, and not a slot another task took over");
        assert!(halves(&state, "billing", &app.worktree).is_empty());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_mark_lives_in_kebbies_state_and_is_there_only_while_the_task_is_sandboxed() {
        let base = crate::sh::test_dir("mark");
        let (state, worktree) = (base.join("state"), base.join("wt1.app"));
        std::fs::create_dir_all(&worktree).unwrap();
        mark(&state, &worktree, true).unwrap();
        assert!(marked(&state, &worktree));
        assert!(mark_file(&state, &worktree).starts_with(&state), "out of the worktree's git dir, which a sibling's shell can write");
        assert_eq!(marked_worktrees(&state), [worktree.canonicalize().unwrap()]);
        mark(&state, &worktree, false).unwrap();
        assert!(!marked(&state, &worktree));
        mark(&state, &worktree, false).unwrap();
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_new_slot_joins_what_every_running_sandboxed_task_in_its_repo_may_not_write() {
        let roles = crate::sh::test_dir("new-slot");
        let sandboxed = json!({ "permissions": { "deny": ["Bash(git push:*)"] }, "sandbox": { "filesystem": { "denyWrite": ["/x"] } } });
        let open = json!({ "permissions": { "deny": ["Bash(git push:*)"] } });
        for (name, settings) in [("billing@app", &sandboxed), ("search@app", &open), ("billing@api", &sandboxed)] {
            std::fs::write(roles.join(format!("{name}.settings.json")), settings.to_string()).unwrap();
        }
        let slot = Path::new("/src/app/.git/worktrees/wt4.app");
        deny_new_slot(&roles, "app", slot).unwrap();
        deny_new_slot(&roles, "app", slot).unwrap();
        let read = |name: &str| serde_json::from_str::<Value>(&std::fs::read_to_string(roles.join(format!("{name}.settings.json"))).unwrap()).unwrap();
        let billing = read("billing@app");
        assert_eq!(billing["sandbox"]["filesystem"]["denyWrite"], json!(["/x", "/src/app/.git/worktrees/wt4.app"]), "once, however often it runs");
        assert_eq!(billing["permissions"]["deny"], json!(["Bash(git push:*)", "Edit(//src/app/.git/worktrees/wt4.app/**)"]));
        assert_eq!(read("search@app"), open, "a task without the sandbox keeps 0.25.0's settings");
        assert_eq!(read("billing@api"), sandboxed, "another repo's task is not touched");
        std::fs::remove_dir_all(roles).unwrap();
    }

    #[test]
    fn the_probe_reads_the_nearest_folder_that_exists_and_creates_none() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::sh::test_dir("nearest");
        let missing = dir.join(".local/state/kebbie");
        assert_eq!(writes_nearest(&missing), Some(true));
        assert!(!dir.join(".local").exists(), "nothing made on the way");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        assert_eq!(writes_nearest(&missing), Some(false), "a sandbox that keeps the home closed reads as confined, state dir or not");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_task_command_that_failed_in_the_sandbox_says_how_to_run_it_outside() {
        let said = ran_sandboxed("report", "Operation not permitted", true);
        assert!(said.starts_with("Operation not permitted\n") && said.contains("run kebbie report on its own, without cd, pipes, redirects or $(…)"), "{said}");
        assert_eq!(ran_sandboxed("routine ok", "x", true).lines().count(), 2);
        assert_eq!(ran_sandboxed("report", "x", false), "x", "outside the sandbox the error is the error");
        assert_eq!(ran_sandboxed("push", "x", true), "x", "a user-only command is refused on its own terms");
    }

    #[test]
    fn a_claude_code_older_than_the_strict_sandbox_cannot_run_a_sandboxed_task() {
        assert_eq!(claude_too_old("2.1.308 (Claude Code)"), None);
        assert_eq!(claude_too_old("2.1.285 (Claude Code)"), None);
        let old = claude_too_old("2.1.284 (Claude Code)").unwrap();
        assert!(old.contains("Claude Code 2.1.285 or later (this is 2.1.284)"), "{old}");
        assert!(refusal(&[old]).unwrap_err().contains("install a newer Claude Code (claude update)"));
        assert_eq!(claude_too_old("not a version"), None);
    }

    #[test]
    fn a_profile_cannot_take_kebbie_herdr_or_a_multiplexer_out_of_the_sandbox() {
        for refused in ["kebbie push x", "/opt/kebbie/target/release/kebbie *", "herdr *", "tmux *", "/usr/bin/screen -dm x", "zellij *", "*", "* push", ""] {
            assert!(excluded_refusal(refused).is_some(), "{refused}");
        }
        for kept in OPS_COMMANDS.iter().chain(&["docker compose *", "git fetch *"]) {
            assert_eq!(excluded_refusal(kept), None, "{kept}");
        }
    }

    #[test]
    fn a_profile_cannot_open_kebbies_herdrs_or_claudes_own_files_to_the_shell() {
        let home = Path::new("/home/ada");
        for refused in ["~", "~/", "/", "~/.config", "~/.config/kebbie", "~/.local", "~/.local/state/kebbie/roles", "/home/ada/.claude", "~/.codex", "~/.codex/plugins", "/home/ada/.codex/plugins/cache/kebbie/kebbie/local/bin", "~/.zshrc", "~/.local/share/*", "build", "./cache"] {
            assert!(write_refusal(refused, home).is_some(), "{refused}");
        }
        for kept in WRITES.iter().chain(&["~/.gradle", "/tmp/build", "~/.local/share/pnpm"]) {
            assert_eq!(write_refusal(kept, home), None, "{kept}");
        }
    }

    #[test]
    fn a_probe_in_a_folder_the_process_cannot_write_reads_as_confined_and_leaves_nothing_behind() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::sh::test_dir("sandbox-probe");
        assert_eq!(writes_in(&dir), Some(true));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the probe file is gone");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        assert_eq!(writes_in(&dir), Some(false), "a folder the process cannot write is no proof of the user");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(writes_in(&dir.join("deeper")), None, "a missing folder says nothing");
        assert!(!dir.join("deeper").exists(), "and the probe does not create it");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_tasks_file_tools_write_in_its_worktree_and_temp_folder_and_nowhere_that_runs_as_the_user() {
        let base = crate::sh::test_dir("file-refusal").canonicalize().unwrap();
        let (worktree, tmp, claude, home) = (base.join("wt1.app"), base.join("app/.git/worktrees/1/kebbie-tmp"), base.join("home/.claude"), base.join("home"));
        for dir in [&worktree, &tmp, &claude.join("projects"), &worktree.join("src")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let mut dirs = TaskDirs::new(&worktree, &tmp);
        let refused = |path: &Path| file_refusal(path, &dirs, &claude);
        for kept in [worktree.join("src/main.rs"), worktree.join("new/deep/file.rs"), tmp.join("plan.md"), crate::agent::claude::transcripts_dir(&claude, &worktree).join("memory/MEMORY.md"), crate::agent::claude::transcripts_dir(&claude, &base.join("app")).join("memory/MEMORY.md"), worktree.join(".claude/CLAUDE.md"), worktree.join("CLAUDE.md")] {
            assert_eq!(refused(&kept), None, "{}", kept.display());
        }
        for loaded in [".claude/settings.json", ".claude/settings.local.json", ".claude/hooks/x.sh", ".claude/skills/a/SKILL.md", ".claude/agents/a.md", ".claude/commands/a.md", ".mcp.json", ".git"] {
            assert!(refused(&worktree.join(loaded)).is_some_and(|said| said.contains("Ask the user")), "{loaded}");
        }
        for outside in [claude.join("projects/-Users-ada-projects-other/memory/MEMORY.md"), home.join(".zshrc"), home.join(".claude/settings.json"), home.join(".claude.json"), base.join("app/.git/config"), base.join("app/.git/hooks/pre-commit"), base.join("app/.claude/settings.local.json"), base.join("other-repo/build.rs"), worktree.join("../app/.mcp.json"), worktree.join("src/../../home/.gitconfig")] {
            assert!(refused(&outside).is_some_and(|said| said.contains("outside this task's worktree")), "{}", outside.display());
        }
        std::os::unix::fs::symlink(&home, worktree.join("src/home")).unwrap();
        assert!(refused(&worktree.join("src/home/.zshrc")).is_some(), "a folder link out of the worktree leads outside it");
        std::fs::create_dir_all(worktree.join(".claude")).unwrap();
        std::os::unix::fs::symlink(worktree.join(".claude"), worktree.join("alias")).unwrap();
        assert!(refused(&worktree.join("alias/settings.json")).is_some(), "a link to the worktree's .claude is its .claude");
        let (other, other_tmp) = (base.join("wt2.api"), base.join("api/.git/worktrees/2/kebbie-tmp"));
        assert!(file_refusal(&other.join("src/lib.rs"), &dirs, &claude).is_some(), "another repo's worktree is not this task's without a half there");
        dirs.halves.push(TaskDirs::new(&other, &other_tmp));
        assert_eq!(file_refusal(&other.join("src/lib.rs"), &dirs, &claude), None, "the task's half in another repo is its own");
        assert_eq!(file_refusal(&other_tmp.join("plan.md"), &dirs, &claude), None);
        assert!(file_refusal(&other.join(".claude/settings.json"), &dirs, &claude).is_some_and(|said| said.contains("Ask the user")), "a half's own configuration stays out of reach");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_file_hook_denies_in_claude_codes_hook_answer_and_denies_what_it_cannot_place() {
        let input = |path: &str| json!({ "tool_name": "Write", "tool_input": { "file_path": path, "content": "x" } }).to_string();
        let hook = |input: &str| file_hook(Some("billing"), Some("/nowhere/wt"), Some("/nowhere/tmp"), input);
        let denied: Value = serde_json::from_str(&hook(&input("/etc/zshrc"))).unwrap();
        assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(denied["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(hook(&input("/nowhere/wt/src/a.rs")), "");
        for unplaced in [input("relative/a.rs"), "{}".to_string(), "not json".to_string()] {
            assert!(hook(&unplaced).contains("\"deny\""), "{unplaced}");
        }
        assert!(file_hook(None, None, None, &input("/nowhere/wt/a.rs")).contains("\"deny\""));
        let notebook = json!({ "tool_name": "NotebookEdit", "tool_input": { "notebook_path": "/etc/x.ipynb" } }).to_string();
        assert!(hook(&notebook).contains("\"deny\""));
    }

    #[test]
    fn a_tasks_shell_and_file_tools_never_write_another_pooled_slots_git_dir() {
        let base = crate::sh::test_dir("siblings");
        for slot in ["1", "2", "3"] {
            std::fs::create_dir_all(base.join("app/.git/worktrees").join(slot)).unwrap();
        }
        let dirs = TaskDirs::new(&base.join("wt1.app"), &base.join("app/.git/worktrees/1/kebbie-tmp"));
        let others = [base.join("app/.git/worktrees/2"), base.join("app/.git/worktrees/3")];
        assert_eq!(sibling_git_dirs(&dirs), others);
        let denied: Vec<String> = block("/k", &Policy::default(), Some(&dirs), false)["filesystem"]["denyWrite"].as_array().unwrap().iter().filter_map(|path| path.as_str().map(str::to_string)).collect();
        for other in &others {
            assert!(denied.contains(&other.display().to_string()), "{} in {denied:?}", other.display());
        }
        assert!(!denied.contains(&base.join("app/.git/worktrees/1").display().to_string()), "its own git dir holds its temp folder");
        let rules = Protected { path: Vec::new(), install: None, ..Protected::here() }.deny_rules("/k", Some(&dirs));
        assert!(rules.contains(&format!("Edit(/{}/**)", others[0].display())), "{rules:?}");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_linked_worktrees_git_dirs_are_read_from_its_temp_folder() {
        let dirs = TaskDirs::new(Path::new("/src/wt1.app"), Path::new("/src/app/.git/worktrees/1/kebbie-tmp"));
        assert_eq!((dirs.git_dir(), dirs.common_dir(), dirs.main_checkout()), (PathBuf::from("/src/app/.git/worktrees/1"), PathBuf::from("/src/app/.git"), Some(PathBuf::from("/src/app"))));
        let main = TaskDirs::new(Path::new("/src/app"), Path::new("/src/app/.git/kebbie-tmp"));
        assert_eq!((main.common_dir(), main.main_checkout()), (PathBuf::from("/src/app/.git"), None));
    }

    #[test]
    fn the_test_binary_reads_as_unconfined_through_its_sealed_state_dir() {
        assert!(!confined());
    }
}
