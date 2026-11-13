use std::path::{Path, PathBuf};

use kebbie_hub_protocol::{AcceptStep, BLOB_OVERHEAD, DiffLine, FileStatus, Hunk, LineKind, MAX_BLOB_BYTES, Outcome, Reason, Review, ReviewComment, ReviewComments, ReviewFile, ReviewOffer, ReviewOpen, ReviewRead, ReviewStep, Side};

use super::commands::{Done, plain};
use crate::board::review_data::{self as data, Entry, Mark};
use crate::permit::Caller;

// What a review's files may take together, the step's own text and the JSON around them aside.
const FILES_BUDGET: usize = MAX_BLOB_BYTES - BLOB_OVERHEAD - 16 * 1024;

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn refused(reason: Reason, text: &str) -> Done {
    (Outcome::Refused { reason }, text.to_string())
}

// The step a task's review waits on: its first committed step not yet accepted.
pub fn waiting_step(steps: &crate::tasks::Steps) -> Option<(u32, &str)> {
    steps.items.iter().find_map(|step| match &step.state {
        crate::tasks::StepState::Committed { sha, accepted: false, .. } => Some((count(step.index + 1), sha.as_str())),
        _ => None,
    })
}

// A step's files, read once per commit for the process, since a full sha's tree never changes.
fn files_in(path: &Path, sha: &str) -> Option<u32> {
    let names = crate::repo::git_fixed(path, &["diff-tree", "--no-commit-id", "--name-only", "-r", "--no-renames", "-z", "--root", sha]).ok()?;
    Some(count(names.split('\0').filter(|name| !name.is_empty()).count()))
}

// The board row's offer of a review: the step that waits for the user's acceptance and how many files it touches.
pub fn offer(item: &crate::status::Item) -> Option<ReviewOffer> {
    let (step, sha) = waiting_step(&item.steps)?;
    let files = files_in(item.path.as_ref()?, sha)?;
    Some(ReviewOffer { step: Some(step), files })
}

fn task_path(address: &str) -> std::result::Result<PathBuf, Done> {
    let (task, place) = address.split_once('@').unwrap_or((address, ""));
    let repo = crate::repo::pooled_checkouts().into_iter().find(|repo| crate::repo::name_of(repo) == place).ok_or_else(|| refused(Reason::UnknownRepo, &format!("No repo {place} on this machine")))?;
    crate::tasks::find_task(&repo, Some(task), &repo).map(|slot| slot.path).map_err(|error| refused(Reason::UnknownRepo, &plain(&error)))
}

// The step the split and the phone review: the first committed one not accepted, among the task's steps or else its commits.
fn waiting_in(path: &Path) -> Option<Entry> {
    data::kebbie_marks(path)?.items.into_iter().find(|entry| entry.mark == Mark::Committed && entry.sha.is_some())
}

fn nothing_waits(address: &str) -> Done {
    refused(Reason::AlreadySettled, &format!("Nothing in {address} waits for review now"))
}

fn object(root: &Path, spec: &str) -> Option<String> {
    crate::repo::git(root, &["rev-parse", "--verify", "--quiet", spec]).ok().filter(|id| crate::repo::is_sha(id))
}

// A file's content id as the read marks hold it: its blob in the commit, or its blob before the commit when the commit deletes it.
fn content_of(root: &Path, path: &str, sha: &str) -> Option<(String, FileStatus)> {
    let (old, new) = (object(root, &format!("{sha}^:{path}")), object(root, &format!("{sha}:{path}")));
    match (old, new) {
        (None, Some(new)) => Some((new, FileStatus::Added)),
        (Some(old), None) => Some((old, FileStatus::Deleted)),
        (Some(_), Some(new)) => Some((new, FileStatus::Modified)),
        (None, None) => None,
    }
}

fn start_of(range: &str) -> u32 {
    range.split(',').next().and_then(|start| start.parse().ok()).unwrap_or(0)
}

