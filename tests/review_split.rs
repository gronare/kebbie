use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../src/executable.rs"]
mod executable;
#[path = "../src/test_seal.rs"]
mod test_seal;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/review.sh");

// A task worktree on `billing` with a commit of its own, and a herdr stand-in that logs its calls, opens pane p_review and knows the panes listed in alive.
struct Split {
    root: PathBuf,
    slot: PathBuf,
}

impl Split {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!("kebbie-split-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (app, origin, slot) = (root.join("app"), root.join("origin.git"), root.join("wt1.app"));
        fs::create_dir_all(&app).unwrap();
        git(&app, &["init", "--quiet", "--initial-branch=main"]);
        commit(&app, "a", "base\n", "first");
        git(&root, &["init", "--quiet", "--bare", "--initial-branch=main", origin.to_str().unwrap()]);
        git(&app, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&app, &["push", "--quiet", "origin", "main"]);
        git(&app, &["remote", "set-head", "origin", "main"]);
        git(&app, &["worktree", "add", "--quiet", "-b", "billing", slot.to_str().unwrap()]);
        commit(&slot, "b", "two\n", "Add b");
        let herdr = format!(concat!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{0}/herdr.log'\ncase \"$1 $2\" in\n",
            "  'pane split') echo '{{\"result\":{{\"pane\":{{\"pane_id\":\"p_review\"}}}}}}' ;;\n",
            "  'pane get') grep -qxF \"$3\" '{0}/alive' ;;\n",
            "esac\n"), root.display());
        executable::write(root.join("herdr"), herdr).unwrap();
        fs::write(root.join("alive"), "p_claude\n").unwrap();
        Split { root, slot }
    }

    fn run(&self, args: &[&str], stdin: &str) -> String {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new("sh").arg(SCRIPT).args(args).current_dir(&self.slot)
            .env("KEBBIE_STATE_DIR", self.root.join("state")).env("KEBBIE_BIN", "/k/kebbie").env("HERDR_BIN_PATH", self.root.join("herdr"))
            .env("HERDR_PANE_ID", "p_claude").env("HERDR_ENV", "1").env("CLAUDE_PLUGIN_OPTION_AUTO_OPEN", "true").env("GIT_CONFIG_NOSYSTEM", "1").env_remove("XDG_STATE_HOME")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "review.sh {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn dir(&self) -> PathBuf {
        self.root.join("state/reviews/p_claude")
    }

    fn herdr_calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("herdr.log")).unwrap_or_default().lines().map(str::to_string).collect()
    }

    fn forget_calls(&self) {
        let _ = fs::remove_file(self.root.join("herdr.log"));
    }

    // The view writes its pid to live while it runs; this test's own process stands in for it.
    fn view_running(&self) {
        fs::write(self.dir().join("live"), std::process::id().to_string()).unwrap();
    }
}

