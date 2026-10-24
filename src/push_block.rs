use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::repo::{git, main_checkout};
use crate::sh::Result;

pub const TOKEN_VAR: &str = "KEBBIE_PUSH_TOKEN";
const BLOCKED: &str = "kebbie-blocks-push:";
const REFUSAL: &str = "kebbie: a task's worktree does not push. The user pushes from the board, or the lead with kebbie push for a task the user gave push permission. The block stops an accidental push, not a determined one.";
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
    crate::sh::quote(text)
}

// One push's proof that kebbie runs it: the hook finds the file only while that push runs, and the value is never written where a task reads it.
pub struct Token {
    file: PathBuf,
    pub value: String,
}

impl Token {
    pub fn new(state: &Path) -> Result<Token> {
        let mut bytes = [0u8; 16];
        fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes)).map_err(|error| error.to_string())?;
        let value: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let file = tokens_dir(state).join(&value);
        crate::state::write(&file, "")?;
        Ok(Token { file, value })
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
    }
}

// Every hook the repo has runs as before; pre-push first asks for the token of a push kebbie runs.
pub fn hook_script(original: &Path, state: &Path, main: &Path) -> String {
    format!(
        "#!/bin/sh\nhook=$(basename \"$0\")\nrefuse() {{\n  echo {refusal} >&2\n  printf {for_a_pr} {main} \"$(git branch --show-current 2>/dev/null)\" >&2\n  exit 1\n}}\nif [ \"$hook\" = pre-push ]; then\n  token=\"${{{TOKEN_VAR}:-}}\"\n  case \"$token\" in\n    ''|*[!0123456789abcdef]*) refuse ;;\n  esac\n  if [ ${{#token}} -ne 32 ] || [ ! -f {tokens}/\"$token\" ]; then\n    refuse\n  fi\nfi\nif [ -x {original}/\"$hook\" ]; then\n  exec {original}/\"$hook\" \"$@\"\nfi\nexit 0\n",
        refusal = quoted(REFUSAL), for_a_pr = quoted(FOR_A_PR), main = quoted(&main.to_string_lossy()),
        tokens = quoted(&tokens_dir(state).to_string_lossy()), original = quoted(&original.to_string_lossy()),
    )
}

// The hooks the worktree ran before kebbie's: a hooksPath set anywhere but in its own worktree config, else the repo's hooks folder.
pub fn original_hooks(worktree: &Path) -> Result<PathBuf> {
    let scoped = git(worktree, &["config", "--show-scope", "--get-all", "core.hooksPath"]).unwrap_or_default();
    let set = scoped.lines().filter_map(|line| line.split_once('\t')).filter(|(scope, _)| !["worktree", "command"].contains(scope)).map(|(_, value)| value.trim()).next_back();
    Ok(match set {
        Some(path) if path.starts_with("~/") => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(&path[2..]),
        Some(path) => worktree.join(path),
        None => PathBuf::from(git(worktree, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?).join("hooks"),
    })
}

pub fn hooks_in_worktree(worktree: &Path) -> Result<Option<PathBuf>> {
    let original = original_hooks(worktree)?;
    let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Ok(real(&original).starts_with(real(worktree)).then_some(original))
}

fn hooks_dir(worktree: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(worktree, &["rev-parse", "--absolute-git-dir"])?).join("kebbie-hooks"))
}

#[derive(Debug, PartialEq)]
pub enum Block {
    On,
    Off(String),
}

// git's docs ask core.worktree and core.bare out of the shared config before extensions.worktreeConfig is turned on; kebbie leaves such a repo unblocked rather than move them.
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

// The block lives in the worktree's own config, which the task's env cannot unset: a pre-push hook kebbie answers, and a push URL rewrite for git push --no-verify.
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
        eprintln!("kebbie: {why}");
    }
}

