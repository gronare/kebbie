use std::path::{Path, PathBuf};

use crate::sh::{Result, run, run_visibly, succeeds};

pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
}

pub fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    run("git", args, Some(cwd))
}

const INDEX_RETRIES: [std::time::Duration; 3] = [std::time::Duration::from_millis(250), std::time::Duration::from_millis(500), std::time::Duration::from_secs(1)];

// A git that takes index.lock fails at once while another git holds it; it is tried a few times, then the lock file is named.
pub fn git_writing(cwd: &Path, args: &[&str]) -> Result<String> {
    let lock = || git(cwd, &["rev-parse", "--path-format=absolute", "--git-path", "index.lock"]).unwrap_or_else(|_| "index.lock".to_string());
    index_retrying(|| git(cwd, args), std::thread::sleep, lock)
}

fn index_retrying(mut attempt: impl FnMut() -> Result<String>, mut pause: impl FnMut(std::time::Duration), lock: impl Fn() -> String) -> Result<String> {
    for wait in INDEX_RETRIES {
        match attempt() {
            Err(error) if error.contains("index.lock") => pause(wait),
            other => return other,
        }
    }
    attempt().map_err(|error| if error.contains("index.lock") { format!("{error}\n{} stays: another git still runs in this checkout, or one that crashed left it; remove it once no git runs there", lock()) } else { error })
}

pub fn main_checkout(cwd: &Path) -> Result<PathBuf> {
    let common_dir = git(cwd, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    Ok(Path::new(&common_dir).parent().ok_or("git common dir has no parent")?.to_path_buf())
}

// origin/HEAD is unset in some older clones.
pub fn default_branch(repo: &Path) -> Result<String> {
    if let Ok(branch) = git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        return Ok(branch);
    }
    for candidate in ["origin/main", "origin/master"] {
        if succeeds("git", &["show-ref", "--verify", "--quiet", &format!("refs/remotes/{candidate}")], Some(repo)) {
            return Ok(candidate.to_string());
        }
    }
    Err(format!("Cannot tell {}'s default branch; run: git remote set-head origin -a", repo.display()))
}

pub fn worktrees(repo: &Path) -> Result<Vec<Worktree>> {
    let mut list = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    for line in git(repo, &["worktree", "list", "--porcelain"])?.lines().chain(std::iter::once("")) {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
            branch = None;
        } else if let Some(value) = line.strip_prefix("branch ") {
            branch = Some(value.trim_start_matches("refs/heads/").to_string());
        } else if line.is_empty()
            && let Some(path) = path.take()
        {
            list.push(Worktree { path, branch: branch.take() });
        }
    }
    Ok(list)
}

pub fn changed_count(path: &Path) -> Result<usize> {
    Ok(git(path, &["status", "--porcelain"])?.lines().count())
}

pub fn is_clean(path: &Path) -> Result<bool> {
    Ok(changed_count(path)? == 0)
}

// Tracked files with changes not committed yet; untracked files are not counted.
pub fn uncommitted(path: &Path) -> Result<Vec<String>> {
    Ok(git(path, &["diff", "--name-only", "HEAD"])?.lines().map(str::to_string).collect())
}

// Files git status never shows: skip-worktree (S) or assume-unchanged (a lowercase tag).
pub fn hidden_from_status(path: &Path) -> Result<Vec<String>> {
    Ok(git(path, &["ls-files", "-v"])?.lines().filter_map(|line| {
        let (tag, file) = line.split_once(' ')?;
        (tag == "S" || tag.chars().all(|character| character.is_ascii_lowercase())).then(|| file.to_string())
    }).collect())
}

pub fn rebase_in_progress(path: &Path) -> bool {
    ["rebase-merge", "rebase-apply"].iter().any(|name| git(path, &["rev-parse", "--path-format=absolute", "--git-path", name]).is_ok_and(|dir| Path::new(&dir).exists()))
}

pub fn ahead_behind(path: &Path, upstream: &str) -> Result<(usize, usize)> {
    let counts = git(path, &["rev-list", "--left-right", "--count", &format!("HEAD...{upstream}")])?;
    let mut numbers = counts.split_whitespace().map(|count| count.parse::<usize>().unwrap_or(0));
    Ok((numbers.next().unwrap_or(0), numbers.next().unwrap_or(0)))
}

pub fn push_as_user(path: &Path, refspecs: &[&str]) -> Result<()> {
    push_from(path, refspecs, &crate::lead::state_dir()?)
}

