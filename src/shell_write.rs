use std::path::{Component, Path, PathBuf};

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Output,
    Input,
    Separator,
}

// Words and the operators that matter for writes; anything this cannot read stays a word, so an odd command reads as no write.
fn tokens(command: &str) -> Vec<Token> {
    let chars: Vec<char> = command.chars().collect();
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut index = 0;
    let end_word = |word: &mut String, started: &mut bool, tokens: &mut Vec<Token>| {
        if *started {
            tokens.push(Token::Word(std::mem::take(word)));
            *started = false;
        }
    };
    while index < chars.len() {
        let character = chars[index];
        match character {
            '\'' => {
                started = true;
                index += 1;
                while index < chars.len() && chars[index] != '\'' {
                    word.push(chars[index]);
                    index += 1;
                }
            }
            '"' => {
                started = true;
                index += 1;
                while index < chars.len() && chars[index] != '"' {
                    if chars[index] == '\\' && index + 1 < chars.len() {
                        index += 1;
                    }
                    word.push(chars[index]);
                    index += 1;
                }
            }
            '\\' => {
                started = true;
                index += 1;
                if let Some(next) = chars.get(index).filter(|next| **next != '\n') {
                    word.push(*next);
                }
            }
            ' ' | '\t' => end_word(&mut word, &mut started, &mut tokens),
            '\n' => {
                end_word(&mut word, &mut started, &mut tokens);
                tokens.push(Token::Separator);
                for (delimiter, strip) in std::mem::take(&mut heredocs) {
                    loop {
                        let line_end = chars[index + 1..].iter().position(|next| *next == '\n').map_or(chars.len(), |at| index + 1 + at);
                        let line: String = chars[(index + 1).min(chars.len())..line_end].iter().collect();
                        index = line_end;
                        let line = if strip { line.trim_start_matches('\t') } else { line.as_str() };
                        if line == delimiter || index >= chars.len() {
                            break;
                        }
                    }
                }
            }
            ';' | '|' | '(' | ')' | '`' => {
                end_word(&mut word, &mut started, &mut tokens);
                tokens.push(Token::Separator);
            }
            '&' if chars.get(index + 1) == Some(&'>') => {
                end_word(&mut word, &mut started, &mut tokens);
                index += if chars.get(index + 2) == Some(&'>') { 2 } else { 1 };
                tokens.push(Token::Output);
            }
            '&' => {
                end_word(&mut word, &mut started, &mut tokens);
                tokens.push(Token::Separator);
            }
            '>' => {
                if started && !word.is_empty() && word.chars().all(|digit| digit.is_ascii_digit()) {
                    word.clear();
                    started = false;
                }
                end_word(&mut word, &mut started, &mut tokens);
                if matches!(chars.get(index + 1), Some('>') | Some('|')) {
                    index += 1;
                }
                if chars.get(index + 1) == Some(&'&') {
                    index += 2;
                    while chars.get(index).is_some_and(|next| next.is_ascii_digit() || *next == '-') {
                        index += 1;
                    }
                    continue;
                }
                tokens.push(Token::Output);
            }
            '<' => {
                end_word(&mut word, &mut started, &mut tokens);
                if chars.get(index + 1) == Some(&'<') && chars.get(index + 2) != Some(&'<') {
                    index += 2;
                    let strip = chars.get(index) == Some(&'-');
                    if strip {
                        index += 1;
                    }
                    while chars.get(index) == Some(&' ') {
                        index += 1;
                    }
                    let mut delimiter = String::new();
                    while let Some(next) = chars.get(index).filter(|next| !next.is_whitespace() && !matches!(next, ';' | '|' | '&' | '>' | '<')) {
                        if !matches!(next, '\'' | '"' | '\\') {
                            delimiter.push(*next);
                        }
                        index += 1;
                    }
                    heredocs.push((delimiter, strip));
                    continue;
                }
                index += if chars.get(index + 1) == Some(&'<') { 2 } else { 0 };
                tokens.push(Token::Input);
            }
            '$' if chars.get(index + 1) == Some(&'(') && chars.get(index + 2) == Some(&'(') => {
                started = true;
                word.push('$');
                index += 3;
                while index + 1 < chars.len() && !(chars[index] == ')' && chars[index + 1] == ')') {
                    index += 1;
                }
                index += 1;
            }
            other => {
                started = true;
                word.push(other);
            }
        }
        index += 1;
    }
    end_word(&mut word, &mut started, &mut tokens);
    tokens
}

fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

