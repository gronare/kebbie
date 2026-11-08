use serde_json::{Value, json};

const REASON: &str = "A task never pushes: kebbie refuses git push in a task's shell in every form. The user pushes from the board, or the lead with kebbie push for a task the user gave push permission; report ready when the work is done.";
const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "ksh", "fish"];
const WRAPPERS: [&str; 13] = ["env", "command", "exec", "nohup", "time", "nice", "sudo", "doas", "xargs", "timeout", "stdbuf", "caffeinate", "builtin"];
const PUSHING: [&str; 4] = ["push", "send-pack", "http-push", "receive-pack"];
// git's global options that take the next word as their value when written without `=`.
const GIT_VALUED: [&str; 8] = ["-c", "-C", "--git-dir", "--work-tree", "--namespace", "--config-env", "--super-prefix", "--attr-source"];

#[derive(Default)]
struct Parsed {
    words: Vec<String>,
    redirects: Vec<String>,
}

// The words of one shell command line, split into simple commands, each with the files its redirects name; a $(…) or `…` inside double quotes is a command line of its own.
fn commands(line: &str) -> Vec<Parsed> {
    let mut found: Vec<Parsed> = vec![Parsed::default()];
    let mut word: Option<String> = None;
    let mut redirect = false;
    let mut chars = line.chars().peekable();
    let end_word = |found: &mut Vec<Parsed>, word: &mut Option<String>, redirect: &mut bool| {
        if let Some(done) = word.take() {
            let last = found.last_mut().unwrap();
            if std::mem::take(redirect) { last.redirects.push(done) } else { last.words.push(done) }
        }
    };
    while let Some(character) = chars.next() {
        match character {
            '\'' => {
                let text = word.get_or_insert_with(String::new);
                for quoted in chars.by_ref() {
                    if quoted == '\'' {
                        break;
                    }
                    text.push(quoted);
                }
            }
            '"' => {
                let text = word.get_or_insert_with(String::new);
                while let Some(quoted) = chars.next() {
                    match quoted {
                        '"' => break,
                        '\\' => text.extend(chars.next()),
                        '$' if chars.peek() == Some(&'(') => {
                            chars.next();
                            let inner = until_close(&mut chars, '(', ')');
                            found.extend(commands(&inner));
                            text.push_str(&format!("$({inner})"));
                        }
                        '`' => {
                            let inner: String = chars.by_ref().take_while(|next| *next != '`').collect();
                            found.extend(commands(&inner));
                        }
                        other => text.push(other),
                    }
                }
            }
            '\\' => {
                if let Some(next) = chars.next().filter(|next| *next != '\n') {
                    word.get_or_insert_with(String::new).push(next);
                }
            }
            '<' | '>' => {
                end_word(&mut found, &mut word, &mut redirect);
                redirect = true;
            }
            '&' if redirect && word.is_none() || chars.peek() == Some(&'>') => {}
            ';' | '&' | '|' | '\n' | '(' | ')' | '`' | '{' | '}' => {
                end_word(&mut found, &mut word, &mut redirect);
                redirect = false;
                found.push(Parsed::default());
            }
            space if space.is_whitespace() => end_word(&mut found, &mut word, &mut redirect),
            other => word.get_or_insert_with(String::new).push(other),
        }
    }
    end_word(&mut found, &mut word, &mut redirect);
    found.retain(|command| !command.words.is_empty() || !command.redirects.is_empty());
    found
}

fn until_close(chars: &mut std::iter::Peekable<std::str::Chars>, open: char, close: char) -> String {
    let (mut depth, mut inner) = (1, String::new());
    for next in chars.by_ref() {
        if next == open {
            depth += 1;
        } else if next == close {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
        inner.push(next);
    }
    inner
}

pub(crate) fn base(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

pub(crate) fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit()))
}

// Past leading NAME=value words and wrappers such as env, command or nice, with their options.
fn program(words: &[String]) -> &[String] {
    let mut rest = words;
    loop {
        while rest.first().is_some_and(|word| assignment(word)) {
            rest = &rest[1..];
        }
        let Some(first) = rest.first() else { return rest };
        if !WRAPPERS.contains(&base(first)) {
            return rest;
        }
        rest = &rest[1..];
        while let Some(word) = rest.first().filter(|word| word.starts_with('-') || assignment(word) || word.chars().all(|c| c.is_ascii_digit() || c == '.')) {
            let valued = ["-u", "-n", "-I", "-s", "-k", "-S", "-g", "-C"].contains(&word.as_str());
            rest = &rest[if valued { 2.min(rest.len()) } else { 1 }..];
        }
    }
}