// Pushes to origin as the user from the board, where nobody may see a prompt: git and ssh fail instead of waiting for a password or passphrase.
pub fn push_from(path: &Path, refspecs: &[&str], state: &Path) -> Result<()> {
    let token = crate::push_block::Token::new(state)?;
    let mut command = std::process::Command::new("git");
    command.args(crate::push_block::unblock_args(path)?).args(["push", "--quiet", "origin"]).args(refspecs).current_dir(path).stdin(std::process::Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0").env(crate::push_block::TOKEN_VAR, &token.value);
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let output = command.output().map_err(|error| format!("git push: {error}"))?;
    if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).trim().to_string()) }
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    succeeds("git", &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")], Some(repo))
}

pub fn name_of(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default()
}

pub fn slot_path(repo: &Path, number: usize) -> PathBuf {
    repo.with_file_name(format!("wt{number}.{}", name_of(repo)))
}

pub fn setup_note(worktree: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(worktree, &["rev-parse", "--absolute-git-dir"])?).join("kelpie-setup.md"))
}

pub fn setup_line(worktree: &Path) -> Option<String> {
    let note = std::fs::read_to_string(setup_note(worktree).ok()?).ok()?;
    let line = note.lines().next()?.trim_start_matches('#').trim();
    (!line.is_empty()).then(|| line.to_string())
}

// set-up-worktree records the worktree's port as PORT in its .env, else as `port <n>` on the setup note's dev server line.
pub fn worktree_port(worktree: &Path) -> Option<u16> {
    let env = std::fs::read_to_string(worktree.join(".env")).ok();
    env.as_deref().and_then(port_in_env)
        .or_else(|| port_in_note(&std::fs::read_to_string(setup_note(worktree).ok()?).ok()?))
}

pub fn port_in_env(text: &str) -> Option<u16> {
    text.lines().rev().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);
        let value = line.strip_prefix("PORT")?.trim_start().strip_prefix('=')?;
        let value = value.split('#').next()?.trim().trim_matches(|character| character == '"' || character == '\'');
        value.parse().ok().filter(|port| *port > 0)
    })
}

pub fn port_in_note(text: &str) -> Option<u16> {
    text.lines().find_map(|line| {
        let lower = line.to_lowercase();
        lower.match_indices("port").find_map(|(at, word)| {
            let rest = lower[at + word.len()..].trim_start_matches([' ', ':', '=', '\t', '`']);
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok().filter(|port| *port > 0)
        })
    })
}

pub fn slot_number(repo: &Path, path: &Path) -> Option<usize> {
    let name = name_of(path);
    let digits = name.strip_suffix(&format!(".{}", name_of(repo)))?.strip_prefix("wt")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn roots() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = std::env::var("KELPIE_REPO_DIRS").unwrap_or_else(|_| format!("{home}/projects:{home}/personal:{home}/src:{home}/code"));
    dirs.split(':').filter(|dir| !dir.is_empty()).map(PathBuf::from).filter(|dir| dir.is_dir()).collect()
}

// Repos sit in a root or one level down (~/projects/<org>/<repo>).
fn checkouts_under(root: &Path) -> Vec<PathBuf> {
    let children = |dir: &Path| -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
        paths.sort();
        paths
    };
    let mut found = Vec::new();
    for child in children(root) {
        if child.join(".git").is_dir() {
            found.push(child);
        } else {
            found.extend(children(&child).into_iter().filter(|grandchild| grandchild.join(".git").is_dir()));
        }
    }
    found
}

fn origin(repo: &Path) -> Option<String> {
    git(repo, &["remote", "get-url", "origin"]).ok()
}

// Clones from the first owner among your checkouts that has the repo, next to that owner's other repos.
fn clone(name: &str, roots: &[PathBuf]) -> Result<PathBuf> {
    let mut tried = Vec::new();
    for sibling in roots.iter().flat_map(|root| checkouts_under(root)) {
        let Some(url) = origin(&sibling) else { continue };
        let Some(base) = url.strip_suffix(".git").unwrap_or(&url).rsplit_once('/').map(|(base, _)| base.to_string()) else { continue };
        let parent = sibling.parent().ok_or("checkout has no parent")?.to_path_buf();
        if tried.contains(&(base.clone(), parent.clone())) {
            continue;
        }
        tried.push((base.clone(), parent.clone()));
        let candidate = format!("{base}/{name}.git");
        if succeeds("git", &["ls-remote", "--quiet", &candidate, "HEAD"], None) {
            let path = parent.join(name);
            eprintln!("Cloning {candidate} into {}…", path.display());
            run_visibly("git", &["clone", &candidate, &path.to_string_lossy()], None)?;
            return Ok(path);
        }
    }
    Err(format!("No checkout of {name} under {} and no remote of your other repos has it", roots.iter().map(|root| root.display().to_string()).collect::<Vec<_>>().join(", ")))
}

