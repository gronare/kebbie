# Show task state in herdr's sidebar

Tag each Space in herdr's sidebar with its task's next step, model and context, and with `server` or `worker` while those run.

1. Add the sidebar sync to herdr's `config.toml`:

   ```toml
   [ui]
   tab_bar_right = [
     { type = "command", command = "~/.local/share/kebbie/current/scripts/sync.sh", interval_seconds = 3, timeout_seconds = 3 },
   ]
   ```

   It prints nothing; it keeps the tags up to date.

2. Lay out the sidebar to show them. [`examples/herdr-config.toml`](../../examples/herdr-config.toml) is a complete herdr config with kebbie's keys, the sync and a sidebar layout that uses each tag.

To stop the sync, remove its line from `tab_bar_right`.