// A unified diff as hunks whose lines carry their old and new numbers, and whether git called the file binary.
pub fn parse_diff(diff: &str) -> (Vec<Hunk>, bool) {
    let (mut hunks, mut binary): (Vec<Hunk>, bool) = (Vec::new(), false);
    let (mut old, mut new) = (0u32, 0u32);
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("@@ -") {
            let mut ranges = rest.split_whitespace();
            old = ranges.next().map_or(0, start_of);
            new = ranges.next().and_then(|range| range.strip_prefix('+')).map_or(0, start_of);
            hunks.push(Hunk { head: line.to_string(), lines: Vec::new() });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            binary |= line.starts_with("Binary files ") || line == "GIT binary patch";
            continue;
        };
        let (kind, text) = match line.chars().next() {
            Some(' ') => (LineKind::Context, &line[1..]),
            Some('+') => (LineKind::Add, &line[1..]),
            Some('-') => (LineKind::Del, &line[1..]),
            None => (LineKind::Context, ""),
            _ => continue,
        };
        let (at_old, at_new) = match kind {
            LineKind::Context => (Some(old), Some(new)),
            LineKind::Add => (None, Some(new)),
            LineKind::Del => (Some(old), None),
        };
        old += u32::from(at_old.is_some());
        new += u32::from(at_new.is_some());
        hunk.lines.push(DiffLine { kind, old: at_old, new: at_new, text: text.to_string() });
    }
    (hunks, binary)
}

fn review_dir(state: &Path, address: &str) -> Option<PathBuf> {
    let pane = crate::peers::sessions().ok()?.into_iter().find(|session| session.address == address && session.task.is_some())?.pane?;
    Some(state.join("reviews").join(pane.replace([':', '/'], "_")))
}

fn diff_of(root: &Path, path: &str, sha: &str) -> String {
    let argv = data::diff_argv(root, "HEAD", path, Some(sha));
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    crate::sh::attempt("git", &args, Some(root)).ok().map(|answer| answer.unwrap_or_else(|out| out)).unwrap_or_default()
}

// The step's files in the split's order, each with its hunks, a binary file or one past what is left of the budget marked omitted; a path the protocol cannot carry is left out and counted.
fn files_of(root: &Path, dir: &Path, sha: &str) -> (Vec<ReviewFile>, usize) {
    let mut left = FILES_BUDGET;
    let (listed, unnamed): (Vec<_>, Vec<_>) = data::files(dir, root, "HEAD", Some(sha)).into_iter().partition(|listed| kebbie_hub_protocol::is_relative_label(&listed.path));
    let files = listed.into_iter().filter_map(|listed| {
        let (content, status) = content_of(root, &listed.path, sha)?;
        let numbered = listed.added != "-" && listed.deleted != "-";
        let (hunks, binary) = if numbered { parse_diff(&diff_of(root, &listed.path, sha)) } else { (Vec::new(), true) };
        let counted = |kind| count(hunks.iter().flat_map(|hunk| &hunk.lines).filter(|line| line.kind == kind).count());
        let (add, del) = (counted(LineKind::Add), counted(LineKind::Del));
        let file = ReviewFile { path: listed.path.clone(), old_path: None, status, add, del, content, read: listed.read, omitted: false, hunks };
        let size = serde_json::to_vec(&file).map_or(usize::MAX, |bytes| bytes.len());
        if binary || size > left {
            let number = |text: &str| text.parse::<u32>().unwrap_or(0);
            return Some(ReviewFile { omitted: true, hunks: Vec::new(), add: number(&listed.added), del: number(&listed.deleted), ..file });
        }
        left -= size;
        Some(file)
    }).collect();
    (files, unnamed.len())
}

fn left_out(unnamed: usize) -> Option<String> {
    match unnamed {
        0 => None,
        1 => Some("1 file whose name the phone cannot show; review it at the machine".to_string()),
        many => Some(format!("{many} files whose names the phone cannot show; review them at the machine")),
    }
}

pub fn build(root: &Path, address: &str, dir: &Path, step: &Entry) -> std::result::Result<Review, String> {
    let sha = step.sha.clone().ok_or("the step has no commit")?;
    let head = object(root, "HEAD").ok_or("HEAD is not a commit")?;
    let since = crate::repo::default_branch(root).map(|branch| branch.trim_start_matches("origin/").to_string()).unwrap_or_else(|_| "main".to_string());
    let message = crate::repo::git(root, &["log", "-1", "--format=%s%x00%b", &sha])?;
    let (subject, body) = message.split_once('\0').unwrap_or((&message, ""));
    let step = ReviewStep { number: count(step.number), title: step.title.clone(), sha: sha.clone(), subject: subject.trim().to_string(), body: body.trim().to_string() };
    let (files, unnamed) = files_of(root, dir, &sha);
    let review = Review { address: address.to_string(), since, head, step: Some(step), guide: Vec::new(), left_out: left_out(unnamed), files };
    review.check().map_err(str::to_string)?;
    Ok(review)
}

