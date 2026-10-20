# kelpie

Run many Claude Code tasks at once in [herdr](https://herdr.dev), each in its own git worktree, and land them on your default branch after a review you control.

<!-- demo: docs/assets/demo.gif, recorded later -->

A kelpie is the working dog that keeps a herd moving while the drover directs. herdr holds the agents; kelpie keeps their work moving towards you.

## What you get

- **A task is a worktree, a Space and a Claude.** `kelpie start billing "Fix the rounding in invoices"` takes a free worktree from a reusable pool, puts it on a new branch, opens it as a herdr Space and starts Claude there. Your main checkout stays untouched.
- **A board that sorts work by what it needs from you.** Needs you, working, to review, ready to push. Questions from tasks wait there with their options and a recommendation, and you answer them in one window.
- **A lead Claude that holds the picture.** It finds work in your backlog, writes informed task prompts, checks finished work against your rules and relays what one task needs to know from another.
- **Review next to Claude.** A split shows the task's changes as diffs in nvim. Comment on a line and the comment is typed into Claude's input box.
- **Trunk-based by design.** kelpie rebases a task onto the default branch and pushes its commits there. No pull requests and no merges.
- **A read-only reviewer that gates the push.** Send a risky task to review and it cannot be pushed until a separate Claude, limited to reading the code, passes its current commits.
- **Optional vault integration.** With the [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) plugin, every task gets a note, edits wait until the problem is written down, and pushes are logged.
- **Update channels and fresh sessions.** Follow tagged releases or the main branch, and the board tells you when a new version is out. One `kelpie update` brings everything along, and idle Claude sessions resume on the new code with their conversation intact.

## Requirements

macOS or Linux with `git`, `herdr` 0.9.0 or later, Claude Code, `jq` and `cargo`. The review split also needs `fzf` 0.71+, `nvim` 0.10+ and `curl`; the history split needs `tig`. `kelpie doctor` checks all of it and prints the fix for anything missing.

## Install

```bash
herdr plugin install gronare/kelpie          # herdr clones and builds kelpie
# restart herdr, then:
ln -s ~/.local/share/kelpie/current/target/release/kelpie ~/.local/bin/kelpie
claude plugin marketplace add ~/.local/share/kelpie/current
claude plugin install kelpie@kelpie
kelpie doctor
```

Add the keys to your herdr config (see [Getting started](docs/getting-started.md)), press `prefix+space` and tell the lead what you want done.

## Documentation

- [Getting started](docs/getting-started.md): install, keys and your first task
- [The board](docs/board.md): layout, groups and every key
- [Tasks](docs/tasks.md): start, land, push, done and the rest of a task's life
- [Review](docs/review.md): the review split and the reviewer gate
- [Worktrees](docs/worktrees.md): the pool, setup, dev servers and URLs
- [The lead](docs/lead.md): the lead, away mode, questions, messages and notifications
- [Backlog](docs/backlog.md): where work comes from
- [Areas](docs/areas.md): one page per repo, with its own actions
- [Vault](docs/vault.md): the optional vault integration
- [Configuration](docs/configuration.md): every key, team setups and the environment
- [Updating](docs/updating.md): channels, overrides and stale sessions
- [Commands](docs/commands.md): every `kelpie` command
- [Security](docs/security.md): what kelpie guards against, and what it does not
- [Troubleshooting](docs/troubleshooting.md): `kelpie doctor` and common fixes
- [Releasing](docs/releasing.md): for maintainers

## License

MIT, see [LICENSE](LICENSE).
