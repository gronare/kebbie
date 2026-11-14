use super::*;
use crate::tasks::test_support::*;

fn titles(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn commit_of(sha: &str, subject: &str, accepted: bool) -> Unpushed {
    Unpushed { sha: sha.to_string(), subject: subject.to_string(), code: format!("code of {sha}"), accepted }
}

fn part(id: &str, title: &str, done: bool) -> Part {
    Part { id: id.to_string(), title: title.to_string(), done, ..Part::default() }
}

fn seen(subtask: &str, codes: &[&str]) -> Vec<Seen> {
    codes.iter().map(|code| Seen { sha: String::new(), code: code.to_string(), subject: format!("subject of {code}"), subtask: subtask.to_string() }).collect()
}

fn plan(parts: Vec<Part>, commits: Vec<Seen>) -> Plan {
    Plan { base: "base".to_string(), parts, commits, ..Plan::default() }
}

fn shas(subtasks: &Subtasks) -> Vec<(String, Vec<&str>)> {
    subtasks.items.iter().map(|item| (item.id.clone(), item.commits.iter().map(|commit| commit.sha.as_str()).collect())).collect()
}

fn states(subtasks: &Subtasks) -> Vec<State> {
    subtasks.items.iter().map(|item| item.state.clone()).collect()
}

fn rewritten_code(sha: &str, code: &str) -> Unpushed {
    Unpushed { code: code.to_string(), ..commit_of(sha, &format!("subject {sha}"), false) }
}

fn ids_of(plan: &Plan) -> Vec<&str> {
    plan.parts.iter().map(|part| part.id.as_str()).collect()
}

#[test]
fn a_first_list_gets_short_ids_from_the_titles_and_a_list_again_keeps_each_subtask_by_title_or_by_place() {
    let first = relisted(None, None, "base", &titles(&["Add the model", "  Wire\nthe command ", "Document it", "Add the model page"]), None).unwrap();
    assert_eq!(first.parts.iter().map(|part| (part.id.as_str(), part.title.as_str())).collect::<Vec<_>>(), [("add-model", "Add the model"), ("wire-command", "Wire the command"), ("document", "Document it"), ("add-model-2", "Add the model page")], "each title is one line and each id its own");
    let held = Plan { commits: seen("wire-command", &["b"]), ..first.clone() };
    let holding = Subtasks { items: vec![Subtask { id: "wire-command".to_string(), number: 2, title: "Wire the command".to_string(), state: State::InProgress, commits: vec![Commit { sha: "b".to_string(), subject: String::new(), accepted: false }], pushed: Vec::new(), extra: false, added: None, contiguous: true, waits_for: None }], ..Subtasks::default() };
    let again = relisted(Some(&held), Some(&holding), "later", &titles(&["Document it", "Wire it differently", "Test it"]), None).unwrap();
    assert_eq!(again.base, "base", "the base stays the one the subtasks were first listed on");
    assert_eq!(again.parts.iter().map(|part| (part.id.as_str(), part.title.as_str(), part.extra)).collect::<Vec<_>>(), [("document", "Document it", false), ("wire-command", "Wire it differently", false), ("test", "Test it", false)], "the same title keeps its id; a new title takes the place of the one holding commits at its position, and one holding none gives its place up");
    assert_eq!(again.commits, held.commits, "the commits stay with their subtask");
    let because = Added { at: 9, reason: "the lint needs frontmatter".to_string(), by: "task".to_string() };
    let added = relisted(Some(&held), None, "later", &titles(&["Add the model", "Wire the command", "Document it", "Add the model page", "Store frontmatter"]), Some(&because)).unwrap();
    assert_eq!((added.parts[4].id.as_str(), added.parts[4].added.as_ref()), ("store-frontmatter", Some(&because)), "a subtask new to the list carries why");
    let dropped = relisted(Some(&held), Some(&holding), "later", &titles(&["Add the model"]), None).unwrap();
    assert_eq!(dropped.parts.iter().map(|part| (part.id.as_str(), part.extra)).collect::<Vec<_>>(), [("add-model", false), ("wire-command", true)], "a subtask left out of the list that holds commits stays, not in the list");
    assert!(relisted(Some(&held), None, "base", &[], None).is_err() && relisted(None, None, "base", &titles(&["ok", " "]), None).is_err(), "no empty list and no empty title");
    assert_eq!(new_id("2", &[]), "s2", "an id is never a bare number, which an older kebbie's steps have");
}

#[test]
fn commits_go_to_the_subtask_in_progress_until_it_is_done_and_then_to_the_next() {
    let listed = plan(vec![part("model", "Model", false), part("command", "Command", false), part("docs", "Docs", false)], Vec::new());
    assert_eq!(states(&derive(Some(&listed), &[], &[], true).0), [State::InProgress, State::Planned, State::Planned], "the first subtask is under way while its agent works");
    assert_eq!(states(&derive(Some(&listed), &[], &[], false).0), [State::Planned, State::Planned, State::Planned], "and planned while nobody works on it");
    let two = [commit_of("aaa", "Add the model", true), commit_of("bbb", "Test the model", false)];
    let (working, noted) = derive(Some(&listed), &[], &two, false);
    assert_eq!((shas(&working), states(&working)), (vec![("model".to_string(), vec!["aaa", "bbb"]), ("command".to_string(), vec![]), ("docs".to_string(), vec![])], vec![State::InProgress, State::Planned, State::Planned]));
    assert_eq!(label(&working.items[0]), "in progress · 2 commits");
    let mut marked = noted;
    marked.parts[0].done = true;
    let three = [two[0].clone(), two[1].clone(), commit_of("ccc", "Add the command", false)];
    let (next, _) = derive(Some(&marked), &[], &three, true);
    assert_eq!((shas(&next)[1].1.clone(), states(&next)), (vec!["ccc"], vec![State::Ready, State::InProgress, State::Planned]), "once done, the next commit opens the next subtask");
    let accepted: Vec<Unpushed> = three.iter().map(|commit| Unpushed { accepted: commit.sha != "ccc", ..commit.clone() }).collect();
    assert_eq!(derive(Some(&marked), &[], &accepted, true).0.items[0].state, State::Accepted, "a done subtask whose every commit is accepted is accepted");
}

#[test]
fn a_subtasks_commits_need_not_lie_together_and_one_added_ahead_takes_the_next_commits() {
    let listed = plan(vec![part("gateway", "Gateway", false), part("docs", "Docs", false)], seen("gateway", &["code of aaa"]));
    let (prepared, id) = inserted(listed.clone(), "Name the currency", current(&listed).as_deref(), Added { at: 1, reason: "amounts carry no currency".to_string(), by: "investigator".to_string() }).unwrap();
    assert_eq!((id.as_str(), ids_of(&prepared)), ("name-currency", vec!["name-currency", "gateway", "docs"]), "a preparatory subtask goes ahead of the one it prepares");
    let commits = [commit_of("aaa", "Add the gateway", false), commit_of("bbb", "Carry the currency", false), commit_of("ccc", "Test the currency", false)];
    let (subtasks, mut noted) = derive(Some(&prepared), &[], &commits, true);
    assert_eq!(shas(&subtasks)[..2], [("name-currency".to_string(), vec!["bbb", "ccc"]), ("gateway".to_string(), vec!["aaa"])], "the gateway keeps its commit, and the new ones are the preparatory subtask's");
    noted.parts[0].done = true;
    let more = [commits[0].clone(), commits[1].clone(), commits[2].clone(), commit_of("ddd", "Charge through the gateway", false)];
    let (later, _) = derive(Some(&noted), &[], &more, true);
    let gateway = later.named("gateway").unwrap();
    assert_eq!((gateway.commits.iter().map(|commit| commit.sha.as_str()).collect::<Vec<_>>(), gateway.contiguous), (vec!["aaa", "ddd"], false), "a subtask's commits with another's between them stay its own");
    assert_eq!(later.order.iter().map(|row| (row.sha.as_str(), row.subtask.as_str())).collect::<Vec<_>>(), [("aaa", "gateway"), ("bbb", "name-currency"), ("ccc", "name-currency"), ("ddd", "gateway")], "the push order is the branch's");
}

#[test]
fn a_rewritten_commit_stays_in_the_subtask_that_lost_its_change_and_a_new_one_in_between_goes_where_commits_go_now() {
    let listed = plan(vec![part("model", "Model", true), part("command", "Command", false), part("docs", "Docs", false)], [seen("model", &["a1", "a2"]), seen("command", &["b1", "b2"])].concat());
    let rebased = [rewritten_code("x1", "a1"), rewritten_code("x2", "a2"), rewritten_code("x3", "b1"), rewritten_code("x4", "b2")];
    assert_eq!(shas(&derive(Some(&listed), &[], &rebased, false).0)[..2], [("model".to_string(), vec!["x1", "x2"]), ("command".to_string(), vec!["x3", "x4"])], "new shas, the same changes");
    let into_last_of_first = [rewritten_code("x1", "a1"), rewritten_code("x2", "a2+fix"), rewritten_code("x3", "b1"), rewritten_code("x4", "b2")];
    assert_eq!(shas(&derive(Some(&listed), &[], &into_last_of_first, false).0)[0].1, ["x1", "x2"]);
    let into_first_of_second = [rewritten_code("x1", "a1"), rewritten_code("x2", "a2"), rewritten_code("x3", "b1+fix"), rewritten_code("x4", "b2")];
    assert_eq!(shas(&derive(Some(&listed), &[], &into_first_of_second, false).0)[1].1, ["x3", "x4"]);
    let new_between = [rewritten_code("x1", "a1"), rewritten_code("x2", "a2"), rewritten_code("x5", "new"), rewritten_code("x3", "b1"), rewritten_code("x4", "b2")];
    assert_eq!(shas(&derive(Some(&listed), &[], &new_between, false).0)[..2], [("model".to_string(), vec!["x1", "x2"]), ("command".to_string(), vec!["x5", "x3", "x4"])], "a commit nobody lost goes to the subtask in progress, wherever it sits");
}

#[test]
fn an_accepted_subtask_that_gains_a_commit_is_ready_again_with_only_the_new_commit_unaccepted() {
    let listed = plan(vec![part("model", "Model", true), part("docs", "Docs", false)], seen("model", &["code of aaa"]));
    let accepted = [commit_of("aaa", "Add the model", true)];
    assert_eq!(states(&derive(Some(&listed), &[], &accepted, false).0)[0], State::Accepted);
    let on = Plan { on: Some("model".to_string()), parts: vec![part("model", "Model", false), part("docs", "Docs", false)], ..listed };
    let (back, noted) = derive(Some(&on), &[], &[accepted[0].clone(), commit_of("bbb", "Fix the model", false)], true);
    assert_eq!((shas(&back)[0].1.clone(), back.items[0].state.clone()), (vec!["aaa", "bbb"], State::InProgress), "kebbie subtasks on sends the next commit to it");
    let done = Plan { parts: vec![part("model", "Model", true), part("docs", "Docs", false)], on: None, ..noted };
    let (ready, _) = derive(Some(&done), &[], &[accepted[0].clone(), commit_of("bbb", "Fix the model", false)], true);
    assert_eq!((ready.items[0].state.clone(), ready.items[0].commits.iter().map(|commit| commit.accepted).collect::<Vec<_>>()), (State::Ready, vec![true, false]), "the commit read before stays accepted");
    assert_eq!(ready.pushable.as_ref().map(|run| (run.count, run.through.as_str())), Some((1, "aaa")), "the accepted commit stays pushable; the new one stops the run from its own place");
    assert_eq!(ready.order.iter().map(|row| row.accepted).collect::<Vec<_>>(), [true, false], "each row says whether its own commit is accepted");
    assert_eq!(label(&ready.items[0]), "ready · 1 accepted · 1 new commit");
}

#[test]
fn a_group_is_never_split_by_the_run_and_a_moved_commit_says_where_it_came_from_until_it_is_gone() {
    let mut listed = plan(vec![part("model", "Model", true), part("docs", "Docs", true)], [seen("model", &["code of aaa", "code of bbb"]), seen("docs", &["code of ccc"])].concat());
    listed.groups = vec![Bound { id: "g1".to_string(), codes: vec!["code of bbb".to_string(), "code of ccc".to_string(), "code of gone".to_string()], reason: "the docs describe the model".to_string() }];
    listed.moved = vec![MovedAt { code: "code of bbb".to_string(), from: 3, reason: "it prepares the docs".to_string() }, MovedAt { code: "code of gone".to_string(), from: 1, reason: "x".to_string() }];
    let commits = [commit_of("aaa", "a", true), commit_of("bbb", "b", true), commit_of("ccc", "c", false)];
    let (subtasks, noted) = derive(Some(&listed), &[], &commits, false);
    assert_eq!(subtasks.order.iter().map(|row| row.group.as_deref()).collect::<Vec<_>>(), [None, Some("g1"), Some("g1")]);
    assert_eq!(subtasks.pushable.as_ref().map(|run| (run.count, run.through.as_str())), Some((1, "aaa")), "bbb is accepted, but its group's other commit is not, so it waits with it");
    assert_eq!(subtasks.order[1].moved.as_ref().map(|moved| (moved.from, moved.reason.as_str())), Some((3, "it prepares the docs")));
    assert_eq!((noted.groups[0].codes.len(), noted.moved.len()), (2, 1), "a commit no longer open leaves its group and its move behind");
    let alone = commits.iter().map(|commit| Unpushed { accepted: true, ..commit.clone() }).collect::<Vec<_>>();
    assert_eq!(derive(Some(&listed), &[], &alone, false).0.pushable.map(|run| run.count), Some(3));
    let one_left = [commit_of("aaa", "a", true), commit_of("bbb", "b", true)];
    assert!(derive(Some(&listed), &[], &one_left, false).1.groups.is_empty(), "a group of one is no group");
}

#[test]
fn the_pushable_run_is_the_longest_from_the_top_whose_every_commit_is_accepted_and_one_behind_says_what_it_waits_for() {
    let listed = plan(vec![part("lint", "Lint the index", true), part("store", "Store frontmatter", true), part("docs", "Docs", true)], [seen("lint", &["code of aaa"]), seen("store", &["code of bbb"]), seen("docs", &["code of ccc"]), seen("lint", &["code of ddd"])].concat());
    let commits = |accepted: &[bool]| ["aaa", "bbb", "ccc", "ddd"].iter().zip(accepted).map(|(sha, accepted)| commit_of(sha, sha, *accepted)).collect::<Vec<_>>();
    let (all, _) = derive(Some(&listed), &[], &commits(&[true, true, true, true]), false);
    assert_eq!(all.pushable, Some(Run { count: 4, through: "ddd".to_string(), subtasks: vec!["lint".to_string(), "store".to_string(), "docs".to_string()] }));
    let (held, _) = derive(Some(&listed), &[], &commits(&[true, true, true, false]), false);
    assert_eq!(held.pushable.as_ref().map(|run| (run.count, run.through.as_str())), Some((3, "ccc")), "acceptance is per commit: lint's last commit waits, the three accepted above it go");
    let (behind, _) = derive(Some(&listed), &[], &commits(&[true, true, false, true]), false);
    assert_eq!(behind.pushable.as_ref().map(|run| (run.count, run.through.as_str())), Some((2, "bbb")));
    let docs = behind.named("docs").unwrap();
    assert_eq!(docs.state, State::Ready);
    let accepted_behind = derive(Some(&Plan { parts: vec![part("store", "Store frontmatter", true), part("lint", "Lint the index", false), part("docs", "Docs", true)], ..listed.clone() }), &[], &commits(&[false, true, true, true]), false).0;
    assert_eq!(accepted_behind.pushable, None, "lint's first commit is not accepted");
    let store = accepted_behind.named("store").unwrap();
    assert_eq!((store.waits_for.as_deref(), label(store)), (Some("Lint the index"), "accepted · waits for Lint the index · 1 commit".to_string()));
}

#[test]
fn a_commit_counts_as_pushed_once_the_default_branch_gained_its_change_and_a_subtask_with_all_of_its_pushed_is_pushed() {
    let listed = plan(vec![part("model", "Model", true), part("command", "Command", true), part("docs", "Docs", false)], [seen("model", &["code of aaa", "code of bbb"]), seen("command", &["code of ccc"])].concat());
    let commits = [commit_of("aaa", "Add the model", false), commit_of("bbb", "Test it", false), commit_of("ccc", "Add the command", false)];
    let (subtasks, _) = derive(Some(&listed), &["code of aaa".to_string(), "code of bbb".to_string()], &commits, false);
    assert_eq!((subtasks.pushed.iter().map(|subtask| subtask.id.as_str()).collect::<Vec<_>>(), shas(&subtasks)), (vec!["model"], vec![("command".to_string(), vec!["ccc"]), ("docs".to_string(), vec![])]), "cherry-picked copies on main take the subtask out of sight");
    let (half, _) = derive(Some(&listed), &["code of aaa".to_string()], &commits, false);
    assert_eq!((half.pushed.len(), half.items[0].commits.len(), half.order.len()), (0, 1, 2), "half a subtask on main leaves its other half in the push order");
    let (second, _) = derive(Some(&listed), &["code of ccc".to_string()], &commits, false);
    assert_eq!((second.pushed.iter().map(|subtask| subtask.id.as_str()).collect::<Vec<_>>(), second.items.len()), (vec!["command"], 2), "any subtask whose every commit landed is pushed, first or not");
}

#[test]
fn an_open_commit_with_the_same_change_as_a_pushed_one_stays_in_sight() {
    let same = |sha: &str| Unpushed { sha: sha.to_string(), subject: format!("subject {sha}"), code: "same".to_string(), accepted: false };
    let listed = plan(vec![part("model", "Model", true), part("again", "Again", true)], [seen("model", &["same"]), seen("again", &["same"])].concat());
    let (subtasks, _) = derive(Some(&listed), &["same".to_string()], &[same("aaa"), same("bbb")], false);
    assert_eq!((subtasks.pushed.len(), shas(&subtasks)), (1, vec![("again".to_string(), vec!["bbb"])]), "one landed copy pushes one commit and hides one");
    let (subtasks, _) = derive(Some(&listed), &["same".to_string(), "same".to_string()], &[same("aaa"), same("bbb")], false);
    assert_eq!((subtasks.pushed.len(), subtasks.items.len()), (2, 0), "two landed copies push both");
}

#[test]
fn commits_after_every_subtask_is_done_make_one_extra_subtask_titled_by_its_first_subject() {
    let listed = plan(vec![part("model", "Model", true)], seen("model", &["code of aaa"]));
    let (found, noted) = derive(Some(&listed), &[], &[commit_of("aaa", "Add the model", false), commit_of("bbb", "Fix a typo", false), commit_of("ccc", "Fix another", false)], true);
    assert_eq!(shas(&found), [("model".to_string(), vec!["aaa"]), ("fix-typo".to_string(), vec!["bbb", "ccc"])], "nothing is hidden");
    assert_eq!((found.items[1].title.as_str(), found.items[1].extra, &found.items[1].state), ("Fix a typo", true, &State::InProgress));
    assert_eq!(noted.parts.len(), 2, "the extra subtask is noted with its commits");
    let (gone, renoted) = derive(Some(&noted), &[], &[commit_of("aaa", "Add the model", false)], true);
    assert_eq!((gone.items.len(), renoted.parts.len()), (1, 1), "an extra subtask that lost its commits goes");
}

#[test]
fn steps_an_older_kebbie_noted_read_as_subtasks_named_by_their_numbers() {
    let dir = crate::sh::test_dir("plan-older");
    crate::state::write(&file(&dir, "billing@app"), json!({ "titles": ["Model", "Command", "Docs"], "base": "base", "held": [{ "seen": [{ "code": "code of aaa", "subject": "Add the model" }, { "code": "code of bbb", "subject": "Test the model" }], "done": true }, { "seen": [{ "code": "code of ccc", "subject": "Add the command" }], "done": false }] }).to_string()).unwrap();
    let old = load(&dir, "billing@app").unwrap();
    assert_eq!(ids_of(&old), ["1", "2", "3"], "an older app's accept-step n still names step n");
    let (subtasks, _) = derive(Some(&old), &[], &[commit_of("aaa", "Add the model", true), commit_of("bbb", "Test the model", false), commit_of("ccc", "Add the command", false)], false);
    assert_eq!((shas(&subtasks), states(&subtasks)), (vec![("1".to_string(), vec!["aaa", "bbb"]), ("2".to_string(), vec!["ccc"]), ("3".to_string(), vec![])], vec![State::Ready, State::InProgress, State::Planned]));
    crate::state::write(&file(&dir, "search@app"), json!({ "titles": ["Model", "Command"], "base": "base", "seen": [{ "code": "code of aaa", "subject": "Add the model" }] }).to_string()).unwrap();
    let oldest = load(&dir, "search@app").unwrap();
    assert_eq!((oldest.parts[0].done, oldest.commits[0].subtask.as_str()), (true, "1"), "the oldest shape's one commit a step reads as a done step");
    save(&dir, "billing@app", &old).unwrap();
    assert_eq!(load(&dir, "billing@app").unwrap(), old, "written back in the new shape and read the same");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_task_without_subtasks_has_none() {
    let (none, _) = derive(None, &[], &[commit_of("aaa", "Add the model", false)], true);
    assert_eq!(none, Subtasks::default());
    assert!(!none.listed(), "callers fall back to the rows they show today");
    assert!(!derive(Some(&Plan::default()), &[], &[commit_of("aaa", "x", false)], true).0.listed());
    assert!(derive(Some(&plan(vec![part("model", "Model", true)], seen("model", &["a"]))), &["a".to_string()], &[], false).0.listed(), "a list pushed in full is still a list");
}

fn landed_task() -> (PathBuf, PathBuf, crate::tasks::Slot, String, PathBuf, String) {
    let (dir, origin, slot, upstream) = task_with_a_commit("billing");
    let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
    let state = state_of(&dir);
    list_at(&state, &slot.path, &upstream, "billing@app", &["Model", "Command"]).unwrap();
    done_at(&state, &slot.path, &upstream, "billing@app").unwrap();
    commit(&slot.path, "e", "fifth");
    assert!(listed_at(&state, &slot.path, &upstream, "billing@app").pushed.is_empty());
    (dir, origin, slot, upstream, state, fourth)
}

fn after_landing(state: &Path, slot: &crate::tasks::Slot, upstream: &str) -> (usize, Vec<(String, String)>) {
    let subtasks = listed_at(state, &slot.path, upstream, "billing@app");
    (subtasks.pushed.len(), subtasks.items.iter().map(|item| (item.id.clone(), item.commits.first().map(|commit| commit.subject.clone()).unwrap_or_default())).collect())
}

#[test]
fn a_subtask_staged_on_main_and_pushed_by_hand_counts_as_pushed() {
    let (dir, origin, slot, upstream, state, fourth) = landed_task();
    pick(&dir, &fourth);
    git(&dir, &["push", "--quiet", "origin", "HEAD"]).unwrap();
    git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
    assert_eq!(git(&slot.path, &["rev-list", "--count", &format!("{upstream}..HEAD")]).unwrap(), "2", "the branch still holds both, unrebased");
    assert_eq!(after_landing(&state, &slot, &upstream), (1, vec![("command".to_string(), "fifth".to_string())]));
    cleanup(dir, origin, &slot);
}

#[test]
fn the_first_commits_pushed_with_plain_git_from_the_main_checkout_count_as_pushed() {
    let (dir, origin, slot, upstream, state, fourth) = landed_task();
    git(&dir, &["push", "--quiet", "origin", &format!("{fourth}:refs/heads/{}", upstream.trim_start_matches("origin/"))]).unwrap();
    git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
    assert_eq!(after_landing(&state, &slot, &upstream), (1, vec![("command".to_string(), "fifth".to_string())]));
    cleanup(dir, origin, &slot);
}

// The PostToolUse input of https://code.claude.com/docs/en/hooks for a Bash command run in the task's worktree.
fn bash_in(path: &Path, command: &str) -> Value {
    json!({
        "session_id": "abc123", "transcript_path": "/t.jsonl", "cwd": path, "permission_mode": "default", "hook_event_name": "PostToolUse",
        "tool_name": "Bash", "tool_input": { "command": command, "description": "Commit the subtask" },
        "tool_response": { "stdout": "", "stderr": "", "interrupted": false, "isImage": false },
        "tool_use_id": "toolu_01ABC123", "duration_ms": 41,
    })
}

fn hooked_task() -> (PathBuf, PathBuf, crate::tasks::Slot, String, PathBuf, String) {
    let (dir, origin, slot, upstream) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let address = peers::address("billing", &name_of(&dir));
    list_at(&state, &slot.path, &upstream, &address, &["Model", "Command"]).unwrap();
    assert!(noted_after(&state, &address, &bash_in(&slot.path, "git commit -m fourth")), "the first commit is noted by the hook after it");
    (dir, origin, slot, upstream, state, address)
}

#[test]
fn a_commit_noted_by_the_bash_hook_counts_as_pushed_after_a_plain_push_with_nothing_else_listing_the_subtasks() {
    let (dir, origin, slot, upstream, state, address) = hooked_task();
    let fourth = git(&slot.path, &["rev-parse", "HEAD"]).unwrap();
    done_at(&state, &slot.path, &upstream, &address).unwrap();
    commit(&slot.path, "e", "fifth");
    assert!(noted_after(&state, &address, &bash_in(&slot.path, "git commit -m fifth")));
    git(&dir, &["push", "--quiet", "origin", &format!("{fourth}:refs/heads/{}", upstream.trim_start_matches("origin/"))]).unwrap();
    git(&dir, &["fetch", "--quiet", "origin"]).unwrap();
    let subtasks = listed_at(&state, &slot.path, &upstream, &address);
    assert_eq!((subtasks.pushed.len(), subtasks.items.iter().map(|item| (item.id.as_str(), item.commits.iter().map(|commit| commit.subject.as_str()).collect::<Vec<_>>())).collect::<Vec<_>>()), (1, vec![("command", vec!["fifth"])]), "the subtask after the pushed one keeps its own commit");
    cleanup(dir, origin, &slot);
}

#[test]
fn the_bash_hook_notes_nothing_while_head_stays_where_it_was_noted() {
    let (dir, origin, slot, _, state, address) = hooked_task();
    let noted = load(&state, &address).unwrap();
    assert_eq!(noted.commits.len(), 1);
    save(&state, &address, &Plan { commits: Vec::new(), ..noted }).unwrap();
    assert!(!noted_after(&state, &address, &bash_in(&slot.path, "ls -la")), "an unchanged HEAD notes nothing");
    assert!(load(&state, &address).unwrap().commits.is_empty(), "and leaves the subtasks as they were");
    commit(&slot.path, "e", "fifth");
    let mut edit = bash_in(&slot.path, "git commit -m fifth");
    edit["tool_name"] = json!("Edit");
    assert!(!noted_after(&state, &address, &edit), "only Bash is noted after");
    assert!(!noted_after(&state, "search@app", &bash_in(&slot.path, "git commit -m fifth")), "a task without subtasks notes nothing");
    assert!(noted_after(&state, &address, &bash_in(&slot.path, "git commit -m fifth")));
    assert_eq!(load(&state, &address).unwrap().commits.iter().map(|seen| seen.subtask.as_str()).collect::<Vec<_>>(), ["model", "model"], "both commits are the subtask in progress's");
    cleanup(dir, origin, &slot);
}

#[test]
fn done_marks_the_subtask_in_progress_or_the_one_named_ready_once_it_holds_a_commit_and_only_by_its_own_task() {
    let (dir, origin, slot, upstream) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let task = Task { path: &slot.path, upstream: &upstream, address: "billing@app" };
    assert!(done_as(&Caller::User, &state, &task, None, false).unwrap_err().contains("has no subtasks"));
    list_at(&state, &slot.path, &upstream, "billing@app", &["Model", "Command"]).unwrap();
    assert!(done_as(&Caller::Task("search@app".to_string()), &state, &task, None, false).unwrap_err().contains("writes its own subtasks"));
    let unformed = done_as(&Caller::Task("billing@app".to_string()), &state, &task, None, true).unwrap_err();
    assert!(unformed.contains("Not marked done: a commit of Model breaks rule 1") && unformed.contains("fourth: no body"), "a subtask goes to the user well formed: {unformed}");
    crate::tasks::reword(&slot.path, &git(&slot.path, &["rev-parse", "HEAD"]).unwrap(), "fourth\n\nThe model holds refunds now.").unwrap();
    assert!(done_as(&Caller::Task("billing@app".to_string()), &state, &task, None, true).unwrap().contains("Model is ready"));
    let subtasks = listed_at(&state, &slot.path, &upstream, "billing@app");
    assert_eq!((&subtasks.items[0].state, subtasks.items[0].commits.len(), &subtasks.items[1].state), (&State::Ready, 1, &State::Planned));
    let refused = done_as(&Caller::Lead, &state, &task, None, false).unwrap_err();
    assert!(refused.contains("Command holds no commit yet"), "{refused}");
    assert!(done_as(&Caller::Lead, &state, &task, Some("nope"), false).unwrap_err().contains("has no subtask nope"));
    commit(&slot.path, "e", "fifth");
    done_as(&Caller::Lead, &state, &task, Some("command"), false).unwrap();
    assert_eq!(states(&listed_at(&state, &slot.path, &upstream, "billing@app")), [State::Ready, State::Ready]);
    cleanup(dir, origin, &slot);
}

#[test]
fn add_needs_a_reason_and_goes_before_the_subtask_it_prepares_and_on_sends_the_next_commits_to_one() {
    let (dir, origin, slot, upstream) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let task = Task { path: &slot.path, upstream: &upstream, address: "billing@app" };
    list_at(&state, &slot.path, &upstream, "billing@app", &["Model", "Command"]).unwrap();
    assert!(add_as(&Caller::User, &state, &task, "Store frontmatter", "  ", None, None, 5).unwrap_err().contains("--because"));
    assert!(add_as(&Caller::User, &state, &task, "Store frontmatter", "the lint reads it", Some("nope"), None, 5).unwrap_err().contains("no subtask nope to prepare"));
    assert_eq!(add_as(&Caller::Task("billing@app".to_string()), &state, &task, "Store frontmatter", "the lint\nreads it", Some("command"), None, 5).unwrap(), "Added Store frontmatter [store-frontmatter].");
    let plan = load(&state, "billing@app").unwrap();
    assert_eq!((ids_of(&plan), plan.parts[1].added.clone()), (vec!["model", "store-frontmatter", "command"], Some(Added { at: 5, reason: "the lint reads it".to_string(), by: "task".to_string() })));
    on_as(&Caller::Task("billing@app".to_string()), &state, &task, "command").unwrap();
    commit(&slot.path, "e", "fifth");
    let subtasks = listed_at(&state, &slot.path, &upstream, "billing@app");
    assert_eq!(shas(&subtasks).iter().map(|(id, shas)| (id.as_str(), shas.len())).collect::<Vec<_>>(), [("model", 1), ("store-frontmatter", 0), ("command", 1)]);
    let id = add_preparatory(&state, &slot.path, "billing@app", "Split the index", "the index stores titles only", 7).unwrap();
    assert_eq!(id.as_deref(), Some("split-index"));
    let plan = load(&state, "billing@app").unwrap();
    assert_eq!((ids_of(&plan), plan.on.as_deref()), (vec!["model", "store-frontmatter", "split-index", "command"], Some("split-index")), "the investigator's subtask goes ahead of the one commits went to, and they go to it now");
    assert_eq!(add_preparatory(&state, &slot.path, "search@app", "x", "y", 7).unwrap(), None, "a task without subtasks gets none");
    cleanup(dir, origin, &slot);
}

#[test]
fn the_boards_read_shows_the_subtasks_and_notes_nothing() {
    let (dir, origin, slot, upstream) = task_with_a_commit("billing");
    let state = state_of(&dir);
    list_at(&state, &slot.path, &upstream, "billing@app", &["Model", "Command"]).unwrap();
    let before = load(&state, "billing@app").unwrap();
    let tips = tips_of(&Task { path: &slot.path, upstream: &upstream, address: "billing@app" }).unwrap();
    let read = read_at(&state, &slot.path, "billing@app", &tips, false);
    assert_eq!((read.items.len(), &read.items[0].state, &read.items[0].commits), (2, &State::InProgress, &vec![Commit { sha: tips.head.clone(), subject: "fourth".to_string(), accepted: false }]));
    assert_eq!(load(&state, "billing@app").unwrap(), before, "a read writes no plan");
    assert_eq!(listed_at(&state, &slot.path, &upstream, "billing@app"), read, "kebbie subtasks reads the same");
    assert_ne!(load(&state, "billing@app").unwrap().commits, before.commits, "and it notes the commit");
    cleanup(dir, origin, &slot);
}

#[test]
fn a_task_writes_only_its_own_subtasks_and_the_lead_and_the_user_write_any() {
    let (dir, origin, slot, upstream) = task_with_a_commit("billing");
    let state = state_of(&dir);
    let task = Task { path: &slot.path, upstream: &upstream, address: "billing@app" };
    let refused = write_as(&Caller::Task("search@app".to_string()), &state, &task, &titles(&["Model"]), None).unwrap_err();
    assert!(refused.contains("writes its own subtasks"), "{refused}");
    assert_eq!(load(&state, "billing@app"), None);
    for caller in [Caller::Agent, Caller::Sandboxed, Caller::LeadSandboxed, Caller::hub("acc_01")] {
        let refused = write_as(&caller, &state, &task, &titles(&["Model"]), Some("because")).unwrap_err();
        assert!(refused.contains("changes no task's plan"), "{caller:?}: {refused}");
    }
    assert_eq!(load(&state, "billing@app"), None, "none of them is recorded as kebbie or the lead");
    for caller in [Caller::Task("billing@app".to_string()), Caller::Lead, Caller::User] {
        write_as(&caller, &state, &task, &titles(&["Model"]), None).unwrap();
    }
    let written = load(&state, "billing@app").unwrap();
    assert_eq!((written.parts.iter().map(|part| part.title.as_str()).collect::<Vec<_>>(), written.base), (vec!["Model"], git(&slot.path, &["rev-parse", &upstream]).unwrap()));
    cleanup(dir, origin, &slot);
}

#[test]
fn the_shown_subtasks_mark_each_state_with_its_commits_and_what_is_pushable() {
    let listed = plan(vec![part("model", "Model", true), part("command", "Command", true), part("docs", "Docs", false), part("tests", "Tests", false)], [seen("model", &["a"]), seen("command", &["code of abcdef0123", "code of 1234567890"])].concat());
    let landed = ["a".to_string()];
    let commits = [commit_of("abcdef0123", "Add the command", true), commit_of("1234567890", "Wire the command", true), commit_of("fedcba9876", "Write the docs", false)];
    let text = shown("billing@app", &derive(Some(&listed), &landed, &commits, true).0);
    assert_eq!(text, "billing@app's subtasks:\n  ↑ 1 pushed\n  ✓ 2. Command [command]  accepted · 2 commits\n       abcdef0 Add the command\n       1234567 Wire the command\n  ◐ 3. Docs [docs]  in progress · 1 commit\n       fedcba9 Write the docs\n  ○ 4. Tests [tests]\nPushable: the first 2 commits through 1234567");
    assert!(shown("billing@app", &Subtasks::default()).contains("has no subtasks"));
    let value = to_json("billing@app", &derive(Some(&listed), &landed, &commits, false).0, Some("abc"));
    assert_eq!((value["pushed"].as_u64(), value["last_accepted"].as_str(), value["subtasks"][0]["id"].as_str(), value["subtasks"][0]["state"].as_str(), value["subtasks"][1]["state"].as_str(), value["subtasks"][2]["state"].as_str()), (Some(1), Some("abc"), Some("command"), Some("accepted"), Some("in-progress"), Some("planned")));
    assert_eq!(value["subtasks"][0]["commits"], json!([{ "sha": "abcdef0123", "subject": "Add the command", "accepted": true }, { "sha": "1234567890", "subject": "Wire the command", "accepted": true }]));
    assert_eq!((value["pushable"]["through"].as_str(), value["order"][2]["subtask"].as_str()), (Some("1234567890"), Some("docs")));
}

#[test]
fn a_rebase_stopped_on_a_conflict_leaves_each_subtask_its_commits_and_ready_marks() {
    let (dir, origin, slot, upstream, state, address) = hooked_task();
    done_at(&state, &slot.path, &upstream, &address).unwrap();
    commit(&slot.path, "e", "fifth");
    assert!(noted_after(&state, &address, &bash_in(&slot.path, "git commit -m fifth")));
    done_at(&state, &slot.path, &upstream, &address).unwrap();
    let held = |subtasks: &Subtasks| subtasks.items.iter().map(|item| (item.id.clone(), item.commits.iter().map(|commit| commit.subject.clone()).collect::<Vec<_>>(), item.state.clone())).collect::<Vec<_>>();
    let before = held(&read_now(&state, &slot.path, &upstream, &address));
    upstream_commit(&dir, "d", "main changed the model's file", "main touches d");
    assert!(git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "rebase", "--quiet", &upstream]).is_err(), "the rebase stops on its conflict");
    assert!(!noted_after(&state, &address, &bash_in(&slot.path, "git rebase origin/main")), "nothing is noted from a half-replayed branch");
    listed_at(&state, &slot.path, &upstream, &address);
    std::fs::write(slot.path.join("d"), "resolved").unwrap();
    git(&slot.path, &["add", "d"]).unwrap();
    git(&slot.path, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "-c", "core.editor=true", "rebase", "--continue"]).unwrap();
    noted_after(&state, &address, &bash_in(&slot.path, "git rebase --continue"));
    let after = held(&read_now(&state, &slot.path, &upstream, &address));
    cleanup(dir, origin, &slot);
    assert_eq!(after, before, "the subtasks hold what they held before the rebase");
}