fn pushes_git(words: &[String]) -> bool {
    let mut rest = &words[1..];
    while let Some(word) = rest.first().filter(|word| word.starts_with('-')) {
        let value = if GIT_VALUED.contains(&word.as_str()) { rest.get(1).map(String::as_str) } else { word.split_once('=').map(|(_, value)| value) };
        if ["-c", "--config-env"].iter().any(|option| word == option || word.starts_with(&format!("{option}="))) && value.is_some_and(|value| value.to_ascii_lowercase().starts_with("alias.")) {
            return true;
        }
        rest = &rest[if GIT_VALUED.contains(&word.as_str()) { 2.min(rest.len()) } else { 1 }..];
    }
    match rest.first().map(String::as_str) {
        Some(command) if PUSHING.contains(&command) => true,
        Some("subtree") => rest.iter().any(|word| word == "push"),
        _ => false,
    }
}

// One simple command a line runs: the assignments and wrappers before its program, the program with its arguments, and the files its redirects name.
pub(crate) struct Simple {
    pub before: Vec<String>,
    pub words: Vec<String>,
    pub redirects: Vec<String>,
}

// Every simple command of a line, with the lines an eval or a shell's -c runs taken in as their own.
pub(crate) fn simple_commands(line: &str) -> Vec<Simple> {
    let mut found = Vec::new();
    for Parsed { words, redirects } in commands(line) {
        let rest = program(&words);
        let before = words[..words.len() - rest.len()].to_vec();
        match rest.first().map(|first| base(first)) {
            Some("eval") => found.extend(simple_commands(&rest[1..].join(" "))),
            Some(shell) if SHELLS.contains(&shell) => {
                let script = rest.iter().position(|word| word.starts_with('-') && !word.starts_with("--") && word.contains('c')).and_then(|at| rest.get(at + 1));
                found.extend(script.map(|script| simple_commands(script)).unwrap_or_default());
            }
            _ => {}
        }
        found.push(Simple { before, words: rest.to_vec(), redirects });
    }
    found
}

fn pushes_program(words: &[String]) -> bool {
    let Some(first) = words.first() else { return false };
    let name = base(first);
    name == "git" && pushes_git(words) || PUSHING.iter().any(|command| name == format!("git-{command}"))
}

// Whether a shell command line runs git push, in the forms a shell reaches it by: behind env, command or a shell's -c, after git's own options, or through an alias set on the command line.
pub fn pushes(line: &str) -> bool {
    simple_commands(line).iter().any(|command| pushes_program(&command.words))
}

