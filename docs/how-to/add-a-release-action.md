# Add a release action to a repo

Release a repo from the board's Areas page with one key: bump the version, build, commit, tag and push.

![The Areas page: areas with their repos, actions and vault log](../assets/clips/areas-page.gif)

The Areas page needs the vault. The main checkout must be on the default branch, clean and not behind origin.

1. Add an `[[actions]]` table to the repo config, `~/.config/kebbie/repos/billing.toml`:

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
   kebbie doctor
   ```

3. On the board, press `2` for the Areas page.

4. Select the repo's area and press `R`.

5. Enter the version. The next patch version is prefilled.

6. Read the steps in the window, including the exact lines the bump changes, and press `y` to run them.

7. Press `y` again to push.

Every key: [`[[actions]]`](../reference/configuration.md#actions).

## Hand the rest to a task

An action can start a task instead of running a script. With `then`, one action offers another when it is done.

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
# ~/.config/kebbie/repos/web.toml
[[actions]]
id = "bump-client"
key = "P"
title = "Bump the client"
ask = ["package", "version"]
task = "Bump {package} to {version}; commit."
```
