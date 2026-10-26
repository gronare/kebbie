# Environment variables

The variables kebbie reads, the ones it sets for the commands it runs, and how to run it without touching your real state.

## Read by kebbie

| Variable | Default | What it does |
| --- | --- | --- |
| `KEBBIE_CONFIG` | `$XDG_CONFIG_HOME/kebbie/config.toml`, else `~/.config/kebbie/config.toml` | The machine config. The local `backlog.md` is read next to it. |
| `KEBBIE_REPOS_DIR` | `$XDG_CONFIG_HOME/kebbie/repos`, else `~/.config/kebbie/repos` | The folder of repo configs. |
| `KEBBIE_PROFILES_DIR` | `$XDG_CONFIG_HOME/kebbie/profiles`, else `~/.config/kebbie/profiles` | The folder of [profiles](configuration.md#profiles). |
| `KEBBIE_STATE_DIR` | `$XDG_STATE_HOME/kebbie`, else `~/.local/state/kebbie` | The state directory. Point it at a temp directory for a throwaway run. |
| `KEBBIE_REPO_DIRS` | `~/projects:~/personal:~/src:~/code` | Colon-separated folders where `start --repo` looks for checkouts, in them or one level down. |
| `KEBBIE_LEAD_DIR` | `lead` in the state directory | The lead Claude's working directory. |
| `KEBBIE_BIN` | `~/.local/share/kebbie/current/target/release/kebbie` | The binary the lead's role, the task role, kebbie's skills and the sidebar sync run. |
| `KEBBIE_CLAUDE_BIN` | `claude` | The Claude Code binary `kebbie setup vault` runs. |
| `KEBBIE_GH_BIN` | `gh` | The GitHub CLI kebbie watches CI with. |
| `GH_HOST` | none | A GitHub Enterprise host whose remotes kebbie watches CI for, besides github.com. |
| `KEBBIE_RELEASES_URL` | `https://github.com/<owner>/<repo>/releases/download` for the install's GitHub origin | Where a release's prebuilt binaries are downloaded from. |
| `KEBBIE_CHANNEL` | `update.channel` | Read by `scripts/build.sh`: `release` never builds from source. |
| `KEBBIE_VAULT_BIN` | none | A vault command that stands in for the vault plugin, for tests. |
| `KEBBIE_IMAGES` | detected | How the board draws images such as a sign-off card's screenshots: `kitty`, `sixel`, `iterm2`, `halfblocks` or `off`. |
| `KEBBIE_QR` | detected | How the hub's QR codes are drawn: `octant` or `braille`. |
| `KEBBIE_HUB_DEV` | unset | Use the dev hub at `KEBBIE_HUB_DEV_URL` instead of [`[hub] url`](configuration.md#keys). |
| `KEBBIE_HUB_DEV_URL` | `http://localhost:8470` | The dev hub's address while `KEBBIE_HUB_DEV` is set; plain `http` only on this machine. |
| `HERDR_BIN_PATH` | `herdr` | The herdr binary. `false` makes kebbie reach no herdr at all. |
| `CLAUDE_CONFIG_DIR` | `~/.claude` | Where kebbie reads Claude Code's installed plugins and their settings. |
| `CODEX_HOME` | `~/.codex` | Codex's home: where `kebbie setup codex` installs kebbie's plugin, and where kebbie reads a Codex task's session files and the folders Codex trusts. |
| `XDG_CONFIG_HOME` | `~/.config` | Base for the config files. |
| `XDG_STATE_HOME` | `~/.local/state` | Base for the state directory. |
| `XDG_DATA_HOME` | `~/.local/share` | Where herdr's startup writes the `current` link. |

`KEBBIE_LEAD_DIR`: kebbie moves the lead's chat pane there before each start, as Claude Code never remembers trust for the home folder.

`KEBBIE_IMAGES`:

- `kitty`, `sixel` or `iterm2` use that graphics protocol. `halfblocks` draws a preview in text cells without querying the terminal. `off` shows their paths instead.
- Detected, it asks the terminal when the first image is about to show. Inside herdr (`TERM_PROGRAM=herdr`) it asks nothing and uses Kitty, and the board not on a terminal shows paths.

`KEBBIE_QR` draws in the terminal's own colours, with the glyphs as the light modules: `octant` in octant blocks, `braille` in braille dots. Detected, it is `octant` in these terminals and `braille` anywhere else:

- Ghostty: `GHOSTTY_RESOURCES_DIR` set, as inside herdr in Ghostty, or `TERM_PROGRAM=ghostty`;
- kitty: `KITTY_WINDOW_ID` set or `TERM=xterm-kitty`;
- WezTerm: `WEZTERM_PANE` set or `TERM_PROGRAM=WezTerm`.

`KEBBIE_HUB_DEV`, set to anything but `0` or `false`:

- Every `kebbie hub` command, the board's `H` window and `kebbie setup`'s hub step use the dev hub, with a separate identity in `hub-dev/` in the state directory. `[hub] url` and `hub/` are neither read nor written.
- `kebbie hub status`, the `H` window and the connect card say `dev hub · localhost:8470`.
- The keeper posts the board to each hub this machine is connected to, `hub/` and `hub-dev/` alike, whether it was started with the variable or not.

`XDG_DATA_HOME`: kebbie reads the `current` link under `~/.local/share`, so `kebbie doctor` warns when this is set elsewhere.

The vault index is located from the vault plugin's options, or from `VAULT_PATH`, `S3_ENDPOINT`, `S3_BUCKET`, `S3_PREFIX` and `VAULT_CACHE_DIR` as notes-vault-mcp takes them.

## Set by kebbie

For `notify.command`:

| Variable | Value |
| --- | --- |
| `KEBBIE_EVENT` | `needs-you`, `ready`, `question`, `summary`, `ci-failed`, `ci-green`, `api-error`, `routine-findings` or `routine-failed` |
| `KEBBIE_TASK` | The task's name |
| `KEBBIE_REPO` | The task's repo |
| `KEBBIE_TEXT` | The notification's one line |

For every command a Codex task runs:

| Variable | Value |
| --- | --- |
| `KEBBIE_ADDRESS` | The task's address, `<task>@<repo>` |
| `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_0`, `GIT_CONFIG_VALUE_0` | The push block's `url.kebbie-blocks-push:.pushInsteadOf` |
| `TMPDIR` | The task's temp folder, in its worktree's git dir |

For `worktree.dev_server` and `worktree.dev_worker`:

| Variable | Value |
| --- | --- |
| `PORT` | The worktree's port: `PORT` in its `.env`, else the port in its setup note. Unset when neither has one. |

## Internal

`KEBBIE_BOARD_STATE`, `KEBBIE_PUSH_TOKEN` and `KEBBIE_REVIEWER_BIN` pass state between kebbie's own processes. Never set them.

## A throwaway run

To run kebbie without touching your real state or herdr:

```bash
KEBBIE_STATE_DIR=$(mktemp -d) HERDR_BIN_PATH=false kebbie status
```