// Main checkouts with a kelpie pool (a wt<N>.<repo> next to them), so a task shows even when no Space of its repo is open.
pub fn pooled_checkouts() -> Vec<PathBuf> {
    pooled_under(&roots())
}

fn pooled_under(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots.iter().flat_map(|root| checkouts_under(root)).filter(|repo| {
        let Some(parent) = repo.parent() else { return false };
        std::fs::read_dir(parent).into_iter().flatten().flatten().any(|entry| slot_number(repo, &entry.path()).is_some())
    }).collect()
}

pub fn checkouts() -> Vec<PathBuf> {
    roots().iter().flat_map(|root| checkouts_under(root)).collect()
}

pub fn checkout_named(name: &str) -> Option<PathBuf> {
    roots().iter().flat_map(|root| checkouts_under(root)).find(|path| name_of(path) == name)
}

// A repo is named by its folder, so the clone lands next to your other checkouts and never outside them.
fn folder_name(name: &str) -> Result<&str> {
    let plain = !name.is_empty() && !name.starts_with(['.', '-']) && !name.contains(['/', '\\']) && !name.chars().any(char::is_control);
    if plain { Ok(name) } else { Err(format!("--repo \"{name}\" is not a repo's folder name: name the checkout's folder, as in --repo api")) }
}

