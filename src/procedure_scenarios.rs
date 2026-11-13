// What the role and skill texts tell an agent in the situations real sign-off cards showed; each test names its situation, and the texts are read as the agent gets them.
use std::fs;
use std::path::Path;

use crate::config::{AlsoNoticed, Source};
use crate::peers::{fill, noticed_section, steps_section};

fn file(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap_or_default()
}

fn task_role() -> String {
    let template = file("roles/task.md");
    let noticed = noticed_section(AlsoNoticed::Backlog, true, "/bin/kebbie");
    let steps = steps_section(true, "/bin/kebbie");
    fill(&template, &[("kebbie", "/bin/kebbie"), ("noticed", &noticed), ("steps", &steps)])
}

fn lead_role() -> String {
    crate::lead::role_text(&file("roles/lead.md"), "kebbie", &[Source::Vault], true, true)
}

fn playbook(page: &str) -> String {
    file(&format!("skills/playbook/{page}"))
}

fn investigator() -> String {
    file("skills/investigate/SKILL.md")
}

fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let Some(start) = text.find(heading) else { return "" };
    let rest = &text[start + heading.len()..];
    &rest[..rest.find("\n## ").unwrap_or(rest.len())]
}

fn says(text: &str, needed: &[&str], situation: &str) {
    for phrase in needed {
        assert!(text.contains(phrase), "{situation}: the text never says {phrase:?}");
    }
}

#[test]
fn a_plan_step_that_needs_what_the_code_lacks_stops_and_is_raised_as_an_obstacle_with_every_way_listed() {
    let role = task_role();
    let questions = section(&role, "## Questions");
    says(questions, &["**obstacle**", "a precondition the work needs is missing", "only by a workaround", "Stop the work it blocks", "/bin/kebbie tell lead \"obstacle: ", "each way you see, the workaround among them", "go on with work it does not touch", "`--state blocked`", "kebbie:playbook"], "a planned step reads a field the index does not store");
    says(&playbook("OBSTACLE.md"), &["Done when", "file:line", "what you read to know it", "workaround", "the decision"], "the task writes the obstacle so the investigator decides without asking back");
    let rules = section(&role, "## Rules");
    assert!(rules.contains("its decision on an obstacle you raised"), "the lead's decision on an obstacle is the call the task follows, not information to weigh");
}

#[test]
fn a_part_of_an_approved_plan_that_cannot_be_built_is_a_revision_told_with_its_reason_and_asked_before_the_build() {
    let role = task_role();
    says(section(&role, "## Questions"), &["Your plan is living", "plan, build, finding, revised plan", "/bin/kebbie tell lead \"plan: ", "because", "changes what the user gets", "before you build"], "a planned step was dropped and only confirmed on the card");
    says(&playbook("PLAN.md"), &["revision", "reason", "the lead", "kebbie ask", "make the change easy, then make the easy change"], "the plan page carries the loop");
}

#[test]
fn a_trade_off_that_is_the_users_is_asked_before_building_and_never_left_on_the_card_to_confirm() {
    let role = task_role();
    says(section(&role, "## Questions"), &["before you build the part it decides"], "decisions to confirm on a card were trade-offs settled for the user");
    let card = section(&role, "## Reports");
    let card = &card[card.find("`--card <file>`").expect("the reports name the card")..];
    let card = &card[..card.find("\n- ").unwrap_or(card.len())];
    assert!(!card.to_lowercase().contains("confirm"), "the card's description invites lines for the user to confirm: {card}");
    says(card, &["**Built**", "**Decisions**", "**Also noticed**"], "the card's sections");
}

#[test]
fn a_check_the_task_cannot_run_is_an_obstacle_and_a_check_only_the_user_can_make_is_asked_on_its_own() {
    let role = task_role();
    says(section(&role, "## Questions"), &["a check you cannot run"], "the device was locked and the check went into --next");
    says(section(&role, "## Reports"), &["A check only the user can make", "/bin/kebbie ask", "never folded into a ready report"], "--next said check this, then push");
}

#[test]
fn a_red_test_is_never_called_pre_existing_without_its_cause_found() {
    let role = task_role();
    says(section(&role, "## Reports"), &["A red test", "find its cause", "pre-existing", "an obstacle", "before you report ready"], "eight reports called a red test already there");
    says(&playbook("CARD.md"), &["the latest test run on this code is green, or its red is an obstacle you raised"], "the card is written only once nothing waits");
}