// For kebbie's own push from a worktree: origin's URLs rewritten to where the main checkout pushes, a longer match than the block's rewrite.
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

    // The task's own commit, in its own shell's git, which runs the repo's hooks.
    fn commit(dir: &Path, file: &str) {
        fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", file]).unwrap();
        assert!(run(dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", file], &[]).status.success());
    }

    struct Fixture {
        base: PathBuf,
        main: PathBuf,
        origin: PathBuf,
        slot: PathBuf,
        state: PathBuf,
    }

    fn fixture(name: &str) -> Fixture {
        let base = std::env::temp_dir().join(format!("kebbie-push-block-{name}-{}", std::process::id()));
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
    fn a_tasks_own_push_fails_in_every_form_while_kebbies_push_and_the_main_checkouts_go_through() {
        let fixture = fixture("forms");
        let Fixture { main, origin, slot, state, .. } = &fixture;
        crate::executable::write(main.join(".git/hooks/pre-commit"), format!("#!/bin/sh\ntouch {}\n", quoted(&fixture.base.join("pre-commit-ran").to_string_lossy()))).unwrap();
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
        assert!(String::from_utf8_lossy(&past_the_rewrite.stderr).contains("kebbie push"), "{}", String::from_utf8_lossy(&past_the_rewrite.stderr));
        let forged = run(slot, &["push", "origin", "HEAD:main"], &[(TOKEN_VAR, "0123456789abcdef0123456789abcdef")]);
        assert!(!forged.status.success(), "a token kebbie did not issue opens nothing");
        assert_eq!(git(origin, &["rev-parse", "main"]).unwrap(), before);

        let token = Token::new(state).unwrap();
        let mut args = unblock_args(slot).unwrap();
        args.extend(["push", "--quiet", "origin", "HEAD:main"].map(str::to_string));
        let kebbies = run(slot, &args.iter().map(String::as_str).collect::<Vec<_>>(), &[(TOKEN_VAR, &token.value)]);
        assert!(kebbies.status.success(), "{}", String::from_utf8_lossy(&kebbies.stderr));
        assert_eq!(git(origin, &["rev-parse", "main"]).unwrap(), git(slot, &["rev-parse", "HEAD"]).unwrap());
        let file = tokens_dir(state).join(&token.value);
        drop(token);
        assert!(!file.exists(), "the token is gone once kebbie's push is done");

        git(main, &["merge", "--quiet", "--ff-only", "billing"]).unwrap();
        commit(main, "c");
        let users = run(main, &["push", "--quiet", "origin", "main"], &[]);
        assert!(users.status.success(), "the user's push from the main checkout is untouched: {}", String::from_utf8_lossy(&users.stderr));
        assert!(unblock_args(main).unwrap().is_empty());
        fs::remove_dir_all(&fixture.base).unwrap();
    }

    fn pre_push(dir: &Path, ran: &Path) {
        fs::create_dir_all(dir).unwrap();
        crate::executable::write(dir.join("pre-push"), format!("#!/bin/sh\ntouch {}\n", quoted(&ran.to_string_lossy()))).unwrap();
    }

    #[test]
    fn the_users_push_runs_the_repos_pre_push_hook_unless_a_sandboxed_task_holds_it() {
        let fixture = fixture("user-push");
        let Fixture { main, slot, state, .. } = &fixture;
        let ran = fixture.base.join("pre-push-ran");
        pre_push(&main.join(".git/hooks"), &ran);
        install(slot, state).unwrap();
        crate::sandbox::mark(&crate::lead::state_location().unwrap(), slot, true).unwrap();
        commit(slot, "b");
        crate::repo::push_from(slot, &["HEAD:refs/heads/billing"], state).unwrap();
        assert!(ran.exists(), "the main checkout's pre-push hook runs on the push the user chose, sandboxed task or not");

        let fixture = fixture_with_husky("user-push-husky");
        let Fixture { slot, state, origin, .. } = &fixture;
        let ran = fixture.base.join("pre-push-ran");
        pre_push(&slot.join(".husky/_"), &ran);
        commit(slot, "b");
        let refused = crate::repo::push_from(slot, &["HEAD:refs/heads/billing"], state).unwrap_err();
        let main = fixture.main.canonicalize().unwrap_or(fixture.main.clone());
        assert!(refused.contains("the repo's pre-push hook lives in the task's worktree") && refused.contains("the task may have written it") && refused.contains("kebbie did not run it"), "{refused}");
        assert!(refused.contains(&format!("git -C {} push origin billing", crate::sh::quote(&main.to_string_lossy()))) || refused.contains(&format!("git -C {} push origin billing", crate::sh::quote(&fixture.main.to_string_lossy()))), "the push it names is one the block lets through, from the main checkout: {refused}");
        assert!(!ran.exists(), "a hook the task could have written does not run as the user");
        assert!(git(origin, &["rev-parse", "--verify", "--quiet", "refs/heads/billing"]).is_err(), "and nothing was pushed without it");

        crate::sandbox::mark(&crate::lead::state_location().unwrap(), slot, false).unwrap();
        crate::repo::push_from(slot, &["HEAD:refs/heads/billing"], state).unwrap();
        assert!(ran.exists(), "without the sandbox the worktree's pre-push hook runs, as before");
    }

    #[test]
    fn a_push_in_a_repo_with_sandboxed_tasks_runs_the_repos_hooks_not_what_a_slot_holds() {
        let fixture = fixture("sibling");
        let Fixture { base, main, slot, state, .. } = &fixture;
        let (ran, tampered) = (base.join("pre-push-ran"), base.join("tampered"));
        pre_push(&main.join(".git/hooks"), &ran);
        install(slot, state).unwrap();
        let other = base.join("wt2.app");
        git(main, &["worktree", "add", "--quiet", "-b", "api", &other.to_string_lossy()]).unwrap();
        install(&other, state).unwrap();
        crate::sandbox::mark(&crate::lead::state_location().unwrap(), &other, true).unwrap();
        pre_push(&hooks_dir(slot).unwrap(), &tampered);
        commit(slot, "b");
        crate::repo::push_from(slot, &["HEAD:refs/heads/billing"], state).unwrap();
        assert!(ran.exists(), "the repo's own pre-push hook runs");
        assert!(!tampered.exists(), "a slot's kebbie-hooks, which another sandboxed task could reach, never run on the user's push");
        fs::remove_dir_all(base).unwrap();
    }

    fn fixture_with_husky(name: &str) -> Fixture {
        let fixture = fixture(name);
        git(&fixture.main, &["config", "core.hooksPath", ".husky/_"]).unwrap();
        install(&fixture.slot, &fixture.state).unwrap();
        crate::sandbox::mark(&crate::lead::state_location().unwrap(), &fixture.slot, true).unwrap();
        fixture
    }

    #[test]
    fn a_hook_the_repo_gets_after_install_runs_in_the_tasks_worktree() {
        let fixture = fixture("later-hook");
        let Fixture { main, slot, state, .. } = &fixture;
        git(main, &["config", "core.hooksPath", ".husky/_"]).unwrap();
        assert_eq!(install(slot, state).unwrap(), Block::On);
        fs::create_dir_all(slot.join(".husky/_")).unwrap();
        let ran = fixture.base.join("commit-msg-ran");
        crate::executable::write(slot.join(".husky/_/commit-msg"), format!("#!/bin/sh\ntouch {}\n", quoted(&ran.to_string_lossy()))).unwrap();
        commit(slot, "b");
        assert!(ran.exists(), "a hook husky writes during npm install, after kebbie's install, still runs");
        fs::remove_file(&ran).unwrap();
        crate::sandbox::mark(&crate::lead::state_location().unwrap(), slot, true).unwrap();
        fs::write(slot.join("c"), "c").unwrap();
        git(slot, &["add", "c"]).unwrap();
        git(slot, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", "c"]).unwrap();
        assert!(!ran.exists(), "kebbie's own git never runs a hook a sandboxed task's worktree holds");
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
    fn the_repos_own_pre_push_hook_still_runs_for_kebbies_push_and_can_refuse_it() {
        let fixture = fixture("own-hook");
        let Fixture { main, slot, state, .. } = &fixture;
        fs::create_dir_all(main.join(".hooks")).unwrap();
        crate::executable::write(main.join(".hooks/pre-push"), "#!/bin/sh\necho repo hook refuses >&2\nexit 1\n").unwrap();
        git(main, &["config", "core.hooksPath", ".hooks"]).unwrap();
        fs::create_dir_all(slot.join(".hooks")).unwrap();
        crate::executable::write(slot.join(".hooks/pre-push"), fs::read(main.join(".hooks/pre-push")).unwrap()).unwrap();
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

    #[test]
    fn the_hook_script_quotes_every_path_so_a_space_or_an_apostrophe_in_one_breaks_nothing() {
        assert_eq!(quoted("plain"), "'plain'");
        assert_eq!(quoted("it's here"), "'it'\\''s here'");
        assert_eq!(quoted("$(rm -rf /) `x` \"y\""), "'$(rm -rf /) `x` \"y\"'");
        let base = std::env::temp_dir().join(format!("kebbie-hook-quoting-{}", std::process::id())).join("it's a dir");
        let _ = fs::remove_dir_all(base.parent().unwrap());
        let (original, state, main) = (base.join("hooks"), base.join("state"), base.join("app"));
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&main).unwrap();
        let script = hook_script(&original, &state, &main);
        let file = base.join("pre-push");
        fs::write(&file, &script).unwrap();
        let syntax = std::process::Command::new("sh").arg("-n").arg(&file).output().unwrap();
        assert!(syntax.status.success(), "{}", String::from_utf8_lossy(&syntax.stderr));
        let refused = std::process::Command::new("sh").arg(&file).current_dir(&main).env_remove(TOKEN_VAR).output().unwrap();
        assert!(!refused.status.success());
        let said = String::from_utf8_lossy(&refused.stderr);
        assert!(said.contains(REFUSAL) && said.contains(&format!("git -C {} push origin", main.display())), "{said}");
        crate::executable::write(original.join("post-commit"), "#!/bin/sh\necho ran \"$@\"\n").unwrap();
        let other = base.join("post-commit");
        fs::write(&other, &script).unwrap();
        let chained = std::process::Command::new("sh").arg(&other).arg("one").arg("two words").output().unwrap();
        assert!(chained.status.success());
        assert_eq!(String::from_utf8_lossy(&chained.stdout).trim(), "ran one two words", "the repo's own hook gets its arguments whole");
        let none = std::process::Command::new("sh").arg(base.join("pre-rebase")).output();
        fs::write(base.join("pre-rebase"), &script).unwrap();
        drop(none);
        assert!(std::process::Command::new("sh").arg(base.join("pre-rebase")).output().unwrap().status.success(), "no hook of that name in the repo is a pass");
        fs::remove_dir_all(base.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_pre_push_hook_takes_only_a_32_hex_token_that_kebbie_issued() {
        let base = std::env::temp_dir().join(format!("kebbie-hook-token-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (original, state, main) = (base.join("hooks"), base.join("state"), base.join("app"));
        fs::create_dir_all(&main).unwrap();
        let file = base.join("pre-push");
        fs::write(&file, hook_script(&original, &state, &main)).unwrap();
        let push = |token: &str| std::process::Command::new("sh").arg(&file).current_dir(&main).env(TOKEN_VAR, token).output().unwrap().status.success();
        let token = Token::new(&state).unwrap();
        assert_eq!(token.value.len(), 32);
        assert!(token.value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        assert!(push(&token.value));
        assert!(!push(&token.value[..31]));
        assert!(!push(&format!("{}/", &token.value[..31])), "a slash never reaches the tokens folder");
        assert!(!push("../away"));
        assert!(!push(""));
        assert!(!push("0000000000000000000000000000000000000000000000000000000000000000"));
        let other = Token::new(&state).unwrap();
        assert_ne!(other.value, token.value);
        drop(token);
        assert!(!push(&other.value[..0]) && push(&other.value));
        let upper = other.value.to_uppercase();
        if upper != other.value {
            let refused = std::process::Command::new("sh").arg(&file).current_dir(&main).env(TOKEN_VAR, &upper).env("LC_ALL", "en_US.UTF-8").output().unwrap();
            assert!(!refused.status.success(), "an upper-case spelling never passes, in any locale or on a case-insensitive disk");
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_repo_with_an_apostrophe_and_a_space_in_its_path_is_blocked_like_any_other() {
        let fixture = fixture("it's spaced");
        let Fixture { origin, slot, state, .. } = &fixture;
        assert_eq!(install(slot, state).unwrap(), Block::On);
        commit(slot, "b");
        let before = git(origin, &["rev-parse", "main"]).unwrap();
        let url = git(slot, &["config", "remote.origin.url"]).unwrap();
        let refused = run(slot, &["-c", &format!("url.{url}.pushInsteadOf={url}"), "push", "origin", "HEAD:main"], &[]);
        assert!(!refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("kebbie push"), "{}", String::from_utf8_lossy(&refused.stderr));
        assert_eq!(git(origin, &["rev-parse", "main"]).unwrap(), before);
        let token = Token::new(state).unwrap();
        let mut args = unblock_args(slot).unwrap();
        args.extend(["push", "--quiet", "origin", "HEAD:main"].map(str::to_string));
        let kebbies = run(slot, &args.iter().map(String::as_str).collect::<Vec<_>>(), &[(TOKEN_VAR, &token.value)]);
        assert!(kebbies.status.success(), "{}", String::from_utf8_lossy(&kebbies.stderr));
        fs::remove_dir_all(&fixture.base).unwrap();
    }

    #[test]
    fn the_pushurl_rewrite_catches_every_url_a_task_could_push_to() {
        let fixture = fixture("rewrite");
        let Fixture { slot, state, .. } = &fixture;
        install(slot, state).unwrap();
        commit(slot, "b");
        for remote in ["https://github.com/gronare/kebbie.git", "git@github.com:gronare/kebbie.git", "ssh://git@github.com/gronare/kebbie.git", fixture.origin.to_str().unwrap()] {
            let output = run(slot, &["push", "--no-verify", remote, "HEAD:refs/heads/elsewhere"], &[]);
            assert!(!output.status.success(), "a push to {remote} by url got through");
            let said = String::from_utf8_lossy(&output.stderr);
            assert!(said.contains("kebbie-blocks-push") || said.contains("kebbie"), "{said}");
        }
        assert!(!run(slot, &["push", "--no-verify", "--force", "origin", "HEAD:main"], &[]).status.success());
        assert!(!run(slot, &["push", "--no-verify", "--mirror", "origin"], &[]).status.success());
        fs::remove_dir_all(&fixture.base).unwrap();
    }
}
