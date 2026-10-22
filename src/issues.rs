use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::permit::Caller;
use crate::sh::Result;

pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
pub const KINDS: [&str; 2] = ["bug", "wish"];
pub const CLOSED: [&str; 3] = ["fixed", "wontfix", "duplicate"];
const MARKER: &str = "kelpie-issue:";
// GitHub answers 414 to a longer new-issue URL.
pub const URL_LIMIT: usize = 7000;
const SUBJECT_LIMIT: usize = 80;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Issue {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub version: String,
    pub commit: String,
    pub filed: String,
    pub by: String,
    pub github: Option<String>,
    pub sent: Option<String>,
    pub body: String,
}

impl Issue {
    pub fn is_open(&self) -> bool {
        !CLOSED.contains(&self.status.as_str())
    }

    pub fn built(&self) -> String {
        if self.commit.is_empty() { self.version.clone() } else { format!("{} ({})", self.version, self.commit) }
    }

    pub fn link_label(&self) -> Option<String> {
        self.github.as_deref().map(|url| github_number(url).map_or_else(|| url.to_string(), |number| format!("#{number}")))
    }

    fn to_json(&self) -> Value {
        json!({ "id": self.id, "title": self.title, "kind": self.kind, "status": self.status, "version": self.version, "commit": self.commit, "filed": self.filed, "by": self.by, "github": self.github, "sent": self.sent, "body": self.body })
    }

    fn text(&self) -> String {
        let quoted = crate::routine_file::quoted;
        let mut lines = vec!["+++".to_string()];
        for (key, value) in [("id", &self.id), ("title", &self.title), ("kind", &self.kind), ("status", &self.status), ("version", &self.version), ("commit", &self.commit), ("filed", &self.filed), ("by", &self.by)] {
            lines.push(format!("{key} = {}", quoted(value)));
        }
        for (key, value) in [("github", &self.github), ("sent", &self.sent)] {
            if let Some(value) = value {
                lines.push(format!("{key} = {}", quoted(value)));
            }
        }
        lines.push("+++".to_string());
        format!("{}\n\n{}\n", lines.join("\n"), self.body.trim_end())
    }
}

pub fn parse(text: &str) -> Option<Issue> {
    let rest = text.strip_prefix("+++\n")?;
    let (front, body) = rest.split_once("\n+++\n").or_else(|| rest.strip_suffix("\n+++").map(|front| (front, "")))?;
    let table: toml::Table = toml::from_str(front).ok()?;
    let field = |key: &str| table.get(key).and_then(|value| value.as_str()).map(str::to_string);
    Some(Issue {
        id: field("id").filter(|id| !id.is_empty())?,
        title: field("title").unwrap_or_default(),
        kind: field("kind").unwrap_or_else(|| "bug".to_string()),
        status: field("status").unwrap_or_else(|| "open".to_string()),
        version: field("version").unwrap_or_default(),
        commit: field("commit").unwrap_or_default(),
        filed: field("filed").unwrap_or_default(),
        by: field("by").unwrap_or_default(),
        github: field("github"),
        sent: field("sent"),
        body: body.trim().to_string(),
    })
}

pub fn dir(state: &Path) -> PathBuf {
    state.join("issues")
}

fn file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.md"))
}

pub fn load(dir: &Path) -> Vec<Issue> {
    let mut issues: Vec<Issue> = fs::read_dir(dir).into_iter().flatten().flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "md"))
        .filter_map(|entry| fs::read_to_string(entry.path()).ok().and_then(|text| parse(&text)))
        .collect();
    issues.sort_by(|one, other| (one.filed.as_str(), one.id.as_str()).cmp(&(other.filed.as_str(), other.id.as_str())));
    issues
}

fn find(dir: &Path, id: &str) -> Result<Issue> {
    let id = id.trim();
    if id.is_empty() || !id.chars().all(|character| character.is_ascii_alphanumeric()) {
        return Err(format!("\"{id}\" is not an issue id; kelpie issues --all lists them"));
    }
    fs::read_to_string(file(dir, id)).ok().and_then(|text| parse(&text)).ok_or_else(|| format!("No kelpie issue {id}; kelpie issues --all lists them"))
}

fn save(dir: &Path, issue: &Issue) -> Result<()> {
    crate::state::write(&file(dir, &issue.id), issue.text())
}

