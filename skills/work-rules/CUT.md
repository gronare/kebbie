# Cutting the work into increments

"For each desired change, make the change easy (warning: this may be hard), then make the easy change" (Kent Beck). The hard part is the cut: which increments, in which order, each one deployable alone. Plan it before the first line of code, and check the branch against it before you report ready.

## Plan the cut

1. **The deliverable.** Say in one sentence what the user gets when the last commit is out. Done when the sentence names a behaviour, not a file.
2. **What makes it hard.** Find the code that has to change shape before the change is small: a function in the wrong place, a name that will mislead, an interface the change cannot reach through, a table without the column. Each becomes a preparatory increment with no behaviour change. Done when the behaviour change, written on top of them, would be a few lines and their tests.
3. **The risky steps.** Find what a revert cannot take back: a data migration, a merge of identities, a deletion, the contract that drops an old shape. Done when each is an increment of its own, placed last.
4. **Dark until whole.** For each behaviour increment, decide how it stays safe if the next one never comes: unreached until a later commit wires it in, behind a flag that is off, or a shape change split into expand, migrate and contract. Done when every increment answers "what does a user meet if only this much is deployed?" with "nothing broken".
5. **The list.** Write the increments in push order, one line each: its deliverable and the test that shows it. With subtasks on, group them into subtasks with `kebbie subtasks`; with them off, the list is your plan, told to the lead. Done when every line would pass the branch check below.

## Check the branch

Read every commit, oldest first, and give each a verdict on each question. A commit that fails one breaks the rule in brackets.

- **Green alone** (rule 5): a green run is recorded at the sha of each part's last commit (`kebbie evidence`), or `kebbie order` marks the commit `tests ✓` where kebbie tests each commit; a commit before a part's last is judged by its diff. Reading the diff, it calls nothing, and its tests expect nothing, that only a later commit adds.
- **One obvious purpose** (rule 2): its subject names one change, and every hunk serves it.
- **Self-explanatory** (rules 1 and 8): the message, names, comments and tests make sense with only this commit and the ones before it in view; no name, parameter or test is there for a later commit.
- **Safe to deploy alone** (rule 6): with only the commits up to this one deployed, a user meets no broken path, and stored data stays readable by the code before it.
- **Ordered** (rule 7): a commit with no behaviour change sits before the behaviour change it prepares; a risky or irreversible commit sits last and carries nothing else.

Done when each commit has a verdict on each question. Fix a break before you report ready: fold a commit into the one it completes, move a preparatory one up with `kebbie order move`, bind commits that only work together with `kebbie order group`, or split a commit that holds two purposes by building its first half again from the commit before it.

## Ways to keep an increment safe

- **Preparatory refactoring** (Martin Fowler): restructure until the change fits, as commits of their own with no behaviour change.
- **Tidy first** (Kent Beck): structure changes and behaviour changes go in separate commits, the tidying first when it makes the change cheaper.
- **Parallel change** (Danilo Sato): expand the interface to take both the old and the new shape, migrate the callers, then contract by dropping the old; three increments, the contract last.
- **Dark launch and feature flags** (Martin Fowler, Pete Hodgson): the new path runs, or sits ready, unseen by users until a later commit or a flag turns it on.

## Worked example: anonymous tracking

A task adds tracking of visitors who have not logged in. Cut so that each commit goes out alone, it takes six increments, in push order:

1. **Move the tracking code** to where both logged-in and anonymous requests reach it. A refactor: no behaviour change, the existing specs pass untouched.
2. **Attribute experiments before login**: an experiment assignment made before login is kept with the visitor. Its specs observe the assignment the server stores, which this commit exposes.
3. **Rotate the device id.** Standalone and server-side, so its specs read the id the server issues.
4. **Bootstrap the client**: the page hands the client its device id. Nothing reads it yet, so deploying it changes nothing a user meets.
5. **Record anonymous interactions**, the first behaviour the user's analytics show, now a small change on top of 1 to 4.
6. **Record anonymous page views**, the same path for page views.

What a re-cut like this costs: each moved commit is built again from the commit before its new place, and specs written against the finished feature are rewritten so that each observes what its own commit exposes, as in 2 and 3, rather than behaviour that only 5 makes visible. The order holds when step 2 of the plan found every preparatory change; a preparatory change found during 5 moves up with `kebbie order move`.

## Sources

- Kent Beck, 25 September 2012: https://x.com/KentBeck/status/250733358307500032
- Kent Beck, *Tidy First?*, O'Reilly, 2023.
- Martin Fowler, An example of preparatory refactoring, 2015: https://martinfowler.com/articles/preparatory-refactoring-example.html
- Danilo Sato, ParallelChange, 2014: https://martinfowler.com/bliki/ParallelChange.html
- Martin Fowler, DarkLaunching, 2020: https://martinfowler.com/bliki/DarkLaunching.html
- Pete Hodgson, Feature Toggles, 2017: https://martinfowler.com/articles/feature-toggles.html
