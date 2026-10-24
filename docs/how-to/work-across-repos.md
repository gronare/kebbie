# Work across several repos

Run one task that changes two repos at once, such as an event one service sends and another consumes.

1. Start the task with one `--repo` per repo:

   ```bash
   kebbie start payload-v2 "Add currency to the payment event." --repo api --repo web
   ```

   Each repo gets a half: a worktree on a branch `payload-v2`, its own Space and its own Claude, each told of the other. The first repo's Space gets focus.

2. Message both halves at once:

   ```bash
   kebbie tell payload-v2 "The field is called currency_code."
   ```

   To reach one half, address it as `payload-v2@web`.

3. On the board, the halves sit next to each other. Push each half on its own with `p`, in the order the change needs: usually the side that accepts the new field first.

## Where kebbie finds the repos

`--repo` takes a folder name. kebbie looks in `~/projects`, `~/personal`, `~/src` and `~/code`, in them or one level down, and clones a missing repo from an owner among your other checkouts' remotes. To look elsewhere:

```bash
export KEBBIE_REPO_DIRS="$HOME/work:$HOME/code"
```

With the vault, both halves share one task note.