fn may_write(caller: &Caller, what: &str) -> Result<String> {
    match caller {
        Caller::User => Ok("user".to_string()),
        Caller::Lead => Ok("lead".to_string()),
        other => Err(format!("{what} is the user's or the lead's; {} cannot. Tell the lead what kelpie did and what you expected, with kelpie tell lead \"<text>\", and it proposes the issue to the user", crate::permit::describe(other))),
    }
}

fn new_id(dir: &Path, title: &str, now: u64) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.subsec_nanos()).unwrap_or_default();
    (0u32..).map(|attempt| sha1_smol::Sha1::from(format!("{title}\n{now}\n{nanos}\n{}\n{attempt}", std::process::id())).digest().to_string()[..6].to_string())
        .find(|id| !file(dir, id).exists()).unwrap_or_default()
}

pub fn today(now: u64) -> String {
    crate::keeper::stamp(now)[..10].to_string()
}

pub struct Filing<'a> {
    pub kind: Option<&'a str>,
    pub title: Option<&'a str>,
    pub body: &'a str,
}

pub fn add_in(dir: &Path, caller: &Caller, filing: &Filing, built: &str, now: u64) -> Result<String> {
    let by = may_write(caller, "Filing a kelpie issue")?;
    let kind = filing.kind.map(str::trim).filter(|kind| KINDS.contains(kind)).ok_or("kelpie issue add needs --kind bug or --kind wish")?;
    let title = filing.title.map(|title| title.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|title| !title.is_empty()).ok_or("kelpie issue add needs --title \"<what kelpie did or lacks>\"")?;
    let (commit, _) = built.split_once(' ').unwrap_or((built, ""));
    let issue = Issue {
        id: new_id(dir, &title, now),
        title,
        kind: kind.to_string(),
        status: "open".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: commit.to_string(),
        filed: today(now),
        by,
        github: None,
        sent: None,
        body: filing.body.trim().to_string(),
    };
    save(dir, &issue)?;
    Ok(format!("Filed kelpie issue {} ({}): {} · kelpie issue show {0} shows it", issue.id, issue.kind, issue.title))
}

pub fn list_in(dir: &Path, all: bool, as_json: bool) -> String {
    let issues: Vec<Issue> = load(dir).into_iter().filter(|issue| all || issue.is_open()).collect();
    if as_json {
        return serde_json::to_string_pretty(&issues.iter().map(Issue::to_json).collect::<Vec<_>>()).unwrap_or_default();
    }
    if issues.is_empty() {
        return if all { "No kelpie issues on this machine.".to_string() } else { "No open kelpie issues; --all lists the closed ones too.".to_string() };
    }
    issues.iter().map(|issue| {
        let link = issue.link_label().map(|label| format!("  {label}")).unwrap_or_default();
        format!("{}  {:<4}  {:<9}  {}  {}{link}", issue.id, issue.kind, issue.status, issue.built(), issue.title)
    }).collect::<Vec<_>>().join("\n")
}

pub fn show_in(dir: &Path, id: &str) -> Result<String> {
    let issue = find(dir, id)?;
    let mut lines = vec![
        format!("{}  {}", issue.id, issue.title),
        format!("{} · {} · filed {} by {} on kelpie {}", issue.kind, issue.status, issue.filed, issue.by, issue.built()),
    ];
    lines.extend(issue.github.as_ref().map(|url| format!("GitHub: {url}")));
    lines.extend(issue.sent.as_ref().map(|date| format!("sent {date}")));
    if !issue.body.is_empty() {
        lines.push(String::new());
        lines.push(issue.body.clone());
    }
    Ok(lines.join("\n"))
}

pub fn close_in(dir: &Path, caller: &Caller, id: &str, status: Option<&str>) -> Result<String> {
    may_write(caller, "Closing a kelpie issue")?;
    let status = status.filter(|status| CLOSED.contains(status)).ok_or("kelpie issue close needs the id and fixed, wontfix or duplicate")?;
    let mut issue = find(dir, id)?;
    issue.status = status.to_string();
    save(dir, &issue)?;
    Ok(format!("kelpie issue {} is {status}.", issue.id))
}

