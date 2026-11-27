use std::path::Path;

use crate::config::{self, Bump, Settings, CURRENT_PLACE, NEXT_PLACE, TASK_PLACE};
use crate::repo::{git, tracking};
use crate::sh::Result;

const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Branches,
    File { file: String, key: String },
    Command(String),
}

// A repo's push.branch and what shapes it, as its repo config names them.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub branch: String,
    pub choices: Vec<String>,
    pub prepare: Option<String>,
    pub source: Source,
    pub bump: Bump,
}

impl Rule {
    pub fn of(settings: &Settings) -> Option<Rule> {
        let branch = settings.push_branch.value.clone()?;
        let source = match (&settings.push_version_file.value, &settings.push_version_command.value) {
            (Some(file), _) => Source::File { file: file.clone(), key: settings.push_version_key.value.clone().unwrap_or_else(|| "version".to_string()) },
            (None, Some(command)) => Source::Command(command.clone()),
            (None, None) => Source::Branches,
        };
        Some(Rule { branch, choices: settings.push_choices.value.clone(), prepare: settings.push_prepare.value.clone(), source, bump: settings.push_version_bump.value })
    }
}

pub fn rule(repo: &Path) -> Result<Option<Rule>> {
    Ok(Rule::of(&config::resolve(Some(repo))?.settings))
}

// Where one push goes: a branch of the task's own, which goes out unrebased, or a branch tasks share, which each push rebases onto.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub branch: String,
    pub version: Option<String>,
    pub exists: bool,
    pub per_task: bool,
}

// Read from origin's refs as last fetched, so a push resolves it after its fetch.
pub fn resolve(repo: &Path, upstream: &str, rule: &Rule, template: &str, task: &str) -> Result<Target> {
    let named = template.replace(TASK_PLACE, task);
    let version = if named.contains(NEXT_PLACE) {
        Some(next(repo, upstream, rule, &named)?)
    } else if named.contains(CURRENT_PLACE) {
        Some(current(repo, &named)?)
    } else {
        None
    };
    let branch = config::fill(&named, task, version.as_deref());
    if !config::valid_branch(&branch) {
        return Err(format!("{template:?} makes {branch:?}, which is no branch name git takes"));
    }
    let exists = crate::sh::succeeds("git", &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{branch}")], Some(repo));
    Ok(Target { branch, version, exists, per_task: template.contains(TASK_PLACE) })
}

fn current(repo: &Path, named: &str) -> Result<String> {
    released(repo, named)?.into_iter().map(|(_, version)| version).max_by(|a, b| order(a, b))
        .ok_or_else(|| format!("no branch on origin matches {named:?}, so there is no current one"))
}

// The open branch, one not yet merged into the default branch, is next until it is; only then does next count up.
fn next(repo: &Path, upstream: &str, rule: &Rule, named: &str) -> Result<String> {
    match &rule.source {
        Source::Command(command) => from_command(repo, command),
        Source::File { file, key } => {
            let text = git(repo, &["show", &format!("{}:{file}", tracking(upstream))]).map_err(|error| format!("cannot read {file} on {upstream}: {error}"))?;
            let version = version_in(&text, key).ok_or_else(|| format!("{file} on {upstream} holds no version under {key:?}"))?;
            Ok(bump(&version, rule.bump))
        }
        Source::Branches => {
            let found = released(repo, named)?;
            let main = tracking(upstream);
            let open = found.iter().filter(|(branch, _)| !crate::sh::succeeds("git", &["merge-base", "--is-ancestor", &format!("refs/remotes/origin/{branch}"), &main], Some(repo)));
            if let Some((_, version)) = open.max_by(|(_, a), (_, b)| order(a, b)) {
                return Ok(version.clone());
            }
            let highest = found.iter().map(|(_, version)| version).max_by(|a, b| order(a, b))
                .ok_or_else(|| format!("no branch on origin matches {named:?} to count the next one from; push the first one yourself, or read the version from a file with push.version.file"))?;
            Ok(bump(highest, rule.bump))
        }
    }
}

fn from_command(repo: &Path, command: &str) -> Result<String> {
    let mut run = std::process::Command::new("sh");
    run.args(["-c", command]).current_dir(repo);
    let ran = crate::sh::output_with_timeout(run, COMMAND_TIMEOUT).map_err(|error| format!("push.version.command {command:?} did not start: {error}"))?
        .ok_or_else(|| format!("push.version.command {command:?} ran past {} seconds", COMMAND_TIMEOUT.as_secs()))?;
    if ran.code != Some(0) {
        return Err(format!("push.version.command {command:?} failed: {}", ran.stderr.trim()));
    }
    let printed = ran.stdout.trim();
    if printed.is_empty() || printed.contains('\n') {
        return Err(format!("push.version.command {command:?} must print one line, the value of {NEXT_PLACE}"));
    }
    Ok(printed.to_string())
}

// Each branch on origin that the template matches, with the version in its place.
fn released(repo: &Path, named: &str) -> Result<Vec<(String, String)>> {
    let place = if named.contains(NEXT_PLACE) { NEXT_PLACE } else { CURRENT_PLACE };
    let (before, after) = named.split_once(place).unwrap_or((named, ""));
    let listed = git(repo, &["for-each-ref", "--format=%(refname:lstrip=3)", "refs/remotes/origin/"])?;
    Ok(listed.lines().filter_map(|branch| {
        let version = branch.strip_prefix(before)?.strip_suffix(after)?;
        parts(version).map(|_| (branch.to_string(), version.to_string()))
    }).collect())
}

fn parts(version: &str) -> Option<Vec<u64>> {
    version.split('.').map(|part| (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())).then(|| part.parse().ok()).flatten()).collect()
}

