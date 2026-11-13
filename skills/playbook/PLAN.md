# A living plan

A task's plan is the work as it knows it now: the next parts concrete, the later ones rough. It is iterated the lean way: plan, build, finding, revised plan. Every revision after the first listing carries its reason, and the lead hears it, so the user can follow why the work went the way it did.

## Steps

1. **Plan.** Cut the work into parts that can each be reviewed and pushed on their own: make the change easy, then make the easy change. A preparatory part (a move, a rename, an interface, a migration) comes before the part it prepares. Done when the next part is concrete enough to start, and each later part has a title.
2. **Build** the next part, its tests with it. Done when it is committed and its tests pass on that commit.
3. **Take in the finding.** What the build taught you sorts by the task role's three questions: an obstacle goes the way of [OBSTACLE.md](OBSTACLE.md), a choice that changes what the user gets is their question, the rest is method and yours. Done when each finding has its place.
4. **Revise.** Change the plan to what you know now, and tell the lead: `kebbie tell lead "plan: <the change>, because <the reason>"`. With steps on, run `kebbie steps` with the new list too; with a vault note, add the revision to its decisions. Done when the plan, the lead and the note say the same thing.

## When a revision is the user's

A revision changes what the user gets when a part of a plan they approved is dropped, replaced or narrowed, or when it changes what the work costs them or how long it takes. That revision is a revision only once they answer: ask with `kebbie ask` before you build on it, by the task role's question rules. A preparatory part added ahead of the rest changes nothing they get; its reason is enough.

## Moving a preparatory part forward

A preparatory part found late belongs ahead of the work it prepares, so each commit can go out on its own. Build it from the commit before that work (a temporary checkout of it), commit it there, and rebase the later commits onto it with `git rebase` (no `-i`). Rebase only commits that are not pushed.