pub fn retry_in(dir: &Path, caller: &Caller, id: &str) -> Result<String> {
    may_write(caller, "Marking a kelpie issue worth retrying")?;
    let mut issue = find(dir, id)?;
    issue.status = "retry".to_string();
    save(dir, &issue)?;
    Ok(format!("kelpie issue {} is worth retrying: try it again, then kelpie issue close {0} fixed, or leave it open.", issue.id))
}

pub fn github_number(url: &str) -> Option<u64> {
    url.rsplit_once("/issues/").and_then(|(_, number)| number.trim_end_matches('/').parse().ok())
}

fn issue_url(target: &str) -> Option<String> {
    let target = target.trim();
    if let Ok(number) = target.trim_start_matches('#').parse::<u64>() {
        return Some(format!("{REPOSITORY}/issues/{number}"));
    }
    (target.starts_with("https://github.com/") && github_number(target).is_some()).then(|| target.trim_end_matches('/').to_string())
}

pub fn link_in(dir: &Path, caller: &Caller, id: &str, target: Option<&str>) -> Result<String> {
    may_write(caller, "Linking a kelpie issue")?;
    let url = target.and_then(issue_url).ok_or_else(|| format!("kelpie issue link needs the id and a GitHub issue number or URL, such as 12 or {REPOSITORY}/issues/12"))?;
    let mut issue = find(dir, id)?;
    issue.github = Some(url.clone());
    save(dir, &issue)?;
    Ok(format!("kelpie issue {} is linked to {url}.", issue.id))
}

fn encode(text: &str) -> String {
    text.bytes().map(|byte| match byte {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (byte as char).to_string(),
        other => format!("%{other:02X}"),
    }).collect()
}

fn github_body(issue: &Issue, text: &str, os: &str) -> String {
    format!("**Kind:** {}\n**kelpie:** {}\n**OS:** {os}\n\n{text}\n\n{MARKER} {}", issue.kind, issue.built(), issue.id)
}

fn new_issue_url(title: &str, body: &str) -> String {
    format!("{REPOSITORY}/issues/new?title={}&body={}", encode(title), encode(body))
}

// The text is cut by encoded length, so the URL stays under the limit whatever characters it holds.
pub fn send_url(issue: &Issue, os: &str) -> String {
    let title: String = issue.title.chars().take(200).collect();
    let whole = new_issue_url(&title, &github_body(issue, &issue.body, os));
    if whole.len() <= URL_LIMIT {
        return whole;
    }
    let note = format!("\n\n(shortened; full text in kelpie issue show {})", issue.id);
    let budget = URL_LIMIT.saturating_sub(new_issue_url(&title, &github_body(issue, &note, os)).len());
    let mut used = 0;
    let kept: String = issue.body.chars().take_while(|character| {
        used += encode(&character.to_string()).len();
        used <= budget
    }).collect();
    new_issue_url(&title, &github_body(issue, &format!("{}{note}", kept.trim_end()), os))
}

pub fn os() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

pub fn send_in(dir: &Path, caller: &Caller, id: &str, opener: Option<&str>, open: &dyn Fn(&str, &str) -> Result<()>, now: u64) -> Result<String> {
    if *caller != Caller::User {
        return Err("Sending a kelpie issue to GitHub is the user's step; it stays logged here".to_string());
    }
    let mut issue = find(dir, id)?;
    let os = os();
    let url = send_url(&issue, &os);
    let opened = match opener {
        Some(program) => match open(program, &url) {
            Ok(()) => "Opened in your browser: read it there, then press Submit yourself.".to_string(),
            Err(error) => format!("Could not open a browser ({error}); open the URL above and press Submit yourself."),
        },
        None => "Open the URL above in a browser and press Submit yourself.".to_string(),
    };
    issue.sent = Some(today(now));
    save(dir, &issue)?;
    Ok(format!(
        "Title: {}\n\n{}\n\nThe kelpie repo is public: check the text for project names, paths and secrets before you submit.\n{url}\n{opened}\nOnce it is on GitHub, kelpie update finds it by its kelpie-issue line, or kelpie issue link {} <number> ties it now.",
        issue.title, github_body(&issue, &issue.body, &os), issue.id,
    ))
}

pub fn open_url(program: &str, url: &str) -> Result<()> {
    crate::sh::run(program, &[url], None).map(|_| ())
}

