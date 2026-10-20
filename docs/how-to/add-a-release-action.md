# Add a release action to a repo

Release a repo from the board's Areas page with one key: bump the version, build, commit, tag and push.

1. Add an `[[actions]]` table to the repo config, `~/.config/kelpie/repos/billing.toml`:

   ```toml
   [[actions]]
   key = "R"
   title = "Release"
   about = "bump, commit, tag, push"
   ask = "version"
   bump = ["Cargo.toml", { file = "Cargo.lock", after = 'name = "billing"' }]
   run = ["cargo build"]
   commit = "{version}"
   tag = "v{version}"
   push = true
   ```

2. Check it parses:

   ```bash
   kelpie doctor
   ```

3. On the board, press `2` for the Areas page, select the repo's area and press `R`.

4. Enter the version; the next patch version is prefilled. The window shows every step, including the exact lines the bump changes. Press `y` to run it, and `y` again to push.

The main checkout must be on the default branch, clean and not behind origin.

## Hand the rest to a task

An action can start a task instead of running a script, and one action can offer another when it is done:

```toml
[[actions]]
key = "R"
title = "Release"
ask = "version"
bump = ["Cargo.toml"]
commit = "{version}"
tag = "v{version}"
push = true
then = ["web:bump-client"]
```

```toml
# ~/.config/kelpie/repos/web.toml
[[actions]]
id = "bump-client"
key = "P"
title = "Bump the client"
ask = ["package", "version"]
task = "Bump {package} to {version}; commit."
```

The Areas page needs the vault. Every key: [`[[actions]]`](../reference/configuration.md#actions).
