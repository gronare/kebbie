<p align="center"><img src="docs/assets/logo.svg" alt="kebbie" width="120" height="120"></p>

<h1 align="center">kebbie</h1>

<p align="center"><strong>Trunk-based development for coding agents.</strong></p>

<p align="center">Tell the lead, a Claude Code session you talk to, what you want done. It plans the work from your backlog and starts the tasks, each with its own agent in its own worktree, side by side in <a href="https://herdr.dev">herdr</a>. One board shows what each one needs from you: a question, a review, a push. Push its commits, each step on its own or together, and they go straight onto main while the agent builds the next. No pull requests, no merges, and a red CI run goes back to the agent that wrote it.</p>

<p align="center">
<a href="https://github.com/gronare/kebbie/releases/latest"><img src="https://img.shields.io/github/v/release/gronare/kebbie" alt="Release"></a>
<a href="https://github.com/gronare/kebbie/actions/workflows/ci.yml"><img src="https://github.com/gronare/kebbie/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
<a href="LICENSE"><img src="https://img.shields.io/github/license/gronare/kebbie" alt="License: MIT"></a>
<a href="https://github.com/gronare/kebbie/stargazers"><img src="https://img.shields.io/github/stars/gronare/kebbie?style=social" alt="GitHub stars"></a>
<a href="https://herdr.dev"><img src="https://img.shields.io/badge/herdr-0.9.0%2B-555" alt="herdr 0.9.0+"></a>
</p>

<p align="center"><a href="https://kebbie.dev">website</a> · <a href="#install">install</a> · <a href="docs/tutorial.md">quick start</a> · <a href="docs/README.md">docs</a> · <a href="https://kebbie.dev/vault/">vault</a> · <a href="CHANGES.md">changelog</a></p>

![kebbie: from a sentence to the lead to a push on main](docs/assets/demo.gif)

A kebbie is a shepherd's crook, the Scots word for a staff with a hooked head. herdr holds the agents; kebbie keeps their work moving towards you.

## What you get

- **A task is a worktree, a Space and a Claude.** `kebbie start billing "Fix the rounding in invoices"` takes a free worktree from a reusable pool, puts it on a new branch, opens it as a herdr Space and starts Claude there. Your main checkout stays untouched.
- **A board that sorts work by what it needs from you.** Needs you, working, in review, ready to push, in the order you set. Questions from tasks wait there with their options and a recommendation, and you answer them in one window.
- **A lead Claude that holds the picture.** It finds work in your backlog, writes informed task prompts, relays what one task needs to know from another, and settles questions of method itself, leaving you the ones about what you want.
- **A read-only reviewer on every ready task.** A separate Claude, limited to reading the code, checks a task's commits against your rules before it can be pushed. A short assessor judges whether the work needs a full review and says why on the task; send it, or let kebbie start or require that review by itself.
- **Review next to Claude.** A split shows the task's changes as diffs in nvim. Comment on a line and the comment is typed into Claude's input box.
- **Work in subtasks.** A task lists its work as subtasks before it writes code, each a slice of the work in small commits that deploy on their own. You accept each subtask once it is ready and push the accepted run from the top of the branch while the task builds on. `task.steps = false` turns it off.
- **Trunk-based by design.** kebbie rebases a task onto the default branch and pushes its commits there, one task at a time per repo. No pull requests and no merges.
- **Try it on local main first.** In a repo you run from its main checkout, `kebbie try` puts a ready task's commits on local main, unpushed, until you push main.
- **Your board on your phone.** Connect a kebbie hub through your browser (`H` on the board): the board goes out sealed to your account, and any phone or browser you sign in on with your passkey reads it. The phone only reads, and notifications through the hub are not available in this version.
- **Optional vault integration.** With a notes vault such as [mindin](https://mindin.dev), every task gets a note, in whatever agent it runs, edits wait until the problem is written down, pushes are logged, an Areas page shows each repo's state, and kebbie on a second machine sharing the vault asks before it starts a note the first one works on.
- **Update channels and fresh sessions.** Follow tagged releases or the main branch, and the board tells you when a new version is out. One `kebbie update` brings everything along, idle Claude sessions resume on the new code with their conversation intact, and the bugs in kebbie you ran into are rechecked against the new commits.
- **Bugs and wishes about kebbie reach its maintainers on your yes.** You see the redacted text before anything is sent. Error signals carry only counts and versions, never your text. A daily install signal carries only a random install id, the version, OS and update channel, so the maintainers can count active installs. Both are off until you say yes in `kebbie setup`.

## Install

You need macOS or Linux with `git`, `herdr` 0.9.0+, Claude Code, `jq` and `curl`; the review split also needs `nvim` 0.10+, and the history split `tig`.

```bash
curl -fsSL https://kebbie.dev/install.sh | sh
```

The script checks those tools, installs the latest release as a herdr plugin with its prebuilt binary (macOS on Apple silicon or Intel, Linux on x86_64 or arm64), and runs `kebbie setup`. Setup is a guide of six steps: this machine's name, the agents it found, the vault, where the backlog lives and error signals, then one look at everything it writes, each tool's files and herdr's keys included, before it writes any of it. It ends with what is still yours to do and an offer to see your board on your phone, then opens your board; inside herdr, that is the lead Space with the lead beside it. It is safe to run again.

Installing from herdr's plugin list instead? `herdr plugin install gronare/kebbie` builds the default branch with `cargo`, so it needs Rust, and runs no setup: the first board you open starts the setup guide, and the next `kebbie update` moves you to the latest release.

Press `prefix+space` and tell the lead what you want done. [Your first task](docs/tutorial.md) walks through it step by step.

## Documentation

[The documentation](docs/README.md) is organized as a tutorial, how-to guides, reference and explanation. Good places to start:

- [Your first task](docs/tutorial.md): install to a pushed task
- [Commands](docs/reference/commands.md) and [Configuration](docs/reference/configuration.md)
- [Board keys](docs/reference/board.md)
- [How kebbie fits together](docs/explanation/architecture.md)
- [The trust boundary](docs/explanation/trust-boundary.md): what kebbie guards against, and what it does not
- [Fix common problems](docs/how-to/troubleshoot.md)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Report vulnerabilities privately, as [SECURITY.md](SECURITY.md) describes.

## License

MIT, see [LICENSE](LICENSE).