pub fn open(_: &Caller, open: &ReviewOpen) -> std::result::Result<Vec<u8>, Done> {
    let path = task_path(&open.address)?;
    let step = waiting_in(&path).ok_or_else(|| nothing_waits(&open.address))?;
    let state = crate::lead::state_dir().map_err(|error| (Outcome::Failed, error))?;
    let dir = review_dir(&state, &open.address).unwrap_or_else(|| state.join("reviews").join("none"));
    let review = build(&path, &open.address, &dir, &step).map_err(|error| (Outcome::Failed, format!("The review of {} was not made: {error}", open.address)))?;
    serde_json::to_vec(&review).map_err(|error| (Outcome::Failed, error.to_string()))
}

// One line per comment, as the review split types it: `path:line[ (before)][ in <sha7>] — text`.
pub fn comment_line(comment: &ReviewComment) -> String {
    let before = if comment.side == Side::Old { " (before)" } else { "" };
    let within = comment.sha.as_deref().map(|sha| format!(" in {}", data::short(sha))).unwrap_or_default();
    format!("{}:{}{before}{within} — {}", comment.path, comment.line, comment.text.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub fn comments_text(comments: &ReviewComments) -> String {
    comments.comments.iter().map(comment_line).collect::<Vec<_>>().join("\n")
}

// Typed into the task's Claude as one prompt, submitted once, never over a draft of the user's.
pub fn comments_with(comments: &ReviewComments, pane: Option<String>, prompt: impl FnOnce(&str, &str) -> std::result::Result<(), crate::typing::Refused>) -> Done {
    let Some(pane) = pane else { return (Outcome::Failed, format!("Not sent: {} has no Claude running now", comments.address)) };
    let many = comments.comments.len();
    let counted = if many == 1 { "1 comment".to_string() } else { format!("{many} comments") };
    match prompt(&pane, &comments_text(comments)) {
        Ok(()) => (Outcome::Done, format!("Sent {counted} to {}'s Claude", comments.address)),
        Err(crate::typing::Refused::Draft) => (Outcome::Failed, format!("Not sent: {}'s Claude has text in its input box; send them again once it is clear", comments.address)),
        Err(refused) => (Outcome::Failed, format!("Not sent: {}'s Claude {}", comments.address, refused.reason())),
    }
}

pub fn comments(_: &Caller, comments: &ReviewComments) -> Done {
    let pane = crate::peers::sessions().ok().and_then(|sessions| sessions.into_iter().find(|session| session.address == comments.address && session.task.is_some()).and_then(|session| session.pane));
    comments_with(comments, pane, |pane, text| crate::typing::prompt(pane, text, &comments.address))
}

// The read mark the split keeps, set only while the file's content is the one the phone saw.
pub fn read_in(root: &Path, step: Option<&Entry>, read: &ReviewRead) -> Done {
    let Some(sha) = step.and_then(|step| step.sha.as_deref()) else { return nothing_waits(&read.address) };
    match content_of(root, &read.path, sha) {
        Some((content, _)) if content == read.content => match data::set_read(root, &read.path, Some(sha), read.read) {
            Ok(()) => (Outcome::Done, format!("Marked {} {}", read.path, if read.read { "read" } else { "unread" })),
            Err(error) => (Outcome::Failed, error),
        },
        _ => refused(Reason::HeadMoved, "It changed since you saw it; open the review again"),
    }
}

pub fn read(_: &Caller, read: &ReviewRead) -> Done {
    match task_path(&read.address) {
        Ok(path) => read_in(&path, waiting_in(&path).as_ref(), read),
        Err(done) => done,
    }
}

// Accepts step N only while N is still the commit the phone saw, through kebbie accept's own path.
pub fn accept_in(root: &Path, entries: Option<data::Entries>, accept: &AcceptStep, run: impl FnOnce(&Path, &str, &str) -> crate::sh::Result<String>) -> Done {
    let Some(entry) = entries.and_then(|entries| entries.items.into_iter().find(|entry| entry.number == accept.step as usize)) else {
        return refused(Reason::HeadMoved, "That step changed since you saw it; open the review again");
    };
    if entry.sha.as_deref() != Some(accept.sha.as_str()) {
        return refused(Reason::HeadMoved, "That step changed since you saw it; open the review again");
    }
    if entry.mark == Mark::Accepted {
        return refused(Reason::AlreadySettled, &format!("Step {} is accepted already", accept.step));
    }
    let task = accept.address.split_once('@').map_or(accept.address.as_str(), |(task, _)| task);
    match run(root, task, &accept.sha) {
        Ok(_) => (Outcome::Done, format!("Accepted step {} of {}", accept.step, accept.address)),
        Err(error) => (Outcome::Failed, plain(&error)),
    }
}

pub fn accept(caller: &Caller, accept: &AcceptStep) -> Done {
    match task_path(&accept.address) {
        Ok(path) => accept_in(&path, data::kebbie_marks(&path), accept, |root, task, sha| crate::tasks::accept_from_device(caller, root, task, sha)),
        Err(done) => done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::review_data::tests::{commit, repo, sh};

    const SHA: &str = "3f9a2d1c8e7b6a5f4d3c2b1a0f9e8d7c6b5a4f3e";

    #[test]
    fn a_diff_reads_as_hunks_with_each_lines_old_and_new_number() {
        let diff = "diff --git a/src/ci.rs b/src/ci.rs\nindex 1..2 100644\n--- a/src/ci.rs\n+++ b/src/ci.rs\n@@ -10,3 +10,4 @@ fn run() {\n keep\n-old\n+new\n+more\n tail\n\\ No newline at end of file\n@@ -40 +41 @@\n-x\n+y\n";
        let (hunks, binary) = parse_diff(diff);
        assert!(!binary);
        assert_eq!(hunks[0].head, "@@ -10,3 +10,4 @@ fn run() {");
        let numbers: Vec<(LineKind, Option<u32>, Option<u32>, &str)> = hunks[0].lines.iter().map(|line| (line.kind, line.old, line.new, line.text.as_str())).collect();
        assert_eq!(numbers, [(LineKind::Context, Some(10), Some(10), "keep"), (LineKind::Del, Some(11), None, "old"), (LineKind::Add, None, Some(11), "new"), (LineKind::Add, None, Some(12), "more"), (LineKind::Context, Some(12), Some(13), "tail")]);
        assert_eq!((hunks[1].lines[0].old, hunks[1].lines[1].new), (Some(40), Some(41)));
        assert!(parse_diff("diff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n").1);
    }

    #[test]
    fn comments_go_in_as_one_prompt_in_the_splits_own_lines() {
        let comment = |side, sha: Option<&str>, text: &str| ReviewComment { path: "src/ci.rs".to_string(), line: 112, side, sha: sha.map(str::to_string), text: text.to_string() };
        let comments = ReviewComments { address: "retry@kebbie".to_string(), step: Some(1), comments: vec![comment(Side::Old, Some(SHA), "Should each try\nget its own deadline?"), comment(Side::New, None, "Keep it")] };
        assert_eq!(comments_text(&comments), "src/ci.rs:112 (before) in 3f9a2d1 — Should each try get its own deadline?\nsrc/ci.rs:112 — Keep it");
        let typed = std::cell::RefCell::new(Vec::new());
        let (outcome, text) = comments_with(&comments, Some("p_9".to_string()), |pane, text| { typed.borrow_mut().push((pane.to_string(), text.to_string())); Ok(()) });
        assert_eq!((outcome, text.as_str()), (Outcome::Done, "Sent 2 comments to retry@kebbie's Claude"));
        assert_eq!(typed.borrow().len(), 1, "submitted once");
        assert_eq!(comments_with(&comments, Some("p_9".to_string()), |_, _| Err(crate::typing::Refused::Draft)).0, Outcome::Failed, "never over the user's draft");
        assert_eq!(comments_with(&comments, None, |_, _| panic!("no pane")).0, Outcome::Failed);
    }

    #[test]
    fn a_review_holds_the_waiting_step_and_its_files_and_read_marks_and_accepts_only_what_the_phone_saw() {
        let lab = repo("hub-review");
        commit(&lab.slot, "logo.bin", "\0\x01\x02binary", "Add a logo");
        let shas: Vec<String> = sh(&lab.slot, &["log", "--reverse", "--format=%H", "-4"]).lines().map(str::to_string).collect();
        let step = Entry { sha: Some(lab.shas[1].clone()), mark: Mark::Committed, number: 2, title: "Add b".to_string() };
        let review = build(&lab.slot, "billing@app", &lab.dir, &step).unwrap();
        assert_eq!((review.since.as_str(), review.head.as_str()), ("main", shas.last().unwrap().as_str()));
        let shown = review.step.as_ref().unwrap();
        assert_eq!((shown.number, shown.subject.as_str(), shown.body.as_str()), (2, "Add b", "Why b exists.\nIt pays."));
        assert_eq!(review.files.len(), 1);
        let file = &review.files[0];
        assert_eq!((file.path.as_str(), file.status, file.add, file.del, file.omitted), ("b", FileStatus::Added, 1, 0, false));
        assert_eq!(file.hunks[0].lines[0].new, Some(1));
        let binary = Entry { sha: Some(shas.last().unwrap().clone()), ..step.clone() };
        let logo = &build(&lab.slot, "billing@app", &lab.dir, &binary).unwrap().files[0];
        assert!(logo.omitted && logo.hunks.is_empty(), "a binary file is left out of the blob");

        let read = |content: &str| ReviewRead { address: "billing@app".to_string(), path: "b".to_string(), read: true, content: content.to_string() };
        assert_eq!(read_in(&lab.slot, Some(&step), &read(&"a".repeat(40))).0, Outcome::Refused { reason: Reason::HeadMoved });
        assert_eq!(read_in(&lab.slot, Some(&step), &read(&file.content)).0, Outcome::Done);
        assert!(data::is_read(&lab.slot, "b", Some(&lab.shas[1])), "the mark the split reads");
        assert!(build(&lab.slot, "billing@app", &lab.dir, &step).unwrap().files[0].read);
        assert_eq!(read_in(&lab.slot, None, &read(&file.content)).0, Outcome::Refused { reason: Reason::AlreadySettled });

        let entries = |mark| data::Entries { items: vec![Entry { mark, ..step.clone() }], ..data::Entries::default() };
        let accept = |step: u32, sha: &str| AcceptStep { address: "billing@app".to_string(), step, sha: sha.to_string() };
        let never = |_: &Path, _: &str, _: &str| -> crate::sh::Result<String> { panic!("not accepted") };
        assert_eq!(accept_in(&lab.slot, Some(entries(Mark::Committed)), &accept(2, &lab.shas[0]), never).0, Outcome::Refused { reason: Reason::HeadMoved });
        assert_eq!(accept_in(&lab.slot, Some(entries(Mark::Committed)), &accept(3, &lab.shas[1]), never).0, Outcome::Refused { reason: Reason::HeadMoved });
        assert_eq!(accept_in(&lab.slot, Some(entries(Mark::Accepted)), &accept(2, &lab.shas[1]), never).0, Outcome::Refused { reason: Reason::AlreadySettled });
        let ran = std::cell::RefCell::new(None);
        let (outcome, text) = accept_in(&lab.slot, Some(entries(Mark::Committed)), &accept(2, &lab.shas[1]), |_, task, sha| { *ran.borrow_mut() = Some((task.to_string(), sha.to_string())); Ok(String::new()) });
        assert_eq!((outcome, text.as_str()), (Outcome::Done, "Accepted step 2 of billing@app"));
        assert_eq!(ran.into_inner(), Some(("billing".to_string(), lab.shas[1].clone())));
        let _ = std::fs::remove_dir_all(&lab.root);
    }

    #[test]
    fn a_file_whose_path_the_protocol_cannot_carry_is_left_out_and_the_rest_still_goes() {
        let lab = repo("hub-review-names");
        std::fs::write(lab.slot.join("at:noon.txt"), "x\n").unwrap();
        sh(&lab.slot, &["add", "--", "at:noon.txt"]);
        let sha = commit(&lab.slot, "ok.txt", "y\n", "Add two files").trim().to_string();
        assert!(sh(&lab.slot, &["show", "--name-only", "--format=", &sha]).contains("at:noon.txt"));
        let step = Entry { sha: Some(sha), mark: Mark::Committed, number: 1, title: "Two".to_string() };
        let review = build(&lab.slot, "billing@app", &lab.dir, &step).unwrap();
        assert_eq!(review.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), ["ok.txt"]);
        assert_eq!(review.left_out.as_deref(), Some("1 file whose name the phone cannot show; review it at the machine"));
        let _ = std::fs::remove_dir_all(&lab.root);
    }

    #[test]
    fn a_task_offers_the_first_step_that_waits_for_acceptance_with_its_file_count() {
        use crate::tasks::{Step, StepState, Steps};
        let lab = repo("hub-offer");
        let step = |index, sha: &str, accepted| Step { index, title: format!("Step {index}"), state: StepState::Committed { sha: sha.to_string(), subject: String::new(), accepted }, extra: false };
        let mut item = crate::status::tests::task(Some("idle"), None, 3);
        item.path = Some(lab.slot.clone());
        assert_eq!(offer(&item), None, "no steps, no offer");
        item.steps = Steps { pushed: 0, items: vec![step(0, &lab.shas[0], true), step(1, &lab.shas[1], false), step(2, &lab.shas[2], false)] };
        assert_eq!(offer(&item), Some(ReviewOffer { step: Some(2), files: 1 }));
        item.steps.items[1] = step(1, &lab.shas[1], true);
        item.steps.items[2] = step(2, &lab.shas[2], true);
        assert_eq!(offer(&item), None, "every step accepted");
        let _ = std::fs::remove_dir_all(&lab.root);
    }
}