pub fn locate(name: &str) -> Result<PathBuf> {
    let name = folder_name(name)?;
    checkout_named(name).map_or_else(|| clone(name, &roots()), Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_git_that_meets_index_lock_is_tried_again_and_a_lock_that_stays_is_named() {
        let held = "fatal: Unable to create '/r/.git/index.lock': File exists.".to_string();
        let mut tries = 0;
        let mut waits = 0;
        assert_eq!(index_retrying(|| { tries += 1; if tries < 3 { Err(held.clone()) } else { Ok("done".to_string()) } }, |_| waits += 1, || "/r/.git/index.lock".to_string()), Ok("done".to_string()));
        assert_eq!((tries, waits), (3, 2));
        let stays = index_retrying(|| Err(held.clone()), |_| {}, || "/r/.git/index.lock".to_string()).unwrap_err();
        assert!(stays.starts_with(&held) && stays.contains("/r/.git/index.lock stays"), "{stays}");
        let mut tries = 0;
        assert_eq!(index_retrying(|| { tries += 1; Err("CONFLICT".to_string()) }, |_| {}, String::new).unwrap_err(), "CONFLICT");
        assert_eq!(tries, 1, "only the lock is retried");
        let dir = crate::sh::test_dir("index-lock");
        git(&dir, &["init", "--quiet"]).unwrap();
        std::fs::write(dir.join(".git/index.lock"), "").unwrap();
        let named = git_writing(&dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "--allow-empty", "-m", "one"]).unwrap_err();
        assert!(named.contains(&format!("{} stays", dir.canonicalize().unwrap().join(".git/index.lock").display())), "{named}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_repo_is_named_by_one_folder_name_and_never_a_path() {
        for good in ["api", "notes-vault-mcp", "app.web", "x_y"] {
            assert_eq!(folder_name(good), Ok(good));
        }
        for bad in ["", "..", "../x", "a/b", "/etc", ".hidden", "-x", "a\\b", "a\nb"] {
            assert!(folder_name(bad).is_err(), "{bad:?}");
            assert!(locate(bad).unwrap_err().contains("is not a repo's folder name"), "{bad:?}: refused before anything is cloned");
        }
    }

    #[test]
    fn the_port_is_read_from_env_lines_in_their_usual_shapes() {
        assert_eq!(port_in_env("RAILS_ENV=development\nPORT=3002\n"), Some(3002));
        assert_eq!(port_in_env("export PORT=\"4001\" # set-up-worktree\n"), Some(4001));
        assert_eq!(port_in_env("PORT = '5173'"), Some(5173));
        assert_eq!(port_in_env("PORT=3000\nPORT=3003\n"), Some(3003), "the last assignment wins, as in a dotenv loader");
        assert_eq!(port_in_env("DATABASE_PORT=5432\nPORTAL=1\n# PORT=3009\n"), None);
        assert_eq!(port_in_env("PORT=\n"), None);
    }

    #[test]
    fn the_port_is_read_from_the_setup_notes_dev_server_line() {
        let note = "# wt2.app set up 2026-10-18 by task search\n- branch search at abc1234\n- dev server `bin/dev` on port 3002\n- verified one test file\n";
        assert_eq!(port_in_note(note), Some(3002));
        assert_eq!(port_in_note("- dev server `bin/rails s --port=3004`"), Some(3004));
        assert_eq!(port_in_note("- dev server PORT=3005 npm run dev"), Some(3005));
        assert_eq!(port_in_note("- reported done\n- no port picked"), None);
    }

    #[test]
    fn a_worktrees_port_comes_from_its_env_else_its_setup_note_else_nowhere() {
        let dir = std::env::temp_dir().join(format!("kelpie-port-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("app");
        let tree = dir.join("wt2.app");
        std::fs::create_dir_all(&main).unwrap();
        for args in [&["init", "--quiet"][..], &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "--allow-empty", "-m", "one"]] {
            git(&main, args).unwrap();
        }
        git(&main, &["worktree", "add", "--quiet", "--detach", tree.to_str().unwrap()]).unwrap();
        assert_eq!(worktree_port(&tree), None);
        std::fs::write(setup_note(&tree).unwrap(), "# wt2.app set up\n- dev server `bin/dev` on port 3002\n").unwrap();
        assert_eq!(worktree_port(&tree), Some(3002));
        std::fs::write(tree.join(".env"), "PORT=3012\n").unwrap();
        assert_eq!(worktree_port(&tree), Some(3012));
        assert_eq!(worktree_port(&main), None, "the main checkout's note is not a worktree's");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_checkout_with_a_worktree_pool_next_to_it_is_found_without_a_space_open() {
        let root = std::env::temp_dir().join(format!("kelpie-pooled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["shop/.git", "wt2.shop", "blog/.git", "wtx.blog", "gronare/api/.git", "gronare/wt1.api", "notes"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        assert_eq!(pooled_under(std::slice::from_ref(&root)), [root.join("gronare/api"), root.join("shop")]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_slot_is_named_wt_number_dot_repo_and_read_back_only_in_that_shape() {
        let repo = Path::new("/src/app");
        assert_eq!(slot_path(repo, 3), PathBuf::from("/src/wt3.app"));
        assert_eq!(slot_number(repo, &slot_path(repo, 12)), Some(12));
        assert_eq!(slot_number(repo, Path::new("/src/wt01.app")), Some(1), "a zero-padded number reads as its number");
        for not_a_slot in ["/src/wt.app", "/src/wtx.app", "/src/wt1.app.bak", "/src/wt1.other", "/src/app", "/src/wt1", "/src/wt-1.app", "/src/wt1.App"] {
            assert_eq!(slot_number(repo, Path::new(not_a_slot)), None, "{not_a_slot}");
        }
        assert_eq!(slot_number(Path::new("/src/my.app"), Path::new("/src/wt2.my.app")), Some(2), "a dot in the repo's name is fine");
        assert_eq!(name_of(Path::new("/src/app/")), "app");
        assert_eq!(name_of(Path::new("/")), "");
    }

    #[test]
    fn the_worktree_list_is_read_from_gits_porcelain_output_with_branches_and_detached_heads() {
        let dir = std::env::temp_dir().join(format!("kelpie-worktrees-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("app");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "--quiet", "--initial-branch=main"]).unwrap();
        git(&main, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "--allow-empty", "-m", "one"]).unwrap();
        let detached = dir.join("wt1.app");
        let on_branch = dir.join("wt2.app");
        git(&main, &["worktree", "add", "--quiet", "--detach", detached.to_str().unwrap()]).unwrap();
        git(&main, &["worktree", "add", "--quiet", "-b", "billing", on_branch.to_str().unwrap()]).unwrap();
        let list = worktrees(&main).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        let find = |path: &Path| list.iter().find(|worktree| worktree.path.canonicalize().ok() == path.canonicalize().ok()).unwrap();
        assert_eq!(find(&detached).branch, None);
        assert_eq!(find(&on_branch).branch.as_deref(), Some("billing"));
        assert_eq!(main_checkout(&on_branch).unwrap().canonicalize().unwrap(), main.canonicalize().unwrap());
        assert_eq!(main_checkout(&main).unwrap().canonicalize().unwrap(), main.canonicalize().unwrap());
        assert!(main_checkout(&dir).is_err(), "outside a repo there is no main checkout");
        assert!(branch_exists(&main, "billing") && !branch_exists(&main, "search"));
        assert_eq!(ahead_behind(&on_branch, "main").unwrap(), (0, 0));
        assert!(default_branch(&main).is_err(), "no origin, no default branch, and the error says how to set one");
        let _ = std::fs::remove_dir_all(dir);
    }
}
