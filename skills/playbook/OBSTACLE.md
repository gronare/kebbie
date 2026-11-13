# An obstacle on the task's path

An **obstacle** stops a piece of the task's work as planned: a precondition the work needs is missing, a part of the plan cannot be built as planned, a trap makes it unsafe, or it goes only by a workaround. A red test the task did not cause and a check it cannot run (no device, a refused permission, a missing tool) are obstacles too. Something outside the task is not one: that is Also noticed. A choice of method is not one either: that is the task's own call.

The investigator decides what happens to an obstacle, in line with the task's goal. The task describes it and raises it; kebbie runs its investigator, records the decision with who took it, and carries it out.

## The task

1. **Stop** the work the obstacle blocks: no commit that builds it another way. Done when your last commit is one you would keep whatever the decision.
2. **Write it** so the investigator can decide without asking back: what the work needs, what is there instead with `file:line`, what you read to know it (the code, a note, a tool's documentation), and each way you see, the workaround among them, without choosing. Done when someone who has not seen your session could open each place you name and see the obstacle.
3. **Raise it**: `kebbie obstacle --on "<the work it blocks>" "<what you wrote>"`, `--on` naming the subtask or part it blocks (the subtask in progress when left out). Done when kebbie answers `Raised <id>`; an answer that it was raised or decided before is your decision.
4. **Go on** with work the obstacle does not touch, or report `--state blocked` with the obstacle as the summary when nothing is left. Done when your report says which.
5. **Follow the decision** when it comes from kebbie: go on with the workaround and the line it names, build the subtask before what it prepares (when you work in subtasks, kebbie adds it, sends your next commits to it and tells you its id: `kebbie subtasks done <id>` once it is built), wait for the task it names, or wait for the user's answer to the question kebbie asked. Done when your plan matches the decision.

Two obstacles from real tasks, as they should have been raised, each as the text of `kebbie obstacle`:

- `the plan's "lint from the index" subtask needs each note's frontmatter; the index stores only titles and paths (src/index.py:88). Read the index schema and its writer. Ways: store frontmatter in the index (a migration of the index); fetch the notes in parallel instead (no index change, slower lint).`
- `the reload fix can only be confirmed on the phone, which this task cannot reach. Read the worker's update path (web/sw.js:40). Ways: ask the user to check it on their phone before the push; push and measure on production.`

## kebbie and the lead

kebbie carries out each decision and sends it to the task:

- **`workaround`**: the task goes on as it proposed; when nothing named replaces it, kebbie files the real fix in the backlog.
- **`subtask`**: kebbie adds the preparatory subtask, with the investigator's reason, ahead of the subtask the task's commits go to now, and sends the next commits to it until the task marks it done. One per task needs nobody; a second becomes the user's question.
- **`task`**: kebbie queues the task it names to start at once, the lead's way, or points the waiting task at the running one that builds it.
- **`ask`**: kebbie asks the user on the board, with the investigator's options and recommendation; their answer goes to the task as theirs.

The same obstacle raised again gets its decision without a second investigation. A second obstacle on the same work, or a task's third, asks the user whether the plan holds.

The lead decides only what reaches it: an obstacle the investigator left without a decision, or every obstacle when `investigate.profile` is `"none"`.

1. **Investigate** as the investigator would: a subagent with high reasoning effort running the skill `kebbie:investigate`, given the task's address, its worktree and the obstacle word for word. Done when its answer block is back.
2. **Decide** with `kebbie obstacles decide <task> "<decision, its reason, and what changes in the plan>"`, and carry out what needs your hands: start the task it names by this playbook's recipes, or have the task ask the user. Done when the task has the decision.
3. **Look across tasks.** When the investigator found the obstacle seen before in another task, bring it to the user once as its own problem, with your recommendation. Done when it is raised or already raised.

A review's finding about scope, the way chosen or intent goes to the investigator the same way, raised by kebbie. Only an `ask`, or a finding the investigator is still unsure of, reaches the user.
