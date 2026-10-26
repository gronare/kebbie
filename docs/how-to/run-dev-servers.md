# Run a task's dev server

Start a task's dev server and worker from the board, each on the worktree's own port, and open it from a link.

1. Set the commands in the repo config:

   ```toml
   # ~/.config/kebbie/repos/app.toml
   [worktree]
   dev_server = "bin/dev"
   dev_worker = "bin/jobs"
   ```

2. Give each worktree a port: `PORT=` in its `.env`, or a port in its setup note. The setup skill picks a free one.
3. Select the task on the board.
4. Press `w`.

The server starts in a `server` tab of the task's Space with `PORT` set, and a `web` line under the task links to it.

- If you want to stop it, press `w` again.
- If you want the worker, press `b`. It starts and stops the same way.
- If a worktree has neither a `PORT=` nor a port in its setup note, no `PORT` is set.
- If kebbie runs on a remote machine and your browser is on your laptop, see [Open dev servers from a remote machine](open-dev-servers-from-a-remote-machine.md).

## Change the link

The default link is `http://localhost:{port}`. Behind a proxy that serves each checkout by folder name:

```toml
# ~/.config/kebbie/config.toml
[worktree]
dev_url = "https://{worktree}.test"
```

`""` turns the link off. See [URL placeholders](../reference/configuration.md#url-placeholders).

## Rename the tabs

```toml
# ~/.config/kebbie/config.toml
[worktree]
server_name = "web"
worker_name = "jobs"
```

The names are also the Space's tags in herdr's sidebar while each runs.
