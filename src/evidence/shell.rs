#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Seq,
    And,
    Or,
    Pipe,
    Background,
}

// The delimiter of a heredoc whose operator ends right before `rest`, its quotes and backslashes gone, whether <<- strips leading tabs, and how many chars it takes.
fn heredoc(rest: &str) -> (String, bool, usize) {
    let strip = rest.starts_with('-');
    let (mut word, mut quote, mut taken) = (String::new(), None::<char>, usize::from(strip));
    let mut chars = rest.chars().skip(taken).peekable();
    while chars.peek().is_some_and(|char| *char == ' ' || *char == '\t') {
        chars.next();
        taken += 1;
    }
    for char in chars {
        match (quote, char) {
            (Some(open), _) if char == open => quote = None,
            (Some(_), _) => word.push(char),
            (None, '\'' | '"') => quote = Some(char),
            (None, '\\') => {}
            (None, char) if char.is_whitespace() || ";&|<>()".contains(char) => break,
            (None, char) => word.push(char),
        }
        taken += 1;
    }
    (word, strip, taken)
}

// Where the bodies of the heredocs opened on a line end, from the byte after that line's newline: past each one's terminator line, or at the end.
fn after_bodies(line: &str, from: usize, pending: &[(String, bool)]) -> usize {
    let mut at = from;
    for (word, strip) in pending {
        while at < line.len() {
            let end = line[at..].find('\n').map_or(line.len(), |found| at + found);
            let text = &line[at..end];
            at = (end + 1).min(line.len());
            if (if *strip { text.trim_start_matches('\t') } else { text }) == word {
                break;
            }
        }
    }
    at
}

// The commands of a shell line, split at ; && || | & and newlines outside quotes and parentheses, each with the operator after it; heredoc bodies are text, not commands.
pub fn pieces(line: &str) -> Vec<(&str, Option<Op>)> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    fn cut<'a>(out: &mut Vec<(&'a str, Option<Op>)>, text: &'a str, op: Option<Op>) {
        if !text.trim().is_empty() {
            out.push((text.trim(), op));
        }
    }
    let (mut out, mut start, mut depth, mut quote, mut index) = (Vec::new(), 0, 0usize, None::<char>, 0);
    let mut pending: Vec<(String, bool)> = Vec::new();
    while index < chars.len() {
        let (at, char) = chars[index];
        let next = chars.get(index + 1).map(|(_, next)| *next);
        let prev = index.checked_sub(1).map(|before| chars[before].1);
        match quote {
            Some('\'') if char == '\'' => quote = None,
            Some('\'') => {}
            Some(_) if char == '\\' => index += 1,
            Some(open) if char == open => quote = None,
            Some(_) => {}
            None => {
                let op = match char {
                    '\\' => { index += 1; None }
                    '\'' | '"' | '`' => { quote = Some(char); None }
                    '(' => { depth += 1; None }
                    ')' => { depth = depth.saturating_sub(1); None }
                    _ if depth > 0 => None,
                    '<' if next == Some('<') && prev != Some('<') && chars.get(index + 2).map(|(_, after)| *after) != Some('<') => {
                        let (word, strip, taken) = heredoc(&line[chars.get(index + 2).map_or(line.len(), |(at, _)| *at)..]);
                        index += 1 + taken;
                        if !word.is_empty() {
                            pending.push((word, strip));
                        }
                        None
                    }
                    '#' if prev.is_none_or(char::is_whitespace) => {
                        while index + 1 < chars.len() && chars[index + 1].1 != '\n' {
                            index += 1;
                        }
                        None
                    }
                    ';' | '\n' => Some((Op::Seq, 1)),
                    '&' if next == Some('&') => Some((Op::And, 2)),
                    '&' if prev == Some('>') || prev == Some('<') || next == Some('>') => None,
                    '&' => Some((Op::Background, 1)),
                    '|' if next == Some('|') => Some((Op::Or, 2)),
                    '|' if next == Some('&') => Some((Op::Pipe, 2)),
                    '|' => Some((Op::Pipe, 1)),
                    _ => None,
                };
                if let Some((op, len)) = op {
                    cut(&mut out, &line[start..at], Some(op));
                    index += len - 1;
                    start = chars.get(index + 1).map_or(line.len(), |(at, _)| *at);
                    if char == '\n' && !pending.is_empty() {
                        start = after_bodies(line, start, &pending);
                        pending.clear();
                        index = chars.partition_point(|(at, _)| *at < start);
                        continue;
                    }
                }
            }
        }
        index += 1;
    }
    cut(&mut out, &line[start..], None);
    out
}

