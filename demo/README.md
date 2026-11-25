# Demo recording

```bash
demo/record.sh                      # the README film and every clip
demo/record.sh demo                 # only the README film
demo/record.sh review-in-nvim       # one clip, named after its tape in demo/clips
```

Recorded with [vhs](https://github.com/charmbracelet/vhs) (`brew install vhs`), the clips turned into animated WebP with `gif2webp` (`brew install webp`); the card images are rendered with headless Chrome, Google Chrome on macOS and `chromium` or `google-chrome` on Linux, or the one `KDEMO_CHROME` names. kebbie is built from the committed HEAD, so uncommitted changes stay out of the recording.

| Tape | Writes | Shows |
| --- | --- | --- |
| `demo.tape` | `docs/assets/demo.gif`, the README film, and the stills `board.png`, `options.png`, `question.png`, `review.png`, `card.png`, `push.png`, `pushed.png` | A sentence to the lead, through the question and the task's two steps, a review comment folded into step 1, both steps accepted in the review view, to the push and its CI |
| `clips/backlog.tape` | `docs/assets/clips/backlog.webp` | The Backlog column by priority; `g` starts an item, and its task puts a plan to you |
| `clips/lead.tape` | `docs/assets/clips/lead.webp` | A sentence to the lead starts saved-carts, whose question lands on the board |
| `clips/getting-started.tape` | `docs/assets/clips/getting-started.webp` | `n` takes what you want done and starts it as a task in its own worktree and Space; it lists two steps, commits each and reports ready |
| `clips/answer-questions.tape` | `docs/assets/clips/answer-questions.webp` | The inbox; the answered task lists its steps, builds them, reports ready and the reviewer passes them |
| `clips/review-in-nvim.tape` | `docs/assets/clips/review-in-nvim.webp` | The review view: `a` accepts step 1, a line comment on step 2 goes to Claude, which folds the change into that step; then the rules check |
| `clips/push-and-card.tape` | `docs/assets/clips/push-and-card.webp` | The card, the push window with both commits accepted, CI watched until green, and the task queued after it starting |
| `clips/away-mode.tape` | `docs/assets/clips/away-mode.webp` | `A`: the lead settles a how-question, and sums up when you are back |
| `clips/areas-page.tape` | `docs/assets/clips/areas-page.webp` | The Areas page, an area's actions and its vault log |
| `clips/vault.tape` | `docs/assets/clips/vault.webp` | notes-vault-mcp in a terminal: `context` for api with the three commits newer than its system note, a search that finds the rounding trap by meaning, `read` of that trap, and the backlog by priority |
| `clips/routines.tape` | `docs/assets/clips/routines.webp` | A health check added from the Routines page |
| `clips/hub-connect.tape` | `docs/assets/clips/hub-connect.webp` | `H` connects the machine to a hub: the card with the code to check and the link |
| `clips/update-and-restarts.tape` | `docs/assets/clips/update-and-restarts.webp` | `U` downloads the newer release, shows what's new (the newest release's notes when Unreleased is empty) and restarts the idle sessions |

Each clip is shown at the top of its docs page.

Each tape gets a fresh world from `setup.sh <name>` under `/tmp/kebbie-demo`, or `KDEMO_DIR` under `/tmp` of at most 70 characters, as herdr's socket lives inside it: its own `HOME` and XDG dirs, its own herdr server on a socket there, two small repos with tasks seeded through `kebbie start`, `steps`, `accept`, `report`, `ask`, `tell` and `backlog add`, with `task.steps` on and the board's message log folded to one line. The repos' origins read as `https://github.com/robin-demo/<repo>`, and git sends their pushes to bare repos under `/tmp/kebbie-demo/remotes`, so kebbie watches CI after a push; `bin/gh` stands in for the GitHub CLI and reports one CI run per pushed commit, green 20 seconds after kebbie first asks.

- The `demo` scene and the `areas-page` clip add a vault index as notes-vault-mcp writes it (`vault-index.py`).
- The `vault` clip installs notes-vault-mcp with its `semantic` extra from the committed HEAD of a checkout beside kebbie (or `KDEMO_VAULT_SRC`) into a venv at `/tmp/kebbie-demo-vault`, with the embedding model cached in `/tmp/kebbie-demo-models`, and writes a real vault of Markdown notes; its world stops before herdr and the tasks, as the clip runs in a plain shell.
- The `push-and-card` clip queues free-shipping after checkout-copy, so it starts by itself once checkout-copy's CI is green.
- The `update-and-restarts` clip installs kebbie as a checkout at its release tag, linked into herdr, with a newer tag on a local origin whose release notes `release-notes.py` cuts and whose binary a local server publishes, so `U` really downloads the release and restarts the sessions; that clip's world builds with your own cargo and rustup toolchain.
- The `hub-connect` clip runs a kebbie hub of its own on `localhost:18471`: put `kebbie-hub` on `PATH` or set `KDEMO_HUB_BIN`.

`record.sh` stops the servers when it is done; `demo/setup.sh --stop` does it by hand. Nothing reaches your own herdr, Claude, GitHub or kebbie state.

`bin/claude` is a stand-in for Claude Code that never calls a model. It draws a session from `/tmp/kebbie-demo/screens/<folder>`, writes what it is sent to its transcript as Claude Code does (kebbie checks there that a prompt arrived), and runs `react/<task>` with the prompt (a pooled worktree that comes back with another task starts on a clean screen): these scripts are what the lead and the tasks do, such as starting saved-carts, asking where carts are kept, or changing the code after a review comment. Asked by kebbie's rules check, it answers as the reviewer: `verdict: ok`, or the verdict in `verdicts/<task>` when the scene wrote one. Asked whether a ready task needs a review, it says no, with a reason. Asked for a review, it reads for 20 minutes before it passes, so a task sent to review stays In review for the film. The lead ends each turn in 1 to 3 lines, as its role asks. `card.html` and `checkout.html` are the pages behind the sign-off cards' screenshots.

## The demo scene

`demo/setup.sh demo` builds the world the README film runs in, and it doubles as a live demo: no model is called and nothing leaves the machine, so it plays the same every time. Attach with `/tmp/kebbie-demo/env.sh herdr` and stop it with `demo/setup.sh --stop`. The board opens on:

| Row | What it shows |
| --- | --- |
| search-filters · shop | Needs you: a question with three options, one recommended |
| vat-rounding · api | Needs you: a preview of the new totals to check, in two steps waiting for your accept |
| rate-limit · api | Working, with a message from search-filters in its Space |
| refunds · api | In review: three steps, the first accepted, while the reviewer reads it |
| checkout-copy · shop | Ready to push: the reviewer's rules check passed it, both commits are accepted, and its card has a screenshot |
| product-images · shop | Ready to push · rules: 1 break: the reviewer found a commit that carries a second change, and its Claude has the finding |
| request-ids · api | On local main · waiting for your push: put on the main checkout to try before it is pushed |
| invoice-archive · api | Queued to start at 06:00 tomorrow, with why it waits |
| price-alerts · shop | Queued after search-filters, with why it waits |

Beside them, the Backlog column reads the vault: the Ongoing note Order export shows as working on devbox, another machine on the same vault, and the backlog notes below it come from there too. Under the tasks, the board says kebbie fixed two of your issues, the ones its recheck found fixed after an update. The Areas page (`2`) reads the same vault. The film then starts saved-carts from the lead, answers its question, comments on step 1 in the review view, accepts both steps and pushes them once the reviewer has checked its rules.
