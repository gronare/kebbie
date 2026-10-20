# Environment variables

## Read by kelpie

| Variable | Default | Meaning |
| --- | --- | --- |
| `KELPIE_CONFIG` | `$XDG_CONFIG_HOME/kelpie/config.toml`, else `~/.config/kelpie/config.toml` | The machine config. The local `backlog.md` is read next to it. |
| `KELPIE_REPOS_DIR` | `$XDG_CONFIG_HOME/kelpie/repos`, else `~/.config/kelpie/repos` | The folder of repo configs. |
| `KELPIE_STATE_DIR` | `$XDG_STATE_HOME/kelpie`, else `~/.local/state/kelpie` | The state directory. Point it at a temp directory for a throwaway run. |
| `KELPIE_REPO_DIRS` | `~/projects:~/personal:~/src:~/code` | Colon-separated folders where `start --repo` looks for checkouts, in them or one level down. |
| `KELPIE_LEAD_DIR` | `lead` in the state directory | The lead Claude's working directory. |
| `KELPIE_BIN` | `~/.local/share/kelpie/current/target/release/kelpie` | The binary the lead's role, the task role, kelpie's skills and the sidebar sync run. |
| `KELPIE_CLAUDE_BIN` | `claude` | The Claude Code binary `kelpie setup vault` runs. |
| `KELPIE_GH_BIN` | `gh` | The GitHub CLI kelpie watches CI with. |
| `GH_HOST` | none | A GitHub Enterprise host whose remotes kelpie watches CI for, besides github.com. |
| `KELPIE_VAULT_BIN` | none | A vault command that stands in for the vault plugin, for tests. |
| `KELPIE_IMAGES` | detected | How the board draws images such as a sign-off card's screenshots: `kitty`, `sixel` or `iterm2` use that graphics protocol, `halfblocks` draws a preview in text cells without querying the terminal, `off` shows their paths instead. Detected, it asks the terminal when the first image is about to show; inside herdr (`TERM_PROGRAM=herdr`) it asks nothing and uses Kitty, and the board not on a terminal shows paths. |
| `HERDR_BIN_PATH` | `herdr` | The herdr binary. `false` makes kelpie reach no herdr at all. |
| `CLAUDE_CONFIG_DIR` | `~/.claude` | Where kelpie reads Claude Code's installed plugins and their settings. |
| `XDG_CONFIG_HOME` | `~/.config` | Base for the config files. |
| `XDG_STATE_HOME` | `~/.local/state` | Base for the state directory. |
| `XDG_DATA_HOME` | `~/.local/share` | Where herdr's startup writes the `current` link. kelpie reads the link under `~/.local/share`, so `kelpie doctor` warns when this is set elsewhere. |

The vault index is located from the vault plugin's options, or from `VAULT_PATH`, `S3_ENDPOINT`, `S3_BUCKET`, `S3_PREFIX` and `VAULT_CACHE_DIR` as notes-vault-mcp takes them.

## Set by kelpie

For `notify.command`:

| Variable | Value |
| --- | --- |
| `KELPIE_EVENT` | `needs-you`, `ready`, `question`, `summary`, `ci-failed` or `ci-green` |
| `KELPIE_TASK` | The task's name |
| `KELPIE_REPO` | The task's repo |
| `KELPIE_TEXT` | The notification's one line |

For `worktree.dev_server` and `worktree.dev_worker`:

| Variable | Value |
| --- | --- |
| `PORT` | The worktree's port: `PORT` in its `.env`, else the port in its setup note. Unset when neither has one. |

## Internal

`KELPIE_BOARD_STATE` and `KELPIE_PUSH_TOKEN` pass state between kelpie's own processes. Never set them.

## A throwaway run

To run kelpie without touching your real state or herdr:

```bash
KELPIE_STATE_DIR=$(mktemp -d) HERDR_BIN_PATH=false kelpie status
```