fn assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else { return false };
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && chars.all(|char| char.is_ascii_alphanumeric() || char == '_')
}

// The end of the leading word, quotes and $(…) counting as part of it.
fn word_end(text: &str) -> usize {
    let (mut quote, mut depth, mut escaped) = (None, 0usize, false);
    for (index, char) in text.char_indices() {
        match (quote, char) {
            _ if escaped => escaped = false,
            (Some('\''), '\'') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => escaped = true,
            (Some(open), _) if char == open => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"' | '`') => quote = Some(char),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, char) if char.is_whitespace() && depth == 0 => return index,
            _ => {}
        }
    }
    text.len()
}

fn split_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    let end = word_end(text);
    (&text[..end], text[end..].trim_start())
}

// The words of options a wrapper takes before the command, where the listed ones take a value of their own.
fn skip_options<'a>(mut rest: &'a str, with_value: &[&str]) -> &'a str {
    loop {
        let (word, after) = split_word(rest);
        if word == "--" {
            return after;
        }
        if !word.starts_with('-') || word.len() < 2 {
            return rest;
        }
        rest = if with_value.contains(&word) { split_word(after).1 } else { after };
    }
}

// The command a simple command runs after assignments and wrappers such as env, timeout, nice, time, nohup, exec and command.
pub fn unwrap(text: &str) -> &str {
    let mut rest = text.trim();
    loop {
        let (word, after) = split_word(rest);
        rest = match word {
            "" => return rest,
            _ if assignment(word) => after,
            "env" => {
                let mut env = skip_options(after, &["-u", "-C", "-S", "--unset", "--chdir"]);
                loop {
                    let (word, after) = split_word(env);
                    if !assignment(word) {
                        break env;
                    }
                    env = after;
                }
            }
            "timeout" => {
                let duration = skip_options(after, &["-s", "-k", "--signal", "--kill-after"]);
                split_word(duration).1
            }
            "nice" => skip_options(after, &["-n", "--adjustment"]),
            "time" => skip_options(after, &[]),
            "nohup" | "exec" => after,
            "command" if !split_word(after).0.starts_with('-') => after,
            _ => return rest,
        };
    }
}

const SETUP: [&str; 16] = ["cd", "pushd", "popd", "export", "set", "unset", "source", ".", "ulimit", "umask", "}", "true", ":", "local", "declare", "shopt"];
const HIDING: [&str; 10] = ["do", "then", "else", "elif", "if", "while", "until", "!", "xargs", "watch"];

#[derive(Clone, Debug, PartialEq)]
pub enum Piece<'a> {
    Setup,
    Simple(&'a str),
    Nested(String),
    Hidden(&'a str, &'a str),
}

fn unquoted(word: &str) -> Option<String> {
    let inner = word.strip_prefix('\'').and_then(|word| word.strip_suffix('\''))
        .or_else(|| word.strip_prefix('"').and_then(|word| word.strip_suffix('"')).filter(|inner| !inner.contains(['"', '\\'])))?;
    (!inner.contains('\'') || word.starts_with('"')).then(|| inner.to_string())
}

// A shell given its script with -c, as in bash -c '…' or sh -ec "…", and the script.
fn shell_script(first: &str, after: &str) -> Option<String> {
    if !["bash", "sh", "zsh", "dash"].contains(&first.rsplit('/').next().unwrap_or(first)) {
        return None;
    }
    let mut rest = after;
    loop {
        let (word, after) = split_word(rest);
        if word.starts_with('-') && word.len() > 1 && !word.starts_with("--") {
            if word.ends_with('c') {
                let (script, _) = split_word(after);
                let script = unquoted(script)?;
                return Some(if word.contains('e') { format!("set -e; {script}") } else { script });
            }
            rest = if word == "-o" { split_word(after).1 } else { after };
            continue;
        }
        return None;
    }
}

