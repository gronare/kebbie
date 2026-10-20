# kelpie

Run many Claude Code tasks at once in [herdr](https://herdr.dev), each in its own git worktree, and land them on your default branch after a review you control.

![kelpie: from a sentence to the lead to a push on main](docs/assets/demo.gif)

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

## Install

You need macOS or Linux with `git`, `herdr` 0.9.0 or later, Claude Code, `jq` and `curl`; the review split also needs `fzf` 0.71+ and `nvim` 0.10+, and the history split `tig`.

A release tag installs a prebuilt binary for macOS (Apple silicon and Intel) and Linux (x86_64 and arm64). Any other checkout, such as the default branch herdr clones without `--ref`, is built with `cargo`. Without Rust, name the latest release tag:

```bash
herdr plugin install --ref vX.Y.Z gronare/kelpie   # herdr clones the tag and installs its binary
# restart herdr, then:
ln -s ~/.local/share/kelpie/current/target/release/kelpie ~/.local/bin/kelpie
claude plugin marketplace add ~/.local/share/kelpie/current
claude plugin install kelpie@kelpie
kelpie doctor
```

Add the keys to your herdr config, press `prefix+space` and tell the lead what you want done. [Your first task](docs/tutorial.md) walks through it step by step.

## Documentation

[The documentation](docs/README.md) is organized as a tutorial, how-to guides, reference and explanation. Good places to start:

- [Your first task](docs/tutorial.md): install to a pushed task
- [Commands](docs/reference/commands.md) and [Configuration](docs/reference/configuration.md)
- [Board keys](docs/reference/board.md)
- [How kelpie fits together](docs/explanation/architecture.md)
- [The trust boundary](docs/explanation/trust-boundary.md): what kelpie guards against, and what it does not
- [Fix common problems](docs/how-to/troubleshoot.md)

## License

MIT, see [LICENSE](LICENSE).
