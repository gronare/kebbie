use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::repo::{git, main_checkout};
use crate::sh::Result;

pub const TOKEN_VAR: &str = "KELPIE_PUSH_TOKEN";
const BLOCKED: &str = "kelpie-blocks-push:";
const REFUSAL: &str = "kelpie: a task's worktree does not push. The user pushes from the board, or the lead with kelpie push for a task the user gave push permission. The block stops an accidental push, not a determined one.";
const FOR_A_PR: &str = "To push this branch for a pull request, run in your own terminal: git -C %s push origin %s\\n";

// Every hook git runs (githooks(5)), so a hook the repo gets after install, as husky, lefthook or pre-commit write them, runs too; not push-to-checkout, whose mere presence changes how git takes a push into the worktree.
const HOOKS: [&str; 27] = [
    "applypatch-msg", "pre-applypatch", "post-applypatch", "pre-commit", "pre-merge-commit", "prepare-commit-msg", "commit-msg", "post-commit",
    "pre-rebase", "post-checkout", "post-merge", "pre-push", "pre-receive", "update", "proc-receive", "post-receive", "post-update",
    "reference-transaction", "pre-auto-gc", "post-rewrite", "sendemail-validate", "fsmonitor-watchman",
    "p4-changelist", "p4-prepare-changelist", "p4-post-changelist", "p4-pre-submit", "post-index-change",
];

fn tokens_dir(state: &Path) -> PathBuf {
    state.join("push-tokens")
}

fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

// One push's proof that kelpie runs it: the hook finds the file only while that push runs, and the value is never written where a task reads it.
pub struct Token {
    file: PathBuf,
    pub value: String,
}

impl Token {
    pub fn new(state: &Path) -> Result<Token> {
        let mut bytes = [0u8; 16];
        fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes)).map_err(|error| error.to_string())?;
        let value: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let dir = tokens_dir(state);
        fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        let file = dir.join(&value);
        fs::write(&file, "").map_err(|error| format!("{}: {error}", file.display()))?;
        Ok(Token { file, value })
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
    }
}

// Every hook the repo has runs as before; pre-push first asks for the token of a push kelpie runs.
pub fn hook_script(original: &Path, state: &Path, main: &Path) -> String {
    format!(
        "#!/bin/sh\nhook=$(basename \"$0\")\nrefuse() {{\n  echo {refusal} >&2\n  printf {for_a_pr} {main} \"$(git branch --show-current 2>/dev/null)\" >&2\n  exit 1\n}}\nif [ \"$hook\" = pre-push ]; then\n  token=\"${{{TOKEN_VAR}:-}}\"\n  case \"$token\" in\n    ''|*[!0-9a-f]*) refuse ;;\n  esac\n  if [ ${{#token}} -ne 32 ] || [ ! -f {tokens}/\"$token\" ]; then\n    refuse\n  fi\nfi\nif [ -x {original}/\"$hook\" ]; then\n  exec {original}/\"$hook\" \"$@\"\nfi\nexit 0\n",
        refusal = quoted(REFUSAL), for_a_pr = quoted(FOR_A_PR), main = quoted(&main.to_string_lossy()),
        tokens = quoted(&tokens_dir(state).to_string_lossy()), original = quoted(&original.to_string_lossy()),
    )
}

// The hooks the worktree ran before kelpie's: a hooksPath set anywhere but in its own worktree config, else the repo's hooks folder.
fn original_hooks(worktree: &Path) -> Result<PathBuf> {
    let scoped = git(worktree, &["config", "--show-scope", "--get-all", "core.hooksPath"]).unwrap_or_default();
    let set = scoped.lines().filter_map(|line| line.split_once('\t')).filter(|(scope, _)| *scope != "worktree").map(|(_, value)| value.trim()).next_back();
    Ok(match set {
        Some(path) if path.starts_with("~/") => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(&path[2..]),
        Some(path) => worktree.join(path),
        None => PathBuf::from(git(worktree, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?).join("hooks"),
    })
}

fn hooks_dir(worktree: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(worktree, &["rev-parse", "--absolute-git-dir"])?).join("kelpie-hooks"))
}