pub fn piece(text: &str) -> Piece<'_> {
    let rest = unwrap(text);
    let (first, after) = split_word(rest);
    if first.is_empty() || SETUP.contains(&first) {
        return Piece::Setup;
    }
    if first == "{" {
        return piece(after);
    }
    if HIDING.contains(&first) {
        return Piece::Hidden(first, after);
    }
    if let Some(inner) = rest.strip_prefix('(').and_then(|inner| inner.trim_end().strip_suffix(')')) {
        return Piece::Nested(inner.to_string());
    }
    if let Some(script) = shell_script(first, after) {
        return Piece::Nested(script);
    }
    Piece::Simple(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_splits_into_its_commands_outside_quotes_and_subshells() {
        let line = "cd x; T=$(mktemp -d; echo); TMPDIR=$T script/test 'a;b' \"c && d\" 2>&1 | tail -6 && echo ok || echo no & sleep 1\nrm -rf $T # gone; really";
        assert_eq!(pieces(line), [
            ("cd x", Some(Op::Seq)), ("T=$(mktemp -d; echo)", Some(Op::Seq)), ("TMPDIR=$T script/test 'a;b' \"c && d\" 2>&1", Some(Op::Pipe)), ("tail -6", Some(Op::And)),
            ("echo ok", Some(Op::Or)), ("echo no", Some(Op::Background)), ("sleep 1", Some(Op::Seq)), ("rm -rf $T # gone; really", None),
        ]);
        assert_eq!(pieces("cargo test &> out.txt |& tee log"), [("cargo test &> out.txt", Some(Op::Pipe)), ("tee log", None)]);
        assert_eq!(pieces("echo 'it''s' \\; done"), [("echo 'it''s' \\; done", None)]);
    }

    #[test]
    fn a_heredoc_body_is_text_and_never_a_command_of_the_line() {
        assert_eq!(pieces("set -e\ncat <<EOF > notes.md\ncargo test\nit's done\nEOF\necho ok"), [("set -e", Some(Op::Seq)), ("cat <<EOF > notes.md", Some(Op::Seq)), ("echo ok", None)]);
        assert_eq!(pieces("cat <<-'END' | tee x\n\tcargo test; npm test\n\tEND\ncargo test"), [("cat <<-'END'", Some(Op::Pipe)), ("tee x", Some(Op::Seq)), ("cargo test", None)]);
        assert_eq!(pieces("cat <<\"A\" <<B\ncargo test\nA\nnpm test\nB\nls"), [("cat <<\"A\" <<B", Some(Op::Seq)), ("ls", None)], "two heredocs on one line, bodies in order");
        assert_eq!(pieces("cat <<EOF\ncargo test"), [("cat <<EOF", Some(Op::Seq))], "a body without its terminator runs to the end");
        assert_eq!(pieces("cat <<< 'x'; cargo test"), [("cat <<< 'x'", Some(Op::Seq)), ("cargo test", None)], "a here-string has no body");
    }

    #[test]
    fn wrappers_and_assignments_come_off_before_the_command() {
        for (text, command) in [
            ("timeout 600 bin/rails test", "bin/rails test"), ("timeout -k 5 -s INT 10m cargo test", "cargo test"), ("nice -n 10 cargo test", "cargo test"), ("nice cargo test", "cargo test"),
            ("time -p pytest", "pytest"), ("nohup npm test", "npm test"), ("env -u HOME CI=1 FOO='a b' npm test", "npm test"), ("TMPDIR=$(mktemp -d) script/test", "script/test"),
            ("exec cargo test", "cargo test"), ("RAILS_ENV=test timeout 60 nice -5 bin/rails test", "bin/rails test"), ("command -v cargo", "command -v cargo"),
        ] {
            assert_eq!(unwrap(text), command, "{text}");
        }
        for (text, kind) in [
            ("export X=1", Piece::Setup), ("T=$(mktemp -d)", Piece::Setup), ("cd /src/app", Piece::Setup), ("set -euo pipefail", Piece::Setup),
            ("bash -c 'cd x && script/test'", Piece::Nested("cd x && script/test".to_string())), ("sh -ec \"cargo test\"", Piece::Nested("set -e; cargo test".to_string())),
            ("(cd x; script/test)", Piece::Nested("cd x; script/test".to_string())), ("{ cargo test", Piece::Simple("cargo test")),
            ("do script/test $f", Piece::Hidden("do", "script/test $f")), ("xargs cargo test", Piece::Hidden("xargs", "cargo test")),
        ] {
            assert_eq!(piece(text), kind, "{text}");
        }
    }
}
