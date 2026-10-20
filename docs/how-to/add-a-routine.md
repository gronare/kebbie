# Add a routine

Have kelpie run a check on a schedule, as a task that reports what it finds or proposes a plan.

## Check a cluster's health every six hours

The cluster's manifests live in a gitops repo, `~/code/infra`, so the routine goes in that repo's config and runs there.

1. Add a `[[routines]]` table to `~/.config/kelpie/repos/infra.toml`:

   ```toml
   [[routines]]
   id = "cluster-health"
   title = "Cluster health"
   every = "6h"
   prompt = """
   Check the cluster with kubectl: every node Ready, no pod crash-looping or pending for long, \
   every volume attached and healthy, and every gitops bundle reconciled.
   """
   ```

   `mode` is left at `"report"`: the task looks and changes nothing.

2. Check it parses and see when it runs:

   ```bash
   kelpie doctor
   kelpie routines
   ```

3. To try it at once, start a run now:

   ```bash
   kelpie routine run cluster-health
   ```

When all is well the task closes itself with `kelpie routine ok` and you hear nothing. When it finds something, it waits under Needs you with a card listing each finding, and your notify command gets a `routine-findings` event.

## Propose dependency updates every Monday

1. Add a routine to the app's repo config, `~/.config/kelpie/repos/app.toml`:

   ```toml
   [[routines]]
   id = "dependency-updates"
   title = "Dependency updates"
   at = "mon 07:00"
   mode = "plan"
   prompt = "Find the outdated dependencies, read their changelogs for breaking changes, and propose which to update and how."
   ```

2. Check it with `kelpie routines`.

On Monday at 07:00, or when the machine wakes after it, the task investigates and puts a plan to you on the board. Approve it there and the task makes the updates in commits and reports ready; you push them as any task.

## A routine in config.toml

A routine in `~/.config/kelpie/config.toml` names its repo, by folder name or path:

```toml
[[routines]]
id = "disk-space"
title = "Disk space"
repo = "~/code/infra"
at = "07:00"
prompt = "Check free disk space on every node and flag any under 15 percent."
```

## Turn one off

Set `enabled = false` in its table. `kelpie routine run <id>` still starts it by hand.

See also: [Routines](../explanation/routines.md), [`[[routines]]`](../reference/configuration.md#routines).