#[derive(Debug, PartialEq)]
pub enum Block {
    On,
    Off(String),
}

// git's docs ask core.worktree and core.bare out of the shared config before extensions.worktreeConfig is turned on; kelpie leaves such a repo unblocked rather than move them.
pub fn unsupported(worktree: &Path) -> Result<Option<String>> {
    let common = PathBuf::from(git(worktree, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?).join("config");
    let common = common.to_string_lossy();
    let bare = || git(worktree, &["config", "--file", &common, "--type=bool", "--get", "core.bare"]).ok().as_deref() == Some("true");
    let shared = git(worktree, &["config", "--file", &common, "--get", "core.worktree"]).ok().map(|_| "core.worktree").or_else(|| bare().then_some("core.bare = true"));
    Ok(shared.map(|key| format!(
        "the push block is off in {}: the repo's shared config {common} sets {key}, which git's per-worktree config needs moved into the main worktree's config.worktree first (git help worktree, CONFIGURATION FILE); a task there can git push",
        worktree.display()
    )))
}

// The block lives in the worktree's own config, which the task's env cannot unset: a pre-push hook kelpie answers, and a push URL rewrite for git push --no-verify.
pub fn install(worktree: &Path, state: &Path) -> Result<Block> {
    if let Some(why) = unsupported(worktree)? {
        return Ok(Block::Off(why));
    }
    let original = original_hooks(worktree)?;
    let dir = hooks_dir(worktree)?;
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let script = hook_script(&original, state, &main_checkout(worktree)?);
    for name in HOOKS {
        let file = dir.join(name);
        fs::write(&file, &script).and_then(|_| fs::set_permissions(&file, fs::Permissions::from_mode(0o755))).map_err(|error| format!("{}: {error}", file.display()))?;
    }
    git(worktree, &["config", "extensions.worktreeConfig", "true"])?;
    git(worktree, &["config", "--worktree", "core.hooksPath", &dir.to_string_lossy()])?;
    git(worktree, &["config", "--worktree", &format!("url.{BLOCKED}.pushInsteadOf"), ""])?;
    Ok(Block::On)
}

pub fn warn_if_off(block: Block) {
    if let Block::Off(why) = block {
        eprintln!("kelpie: {why}");
    }
}

// For kelpie's own push from a worktree: origin's URLs rewritten to where the main checkout pushes, a longer match than the block's rewrite.
pub fn unblock_args(worktree: &Path) -> Result<Vec<String>> {
    let main = main_checkout(worktree)?;
    let linked = git(worktree, &["rev-parse", "--absolute-git-dir"])? != git(worktree, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    if !linked || git(worktree, &["config", "--get-all", "remote.origin.pushurl"]).is_ok() {
        return Ok(Vec::new());
    }
    let url = git(worktree, &["config", "--get", "remote.origin.url"])?;
    let target = git(&main, &["remote", "get-url", "--push", "origin"])?;
    Ok(vec!["-c".to_string(), format!("url.{target}.pushInsteadOf={url}")])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        std::process::Command::new("git").args(args).current_dir(dir).env_remove(TOKEN_VAR).envs(env.iter().copied()).output().unwrap()
    }

    fn commit(dir: &Path, file: &str) {
        fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", file]).unwrap();
        git(dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", file]).unwrap();
    }

    struct Fixture {
        base: PathBuf,
        main: PathBuf,
        origin: PathBuf,
        slot: PathBuf,
        state: PathBuf,
    }

    fn fixture(name: &str) -> Fixture {
        let base = std::env::temp_dir().join(format!("kelpie-push-block-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (main, origin, slot, state) = (base.join("app"), base.join("origin.git"), base.join("wt1.app"), base.join("state"));
        fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "--quiet", "--initial-branch=main"]).unwrap();
        commit(&main, "a");
        git(&base, &["init", "--quiet", "--bare", "--initial-branch=main", &origin.to_string_lossy()]).unwrap();
        git(&main, &["remote", "add", "origin", &origin.to_string_lossy()]).unwrap();
        git(&main, &["push", "--quiet", "origin", "main"]).unwrap();
        git(&main, &["worktree", "add", "--quiet", "-b", "billing", &slot.to_string_lossy()]).unwrap();
        Fixture { base, main, origin, slot, state }
    }

    #[test]
    fn a_tasks_own_push_fails_in_every_form_while_kelpies_push_and_the_main_checkouts_go_through() {
        let fixture = fixture("forms");
        let Fixture { main, origin, slot, state, .. } = &fixture;
        fs::write(main.join(".git/hooks/pre-commit"), format!("#!/bin/sh\ntouch {}\n", quoted(&fixture.base.join("pre-commit-ran").to_string_lossy()))).unwrap();
        fs::set_permissions(main.join(".git/hooks/pre-commit"), fs::Permissions::from_mode(0o755)).unwrap();
        install(slot, state).unwrap();
        commit(slot, "b");
        assert!(fixture.base.join("pre-commit-ran").exists(), "the repo's own hooks still run in the task's worktree");
        let before = git(origin, &["rev-parse", "main"]).unwrap();
        for args in [
            &["push", "origin", "HEAD:main"][..],
            &["-C", ".", "push", "origin", "HEAD:main"],
            &["push", "--no-verify", "origin", "HEAD:main"],
            &["-c", "core.hooksPath=/dev/null", "push", "origin", "HEAD:main"],
        ] {
            let output = run(slot, args, &[]);
            assert!(!output.status.success(), "git {args:?} pushed from the task's worktree");
        }
        let unset_env = run(slot, &["push", "origin", "HEAD:main"], &[("GIT_CONFIG_COUNT", "0")]);
        assert!(!unset_env.status.success(), "the block holds with the task's env rewrite removed");
        let url = git(slot, &["config", "remote.origin.url"]).unwrap();
        let past_the_rewrite = run(slot, &["-c", &format!("url.{url}.pushInsteadOf={url}"), "push", "origin", "HEAD:main"], &[]);
        assert!(!past_the_rewrite.status.success(), "a task that undoes the rewrite still meets the hook");
        assert!(String::from_utf8_lossy(&past_the_rewrite.stderr).contains("kelpie push"), "{}", String::from_utf8_lossy(&past_the_rewrite.stderr));
        let forged = run(slot, &["push", "origin", "HEAD:main"], &[(TOKEN_VAR, "0123456789abcdef0123456789abcdef")]);
        assert!(!forged.status.success(), "a token kelpie did not issue opens nothing");
        assert_eq!(git(origin, &["rev-parse", "main"]).unwrap(), before);

        let token = Token::new(state).unwrap();
        let mut args = unblock_args(slot).unwrap();
        args.extend(["push", "--quiet", "origin", "HEAD:main"].map(str::to_string));
        let kelpies = run(slot, &args.iter().map(String::as_str).collect::<Vec<_>>(), &[(TOKEN_VAR, &token.value)]);
        assert!(kelpies.status.success(), "{}", String::from_utf8_lossy(&kelpies.stderr));
        assert_eq!(git(origin, &["rev-parse", "main"]).unwrap(), git(slot, &["rev-parse", "HEAD"]).unwrap());
        let file = tokens_dir(state).join(&token.value);
        drop(token);
        assert!(!file.exists(), "the token is gone once kelpie's push is done");

        git(main, &["merge", "--quiet", "--ff-only", "billing"]).unwrap();
        commit(main, "c");
        let users = run(main, &["push", "--quiet", "origin", "main"], &[]);
        assert!(users.status.success(), "the user's push from the main checkout is untouched: {}", String::from_utf8_lossy(&users.stderr));
        assert!(unblock_args(main).unwrap().is_empty());
        fs::remove_dir_all(&fixture.base).unwrap();
    }

    #[test]
    fn a_hook_the_repo_gets_after_install_runs_in_the_tasks_worktree() {
        let fixture = fixture("later-hook");
        let Fixture { main, slot, state, .. } = &fixture;
        git(main, &["config", "core.hooksPath", ".husky/_"]).unwrap();
        assert_eq!(install(slot, state).unwrap(), Block::On);
        fs::create_dir_all(slot.join(".husky/_")).unwrap();
        let ran = fixture.base.join("commit-msg-ran");
        fs::write(slot.join(".husky/_/commit-msg"), format!("#!/bin/sh\ntouch {}\n", quoted(&ran.to_string_lossy()))).unwrap();
        fs::set_permissions(slot.join(".husky/_/commit-msg"), fs::Permissions::from_mode(0o755)).unwrap();
        commit(slot, "b");
        assert!(ran.exists(), "a hook husky writes during npm install, after kelpie's install, still runs");
        let url = git(slot, &["config", "remote.origin.url"]).unwrap();
        let refused = run(slot, &["-c", &format!("url.{url}.pushInsteadOf={url}"), "push", "origin", "HEAD:main"], &[]);
        let said = String::from_utf8_lossy(&refused.stderr);
        assert!(!refused.status.success());
        assert!(said.contains("not a determined one"), "the refusal says what the block is for: {said}");
        assert!(said.contains(&format!("git -C {} push origin billing", main.canonicalize().unwrap().display())), "and how the user pushes the branch for a pull request: {said}");
        fs::remove_dir_all(&fixture.base).unwrap();
    }

    #[test]
    fn a_repo_whose_shared_config_sets_core_worktree_or_core_bare_is_left_unblocked_and_unchanged() {
        let fixture = fixture("shared-core");
        let Fixture { main, origin, slot, state, base } = &fixture;
        git(main, &["config", "core.worktree", &main.to_string_lossy()]).unwrap();
        let off = install(slot, state).unwrap();
        assert!(matches!(&off, Block::Off(why) if why.contains("core.worktree") && why.contains("config.worktree")), "{off:?}");
        assert!(git(slot, &["config", "--get", "extensions.worktreeConfig"]).is_err(), "nothing is written to a repo the block would break");
        assert!(git(slot, &["config", "--get", "core.hooksPath"]).is_err());
        let bare_slot = base.join("wt1.bare");
        git(origin, &["worktree", "add", "--quiet", "-b", "search", &bare_slot.to_string_lossy()]).unwrap();
        assert!(unsupported(&bare_slot).unwrap().unwrap().contains("core.bare = true"));
        git(main, &["config", "--unset", "core.worktree"]).unwrap();
        assert_eq!(unsupported(slot).unwrap(), None);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_repos_own_pre_push_hook_still_runs_for_kelpies_push_and_can_refuse_it() {
        let fixture = fixture("own-hook");
        let Fixture { main, slot, state, .. } = &fixture;
        fs::create_dir_all(main.join(".hooks")).unwrap();
        fs::write(main.join(".hooks/pre-push"), "#!/bin/sh\necho repo hook refuses >&2\nexit 1\n").unwrap();
        fs::set_permissions(main.join(".hooks/pre-push"), fs::Permissions::from_mode(0o755)).unwrap();
        git(main, &["config", "core.hooksPath", ".hooks"]).unwrap();
        fs::create_dir_all(slot.join(".hooks")).unwrap();
        fs::copy(main.join(".hooks/pre-push"), slot.join(".hooks/pre-push")).unwrap();
        install(slot, state).unwrap();
        assert!(fs::read_to_string(hooks_dir(slot).unwrap().join("pre-push")).unwrap().contains(&slot.join(".hooks").to_string_lossy().to_string()), "a relative hooksPath is the task's own worktree's");
        let token = Token::new(state).unwrap();
        let mut args = unblock_args(slot).unwrap();
        args.extend(["push", "origin", "HEAD:main"].map(str::to_string));
        let output = run(slot, &args.iter().map(String::as_str).collect::<Vec<_>>(), &[(TOKEN_VAR, &token.value)]);
        assert!(!output.status.success() && String::from_utf8_lossy(&output.stderr).contains("repo hook refuses"), "{}", String::from_utf8_lossy(&output.stderr));
        install(slot, state).unwrap();
        assert_eq!(git(slot, &["config", "--worktree", "--get-all", "core.hooksPath"]).unwrap().lines().count(), 1, "installing again replaces the block");
        fs::remove_dir_all(&fixture.base).unwrap();
    }
}