// None for a path the shell would expand first ($VAR, a glob), which this does not guess at.
fn resolve(word: &str, cwd: &Path) -> Option<PathBuf> {
    if word.is_empty() || word.contains(['$', '*', '?', '[']) {
        return None;
    }
    let path = match word.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => PathBuf::from(std::env::var("HOME").ok()?).join(rest.trim_start_matches('/')),
        Some(_) => return None,
        None => cwd.join(word),
    };
    Some(normalized(&path))
}

fn worktree_of(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

fn options_end(words: &[String]) -> impl Iterator<Item = &String> {
    words.iter().filter(|word| !word.starts_with('-'))
}

// Which writes count: Content is what puts text in a file, Any adds what removes, moves, creates or links one.
#[derive(Clone, Copy, PartialEq)]
pub enum Writes {
    Content,
    Any,
}

// The operands of a command whose options in valued take the next word as their value.
fn operands<'a>(args: &'a [String], valued: &[&str]) -> Vec<&'a String> {
    let mut found = Vec::new();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if word == "--" {
            found.extend(words.by_ref());
        } else if valued.contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with('-') {
            found.push(word);
        }
    }
    found
}

// The files a program clearly writes: cp and mv into it, tee, sed -i, git apply and patch, and with Any also what rm, mkdir, touch, ln, mv and dd change.
fn command_writes(words: &[String], cwd: &Path, writes: Writes, found: &mut Vec<PathBuf>) {
    let start = words.iter().position(|word| (!word.contains('=') || word.starts_with('-')) && !matches!(word.as_str(), "sudo" | "env" | "command" | "nohup" | "time" | "exec"));
    let Some(start) = start else { return };
    let program = Path::new(&words[start]).file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    let args = &words[start + 1..];
    let mut add = |word: &str, base: &Path| found.extend(resolve(word, base));
    match program.as_str() {
        "tee" => options_end(args).for_each(|file| add(file, cwd)),
        "cp" | "mv" | "install" | "ln" => {
            let target = args.iter().position(|word| word == "-t").and_then(|at| args.get(at + 1)).cloned()
                .or_else(|| args.iter().find_map(|word| word.strip_prefix("--target-directory=").map(str::to_string)));
            let operands = operands(args, &["-t", "-S", "-m", "-o", "-g", "--suffix"]);
            if program == "ln" && writes == Writes::Content {
                return;
            }
            if program == "mv" && writes == Writes::Any {
                operands.iter().for_each(|file| add(file, cwd));
            }
            match target {
                Some(target) => add(&target, cwd),
                None if operands.len() >= 2 => add(operands[operands.len() - 1], cwd),
                None => {}
            }
        }
        "rm" | "rmdir" | "unlink" | "mkdir" | "touch" | "truncate" if writes == Writes::Any => {
            operands(args, &["-m", "-r", "-t", "-d", "-s", "-A"]).iter().for_each(|file| add(file, cwd));
        }
        "dd" if writes == Writes::Any => args.iter().filter_map(|word| word.strip_prefix("of=")).for_each(|file| add(file, cwd)),
        "sed" => {
            let in_place = args.iter().any(|word| word.starts_with("--in-place") || (word.starts_with('-') && !word.starts_with("--") && word.contains('i')));
            if !in_place {
                return;
            }
            let scripted = args.iter().any(|word| matches!(word.as_str(), "-e" | "-f" | "--expression" | "--file") || word.starts_with("--expression=") || word.starts_with("--file="));
            let mut operands: Vec<&String> = Vec::new();
            let mut skip = false;
            for word in args {
                if skip {
                    skip = false;
                } else if matches!(word.as_str(), "-e" | "-f" | "--expression" | "--file") {
                    skip = true;
                } else if !word.starts_with('-') && !word.is_empty() {
                    operands.push(word);
                }
            }
            operands.iter().skip(usize::from(!scripted)).for_each(|file| add(file, cwd));
        }
        "git" => {
            let mut dir = cwd.to_path_buf();
            let mut rest = args.iter();
            while let Some(word) = rest.next() {
                match word.as_str() {
                    "-C" => {
                        let Some(next) = rest.next().and_then(|next| resolve(next, &dir)) else { return };
                        dir = next;
                    }
                    "-c" => {
                        rest.next();
                    }
                    "apply" => {
                        if !rest.any(|word| matches!(word.as_str(), "--check" | "--stat" | "--numstat" | "--summary")) {
                            add(".", &dir);
                        }
                        return;
                    }
                    flag if flag.starts_with('-') => {}
                    _ => return,
                }
            }
        }
        "patch" => {
            if args.iter().any(|word| word == "--dry-run") {
                return;
            }
            let dir = args.iter().position(|word| word == "-d").and_then(|at| args.get(at + 1)).and_then(|dir| resolve(dir, cwd)).unwrap_or_else(|| cwd.to_path_buf());
            add(".", &dir);
        }
        _ => {}
    }
}

