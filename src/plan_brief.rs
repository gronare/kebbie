use kebbie_hub_protocol::{MAX_QUESTION_BODY_CHARS, MAX_QUESTION_TITLE_CHARS};

use crate::sh::Result;

pub const SKILL: &str = "kebbie:write-plan-brief";

const NAMED: usize = 3;

const FILE_ENDINGS: [&str; 17] = ["rs", "md", "toml", "json", "yml", "yaml", "ts", "tsx", "py", "rb", "go", "sh", "sql", "lock", "html", "css", "erb"];

// The board's hub shows a control character nowhere but as a line break, so the brief is kept as it shows.
pub fn tidy(text: &str) -> String {
    let plain: String = text.trim().chars().map(|char| if char.is_control() && char != '\n' { ' ' } else { char }).collect();
    plain.lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
}

pub fn title(text: &str) -> &str {
    text.lines().next().unwrap_or_default().trim()
}

pub fn check(text: &str) -> Result<()> {
    let problems = problems(&tidy(text));
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "kebbie ask --plan takes a question for the user, written by the skill `{SKILL}`, and this text is not one yet:\n{}\nWrite one sentence on what the user approves; the plan's details stay in the plan, the paths, shas and commands under its technical context.",
        problems.iter().map(|problem| format!("- {problem}")).collect::<Vec<_>>().join("\n"),
    ))
}

// The first line is all the hub shows as the question, so it alone is held to the user's words; a brief under it is the task's to write.
fn problems(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec!["the text is empty: give the question after the plan, or on stdin with -".to_string()];
    }
    let mut problems = Vec::new();
    let first = title(text);
    let mut lines = text.lines().skip(1);
    let gap = lines.next();
    let body = lines.collect::<Vec<_>>().join("\n");
    let length = first.chars().count();
    if length > MAX_QUESTION_TITLE_CHARS {
        problems.push(format!("the first line has {length} characters and the board shows at most {MAX_QUESTION_TITLE_CHARS}: say what the user approves in one sentence of about 15 words"));
    }
    if gap.is_some_and(|line| !line.trim().is_empty()) {
        problems.push("line 2 is not blank: put a blank line between the first line and the brief".to_string());
    }
    let body_length = body.trim().chars().count();
    if body_length > MAX_QUESTION_BODY_CHARS {
        problems.push(format!("the brief has {body_length} characters and the board shows at most {MAX_QUESTION_BODY_CHARS}: shorten it and leave the details to the plan"));
    }
    if first.contains('`') {
        problems.push(format!("the first line holds backticks ({}): say in plain words what the user approves", listed(&backticked(first))));
    }
    let ids = commit_ids(first);
    if !ids.is_empty() {
        problems.push(format!("the first line names commit ids ({}): say what the change does, and leave the commits to the plan", listed(&ids)));
    }
    let paths = file_paths(first);
    if !paths.is_empty() {
        problems.push(format!("the first line names file paths ({}): name what the user sees or uses instead, and leave the files to the plan", listed(&paths)));
    }
    problems
}

fn listed(found: &[String]) -> String {
    let shown: Vec<&str> = found.iter().take(NAMED).map(String::as_str).collect();
    let more = found.len().saturating_sub(NAMED);
    if more > 0 { format!("{} and {more} more", shown.join(", ")) } else { shown.join(", ") }
}

fn unique(found: impl Iterator<Item = String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for one in found {
        if !kept.contains(&one) {
            kept.push(one);
        }
    }
    kept
}

fn backticked(text: &str) -> Vec<String> {
    let parts: Vec<&str> = text.split('`').collect();
    let closed = parts.iter().enumerate().filter(|(index, _)| index % 2 == 1 && index + 1 < parts.len());
    let spans = unique(closed.map(|(_, span)| span.trim().to_string()).filter(|span| !span.is_empty()));
    if spans.is_empty() { vec!["`".to_string()] } else { spans.into_iter().map(|span| format!("`{span}`")).collect() }
}

// A run of 7 to 40 lowercase hex digits with a letter and a digit in it is a commit id; a plain number or word is neither.
fn commit_ids(text: &str) -> Vec<String> {
    let hex = |word: &&str| (7..=40).contains(&word.len()) && word.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) && word.bytes().any(|byte| byte.is_ascii_digit()) && word.bytes().any(|byte| byte.is_ascii_alphabetic());
    unique(text.split(|char: char| !char.is_alphanumeric()).filter(hex).map(str::to_string))
}

fn file_paths(text: &str) -> Vec<String> {
    let edges: &[char] = &['(', ')', '[', ']', '{', '}', '"', '\'', '«', '»', '”', '“', ',', '.', ';', ':', '!', '?', '*', '_'];
    unique(text.split_whitespace().map(|word| word.trim_matches(edges)).filter(|word| path(word)).map(str::to_string))
}