fn repo_slug() -> &'static str {
    REPOSITORY.trim_start_matches("https://github.com/")
}

// gh missing or logged out skips the lookup; a search that finds the marker links the issue, and a closed GitHub issue makes it worth retrying.
pub fn sync_github(dir: &Path, gh: &str) -> Vec<String> {
    if !matches!(crate::sh::attempt(gh, &["auth", "status", "--hostname", "github.com"], None), Ok(Ok(_))) {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for mut issue in load(dir).into_iter().filter(Issue::is_open) {
        let found = match issue.github.as_deref().and_then(github_number) {
            Some(number) => crate::sh::run(gh, &["api", &format!("repos/{}/issues/{number}", repo_slug())], None).ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .map(|item| (issue.github.clone().unwrap_or_default(), item["state"].as_str().unwrap_or_default().to_string())),
            None => {
                let query = format!("\"{MARKER} {}\" repo:{}", issue.id, repo_slug());
                crate::sh::run(gh, &["api", "-X", "GET", "search/issues", "-f", &format!("q={query}")], None).ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .and_then(|found| found["items"].as_array().and_then(|items| items.first().cloned()))
                    .and_then(|item| Some((item["html_url"].as_str()?.to_string(), item["state"].as_str().unwrap_or_default().to_string())))
            }
        };
        let Some((url, state)) = found else { continue };
        let mut changed = Vec::new();
        if issue.github.as_deref() != Some(url.as_str()) {
            issue.github = Some(url.clone());
            changed.push(format!("linked to {url}"));
        }
        if state == "closed" && issue.status == "open" {
            issue.status = "retry".to_string();
            changed.push("closed on GitHub, so worth retrying".to_string());
        }
        if !changed.is_empty() && save(dir, &issue).is_ok() {
            lines.push(format!("{}: {}", issue.id, changed.join("; ")));
        }
    }
    lines
}

fn known_commit(root: &Path, commit: &str) -> bool {
    !commit.is_empty() && crate::sh::succeeds("git", &["cat-file", "-e", &format!("{commit}^{{commit}}")], Some(root))
}

// Filed on older code: a commit the new one descends from, or one this install's history cannot place.
pub fn filed_before<'a>(issues: &'a [Issue], root: Option<&Path>, new: &str) -> Vec<&'a Issue> {
    issues.iter().filter(|issue| issue.is_open() && issue.commit != new && match root {
        Some(root) if known_commit(root, &issue.commit) => crate::sh::succeeds("git", &["merge-base", "--is-ancestor", &issue.commit, new], Some(root))
            && crate::sh::run("git", &["rev-list", "--count", &format!("{}..{new}", issue.commit)], Some(root)).is_ok_and(|count| count != "0"),
        _ => true,
    }).collect()
}

pub fn subjects_since(root: &Path, older: &[&Issue], new: &str) -> Vec<String> {
    older.iter().filter(|issue| known_commit(root, &issue.commit))
        .filter_map(|issue| crate::sh::run("git", &["log", "--format=%h %s", &format!("{}..{new}", issue.commit)], Some(root)).ok())
        .map(|text| text.lines().map(str::to_string).collect::<Vec<_>>())
        .max_by_key(Vec::len).unwrap_or_default()
}

pub fn lead_text(older: &[&Issue], new: &str, subjects: &[String]) -> String {
    let issues = older.iter().map(|issue| format!("- {} [{}, {}] {} (filed {} on kelpie {})", issue.id, issue.kind, issue.status, issue.title, issue.filed, issue.built())).collect::<Vec<_>>().join("\n");
    let commits = if subjects.is_empty() {
        "kelpie could not list the commits in between from this install's history.".to_string()
    } else {
        let more = subjects.len().saturating_sub(SUBJECT_LIMIT);
        let tail = if more > 0 { format!("\n… and {more} older") } else { String::new() };
        format!("The commits in between, newest first:\n{}{tail}", subjects.iter().take(SUBJECT_LIMIT).map(|subject| format!("- {subject}")).collect::<Vec<_>>().join("\n"))
    };
    format!(
        "kelpie was updated to {new}. These open kelpie issues were filed on older code:\n{issues}\n\n{commits}\n\nJudge from the subjects which issues are likely fixed (kelpie issue show <id> has each one's text) and tell the user which are worth retrying. On their word, mark one with kelpie issue retry <id>, and close one they confirm with kelpie issue close <id> fixed."
    )
}

