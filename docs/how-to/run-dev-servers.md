# Run a task's dev server

Start a task's dev server and worker from the board, each on the worktree's own port, and open it from a link.

1. Set the commands in the repo config:

   ```toml
   # ~/.config/kelpie/repos/app.toml
   [worktree]
   dev_server = "bin/dev"
   dev_worker = "bin/jobs"
   ```

2. Make sure each worktree has a port: `PORT=` in its `.env`, or a port in its setup note. The setup skill picks a free one. Without either, no `PORT` is set.

3. On the board, select the task and press `w`. The server starts in a `server` tab of the task's Space with `PORT` set, and a `web` line under the task links to it. Press `w` again to stop it. `b` does the same for the worker.

## Change the link

The default link is `http://localhost:{port}`. Behind a proxy that serves each checkout by folder name:

```toml
# ~/.config/kelpie/config.toml
[worktree]
dev_url = "https://{worktree}.test"
```

`""` turns the link off. See [URL placeholders](../reference/configuration.md#url-placeholders).

When kelpie runs on a remote machine and your browser is on your laptop, see [Open dev servers from a remote machine](open-dev-servers-from-a-remote-machine.md).

## Rename the tabs

```toml
# ~/.config/kelpie/config.toml
[worktree]
server_name = "web"
worker_name = "jobs"
```

The names are also the Space's tags in herdr's sidebar while each runs.