#[test]
fn a_met_stage_with_commits_left_shows_open_and_its_rest_goes_out_before_the_next_stage() {
    use kebbie_hub_protocol::{RequirementKind, StageState};
    let mut listed = plan(vec![part("a", "Backfill a", true), part("b", "Backfill b", true), part("c", "Drop column", true)], [seen("a", &["code of aaa"]), seen("b", &["code of bbb"]), seen("c", &["code of ccc"])].concat());
    listed.stages = vec![
        StagePlan { id: "fill".to_string(), title: "Backfill".to_string(), kind: RequirementKind::Verified, detail: Some("rows".to_string()), subtasks: vec!["a".to_string(), "b".to_string()], through: Some("f".repeat(40)), pushed_at: Some(1), kept: Some(StageState::Met), met_at: Some(2), note: None },
        StagePlan { id: "cut".to_string(), title: "Drop".to_string(), kind: RequirementKind::Deploy, detail: None, subtasks: vec!["c".to_string()], through: None, pushed_at: None, kept: None, met_at: None, note: None },
    ];
    let open = [commit_of("bbb", "b", true), commit_of("ccc", "c", true)];
    let (subtasks, _) = derive(Some(&listed), &["code of aaa".to_string()], &open, false);
    assert_eq!(subtasks.stages.iter().map(|stage| (stage.id.as_str(), stage.state)).collect::<Vec<_>>(), [("fill", StageState::Open), ("cut", StageState::Locked)]);
    assert_eq!(subtasks.pushable.map(|run| (run.count, run.through)), Some((1, "bbb".to_string())), "the rest of Backfill goes out alone, and Drop waits for it to be met again");
    let (done, _) = derive(Some(&listed), &["code of aaa".to_string(), "code of bbb".to_string()], &open[1..], false);
    assert_eq!(done.stages[0].state, StageState::Met, "a met stage with nothing left stays met");
}
