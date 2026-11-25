use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

pub const KEBBIE: &str = env!("CARGO_BIN_EXE_kebbie");

// A herdr that keeps its Spaces and agents as files, starts an agent by marking its pane as running, and shows a pane's screen from screens/<pane>, or the agent's empty input box while it runs; a bracketed paste shows folded in that box until Enter takes it.
const HERDR: &str = r#"#!/bin/sh
d=$(dirname "$0")
for a in "$@"; do printf '%s\037' "$a" >> "$d/herdr.log"; done
printf '\036' >> "$d/herdr.log"
[ "$1" = --version ] && { echo 'herdr 0.9.5'; exit 0; }
flag() { want=$1; shift; while [ $# -gt 0 ]; do [ "$1" = "$want" ] && { printf '%s' "$2"; return; }; shift; done; }
list() { printf '{"result":{"%s":[' "$1"; sep=; for f in "$d/$1"/*.json; do [ -f "$f" ] || continue; printf '%s' "$sep"; cat "$f"; sep=,; done; printf ']}}\n'; }
argv() { kind=$1; shift; while [ $# -gt 0 ] && [ "$1" != "--" ]; do shift; done; shift; printf '["%s"' "$kind"; for a in "$@"; do printf ',"%s"' "$(printf '%s' "$a" | sed 's/[\\"]/\\&/g')"; done; printf ']'; }
case "$1 $2" in
  'worktree open') label=$(flag --label "$@"); printf '{"workspace_id":"w_%s","label":"%s","focused":false,"worktree":{"repo_root":"%s","checkout_path":"%s"}}' "$label" "$label" "$(flag --cwd "$@")" "$(flag --path "$@")" > "$d/workspaces/$label.json"; printf '{"result":{"workspace":{"workspace_id":"w_%s"},"root_pane":{"pane_id":"p_%s"}}}\n' "$label" "$label"; exit 0;;
  'workspace list') list workspaces; exit 0;;
  'agent list') list agents; exit 0;;
  'workspace close') rm -f "$d/workspaces/${3#w_}.json" "$d/agents/${3#w_}.json" "$d/running-p_${3#w_}"; echo '{"result":{}}'; exit 0;;
  'pane process-info') p=$4; if [ -f "$d/running-$p" ]; then printf '{"result":{"process_info":{"foreground_process_group_id":%s,"shell_pid":1,"foreground_processes":[{"argv0":"agent","argv":%s,"cwd":"%s","pid":%s}]}}}\n' "$(cat "$d/pid")" "$(cat "$d/argv-$p")" "$(cat "$d/cwd-$p" 2>/dev/null)" "$(cat "$d/pid")"; else printf '{"result":{"process_info":{"foreground_process_group_id":1,"shell_pid":1,"foreground_processes":[]}}}\n'; fi; exit 0;;
  'agent start') p=$(flag --pane "$@"); kind=$(flag --kind "$@"); touch "$d/running-$p"; argv "$kind" "$@" > "$d/argv-$p"; [ -f "$d/agents/${p#p_}.json" ] || printf '{"name":"%s","agent":"%s","workspace_id":"w_%s","pane_id":"%s","agent_status":"idle","interactive_ready":true}' "$3" "$kind" "${p#p_}" "$p" > "$d/agents/${p#p_}.json"; [ -f "$d/screens/$p" ] || cp "$d/screens/$kind" "$d/screens/$p"; echo '{"result":{}}'; exit 0;;
  'pane run') echo "${4#cd }" | tr -d "'" > "$d/cwd-$3"; echo '{"result":{}}'; exit 0;;
  'pane send-text') case "$4" in "$(printf '\033')[200~"*) [ -f "$d/screens/$3" ] && [ ! -f "$d/screens/$3.unpasted" ] && { mv "$d/screens/$3" "$d/screens/$3.unpasted"; sed 's/^❯ $/❯ [Pasted text #1 +3 lines]/' "$d/screens/$3.unpasted" > "$d/screens/$3"; };; esac; echo '{"result":{}}'; exit 0;;
  'pane send-keys') case "$*" in *ctrl+c*) rm -f "$d/running-$3" "$d/screens/$3" "$d/screens/$3.unpasted";; *enter*) if [ -f "$d/screens/$3.unpasted" ]; then mv "$d/screens/$3.unpasted" "$d/screens/$3"; elif [ -f "$d/screens/$3.next" ]; then mv "$d/screens/$3.next" "$d/screens/$3"; fi;; *) [ -f "$d/screens/$3.next" ] && mv "$d/screens/$3.next" "$d/screens/$3";; esac; echo '{"result":{}}'; exit 0;;
  'pane read'|'agent read') if [ -f "$d/screens/$3" ]; then cat "$d/screens/$3"; else echo 'you@host $'; fi; exit 0;;
esac
for key in "$1-$2-$3" "$1-$2"; do if [ -f "$d/answers/$key.json" ]; then cat "$d/answers/$key.json"; exit 0; fi; done
echo '{"error":{"code":"not_faked","message":"the fake herdr has no answer"}}' >&2
exit 1
"#;

// A claude that answers kebbie's background readers by what its brief asks: the assessor, the rules check or the full review, each from claude/<role> or a default, and logs the role it played.
const CLAUDE: &str = r#"#!/bin/sh
d=$(dirname "$(dirname "$0")")
case "$*" in
  --version) echo '2.1.316 (Claude Code)'; exit 0;;
  'auth status') echo '{"loggedIn": true}'; exit 0;;
  plugin*) echo "$*" >> "$d/claude-plugin.log"; exit 0;;
  *--advisor*) echo "$*" >> "$d/claude-advisor.log"; log=$(printf '%s\n' "$@" | sed -n '/^--debug-file$/{n;p;}'); [ -n "$log" ] && echo '[AdvisorTool] Server-side tool enabled with claude-fable-5-2 as the advisor model' > "$log"; echo '{"is_error":false,"result":"ok"}'; exit 0;;
esac
input=$(cat)
case "$input" in
  *'needs an independent review'*) role=assess; answer='needed: no
reason: one small function and its test';;
  *'this is a rules check'*) role=rules; answer='verdict: ok';;
  *) role=review; answer='verdict: ok';;
esac
model=$(printf '%s\n' "$@" | sed -n '/^--model$/{n;p;}')
echo "$role $*" >> "$d/claude.log"
[ -f "$d/claude/$role.die" ] && kill -9 $PPID
[ -f "$d/claude/$role" ] && answer=$(cat "$d/claude/$role")
[ -n "$model" ] && [ -f "$d/claude/$role-$model" ] && answer=$(cat "$d/claude/$role-$model")
printf '%s\n' "$answer"
"#;

const CODEX: &str = "#!/bin/sh\ncase \"$*\" in\n  --version) echo 'codex-cli 0.163.0' ;;\n  'login status') echo 'Logged in using ChatGPT' ;;\nesac\n";

pub const CLAUDE_SCREEN: &str = "⏺ Done.\n\n────────────────────────────────────────────────────────────\n❯ \n────────────────────────────────────────────────────────────\n  ? for shortcuts\n";
pub const CODEX_SCREEN: &str = "• Ready.\n\n› \n\n  100% context left\n";

pub struct Lab {
    pub root: PathBuf,
    pub home: PathBuf,
    pub state: PathBuf,
    pub config: PathBuf,
    pub app: PathBuf,
    sleeper: std::process::Child,
}

impl Lab {
    pub fn new(name: &str) -> Self {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!("kebbie-scenario-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in ["home", "state", "bin", "answers", "workspaces", "agents", "screens", "claude"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        crate::executable::write(root.join("herdr"), HERDR).unwrap();
        crate::executable::write(root.join("bin/claude"), CLAUDE).unwrap();
        crate::executable::write(root.join("bin/codex"), CODEX).unwrap();
        crate::executable::write(root.join("bin/herdr"), format!("#!/bin/sh\nexec '{}' \"$@\"\n", root.join("herdr").display())).unwrap();
        for tool in ["bwrap", "socat"] {
            crate::executable::write(root.join("bin").join(tool), "#!/bin/sh\nexit 0\n").unwrap();
        }
        // The agents' process, started after their binaries, so kebbie restart reads no newer Claude Code or Codex on PATH.
        let sleeper = Command::new("sleep").arg("300").spawn().unwrap();
        fs::write(root.join("pid"), sleeper.id().to_string()).unwrap();
        let lab = Lab { config: root.join("config.toml"), home: root.join("home"), state: root.join("state"), app: root.join("app"), root, sleeper };
        for arch in ["x64", "arm64"] {
            let filter = lab.home.join(".npm-global/lib/node_modules/@anthropic-ai/sandbox-runtime/vendor/seccomp").join(arch);
            fs::create_dir_all(&filter).unwrap();
            fs::write(filter.join("apply-seccomp"), "").unwrap();
        }
        fs::write(lab.root.join("screens/claude"), CLAUDE_SCREEN).unwrap();
        fs::write(lab.root.join("screens/codex"), CODEX_SCREEN).unwrap();
        for ok in ["agent-prompt", "pane-report-metadata", "workspace-rename", "workspace-focus", "tab-rename", "pane-send-text", "pane-close"] {
            fs::write(lab.root.join(format!("answers/{ok}.json")), r#"{"result":{}}"#).unwrap();
        }
        fs::write(lab.root.join("answers/plugin-list.json"), r#"{"result":{"plugins":[]}}"#).unwrap();
        fs::write(lab.root.join("answers/workspace-get.json"), r#"{"result":{"workspace":{"tokens":{}}}}"#).unwrap();
        fs::write(lab.root.join("answers/agent-get.json"), r#"{"result":{"agent":{"interactive_ready":true,"agent_status":"idle"}}}"#).unwrap();
        fs::write(lab.root.join("workspaces/lead.json"), r#"{"workspace_id":"w_lead","label":"lead","focused":true}"#).unwrap();
        fs::write(lab.root.join("agents/lead.json"), r#"{"name":"lead","agent":"claude","workspace_id":"w_lead","pane_id":"p_lead","agent_status":"idle","interactive_ready":true}"#).unwrap();
        fs::write(lab.root.join("screens/p_lead"), CLAUDE_SCREEN).unwrap();
        fs::write(&lab.config, "[keeper]\nenabled = false\n").unwrap();
        fs::write(lab.root.join("gitconfig"), format!("[include]\n\tpath = {}\n[user]\n\tname = Ada\n\temail = ada@example.com\n", crate::test_seal::root().join("gitconfig").display())).unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        fs::write(lab.state.join("agent-check.json"), format!(r#"{{"checked_at":{now},"findings":[]}}"#)).unwrap();
        fs::write(lab.state.join("update-check.json"), format!(r#"{{"checked_at":{now},"channel":"release","components":[]}}"#)).unwrap();
        lab.repo();
        lab
    }

    fn repo(&self) {
        let origin = self.root.join("origin.git");
        fs::create_dir_all(&self.app).unwrap();
        git(&self.app, &["init", "--quiet", "--initial-branch=main"]);
        commit(&self.app, "a", "first", "");
        git(&self.root, &["init", "--quiet", "--bare", "--initial-branch=main", origin.to_str().unwrap()]);
        git(&self.app, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&self.app, &["push", "--quiet", "origin", "main"]);
        git(&self.app, &["remote", "set-head", "origin", "main"]);
    }

    pub fn app(&self) -> &str {
        self.app.to_str().unwrap()
    }

    pub fn worktree(&self, number: u32) -> PathBuf {
        self.root.join(format!("wt{number}.app"))
    }

    pub fn path(&self) -> String {
        format!("{}:{}:/usr/bin:/bin", self.root.join("bin").display(), crate::test_seal::root().join("bin").display())
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(KEBBIE);
        command.env("HOME", &self.home)
            .env("KEBBIE_STATE_DIR", &self.state)
            .env("KEBBIE_CONFIG", &self.config)
            .env("HERDR_BIN_PATH", self.root.join("herdr"))
            .env("KEBBIE_GH_BIN", self.root.join("gh"))
            .env("PATH", self.path())
            .env("KEBBIE_CODEX_LATEST", "0.163.0")
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("CLAUDE_CODE_MESSAGING_SOCKET")
            .env_remove("CLAUDE_CODE_MESSAGING_TOKEN")
            .current_dir(&self.root)
            .stdin(Stdio::null());
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    pub fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(output.status.success(), "kebbie {args:?}: {}\n{}", stdout(&output), stderr(&output));
        stdout(&output)
    }

    pub fn run_as(&self, role: &str, args: &[&str]) -> Output {
        let claude = self.root.join("node_modules/@anthropic-ai/claude-code/cli.sh");
        fs::create_dir_all(claude.parent().unwrap()).unwrap();
        crate::executable::write(&claude, "#!/bin/sh\nshift 2\nkebbie=\"$1\"\nshift\n\"$kebbie\" \"$@\"\n").unwrap();
        let command = self.command();
        let mut wrapped = Command::new(&claude);
        wrapped.envs(command.get_envs().filter_map(|(key, value)| Some((key, value?)))).current_dir(&self.root).stdin(Stdio::null());
        wrapped.arg("--settings").arg(self.state.join(format!("roles/{role}.settings.json"))).arg(KEBBIE).args(args).output().unwrap()
    }

    // Detached and without the test's PATH, so ps is the real one and the chain reads as the user's own shell.
    pub fn run_as_user(&self, args: &[&str]) -> Output {
        use std::os::unix::process::ExitStatusExt;
        let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
        let (out, err, code) = (self.root.join("user.out"), self.root.join("user.err"), self.root.join("user.code"));
        let _ = fs::remove_file(&code);
        let line = std::iter::once(KEBBIE).chain(args.iter().copied()).map(quote).collect::<Vec<_>>().join(" ");
        let script = self.root.join("user.sh");
        fs::write(&script, format!("sleep 1\n{line} > {} 2> {}\necho $? > {}.tmp && mv {}.tmp {}\n", quote(&out.to_string_lossy()), quote(&err.to_string_lossy()), code.display(), code.display(), code.display())).unwrap();
        assert!(Command::new("sh").env_clear().envs(self.command().get_envs().filter(|(key, _)| *key != "PATH").filter_map(|(key, value)| Some((key, value?)))).current_dir(&self.root).arg("-c").arg(format!("sh < {} > /dev/null 2>&1 &", quote(&script.to_string_lossy()))).status().unwrap().success());
        assert!(eventually(Duration::from_secs(60), || code.exists()), "kebbie {args:?} as the user never finished");
        let status: i32 = fs::read_to_string(&code).unwrap().trim().parse().unwrap();
        Output { status: std::process::ExitStatus::from_raw(status << 8), stdout: fs::read(&out).unwrap(), stderr: fs::read(&err).unwrap() }
    }

    pub fn herdr_calls(&self) -> Vec<Vec<String>> {
        let log = fs::read_to_string(self.root.join("herdr.log")).unwrap_or_default();
        log.split('\u{1e}').filter(|record| !record.is_empty())
            .map(|record| record.split('\u{1f}').filter(|field| !field.is_empty()).map(str::to_string).collect())
            .collect()
    }

    // What was typed to an agent: herdr's agent prompt, or a start's bracketed paste into its pane.
    pub fn prompts_to(&self, target: &str) -> Vec<String> {
        let pasted = |call: &Vec<String>| call[..2] == ["pane", "send-text"] && call[3].starts_with("\u{1b}[200~");
        self.herdr_calls().into_iter().filter(|call| call.len() > 3 && (call[..2] == ["agent", "prompt"] || pasted(call)) && (call[2] == target || call[2] == format!("p_{target}")))
            .map(|call| call[3].trim_start_matches("\u{1b}[200~").trim_end_matches("\u{1b}[201~").to_string()).collect()
    }

    pub fn screen(&self, pane: &str, text: &str) {
        fs::write(self.root.join("screens").join(pane), text).unwrap();
    }

    pub fn agent(&self, task: &str, change: impl FnOnce(&mut Value)) {
        let file = self.root.join("agents").join(format!("{task}.json"));
        let mut agent: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        change(&mut agent);
        fs::write(file, agent.to_string()).unwrap();
    }

    pub fn status(&self) -> Value {
        serde_json::from_str(&self.ok(&["status", "--json"])).unwrap()
    }

    pub fn item(&self, task: &str) -> Value {
        self.status()["items"].as_array().unwrap().iter().find(|item| item["name"] == task).cloned().unwrap_or_else(|| panic!("{task} is not on the board"))
    }

    pub fn on_board(&self, task: &str) -> bool {
        self.status()["items"].as_array().unwrap().iter().any(|item| item["name"] == task)
    }

    pub fn messages(&self) -> Vec<Value> {
        fs::read_to_string(self.state.join("messages.jsonl")).unwrap_or_default().lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
    }

    pub fn to_lead(&self) -> Vec<String> {
        self.messages().into_iter().filter(|message| message["to"] == "lead").filter_map(|message| message["text"].as_str().map(str::to_string)).collect()
    }

    pub fn claude_roles(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("claude.log")).unwrap_or_default().lines().map(|line| line.split(' ').next().unwrap_or_default().to_string()).collect()
    }

    pub fn claude_calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("claude.log")).unwrap_or_default().lines().map(str::to_string).collect()
    }

    pub fn answer(&self, role: &str, text: &str) {
        fs::write(self.root.join("claude").join(role), text).unwrap();
    }

    // A keeper runs every routine once as it starts, so a start and a stop is one round.
    pub fn keeper_round(&self) {
        fs::write(&self.config, fs::read_to_string(&self.config).unwrap().replace("[keeper]\nenabled = false\n", "")).unwrap();
        let status = self.state.join("keeper.json");
        let _ = fs::remove_file(&status);
        self.ok(&["keeper", "--start"]);
        assert!(eventually(Duration::from_secs(120), || status.exists()), "the keeper ran no round: {}", fs::read_to_string(self.state.join("keeper.log")).unwrap_or_default());
        self.ok(&["keeper", "--stop"]);
        fs::write(&self.config, format!("[keeper]\nenabled = false\n{}", fs::read_to_string(&self.config).unwrap())).unwrap();
    }

    pub fn keeper_log(&self) -> String {
        fs::read_to_string(self.state.join("keeper.log")).unwrap_or_default()
    }

    pub fn config(&self, text: &str) {
        fs::write(&self.config, format!("[keeper]\nenabled = false\n{text}")).unwrap();
    }

    pub fn profile(&self, name: &str, text: &str) {
        let dir = self.home.join(".config/kebbie/profiles");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.toml")), text).unwrap();
    }

    pub fn start(&self, task: &str, prompt: &str, args: &[&str]) -> PathBuf {
        let started = self.run(&[&["-C", self.app(), "start", task, prompt], args].concat());
        assert!(started.status.success(), "{}\n{}", stdout(&started), stderr(&started));
        let item = self.item(task);
        PathBuf::from(item["path"].as_str().unwrap())
    }

    pub fn work(&self, worktree: &Path, file: &str, subject: &str) -> String {
        commit(worktree, file, subject, "\n\nThe page lists it now.")
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        crate::test_seal::stop_keepers_under(&self.root);
        let _ = self.sleeper.kill();
        let _ = self.sleeper.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(args).current_dir(dir).env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub fn commit(dir: &Path, file: &str, subject: &str, body: &str) -> String {
    fs::write(dir.join(file), subject).unwrap();
    git(dir, &["add", file]);
    git(dir, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "-m", &format!("{subject}{body}")]);
    git(dir, &["rev-parse", "HEAD"])
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

pub fn eventually(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    done()
}