#[test]
fn something_on_the_tasks_own_path_is_an_obstacle_and_not_an_also_noticed_line() {
    let role = task_role();
    says(section(&role, "## What you notice outside your task"), &["on your own path is an obstacle"], "half the Also noticed lines lay on the task's own path");
    says(section(&role, "## Questions"), &["outside what the task delivers"], "the sorting by three questions");
}

#[test]
fn a_finding_seen_in_another_task_is_added_to_its_item_and_the_lead_raises_it_once_as_its_own_problem() {
    let role = task_role();
    says(section(&role, "## What you notice outside your task"), &["search the backlog for it first", "add this task to its Source"], "the same failing suite was filed four ways");
    says(&file("skills/write-backlog-item/SKILL.md"), &["Search first", "add your source to its Source"], "the backlog skill searches before it files");
    says(&lead_role(), &["seen in a second task", "its own problem", "once"], "the lead heard about it once and two tasks skipped it");
    says(&investigator(), &["seen before", "root cause"], "a recurring obstacle gets a task for its cause");
}

#[test]
fn an_obstacle_goes_to_the_investigator_and_the_lead_never_picks_a_way_from_memory() {
    let lead = lead_role();
    let obstacles = section(&lead, "## Obstacles and findings");
    says(obstacles, &["not a question of method", "from memory", "kebbie:investigate", "subagent", "effort"], "an obstacle reached the lead as a question");
    says(&playbook("OBSTACLE.md"), &["`workaround`", "`subtask`", "`task`", "`ask`", "kebbie start", "--after", "kebbie ask"], "the lead carries out each decision");
    says(section(&lead, "## Questions from tasks"), &["An obstacle is not a question of method"], "the method rule names its exception");
}

#[test]
fn the_investigator_decides_in_line_with_the_goal_and_the_codes_quality_with_its_sources() {
    let skill = investigator();
    assert!(skill.starts_with("---\nname: investigate\n"), "the lead's subagent runs it by name");
    says(&skill, &["the task's goal", "make the change easy, then make the easy change", "refactor", "context", "search", "official documentation", "`workaround`", "`subtask`", "`task`", "`ask`", "what will replace it", "one preparatory subtask", "recommend", "reason:", "plan:"], "the investigator chooses among the four with its sources");
    assert!(!skill.contains("read-only") && !skill.contains("allowlist"), "the investigator has the tools every agent has");
}

#[test]
fn a_review_finding_about_scope_or_intent_is_resolved_through_the_investigator_and_only_the_uncertain_reaches_the_user() {
    let lead = lead_role();
    says(section(&lead, "## Obstacles and findings"), &["scope", "intent", "investigat", "still uncertain"], "a confirm finding whose answer was in the task's note went to the user");
    says(&file("skills/review/SKILL.md"), &["resolved", "investigat"], "the reviewer's findings are for resolving");
}

#[test]
fn the_card_records_who_decided_and_keeps_a_reversed_decision_with_the_one_that_replaced_it() {
    let card = playbook("CARD.md");
    says(&card, &["\nBuilt:\n", "\nDecisions:\n", "\nAlso noticed:\n", "`you`", "`lead`", "`task`", "`investigator`", "replaced", "no commit or step counts", "outside the task", "English", "vault's language"], "a card rewritten between reports turned a decision without trace");
    assert!(!card.to_lowercase().contains("confirm:"), "a card has no confirm part");
}

#[test]
fn work_done_outside_the_repo_shows_under_built() {
    says(&playbook("CARD.md"), &["outside the repo"], "an ops task with no commits, and a site down for an hour, sat under Confirm");
}

#[test]
fn the_playbook_points_each_branch_at_its_page() {
    let skill = playbook("SKILL.md");
    let description = skill.lines().find(|line| line.starts_with("description:")).unwrap_or_default();
    says(description, &["obstacle", "plan changes", "sign-off card"], "the playbook's description triggers the task's branches");
    says(&skill, &["OBSTACLE.md", "PLAN.md", "CARD.md"], "the playbook links its pages");
}

#[test]
fn away_mode_investigates_obstacles_as_ever_and_only_a_question_waits() {
    says(section(&lead_role(), "## Away mode"), &["obstacle", "investigat", "`ask`", "kept"], "the user away");
}