pub struct CrossCheck {
    pub summary: Option<String>,
    pub lead: Option<String>,
}

pub fn cross_check(state: &Path, root: Option<&Path>, new: &str, gh: &str) -> CrossCheck {
    let dir = dir(state);
    if !load(&dir).iter().any(Issue::is_open) {
        return CrossCheck { summary: None, lead: None };
    }
    let synced = sync_github(&dir, gh);
    let issues: Vec<Issue> = load(&dir).into_iter().filter(Issue::is_open).collect();
    let older = filed_before(&issues, root, new);
    let mut parts = vec![format!("{} open", issues.len())];
    if !synced.is_empty() {
        parts.push(synced.join(", "));
    }
    let lead = (!older.is_empty()).then(|| {
        parts.push(format!("{} filed on older code, so the lead was asked which are likely fixed", older.len()));
        lead_text(&older, new, &root.map(|root| subjects_since(root, &older, new)).unwrap_or_default())
    });
    CrossCheck { summary: Some(format!("kelpie issues: {}", parts.join("; "))), lead }
}

// The install root's HEAD is the code just built, also when an older binary finishes the update.
pub fn after_update(state: &Path, root: Option<&Path>) -> Vec<String> {
    let head = root.and_then(|root| crate::sh::run("git", &["rev-parse", "--short=7", "HEAD"], Some(root)).ok());
    let built = env!("KELPIE_BUILT_FROM");
    let new = head.unwrap_or_else(|| built.split(' ').next().unwrap_or_default().to_string());
    let check = cross_check(state, root, &new, &crate::ci::gh());
    let told = check.lead.map(|text| crate::messages::tell_lead(&text, Some("issues")));
    let mut lines: Vec<String> = check.summary.into_iter().collect();
    if let Some(Err(error)) = told {
        lines.push(format!("kelpie issues: the lead could not be told ({error})"));
    }
    lines
}