// The PreToolUse hook on a task's shell commands, Claude's and Codex's alike; cluster says what the task's kubectl and helm may reach.
pub fn hook(input: &str, cluster: crate::kube_guard::Cluster) -> String {
    let input: Value = serde_json::from_str(input).unwrap_or_default();
    let shell = input["tool_name"].as_str().is_none_or(|tool| tool == "Bash" || tool == "exec_command" || tool == "shell");
    let Some(command) = input["tool_input"]["command"].as_str().filter(|_| shell) else { return String::new() };
    let within = crate::kube_guard::within(input["cwd"].as_str().map(std::path::Path::new));
    let reason = if pushes(command) { Some(REASON.to_string()) } else { crate::kube_guard::refusal(command, cluster, &within) };
    reason.map(|reason| json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason } }).to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_way_a_shell_reaches_git_push_is_refused() {
        for line in [
            "git push", "git push origin HEAD:main", "  git\tpush", "/usr/bin/git push", "git -c url.x.pushInsteadOf=y push origin", "git -C /src/app push", "git --git-dir=/src/app/.git push",
            "git --git-dir /src/app/.git --work-tree /src/app push", "git --no-pager -c a=b -C . push", "env git push", "env -i PATH=/usr/bin git push", "env -u GIT_CONFIG_COUNT git push",
            "GIT_CONFIG_COUNT=0 git push", "command git push", "exec git push", "nohup git push &", "nice -n 5 git push", "timeout 30 git push", "sudo -u ada git push",
            "sh -c 'git push'", "bash -lc \"git push origin main\"", "zsh -c 'cd x && git push'", "eval git push", "eval 'git push'", "git status && git push", "git log; git push",
            "git fetch || git push", "(git push)", "{ git push; }", "echo \"$(git push)\"", "echo `git push`", "x=$(git push)", "git commit -m x && git push --force",
            "git send-pack origin main", "git-push origin", "/usr/libexec/git-core/git-send-pack x", "git http-push x", "git -c alias.up=push up", "git -c alias.p='!git push' p",
            "git --config-env=alias.x=VAR x", "git subtree push --prefix=lib origin main", "xargs git push < remotes", "git pu\\sh", "g''it push", "git \"push\"",
            "cd /src/app\ngit push", "true | sh -c 'git push'",
        ] {
            assert!(pushes(line), "{line:?} gets through");
            let decision: Value = serde_json::from_str(&hook(&json!({ "tool_name": "Bash", "tool_input": { "command": line } }).to_string(), crate::kube_guard::Cluster::None)).unwrap();
            assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny", "{line:?}");
        }
    }

    #[test]
    fn the_roles_claim_only_what_the_task_settings_enforce() {
        let task = include_str!("../roles/task.md");
        let lead = include_str!("../roles/lead.md");
        assert!(task.contains("Your shell has no GitHub token or git credential login, and kebbie refuses `git push` from it in every form") && lead.contains("A task's own shell has no GitHub token or git credential login and kebbie refuses `git push` from it in every form"));
        assert!(task.contains("an ops task keeps the user's ssh agent for its ssh to other machines") && lead.contains("an ops task keeping the user's ssh agent for its ssh to other machines"));
        let settings: Value = serde_json::from_str(&crate::agent::claude::settings("/k", "billing@app", None, None, None)).unwrap();
        assert!(settings["hooks"]["PreToolUse"].as_array().unwrap().iter().any(|entry| entry["matcher"] == "Bash" && entry["hooks"][0]["command"] == "'/k' hook push-guard"), "the guard the roles name runs on every shell command");
        for (name, value) in crate::push_block::NO_LOGINS {
            assert_eq!(settings["env"][name], value, "{name}: the shell has no login");
        }
        let ops: Value = serde_json::from_str(&crate::agent::Agent::Claude.task_settings("/k", "billing@app", None, None, &crate::profiles::built_in(crate::profiles::OPS).unwrap(), false)).unwrap();
        assert!(ops["env"].get(crate::push_block::SSH_AGENT).is_none() && ops["env"]["GH_TOKEN"] == "" && ops["hooks"]["PreToolUse"][0]["hooks"][0]["command"] == "'/k' hook push-guard --own-cluster", "an unsandboxed ops task keeps the ssh agent, drops the token and meets the guard: {ops}");
    }

    #[test]
    fn a_task_commits_fetches_and_talks_about_pushing_as_before() {
        for line in [
            "git status", "git commit -m 'git push later'", "git commit -m \"do not git push yet\"", "git log --grep push", "git stash push -m wip", "git fetch origin", "git rebase origin/main",
            "echo git push", "grep -r 'git push' docs", "git config --get remote.origin.pushurl", "cargo test push", "git branch push-fix", "git checkout -b push", "git show HEAD:push.rs",
            "kebbie report --state ready --next 'review, then push' 'Done'", "git worktree list", "rg \"git push\" src", "git -C /src/app log", "npm run push-docs", "git remote -v",
        ] {
            assert!(!pushes(line), "{line:?} is refused");
            assert_eq!(hook(&json!({ "tool_name": "Bash", "tool_input": { "command": line } }).to_string(), crate::kube_guard::Cluster::None), "", "{line:?}");
        }
        assert_eq!(hook(&json!({ "tool_name": "Edit", "tool_input": { "file_path": "/src/a.rs", "command": "git push" } }).to_string(), crate::kube_guard::Cluster::None), "", "only shell commands");
        assert_eq!(hook("not json", crate::kube_guard::Cluster::None), "");
    }
}