fn order(a: &str, b: &str) -> std::cmp::Ordering {
    parts(a).cmp(&parts(b))
}

pub fn bump(version: &str, bump: Bump) -> String {
    let mut parts = parts(version).unwrap_or_default();
    parts.resize(bump.place() + 1, 0);
    parts[bump.place()] += 1;
    parts.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

// The first value after the key, as `KEY = 3.4.3;` in an Xcode project or `"version": "3.4.3"` in package.json writes it.
pub fn version_in(text: &str, key: &str) -> Option<String> {
    text.match_indices(key).find_map(|(at, _)| {
        let rest = text[at + key.len()..].trim_start_matches('"').trim_start();
        let rest = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'))?.trim_start().trim_start_matches('"');
        let version: String = rest.chars().take_while(|character| character.is_ascii_digit() || *character == '.').collect();
        let version = version.trim_end_matches('.');
        parts(version).map(|_| version.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(source: Source) -> Rule {
        Rule { branch: "release/{next}".to_string(), choices: Vec::new(), prepare: None, source, bump: Bump::Minor }
    }

    fn origin_with(branches: &[(&str, bool)]) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = crate::sh::test_dir("push-target");
        let origin = dir.join("origin.git");
        let repo = dir.join("app");
        let run = |cwd: &Path, args: &[&str]| crate::sh::run("git", args, Some(cwd)).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        run(&dir, &["init", "--quiet", "--bare", "-b", "main", origin.to_str().unwrap()]);
        run(&repo, &["init", "--quiet", "-b", "main"]);
        let commit = |message: &str| run(&repo, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--allow-empty", "-m", message]);
        std::fs::write(repo.join("package.json"), "{\n  \"name\": \"app\",\n  \"version\": \"3.4.3\"\n}\n").unwrap();
        run(&repo, &["add", "package.json"]);
        commit("first");
        run(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        run(&repo, &["push", "--quiet", "origin", "main"]);
        for (branch, merged) in branches {
            run(&repo, &["checkout", "--quiet", "-b", branch, "main"]);
            commit(branch);
            run(&repo, &["push", "--quiet", "origin", branch]);
            run(&repo, &["checkout", "--quiet", "main"]);
            if *merged {
                run(&repo, &["merge", "--quiet", "--ff-only", branch]);
                run(&repo, &["push", "--quiet", "origin", "main"]);
            }
        }
        run(&repo, &["fetch", "--quiet", "origin"]);
        (dir, repo)
    }

    #[test]
    fn next_is_the_open_release_branch_until_it_is_merged_then_one_more_than_the_highest() {
        let (dir, repo) = origin_with(&[("release/3.4", true), ("release/3.10", true), ("feature/login", false)]);
        let branches = rule(Source::Branches);
        let target = resolve(&repo, "origin/main", &branches, "release/{next}", "billing").unwrap();
        assert_eq!(target, Target { branch: "release/3.11".to_string(), version: Some("3.11".to_string()), exists: false, per_task: false }, "all merged: highest by number, not by text, plus one");
        assert_eq!(resolve(&repo, "origin/main", &branches, "release/{current}", "billing").unwrap().branch, "release/3.10");
        let run = |args: &[&str]| crate::sh::run("git", args, Some(&repo)).unwrap();
        run(&["checkout", "--quiet", "-b", "release/3.11", "main"]);
        run(&["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "--quiet", "--allow-empty", "-m", "bump"]);
        run(&["push", "--quiet", "origin", "release/3.11"]);
        run(&["fetch", "--quiet", "origin"]);
        let open = resolve(&repo, "origin/main", &branches, "release/{next}", "billing").unwrap();
        assert_eq!((open.branch.as_str(), open.exists), ("release/3.11", true), "the open branch stays next, so a second task lands on it");
        assert_eq!(resolve(&repo, "origin/main", &branches, "feature/{task}", "billing").unwrap(), Target { branch: "feature/billing".to_string(), version: None, exists: false, per_task: true });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn next_counts_up_the_version_a_file_on_the_default_branch_holds_or_takes_a_commands_output() {
        let (dir, repo) = origin_with(&[]);
        let file = rule(Source::File { file: "package.json".to_string(), key: "version".to_string() });
        assert_eq!(resolve(&repo, "origin/main", &file, "release/{next}", "billing").unwrap().branch, "release/3.5");
        assert_eq!(resolve(&repo, "origin/main", &Rule { bump: Bump::Patch, ..file.clone() }, "release/{next}", "billing").unwrap().branch, "release/3.4.4");
        let none = resolve(&repo, "origin/main", &rule(Source::Branches), "release/{next}", "billing").unwrap_err();
        assert!(none.contains("no branch on origin matches \"release/{next}\""), "{none}");
        assert!(resolve(&repo, "origin/main", &rule(Source::Branches), "release/{current}", "billing").unwrap_err().contains("no current one"));
        let command = rule(Source::Command("echo release/7.0".to_string()));
        assert_eq!(resolve(&repo, "origin/main", &command, "{next}", "billing").unwrap().branch, "release/7.0", "the command's output is the branch");
        let failing = resolve(&repo, "origin/main", &rule(Source::Command("echo nope >&2; exit 3".to_string())), "{next}", "billing").unwrap_err();
        assert!(failing.contains("failed: nope"), "{failing}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_version_is_read_from_an_xcode_project_or_package_json_and_counted_up_by_its_part() {
        assert_eq!(version_in("\t\t\t\tMARKETING_VERSION = 3.4.3;\n\t\t\t\tMARKETING_VERSION = 3.4.3;", "MARKETING_VERSION").as_deref(), Some("3.4.3"));
        assert_eq!(version_in("{\"versionCode\": 9, \"version\": \"2.1.0\"}", "version").as_deref(), Some("2.1.0"), "a longer key with the same start is passed over");
        assert_eq!(version_in("version = ", "version"), None);
        assert_eq!(bump("3.4.3", Bump::Minor), "3.5");
        assert_eq!(bump("3.4", Bump::Patch), "3.4.1");
        assert_eq!(bump("3.4.3", Bump::Major), "4");
        assert_eq!(bump("3", Bump::Minor), "3.1");
    }
}
