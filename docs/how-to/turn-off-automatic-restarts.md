# Turn off automatic restarts

Keep Claude sessions running on the code they started with until you restart them yourself.

1. Add this to `~/.config/kebbie/config.toml`:

   ```toml
   [restart]
   auto = false
   ```

   The board still marks stale sessions with `↻ N stale`, but nothing restarts them on its own, and `kebbie update` restarts none.

2. Restart them when it suits you:

   ```bash
   kebbie restart          # stale sessions that are idle, not the pane you are in
   kebbie restart --all    # every idle session, stale or not
   ```

## Wait longer instead

To keep automatic restarts but give sessions more time, raise the idle time. It is minutes, from 1 to 1440:

```toml
[restart]
idle_minutes = 60
```

See also: [When restarts happen](../explanation/updates-and-restarts.md#when-restarts-happen).