// A link is a place the user opens, so only a path on disk counts: rooted, relative, or a name with a code file's ending.
fn path(word: &str) -> bool {
    if word.contains("://") {
        return false;
    }
    if ["~/", "./", "../"].iter().any(|start| word.starts_with(start)) {
        return true;
    }
    if let Some(rest) = word.strip_prefix('/') {
        return rest.contains('/') || rest.contains('.');
    }
    let name = word.rsplit('/').next().unwrap_or(word);
    let Some((stem, ending)) = name.rsplit_once('.') else { return false };
    !stem.is_empty() && FILE_ENDINGS.contains(&ending)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BRIEF: &str = "Godkänn att frågor om planer får en kort beslutsbrief överst.\n\n**Vad du får**\n- Frågan säger på svenska vad du godkänner, före alternativen.\n\n**Vad det kostar**\n- Uppgifter som skriver en lång fråga får den tillbaka och skriver om den.\n\n**Vad som inte ingår**\n- Planerna i valvet skrivs inte om.\n\n**Beslut som redan är tagna**\n- Hela planen nås via länken Read the whole plan, som förut.";

    fn refused(text: &str) -> String {
        check(text).expect_err(text)
    }

    #[test]
    fn a_question_is_a_clean_first_line_with_or_without_a_brief_under_it() {
        assert!(check(BRIEF).is_ok(), "{:?}", check(BRIEF));
        assert!(check("  Approve the retry plan.  \n\n- You get retries.\n").is_ok(), "space around the text is not the brief");
        assert!(check("Godkänn att frågor om planer läses som en arbetsplan.").is_ok(), "a plan in its note needs no brief under the first line");
        assert!(check("Approve the plan.\n\n   \n").is_ok(), "blank lines under it are no brief, and none is needed");
        for (text, says) in [("", "the text is empty"), ("Approve the plan.\nYou get retries.", "line 2 is not blank")] {
            let refusal = refused(text);
            assert!(refusal.contains(says) && refusal.contains("`kebbie:write-plan-brief`"), "{text:?}: {refusal}");
        }
    }

    #[test]
    fn a_title_or_a_body_the_hub_cannot_show_is_refused_with_its_length() {
        let long = "x".repeat(MAX_QUESTION_TITLE_CHARS + 1);
        assert!(refused(&format!("{long}\n\n- body")).contains(&format!("the first line has {} characters", MAX_QUESTION_TITLE_CHARS + 1)));
        assert!(check(&format!("{}\n\n- body", "x".repeat(MAX_QUESTION_TITLE_CHARS))).is_ok());
        let body = "word ".repeat(MAX_QUESTION_BODY_CHARS / 5 + 1);
        assert!(refused(&format!("Approve it.\n\n{body}")).contains("the brief has"));
    }

    #[test]
    fn commit_ids_paths_and_backticks_in_the_first_line_are_named_in_the_refusal() {
        let refusal = refused("Approve the split of f33621d in src/hub/board.rs with `cargo test`.");
        for named in ["the first line names commit ids (f33621d)", "the first line names file paths (src/hub/board.rs)", "the first line holds backticks (`cargo test`)"] {
            assert!(refusal.contains(named), "{named}: {refusal}");
        }
        assert!(refused("Approve 6a1225a9 and ~/notes and board.rs.").contains("~/notes"));
        assert!(refused("Approve one ` alone").contains("backticks (`)"));
        assert!(refused("Approve /etc/caddy/kebbie.caddy").contains("/etc/caddy/kebbie.caddy"));
        assert!(check("Approve the retry plan.\n\n- It moves src/hub/board.rs and runs `cargo test` at f33621d.").is_ok(), "the brief under the first line is the task's to write");
    }

    #[test]
    fn plain_words_numbers_links_and_dates_in_prose_pass() {
        for fine in ["2026", "1234567", "deadbeef", "facade", "och/eller", "24/7", "t.ex.", "Node.js", "https://example.com/mock/plan.md", "Read the whole plan", "bl.a.", "v0.39.1", "1/2"] {
            assert!(check(&format!("Approve {fine} here.")).is_ok(), "{fine}: {:?}", check(&format!("Approve {fine} here.")));
        }
    }

    #[test]
    fn the_title_is_the_first_line_as_the_hub_splits_it() {
        let text = tidy(&format!("  {BRIEF}\t\n"));
        let (shown, body) = crate::hub::board::title_and_body(&text, None);
        assert_eq!(shown.as_deref(), Some(title(&text)));
        assert_eq!(shown.as_deref(), Some("Godkänn att frågor om planer får en kort beslutsbrief överst."));
        assert!(body.is_some_and(|body| body.starts_with("**Vad du får**\n- Frågan") && body.ends_with("som förut.")));
        assert_eq!(tidy("Title\t\r\n\r\nBody"), "Title\n\nBody", "a tab or a carriage return would make the hub drop the title");
    }
}
