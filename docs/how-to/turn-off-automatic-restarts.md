# Turn off automatic restarts

Keep Claude sessions running on the code they started with until you restart them yourself.

1. Add this to `~/.config/kebbie/config.toml`:

   ```toml
   [restart]
   auto = false
   ```

2. Check the board. It still marks stale sessions with `↻ N stale`, and the lead's line says why it is stale, such as `↻ The lead runs the old kebbie until you restart it · L restarts it`. Nothing restarts them on its own, and `kebbie update` restarts none.

3. Restart the stale sessions that are idle, not the pane you are in, when it suits you:

   ```bash
   kebbie restart
   ```

- If you want every idle session restarted, stale or not, run `kebbie restart --all`.
- If the board's lead line names a fix instead of `L`, such as putting a newer Claude Code first on `PATH`, a restart cannot mend the lead: make that fix, then restart it.
- If you want automatic restarts but more time first, raise `idle_minutes` instead. It is minutes, from 1 to 1440:

  ```toml
  [restart]
  idle_minutes = 60
  ```

See also: [When restarts happen](../explanation/updates-and-restarts.md#when-restarts-happen).
