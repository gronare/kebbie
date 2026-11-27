# Show task state in herdr's sidebar

Tag each Space in herdr's sidebar with its task's next step, model and context, and with `server` or `worker` while those run.

The keeper runs the sidebar sync every three seconds on the machine that has the panes, so the tags and the board's `server` and `worker` stay current even when you attach herdr from another machine.

1. If you run kebbie with the keeper off, add the sidebar sync to herdr's `config.toml`:

   ```toml
   [ui]
   tab_bar_right = [
     { type = "command", command = "~/.local/share/kebbie/current/scripts/sync.sh", interval_seconds = 3, timeout_seconds = 3 },
   ]
   ```

   It prints nothing; it keeps the tags up to date. With the keeper on, the line only adds a run from the attached client's machine.

2. Lay out the sidebar to show the tags. [`examples/herdr-config.toml`](../../examples/herdr-config.toml) is a complete herdr config with kebbie's keys, the sync and a sidebar layout that uses each tag. The tags show within three seconds.

- The sync names an unnamed tab after the program running in it and leaves a tab you named alone.
