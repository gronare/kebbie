# Requirements

What kebbie needs on a machine and in a repo, and what `kebbie doctor` checks. kebbie runs on macOS and Linux, inside herdr 0.9.0 or later.

## Tools

| Tool | Needed for | macOS | Linux |
| --- | --- | --- | --- |
| `git` | everything | `xcode-select --install` | `sudo apt install git` |
| `herdr` 0.9.0+ | everything | `brew install herdr` | `curl -fsSL https://herdr.dev/install.sh \| sh` |
| `claude` (Claude Code) | everything | `curl -fsSL https://claude.ai/install.sh \| bash` | the same |
| `jq` | the sidebar sync and the review hook | `brew install jq` | `sudo apt install jq` |
| `cargo` | building kebbie on the `main` channel or from `kebbie update --from` | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` | the same |
| `nvim` 0.10+ | the review split's diffs | `brew install neovim` | a [release](https://github.com/neovim/neovim/releases) |
| `curl` | downloading a release's prebuilt binary | `brew install curl` | `sudo apt install curl` |
| `tig` | the history split | `brew install tig` | `sudo apt install tig` |

- Without rustup, `rustc` must be at least the `rust-version` in `Cargo.toml`.
- Distro packages of `nvim` are often older than 0.10.
- Rust is only needed where kebbie is built from source. A release tag installs a prebuilt binary on macOS (Apple silicon and Intel) and on Linux with glibc (x86_64 and arm64); see [Prebuilt binaries](../explanation/updates-and-restarts.md#prebuilt-binaries).
- Plan usage on the board needs a Claude.ai plan; an API key gives no usage numbers.

The task sandbox (`task.sandbox`, on by default) needs Claude Code 2.1.285 or later. macOS needs nothing more. On Linux it also needs:

```
sudo apt install bubblewrap socat
```

and the seccomp filter that blocks Unix sockets:

```
npm install -g @anthropic-ai/sandbox-runtime
```

Without them kebbie refuses to start tasks until they are installed or `task.sandbox = false`.

A task whose profile runs Codex needs Codex CLI 0.163.0 or later and kebbie's plugin in it (`kebbie setup codex`). Install Codex with one of:

```
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

```
brew install --cask codex
```

```
npm install -g @openai/codex
```

## What `kebbie doctor` checks

Each line is `ok`, `warn` or `fail`. Every `warn` and `fail` carries a `fix:` line for macOS and Linux. `kebbie doctor` exits non-zero when any line fails.

| Check | Looks at |
| --- | --- |
| `git`, `herdr`, `claude`, `jq`, `nvim`, `tig`, `curl`, `cargo` | Present on `PATH`, at the versions above. A missing `cargo` is fine on the release channel with a downloaded binary. |
| `herdr plugin` | kebbie installed and enabled in herdr. |
| `herdr link` | The folder herdr links kebbie from, for a local link, still holds a kebbie checkout. One that is gone fails, with `kebbie doctor --fix` as the fix. |
| `plugin binary` | The plugin's binary is in place, and whether it was downloaded for a release tag or built from source. |
| `claude plugin` | The Claude Code plugin is installed, and the copy Claude Code runs matches the checkout. |
| `claude plugin source` | The plugin's marketplace matches the update channel. |
| `status line` | The status line for plan usage. |
| `current link` | `~/.local/share/kebbie/current` points at the plugin. |
| `kebbie on PATH` | `kebbie` on `PATH` is the `current` link's binary. |
| `updates` | The last update check, without fetching. |
| `keeper` | Whether the keeper runs, with its pid and uptime, or that `[keeper] enabled = false` turns it off. |
| `state dir` | The state directories. |
| `config`, `repo config` | config.toml and every file in the repos folder parse, with no unknown or misplaced keys and no broken links. |
| `push block` | Inside a repo: whether its task worktrees get the push block. |
| `kelpie leftovers` | Worktrees whose push block is still kelpie's, which refuses kebbie's own push (`kebbie doctor --fix` installs kebbie's), and open tasks whose hooks run kelpie's removed binary, so their test runs and edits go unrecorded (`kebbie restart` gives each its settings again, a busy one once it is idle). |
| `docs` | Inside a repo: whether `docs.required` is on. |
| `comments` | Inside a repo: whether kebbie reads comments on the commits it pushes there, and for how many days, or why not. |
| `setup skill` | The setup skill's plugin is installed and enabled. |
| `hub` | Whether the machine joined a kebbie hub, and how its board posts go. |
| `maintainers` | Where error signals and kebbie's own bug reports go. |
| `reports` | With `report.inbox_token` set only: the reports endpoint and the reports and signals it filed here. |
| `machine` | With the vault on or a hub connected only: the name the phone and the other machines on the vault see. |
| `notify` | Whether `notify.command` is set. It is never run. |
| `sandbox` | Whether tasks run in the sandbox. |
| `vault` | The vault plugin: on, missing (fine, it is optional) or too old. |
| `codex`, `codex plugin`, `codex hooks` | Codex at 0.163.0 or later, kebbie's plugin in it, and its hooks on. |
| `backlog …` | Each backlog source can be read, and an installed source not in use. |

- `docs` suggests turning `docs.required` on when `gh` says the repo is public on GitHub. Without `gh` or a GitHub remote the line is left out.
- `hub`: when the machine has joined, it shows the hub's address and the machine's name, whether it pinned an account key and when the board last went out. It warns when the hub does not answer, the machine holds no account key (so no browser can read its board) or the last post failed.
- `maintainers`: whether error signals go to kebbie's maintainers (`report.signals`), when they last went and whether that failed, and what happens to kebbie's own bug reports (`report.text`). It warns while `"always"` is set but not armed.
- `reports`: how many reports were filed here, the last fetch and its error, and how many reports wait in quarantine on the issues endpoint.
- `comments` warns when `comments.watch` is on but origin is not on GitHub or `gh` is missing or logged out, with the same reason `kebbie ci` gives. With `comments.watch = false` it says it is off.
- `machine` warns while `machine.name` is unset.
- `sandbox` is on and ready; on with what is missing on Linux, a `fail` since tasks then do not start; or off with `task.sandbox = false`, a `warn` that says a task's shell can pass as you.
- `codex` looks for Codex on `PATH`, kebbie's plugin in its cache and current, and nothing in Codex's `config.toml` that turns kebbie's hooks off (`[features] hooks = false`, or one of them turned off in `/hooks`). Only `codex` shows until a profile runs Codex or Codex is on `PATH`. A gap is a `warn` until a profile runs Codex, and a `fail` after.

## Repos

A repo needs a remote named `origin` with a default branch: `origin/HEAD`, else `origin/main` or `origin/master`. Without one, kebbie says to run `git remote set-head origin -a`.

The push block uses git's per-worktree config. A repo whose shared git config sets `core.worktree` or `core.bare = true` gets no push block until those are moved; `kebbie start` and `kebbie doctor` say so.
