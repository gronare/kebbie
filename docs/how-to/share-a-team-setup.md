# Share a team setup

Keep your team's kebbie repo configs in a team repo, so one pull updates everyone.

## Publish the setup

1. In the team repo, add the repo configs, for example `kebbie/repos/app.toml`.

   A repo config is named after the folder each developer clones that repo into. Say which folder name it expects when that differs from the repo's name.

2. Add a short file that lists each file and where it links, such as `kebbie/README.md`:

   ```markdown
   # kebbie setup

   Link each file into kebbie's config folder with `ln -s`, or ask Claude to adopt it with `/kebbie:configure`.

   | File here | Link at |
   | --- | --- |
   | `kebbie/repos/app.toml` | `~/.config/kebbie/repos/app.toml` |
   ```

3. Commit and push it.

## Adopt the setup

1. Ask Claude to "adopt the team's kebbie setup". The `kebbie:configure` skill finds the file, makes the links and checks them.

2. Check the links:

   ```bash
   kebbie doctor
   ```

   It reads `ok` for each link that resolves and `points at nothing` for one whose target is missing.

- If you link the files yourself, create the folder and link each file, then run `kebbie doctor`:

  ```bash
  mkdir -p ~/.config/kebbie/repos
  ```

  ```bash
  ln -s ~/code/team-config/kebbie/repos/app.toml ~/.config/kebbie/repos/app.toml
  ```

- If you want to differ from the team for one repo, replace the link with your own file. A linked file is the team's.
- If you want machine-wide values, use `~/.config/kebbie/config.toml`. A repo config wins over it key by key.
