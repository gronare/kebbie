# Roll a change out in stages

Push a change that must reach production in steps, such as a column rename, one stage at a time, with a deploy, a check or your verification between them.

1. Start the task with the recipe for it:

   ```bash
   kebbie start email "Rename users.mail to users.email" --recipe "rollout-in-stages: the old column goes only once nothing reads it"
   ```

2. The task lists its subtasks and declares its stages in push order, each with its requirement:

   ```bash
   kebbie stages "Add the column, write both:deploy" "Backfill:verified:every row has its email in production" "Drop the old column:check:no query reads users.mail"
   kebbie stages assign add-column add-email
   ```

3. Accept the first stage's subtasks and push. The push window offers the commits up to the end of the open stage, marked `◂ through here`; nothing after it goes out.
4. kebbie follows CI and the deploy for the push. A `deploy` stage is met once the repo's [`ci.deploy`](../reference/configuration.md#ci-updates-and-the-keeper) workflow, or CI without one, is green on the pushed commit; a `check` goes to the task to run; a `verified` stage asks you in the inbox (`i`) and on the phone.
5. Answer Verified when it holds. The next stage opens, and you push it the same way.
6. After the last stage is met with nothing left to push, kebbie closes the task. `kebbie stages email` shows every stage as `met`.

- If you answer Not yet, the stage waits and the question stays.
- If you answer Something is wrong, the stage fails with what you wrote, every later stage stays locked, and the task fixes it in a subtask of that stage; its push asks you again.
- If CI fails on a stage's push or its check does not hold, the stage fails the same way, and the task gets what failed.
- If kebbie cannot see a stage deploy, because CI is not watched or no CI ran, it asks you whether it deployed.
- If the lead pushes for you, it pushes a stage only with the task's own push permission, never one after your verification.
- If the task moves to another machine, its stages start over there from its subtasks and commits: a stage's state lives with the task on its machine.