impl Drop for Split {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(args).current_dir(dir).env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, contents: &str, message: &str) -> String {
    fs::write(dir.join(file), contents).unwrap();
    git(dir, &["add", file]);
    git(dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn split_opens_kebbies_review_view_beside_claude_and_a_second_split_rescopes_the_open_one() {
    let split = Split::new("open");
    assert_eq!(split.run(&["split"], ""), "p_review");
    let dir = split.dir();
    assert_eq!(fs::read_to_string(dir.join("base")).unwrap(), "fork\n", "a task's review is based on its fork point");
    assert_eq!(fs::read_to_string(dir.join("agent")).unwrap(), "p_claude\n", "comments go to this Claude");
    assert_eq!(fs::read_to_string(dir.join("pane")).unwrap(), "p_review\n");
    let calls = split.herdr_calls();
    assert!(calls.iter().any(|call| call.starts_with("pane split --pane p_claude --direction right") && call.ends_with("--focus")), "{calls:?}");
    assert!(calls.contains(&format!("pane run p_review '/k/kebbie' review --view '{}'; exit", dir.display())), "the pane runs the view: {calls:?}");
    split.view_running();
    split.forget_calls();
    assert_eq!(split.run(&["split", "HEAD~1", "b", "./a"], ""), "p_review");
    assert!(split.herdr_calls().is_empty(), "an open review is rescoped, not opened twice");
    assert_eq!(fs::read_to_string(dir.join("scope")).unwrap(), "b\n./a\n");
    assert_eq!(fs::read_to_string(dir.join("base")).unwrap(), "HEAD~1\n");
    assert_eq!(fs::read_to_string(dir.join("label")).unwrap(), "against HEAD~1\n");
}

#[test]
fn a_review_whose_claude_pane_is_gone_is_pruned_when_the_next_one_opens() {
    let split = Split::new("prune");
    let gone = split.root.join("state/reviews/p_gone");
    fs::create_dir_all(&gone).unwrap();
    fs::write(gone.join("agent"), "p_gone\n").unwrap();
    split.run(&["split"], "");
    assert!(!gone.exists(), "herdr never reuses pane ids");
    assert!(split.dir().join("agent").exists());
}

#[test]
fn the_hook_opens_a_tasks_review_once_per_session_without_taking_the_focus() {
    let split = Split::new("hook");
    let input = format!(r#"{{"cwd":"{0}","session_id":"s1","tool_input":{{"file_path":"{0}/b"}}}}"#, split.slot.display());
    split.run(&["hook"], &input);
    let calls = split.herdr_calls();
    assert!(calls.iter().any(|call| call.starts_with("pane split") && call.ends_with("--no-focus")), "{calls:?}");
    assert_eq!(fs::read_to_string(split.dir().join("session")).unwrap(), "s1\n");
    split.forget_calls();
    split.run(&["hook"], &input);
    assert!(split.herdr_calls().is_empty(), "closed from herdr, it stays closed for the session");
    let elsewhere = format!(r#"{{"cwd":"{}","session_id":"s2"}}"#, split.root.join("app").display());
    split.run(&["hook"], &elsewhere);
    assert!(split.herdr_calls().is_empty(), "a main checkout never opens one on its own");
}

#[test]
fn the_script_lists_no_files_itself_and_fzf_is_gone() {
    let output = Command::new("sh").arg(SCRIPT).arg("rows").output().unwrap();
    assert_eq!(output.status.code(), Some(2), "kebbie review --view lists the files");
    assert!(!fs::read_to_string(SCRIPT).unwrap().contains("fzf"));
}

// Drives review.lua in a headless nvim: the diff loads from shown.json, a comment on a visual selection reaches Claude's pane, and tab hands the focus to the view.
#[test]
#[ignore = "needs nvim 0.10 or newer on PATH, which CI does not install: cargo test --test review_split -- --ignored"]
fn nvim_types_a_comment_on_a_selection_into_claudes_pane_and_hands_tab_to_the_view() {
    let split = Split::new("nvim");
    let sha = git(&split.slot, &["rev-parse", "HEAD"]);
    let dir = split.dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("agent"), "p_claude\n").unwrap();
    fs::write(dir.join("pane"), "p_view\n").unwrap();
    split.view_running();
    let shown = format!(r#"{{"path":"b","title":"commit 1","sha":"{sha}","short":"{}","index":1,"count":1,"diff":["git","diff","--no-color","{sha}^","{sha}","--","b"]}}"#, &sha[..7]);
    fs::write(dir.join("shown.json"), shown).unwrap();
    let drive = split.root.join("drive.lua");
    fs::write(&drive, r#"
vim.defer_fn(function()
  vim.api.nvim_win_set_cursor(0, { 2, 0 })
  vim.api.nvim_feedkeys("Vc", "m", false)
  vim.defer_fn(function()
    vim.api.nvim_feedkeys("keep it short", "m", false)
    vim.defer_fn(function()
      vim.api.nvim_feedkeys(vim.keycode("<CR>"), "m", false)
      vim.defer_fn(function()
        vim.api.nvim_feedkeys(vim.keycode("<Tab>"), "mx", false)
        vim.defer_fn(function() vim.cmd("qa!") end, 300)
      end, 500)
    end, 200)
  end, 200)
end, 500)
"#).unwrap();
    let lua = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/review.lua");
    let output = Command::new("nvim").args(["--clean", "--headless", "-S", lua, "-S"]).arg(&drive).current_dir(&split.slot)
        .env("REVIEW_DIR", &dir).env("HERDR_BIN_PATH", split.root.join("herdr")).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let calls = split.herdr_calls();
    assert!(calls.contains(&format!("pane send-text p_claude b:1 in {} — keep it short", &sha[..7])), "the selection's comment, typed and not sent: {calls:?}");
    assert!(calls.contains(&"pane send-keys p_view tab".to_string()), "{calls:?}");
}

// A slow diff of the file shown before must not land over the newer one, or a comment would name one file and quote another.
#[test]
#[ignore = "needs nvim 0.10 or newer on PATH, which CI does not install: cargo test --test review_split -- --ignored"]
fn a_diff_that_finishes_after_a_newer_file_was_shown_is_dropped() {
    let split = Split::new("stale");
    let dir = split.dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("agent"), "p_claude\n").unwrap();
    fs::write(dir.join("pane"), "p_view\n").unwrap();
    split.view_running();
    let shown = |path: &str, script: &str| format!(r#"{{"path":"{path}","title":"all","sha":"","short":"","index":1,"count":2,"diff":["sh","-c","{script}"]}}"#);
    fs::write(dir.join("shown.json"), shown("slow", "sleep 1; echo slow-diff")).unwrap();
    let seen = split.root.join("seen.txt");
    let drive = split.root.join("drive.lua");
    fs::write(&drive, format!(r#"
vim.defer_fn(function()
  local file = io.open("{0}/shown.json", "w")
  file:write('{1}')
  file:close()
  KebbieReview.load()
  vim.defer_fn(function()
    local lines = {{}}
    for _, buf in ipairs(vim.api.nvim_list_bufs()) do
      vim.list_extend(lines, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
    end
    local out = io.open("{2}", "w")
    out:write(table.concat(lines, "\n"))
    out:close()
    vim.cmd("qa!")
  end, 1800)
end, 100)
"#, dir.display(), shown("fast", "echo fast-diff"), seen.display())).unwrap();
    let lua = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/review.lua");
    let output = Command::new("nvim").args(["--clean", "--headless", "-S", lua, "-S"]).arg(&drive).current_dir(&split.slot)
        .env("REVIEW_DIR", &dir).env("HERDR_BIN_PATH", split.root.join("herdr")).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let seen = fs::read_to_string(&seen).unwrap();
    assert!(seen.contains("fast-diff") && !seen.contains("slow-diff"), "the newer file's diff stays: {seen}");
}
