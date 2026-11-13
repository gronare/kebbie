# An obstacle on the task's path

An **obstacle** stops a piece of the task's work as planned: a precondition the work needs is missing, a step cannot be built as planned, a trap makes it unsafe, or it goes only by a workaround. A red test the task did not cause and a check it cannot run (no device, a refused permission, a missing tool) are obstacles too. Something outside the task is not one: that is Also noticed. A choice of method is not one either: that is the task's own call.

The investigator decides what happens to an obstacle, in line with the task's goal. The task describes it; the lead runs the investigator and carries out its decision.

## The task

1. **Stop** the work the obstacle blocks: no commit that builds it another way. Done when your last commit is one you would keep whatever the decision.
2. **Write it** so the investigator can decide without asking back: what the work needs, what is there instead with `file:line`, what you read to know it (the code, a note, a tool's documentation), and each way you see, the workaround among them, without choosing. Done when someone who has not seen your session could open each place you name and see the obstacle.
3. **Raise it**: `kebbie tell lead "obstacle: <what you wrote>"`. Done when the message is sent.
4. **Go on** with work the obstacle does not touch, or report `--state blocked` with the obstacle as the summary when nothing is left. Done when your report says which.
5. **Follow the decision** when it comes: go on with the workaround and the line it names, build the subtask first and ahead of what it prepares, wait for the task it names, or ask the user the question it wrote. Record it in your note's decisions, by `investigator`. Done when your plan matches the decision.

Two obstacles from real tasks, as they should have been raised:

- `obstacle: the plan's "lint from the index" step needs each note's frontmatter; the index stores only titles and paths (src/index.py:88). Read the index schema and its writer. Ways: store frontmatter in the index (a migration of the index); fetch the notes in parallel instead (no index change, slower lint).`
- `obstacle: the reload fix can only be confirmed on the phone, which this task cannot reach. Read the worker's update path (web/sw.js:40). Ways: ask the user to check it on their phone before the push; push and measure on production.`

## The lead

1. **Investigate.** Start a subagent with high reasoning effort whose prompt runs the skill `kebbie:investigate` and gives it the task's address, its worktree, and the obstacle word for word. Do it on the same turn the obstacle arrives, whether or not the user is present. Done when its answer block is back.
2. **Carry it out** by its decision, and tell the task the decision, its reason and what changes in its plan with `kebbie tell <task>`:
   - **`workaround`**: the task goes on as it proposed, and the line names what will replace the workaround.
   - **`subtask`**: the task adds the preparatory work to its plan, first in push order, ahead of what it prepares.
   - **`task`**: start the task it names, by this playbook's recipes, and tell the waiting task which one it waits for. When the blocked task has nothing else to build, its own remaining work can be queued behind it with `kebbie start <name> "<prompt>" --repo <repo> --after <task>`. When a running task already builds it, point the waiting task at that one.
   - **`ask`**: tell the task to ask the user with `kebbie ask`, the investigator's question, options and recommendation as they stand.

   Done when the task has the decision with its reason.
3. **Look across tasks.** When the investigator found the obstacle seen before in another task, bring it to the user once as its own problem, with your recommendation. Done when it is raised or already raised.

A review's finding about scope, the way chosen or intent goes through the same two steps, the finding in place of the obstacle. Only an `ask`, or a finding the investigator is still unsure of, reaches the user.
