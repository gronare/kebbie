# kelpie

Run many Claude Code tasks at once in [herdr](https://herdr.dev), each in its own git worktree, and land them on your default branch after a review you control.

![kelpie: from a sentence to the lead to a push on main](docs/assets/demo.gif)

A kelpie is the working dog that keeps a herd moving while the drover directs. herdr holds the agents; kelpie keeps their work moving towards you.

## What you get

- **A task is a worktree, a Space and a Claude.** `kelpie start billing "Fix the rounding in invoices"` takes a free worktree from a reusable pool, puts it on a new branch, opens it as a herdr Space and starts Claude there. Your main checkout stays untouched.
- **A board that sorts work by what it needs from you.** Needs you, working, to review, ready to push, in the order you set. Questions from tasks wait there with their options and a recommendation, and you answer them in one window.
- **A lead Claude that holds the picture.** It finds work in your backlog, writes informed task prompts, relays what one task needs to know from another, and settles questions of method itself, leaving you the ones about what you want.
- **A read-only reviewer on every ready task.** A separate Claude, limited to reading the code, checks a task's commits against your rules before it can be pushed. Send a risky task to a full review and it waits until that reviewer passes its current commits.
- **Review next to Claude.** A split shows the task's changes as diffs in nvim. Comment on a line and the comment is typed into Claude's input box.
- **Trunk-based by design.** kelpie rebases a task onto the default branch and pushes its commits there, one task at a time per repo. No pull requests and no merges.
- **Try it on local main first.** In a repo you run from its main checkout, `kelpie stage` puts a ready task's commits on local main, unpushed, until you push main.
- **Your board on your phone.** Connect a kelpie hub through your browser (`H` on the board) and add your phone: the board goes to it sealed, with a notification when a task needs you. The phone only reads.
- **Optional vault integration.** With the [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) plugin, every task gets a note, edits wait until the problem is written down, pushes are logged, an Areas page shows each repo's state, and kelpie on a second machine sharing the vault asks before it starts a note the first one works on.
- **Update channels and fresh sessions.** Follow tagged releases or the main branch, and the board tells you when a new version is out. One `kelpie update` brings everything along, idle Claude sessions resume on the new code with their conversation intact, and the issues you filed about kelpie are rechecked against the new commits.
- **kelpie's own bugs reach its maintainers on your yes.** You see the redacted text before anything is sent. Error signals carry only counts and versions, never your text, and `[report] signals = false` turns them off.

## Install

You need macOS or Linux with `git`, `herdr` 0.9.0+, Claude Code, `jq` and `curl`; the review split also needs `fzf` 0.71+ and `nvim` 0.10+, and the history split `tig`.

```bash
herdr plugin install --ref "$(curl -fsSL https://api.github.com/repos/gronare/kelpie/releases/latest | jq -r .tag_name)" gronare/kelpie
# restart herdr, then:
~/.local/share/kelpie/current/target/release/kelpie setup
```

The first command installs the latest release and its prebuilt binary (macOS on Apple silicon or Intel, Linux on x86_64 or arm64). `kelpie setup` links `kelpie` into `~/.local/bin`, installs the Claude Code plugin, asks before it adds kelpie's keys to your herdr config, and ends with what `kelpie doctor` still wants fixed. It is safe to run again.

Press `prefix+space` and tell the lead what you want done. [Your first task](docs/tutorial.md) walks through it step by step.

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
