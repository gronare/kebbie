# Switch update channel

Follow tagged releases, or every push to kebbie's default branch.

1. Set the channel in `~/.config/kebbie/config.toml`. `release` is the default:

   ```toml
   [update]
   channel = "main"
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

- If you switch to `release` from a checkout on a branch, see [Switch to release from a branch](#switch-to-release-from-a-branch).
- If `kebbie doctor` reports `claude plugin source: GitHub, following the default branch, while this machine follows release tags`, see [Point the Claude Code plugin at the install root](#point-the-claude-code-plugin-at-the-install-root).

## Switch to release from a branch

The release channel checks out tags in the install root. It refuses an install root that sits on a branch or has uncommitted changes.

1. Move a clean checkout to the release tag:

   ```bash
   kebbie update --stable
   ```

   The branch keeps its commits.

Develop in another checkout, never in the install root of a release machine.

## Point the Claude Code plugin at the install root

1. Remove the GitHub marketplace:

   ```bash
   claude plugin marketplace remove kebbie
   ```

2. Add the install root as the marketplace:

   ```bash
   claude plugin marketplace add ~/.local/share/kebbie/current
   ```

3. Install the plugin from it:

   ```bash
   claude plugin install kebbie@kebbie
   ```

4. Run `kebbie doctor`. It reads `claude plugin source: loaded in place from` the install root.

See also: [Updates and stale sessions](../explanation/updates-and-restarts.md), [`update.channel`](../reference/configuration.md#keys).
