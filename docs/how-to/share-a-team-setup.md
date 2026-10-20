# Share a team setup

Keep your team's kelpie repo configs in a team repo, so one pull updates everyone.

## Publish the setup

1. In the team repo, add the repo configs, for example `kelpie/repos/app.toml`.

2. Add a short file that lists each file and where it links, such as `kelpie/README.md`:

   ```markdown
   # kelpie setup

   Link each file into kelpie's config folder with `ln -s`, or ask Claude to adopt it with `/kelpie:configure`.

   | File here | Link at |
   | --- | --- |
   | `kelpie/repos/app.toml` | `~/.config/kelpie/repos/app.toml` |
   ```

   A repo config is named after the folder each developer clones that repo into. Say which folder name it expects when that differs from the repo's name.

3. Commit and push it.

## Adopt the setup

Ask Claude to "adopt the team's kelpie setup". The `kelpie:configure` skill finds the file, makes the links and checks them.

Or link each file yourself:

```bash
mkdir -p ~/.config/kelpie/repos
ln -s ~/code/team-config/kelpie/repos/app.toml ~/.config/kelpie/repos/app.toml
kelpie doctor
```

`kelpie doctor` reads `ok` for each link that resolves and `points at nothing` for one whose target is missing.

## Keep a value of your own

A linked file is the team's. To differ from it for one repo, replace the link with your own file. For machine-wide values, use `~/.config/kelpie/config.toml`; a repo config wins over it key by key.