pub fn read_body(arguments: &[String], from_stdin: bool) -> String {
    if from_stdin {
        let mut input = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
        return input;
    }
    arguments.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filing<'a>(title: &'a str, body: &'a str) -> Filing<'a> {
        Filing { kind: Some("bug"), title: Some(title), body }
    }

    const NOW: u64 = 1_792_540_800;

    #[test]
    fn an_issue_is_one_file_that_reads_back_as_written() {
        let dir = crate::sh::test_dir("issues-file");
        let added = add_in(&dir, &Caller::Lead, &filing("kelpie push hid \"git\"'s error", "What happened: nothing.\n\nExpected: the error."), "869bc49 2026-10-21", NOW).unwrap();
        let issues = load(&dir);
        assert_eq!(issues.len(), 1);
        let issue = &issues[0];
        assert_eq!(added, format!("Filed kelpie issue {} (bug): kelpie push hid \"git\"'s error · kelpie issue show {0} shows it", issue.id));
        assert_eq!(issue.id.len(), 6);
        assert_eq!((issue.title.as_str(), issue.kind.as_str(), issue.status.as_str(), issue.commit.as_str(), issue.by.as_str()), ("kelpie push hid \"git\"'s error", "bug", "open", "869bc49", "lead"));
        assert_eq!(issue.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(issue.filed, today(NOW));
        assert_eq!(issue.body, "What happened: nothing.\n\nExpected: the error.");
        let text = fs::read_to_string(dir.join(format!("{}.md", issue.id))).unwrap();
        assert!(text.starts_with("+++\nid = ") && text.contains("\n+++\n\nWhat happened"), "{text}");
        assert_eq!(parse(&issue.text()).as_ref(), Some(issue));
    }

    #[test]
    fn only_the_user_and_the_lead_file_and_change_issues() {
        let dir = crate::sh::test_dir("issues-who");
        for caller in [Caller::Task("billing@app".to_string()), Caller::Agent] {
            let refused = add_in(&dir, &caller, &filing("t", ""), "", NOW).unwrap_err();
            assert!(refused.contains("the user's or the lead's") && refused.contains("kelpie tell lead"), "{refused}");
        }
        assert!(load(&dir).is_empty());
        add_in(&dir, &Caller::User, &filing("mine", ""), "", NOW).unwrap();
        let issue = load(&dir).remove(0);
        assert_eq!(issue.by, "user");
        assert!(issue.commit.is_empty(), "a build outside git files no commit");
        assert!(close_in(&dir, &Caller::Task("a@b".to_string()), &issue.id, Some("fixed")).is_err());
        assert!(add_in(&dir, &Caller::User, &Filing { kind: Some("idea"), title: Some("x"), body: "" }, "", NOW).unwrap_err().contains("--kind"));
        assert!(add_in(&dir, &Caller::User, &Filing { kind: Some("wish"), title: Some("  "), body: "" }, "", NOW).unwrap_err().contains("--title"));
    }

    #[test]
    fn issues_list_open_by_default_and_every_status_with_all() {
        let dir = crate::sh::test_dir("issues-list");
        let user = Caller::User;
        add_in(&dir, &user, &filing("first", ""), "aaaaaaa 2026-10-01", NOW).unwrap();
        add_in(&dir, &user, &Filing { kind: Some("wish"), title: Some("second"), body: "" }, "aaaaaaa 2026-10-01", NOW + 1).unwrap();
        let ids: Vec<String> = load(&dir).iter().map(|issue| issue.id.clone()).collect();
        let first = load(&dir).into_iter().find(|issue| issue.title == "first").unwrap().id;
        let second = ids.iter().find(|id| **id != first).unwrap().clone();
        close_in(&dir, &user, &first, Some("wontfix")).unwrap();
        assert!(close_in(&dir, &user, &first, Some("done")).unwrap_err().contains("fixed, wontfix or duplicate"));
        retry_in(&dir, &user, &second).unwrap();
        link_in(&dir, &user, &second, Some("#12")).unwrap();
        let open = list_in(&dir, false, false);
        assert!(!open.contains("first") && open.contains("second") && open.contains("retry") && open.contains("#12") && open.contains("(aaaaaaa)"), "{open}");
        assert!(list_in(&dir, true, false).contains("wontfix"));
        let listed: Value = serde_json::from_str(&list_in(&dir, true, true)).unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 2);
        let linked = listed.as_array().unwrap().iter().find(|issue| issue["id"] == second.as_str()).unwrap();
        assert_eq!(linked["github"], format!("{REPOSITORY}/issues/12"));
        link_in(&dir, &user, &second, Some("https://github.com/gronare/kelpie/issues/40/")).unwrap();
        assert!(show_in(&dir, &second).unwrap().contains("GitHub: https://github.com/gronare/kelpie/issues/40"));
        assert!(link_in(&dir, &user, &second, Some("soon")).is_err());
        assert!(show_in(&dir, "../etc").unwrap_err().contains("not an issue id"));
        assert!(show_in(&dir, "abcdef").unwrap_err().contains("No kelpie issue"));
        assert_eq!(list_in(&crate::sh::test_dir("issues-none"), false, false), "No open kelpie issues; --all lists the closed ones too.");
    }

    fn issue(id: &str, commit: &str, body: &str) -> Issue {
        Issue { id: id.to_string(), title: "push hid git's error & more".to_string(), kind: "bug".to_string(), status: "open".to_string(), version: "0.24.0".to_string(), commit: commit.to_string(), filed: "2026-10-21".to_string(), by: "lead".to_string(), github: None, sent: None, body: body.to_string() }
    }

    fn decoded(component: &str) -> String {
        let bytes = component.as_bytes();
        let mut out = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                out.push(u8::from_str_radix(&component[index + 1..index + 3], 16).unwrap());
                index += 3;
            } else {
                out.push(bytes[index]);
                index += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn the_send_url_is_encoded_and_carries_the_version_os_and_marker() {
        let url = send_url(&issue("a1b2c3", "869bc49", "Ran kelpie push.\nGot: nothing & no error?\nå"), "macos aarch64");
        assert!(url.starts_with("https://github.com/gronare/kelpie/issues/new?title=push%20hid%20git%27s%20error%20%26%20more&body="), "{url}");
        let (_, body) = url.split_once("&body=").unwrap();
        assert!(!body.contains(['&', ' ', '\n', '?', '#']), "{body}");
        let body = decoded(body);
        assert!(body.contains("**Kind:** bug") && body.contains("**kelpie:** 0.24.0 (869bc49)") && body.contains("**OS:** macos aarch64"), "{body}");
        assert!(body.contains("Got: nothing & no error?\nå") && body.ends_with("\n\nkelpie-issue: a1b2c3"), "{body}");
    }

    #[test]
    fn an_overlong_text_is_cut_to_keep_the_url_under_githubs_limit() {
        let long = "ä & ".repeat(4000);
        let url = send_url(&issue("a1b2c3", "869bc49", &long), "linux x86_64");
        assert!(url.len() <= URL_LIMIT && url.len() > URL_LIMIT - 40, "{}", url.len());
        let body = decoded(url.split_once("&body=").unwrap().1);
        assert!(body.contains("(shortened; full text in kelpie issue show a1b2c3)") && body.ends_with("kelpie-issue: a1b2c3"), "{}", &body[body.len() - 120..]);
        assert!(!send_url(&issue("a1b2c3", "", "short"), "linux").contains("shortened"));
    }

    #[test]
    fn sending_prints_the_text_and_url_opens_it_and_marks_the_issue_sent() {
        let dir = crate::sh::test_dir("issues-send");
        add_in(&dir, &Caller::Lead, &filing("hid an error", "the text"), "869bc49 2026-10-21", NOW).unwrap();
        let id = load(&dir)[0].id.clone();
        let opened = std::cell::RefCell::new(Vec::new());
        let open = |program: &str, url: &str| { opened.borrow_mut().push((program.to_string(), url.to_string())); Ok(()) };
        let printed = send_in(&dir, &Caller::User, &id, Some("open"), &open, NOW).unwrap();
        let url = send_url(&load(&dir)[0], &os());
        assert!(printed.contains("the text") && printed.contains(&url) && printed.contains("public") && printed.contains("press Submit yourself"), "{printed}");
        assert_eq!(opened.borrow().as_slice(), [("open".to_string(), url)]);
        assert_eq!(load(&dir)[0].sent.as_deref(), Some(today(NOW).as_str()));
        assert!(!fs::read_to_string(dir.join(format!("{id}.md"))).unwrap().is_empty() && load(&dir)[0].is_open(), "sending leaves the issue logged here");
        let unopened = send_in(&dir, &Caller::User, &id, None, &|_: &str, _: &str| panic!("no opener, no browser"), NOW).unwrap();
        assert!(unopened.contains("Open the URL above"), "{unopened}");
        let before = load(&dir);
        for caller in [Caller::Lead, Caller::Task("a@b".to_string())] {
            let refused = send_in(&dir, &caller, &id, Some("open"), &|_: &str, _: &str| panic!("nothing opens for {caller:?}"), NOW + 86_400).unwrap_err();
            assert_eq!(refused, "Sending a kelpie issue to GitHub is the user's step; it stays logged here");
        }
        assert_eq!(load(&dir), before, "a refused send leaves the issue as it was");
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let mut full = vec!["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"];
        full.extend_from_slice(args);
        crate::sh::run("git", &full, Some(dir)).unwrap()
    }

    fn history() -> (PathBuf, Vec<String>) {
        let root = crate::sh::test_dir("issues-git");
        git(&root, &["init", "-q", "-b", "main"]);
        let shas = ["first", "fix the push error", "third"].iter().map(|subject| {
            git(&root, &["commit", "-q", "--allow-empty", "-m", subject]);
            git(&root, &["rev-parse", "--short=7", "HEAD"])
        }).collect();
        (root, shas)
    }

    fn fake_gh(dir: &Path, script: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("gh");
        fs::write(&bin, format!("#!/bin/sh\necho \"$*\" >> '{}/gh.log'\n{script}", dir.display())).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        bin.to_string_lossy().to_string()
    }

    #[test]
    fn after_an_update_the_lead_hears_of_the_open_issues_filed_on_older_code() {
        let (root, shas) = history();
        let state = crate::sh::test_dir("issues-update");
        let dir = dir(&state);
        let mut closed = issue("c00001", &shas[0], "");
        closed.status = "fixed".to_string();
        let full = git(&root, &["rev-parse", "HEAD"]);
        for filed in [issue("aaaaaa", &shas[0], ""), issue("bbbbbb", &shas[2], ""), issue("cccccc", "", ""), issue("dddddd", &full, ""), closed] {
            save(&dir, &filed).unwrap();
        }
        let issues = load(&dir);
        let older = filed_before(&issues, Some(&root), &shas[2]);
        assert_eq!(older.iter().map(|issue| issue.id.as_str()).collect::<Vec<_>>(), ["aaaaaa", "cccccc"], "the issues filed on the new commit, however it is spelled, and the closed one are left out; one filed with no commit is kept");
        let check = cross_check(&state, Some(&root), &shas[2], "/no/such/gh-kelpie");
        assert_eq!(check.summary.as_deref(), Some("kelpie issues: 4 open; 2 filed on older code, so the lead was asked which are likely fixed"));
        let lead = check.lead.unwrap();
        assert!(lead.starts_with(&format!("kelpie was updated to {}.", shas[2])), "{lead}");
        assert!(lead.contains(&format!("- aaaaaa [bug, open] push hid git's error & more (filed 2026-10-21 on kelpie 0.24.0 ({}))", shas[0])), "{lead}");
        assert!(lead.contains(&format!("- {} third\n- {} fix the push error", shas[2], shas[1])) && !lead.contains(" first\n"), "{lead}");
        assert!(lead.contains("kelpie issue retry <id>") && !lead.contains("bbbbbb"), "{lead}");
    }

    #[test]
    fn with_no_open_issues_the_update_says_nothing() {
        let state = crate::sh::test_dir("issues-quiet");
        let check = cross_check(&state, None, "abc1234", "/no/such/gh-kelpie");
        assert!(check.summary.is_none() && check.lead.is_none());
    }

    #[test]
    fn gh_missing_or_logged_out_skips_github_quietly() {
        let state = crate::sh::test_dir("issues-no-gh");
        save(&dir(&state), &issue("aaaaaa", "", "")).unwrap();
        assert!(sync_github(&dir(&state), "/no/such/gh-kelpie").is_empty());
        let logged_out = fake_gh(&state, "exit 1\n");
        assert!(sync_github(&dir(&state), &logged_out).is_empty());
        assert_eq!(fs::read_to_string(state.join("gh.log")).unwrap(), "auth status --hostname github.com\n", "nothing past the auth check");
        assert_eq!(load(&dir(&state))[0].status, "open");
    }

    #[test]
    fn gh_finds_an_issue_by_its_marker_and_a_closed_one_is_worth_retrying() {
        let state = crate::sh::test_dir("issues-gh");
        let dir = dir(&state);
        save(&dir, &issue("aaaaaa", "", "")).unwrap();
        let mut linked = issue("bbbbbb", "", "");
        linked.github = Some(format!("{REPOSITORY}/issues/7"));
        save(&dir, &linked).unwrap();
        save(&dir, &issue("cccccc", "", "")).unwrap();
        let gh = fake_gh(&state, r#"case "$*" in
  "auth status"*) exit 0 ;;
  *"kelpie-issue: aaaaaa"*) echo '{"items":[{"number":3,"html_url":"https://github.com/gronare/kelpie/issues/3","state":"open"}]}' ;;
  *"kelpie-issue: cccccc"*) echo '{"items":[]}' ;;
  "api repos/gronare/kelpie/issues/7") echo '{"number":7,"state":"closed"}' ;;
  *) exit 1 ;;
esac
"#);
        let lines = sync_github(&dir, &gh);
        assert_eq!(lines, ["aaaaaa: linked to https://github.com/gronare/kelpie/issues/3", "bbbbbb: closed on GitHub, so worth retrying"]);
        let log = fs::read_to_string(state.join("gh.log")).unwrap();
        assert!(log.contains("api -X GET search/issues -f q=\"kelpie-issue: aaaaaa\" repo:gronare/kelpie"), "{log}");
        let issues = load(&dir);
        assert_eq!(issues.iter().map(|issue| (issue.id.as_str(), issue.status.as_str(), issue.link_label())).collect::<Vec<_>>(), [("aaaaaa", "open", Some("#3".to_string())), ("bbbbbb", "retry", Some("#7".to_string())), ("cccccc", "open", None)]);
    }
}
