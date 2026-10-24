# Switch update channel

Follow tagged releases, or every push to kebbie's default branch.

1. Open `~/.config/kebbie/config.toml` and set the channel:

   ```toml
   [update]
   channel = "main"      # or "release", the default
   ```

2. Install what the channel points at:

   ```bash
   kebbie update
   ```

3. Check the result:

   ```bash
   kebbie update --check
   ```

   It shows the installed binary's version and commit against the channel's latest, such as `kebbie  0.24.0 installed (e70d5d5), 0.25.0 available`.

## Switching to `release` from a checkout on a branch

The release channel checks out tags in the install root and refuses an install root that sits on a branch or has uncommitted changes. Move a clean checkout to the release tag first:

```bash
kebbie update --stable
```

The branch keeps its commits. Develop in another checkout, never in the install root of a release machine.

## The Claude Code plugin

When `kebbie doctor` reports `claude plugin source: GitHub, following the default branch, while this machine follows release tags`, point the marketplace at the install root:

```bash
claude plugin marketplace remove kebbie
claude plugin marketplace add ~/.local/share/kebbie/current
claude plugin install kebbie@kebbie
```

See also: [Updates and stale sessions](../explanation/updates-and-restarts.md), [`update.channel`](../reference/configuration.md#keys).
