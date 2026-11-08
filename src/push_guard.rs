use serde_json::{Value, json};

const REASON: &str = "A task never pushes: kebbie refuses git push in a task's shell in every form. The user pushes from the board, or the lead with kebbie push for a task the user gave push permission; report ready when the work is done.";
const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "ksh", "fish"];
const WRAPPERS: [&str; 13] = ["env", "command", "exec", "nohup", "time", "nice", "sudo", "doas", "xargs", "timeout", "stdbuf", "caffeinate", "builtin"];
const PUSHING: [&str; 4] = ["push", "send-pack", "http-push", "receive-pack"];
// git's global options that take the next word as their value when written without `=`.
const GIT_VALUED: [&str; 8] = ["-c", "-C", "--git-dir", "--work-tree", "--namespace", "--config-env", "--super-prefix", "--attr-source"];

// The words of one shell command line, split into simple commands; a $(…) or `…` inside double quotes is a command line of its own.
fn commands(line: &str) -> Vec<Vec<String>> {
    let mut found: Vec<Vec<String>> = vec![Vec::new()];
    let mut word: Option<String> = None;
    let mut chars = line.chars().peekable();
    let end_word = |found: &mut Vec<Vec<String>>, word: &mut Option<String>| {
        if let Some(done) = word.take() {
            found.last_mut().unwrap().push(done);
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
            ';' | '&' | '|' | '\n' | '(' | ')' | '`' | '{' | '}' | '<' | '>' => {
                end_word(&mut found, &mut word);
                if !matches!(character, '<' | '>') {
                    found.push(Vec::new());
                }
            }
            space if space.is_whitespace() => end_word(&mut found, &mut word),
            other => word.get_or_insert_with(String::new).push(other),
        }
    }
    end_word(&mut found, &mut word);
    found.retain(|command| !command.is_empty());
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

fn base(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn assignment(word: &str) -> bool {
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

fn pushes_words(words: &[String]) -> bool {
    let words = program(words);
    let Some(first) = words.first() else { return false };
    let name = base(first);
    if name == "git" {
        return pushes_git(words);
    }
    if PUSHING.iter().any(|command| name == format!("git-{command}")) {
        return true;
    }
    if name == "eval" {
        return pushes(&words[1..].join(" "));
    }
    if SHELLS.contains(&name) {
        let script = words.iter().position(|word| word.starts_with('-') && !word.starts_with("--") && word.contains('c')).and_then(|at| words.get(at + 1));
        return script.is_some_and(|script| pushes(script));
    }
    false
}

// Whether a shell command line runs git push, in the forms a shell reaches it by: behind env, command or a shell's -c, after git's own options, or through an alias set on the command line.
pub fn pushes(line: &str) -> bool {
    commands(line).iter().any(|words| pushes_words(words))
}

// The PreToolUse hook on a task's shell commands, Claude's and Codex's alike.
pub fn hook(input: &str) -> String {
    let input: Value = serde_json::from_str(input).unwrap_or_default();
    let shell = input["tool_name"].as_str().is_none_or(|tool| tool == "Bash" || tool == "exec_command" || tool == "shell");
    match input["tool_input"]["command"].as_str() {
        Some(command) if shell && pushes(command) => json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": REASON } }).to_string(),
        _ => String::new(),
    }
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
            let decision: Value = serde_json::from_str(&hook(&json!({ "tool_name": "Bash", "tool_input": { "command": line } }).to_string())).unwrap();
            assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny", "{line:?}");
        }
    }

    #[test]
    fn a_task_commits_fetches_and_talks_about_pushing_as_before() {
        for line in [
            "git status", "git commit -m 'git push later'", "git commit -m \"do not git push yet\"", "git log --grep push", "git stash push -m wip", "git fetch origin", "git rebase origin/main",
            "echo git push", "grep -r 'git push' docs", "git config --get remote.origin.pushurl", "cargo test push", "git branch push-fix", "git checkout -b push", "git show HEAD:push.rs",
            "kebbie report --state ready --next 'review, then push' 'Done'", "git worktree list", "rg \"git push\" src", "git -C /src/app log", "npm run push-docs", "git remote -v",
        ] {
            assert!(!pushes(line), "{line:?} is refused");
            assert_eq!(hook(&json!({ "tool_name": "Bash", "tool_input": { "command": line } }).to_string()), "", "{line:?}");
        }
        assert_eq!(hook(&json!({ "tool_name": "Edit", "tool_input": { "file_path": "/src/a.rs", "command": "git push" } }).to_string()), "", "only shell commands");
        assert_eq!(hook("not json"), "");
    }
}
