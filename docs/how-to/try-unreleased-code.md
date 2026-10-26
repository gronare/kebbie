# Try unreleased code

Run a branch or a local checkout of kebbie for a while, then go back to your channel.

1. Install a branch from origin:

   ```bash
   kebbie update --from fix-board
   ```

   Or install another checkout's commits:

   ```bash
   kebbie update --from ~/code/kebbie
   ```

2. Check what runs. `kebbie update --check`, `kebbie doctor` and the board say `running <branch or path> @ <sha>, not the <channel> channel`.

3. Pull in new commits while you try it. A plain `kebbie update` fetches the same branch or checkout again:

   ```bash
   kebbie update
   ```

4. Go back to the channel:

   ```bash
   kebbie update --stable
   ```

   `kebbie update --check` no longer says `running <branch or path> @ <sha>`.

- If the checkout has uncommitted changes, it is refused. Only commits are installed, so commit your changes first.
- If the path is the install root itself, it is refused.

See also: [Updates and stale sessions](../explanation/updates-and-restarts.md).