// Every file a Bash command clearly writes, resolved against the folder each part runs in; anything unclear ($VAR, a glob, a cd this cannot follow) reads as no write, since both guards are guard rails.
pub fn written(command: &str, cwd: &Path, writes: Writes) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut cwd = cwd.to_path_buf();
    let mut words: Vec<String> = Vec::new();
    let mut tokens = tokens(command).into_iter().peekable();
    loop {
        let token = tokens.next();
        match token {
            Some(Token::Word(word)) => words.push(word),
            Some(Token::Output) => {
                if words.first().is_none_or(|first| first != "[[")
                    && let Some(Token::Word(target)) = tokens.peek()
                {
                    found.extend(resolve(target, &cwd));
                }
                tokens.next();
            }
            Some(Token::Input) => {
                tokens.next();
            }
            Some(Token::Separator) | None => {
                if words.first().map(String::as_str) == Some("cd") {
                    match words.get(1).and_then(|dir| resolve(dir, &cwd)) {
                        Some(dir) => cwd = dir,
                        None => return found,
                    }
                } else {
                    command_writes(&words, &cwd, writes, &mut found);
                }
                words.clear();
                if token.is_none() {
                    return found;
                }
            }
        }
    }
}

// Whether a Bash command clearly writes a file inside the worktree it runs in.
pub fn writes_into(command: &str, cwd: &Path) -> bool {
    let Some(worktree) = worktree_of(cwd) else { return false };
    let worktree = normalized(&worktree);
    written(command, cwd, Writes::Content).iter().any(|path| path.starts_with(&worktree))
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn worktree() -> PathBuf {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("kebbie-shell-write-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))).join("wt1.app");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    #[test]
    fn a_command_that_clearly_writes_into_the_worktree_is_a_write() {
        let wt = worktree();
        let sub = wt.join("src");
        for command in [
            "echo x > a.txt", "echo x >> a.txt", "echo x>a.txt", "cat <<'EOF' > src/a.rs\nfn main() {}\nEOF", "printf x 1> a", "ls &> out.log",
            "echo hi | tee a.txt", "tee -a notes.md < /dev/null", "sed -i '' 's/a/b/' src/a.rs", "sed -i.bak -e s/a/b/ a.rs", "sed --in-place s/a/b/ a.rs",
            "cp /tmp/x.rs src/x.rs", "mv /tmp/x src/", "cp -t src /tmp/a /tmp/b", "git apply /tmp/fix.patch", "git -C . apply x.diff", "patch -p1 < /tmp/fix.patch",
            "cd src && echo x > a.rs", "FOO=1 sudo tee src/a.rs",
        ] {
            assert!(writes_into(command, &wt) && writes_into(command, &sub), "{command:?} writes into the worktree");
        }
        assert!(!writes_into(&format!("echo x > {}/a.txt", wt.display()), Path::new("/")), "outside any worktree the gate has nothing to guard");
        assert!(writes_into(&format!("echo x > {}/a.txt", wt.display()), &sub));
        assert!(writes_into("echo x > ../a.txt", &sub), "src/.. is still the worktree");
        std::fs::remove_dir_all(wt.parent().unwrap()).unwrap();
    }

    #[test]
    fn reading_testing_and_writing_elsewhere_are_not_writes() {
        let wt = worktree();
        for command in [
            "cargo test 2>&1 | tail -20", "git status", "ls -la > /dev/null", "grep -rn foo src 2>/dev/null", "echo x > /tmp/scratch.txt", "echo x >&2",
            "cmd 2>&1", "cat src/a.rs", "sed 's/a/b/' src/a.rs", "sed -n 1,20p src/a.rs", "cp src/a.rs /tmp/a.rs", "git apply --check /tmp/x.patch",
            "patch --dry-run -p1 < x.patch", "python3 - <<'EOF'\nif a > b: print(1)\nopen('x') >> y\nEOF", "echo '> a.txt'", "echo x > ../outside.txt",
            "cd /tmp && echo x > a.txt", "echo $((1 > 0))", "[[ a > b ]]", "sed -i '' s/a/b/ /tmp/x", "echo x > $OUT", "git -C /tmp/other apply x.diff", "tee", "",
        ] {
            assert!(!writes_into(command, &wt), "{command:?} is not a write into the worktree");
        }
        std::fs::remove_dir_all(wt.parent().unwrap()).unwrap();
    }
}
