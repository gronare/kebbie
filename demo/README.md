# Demo recording

```bash
demo/record.sh                      # the README film and every clip
demo/record.sh demo                 # only the README film
demo/record.sh review-in-nvim       # one clip, named after its tape in demo/clips
```

Recorded with [vhs](https://github.com/charmbracelet/vhs) (`brew install vhs`); the card images are rendered with headless Chrome. kebbie is built from the committed HEAD, so uncommitted changes stay out of the recording.

| Tape | Writes |
| --- | --- |
| `demo.tape` | `docs/assets/demo.gif`, the README film, and the stills `board.png`, `options.png`, `question.png`, `review.png`, `card.png`, `push.png`, `pushed.png` |
| `clips/<name>.tape` | `docs/assets/clips/<name>.gif`, shown at the top of its docs page |

Each tape gets a fresh world from `setup.sh <name>` under `/tmp/kebbie-demo`: its own `HOME` and XDG dirs, its own herdr server on a socket there, two small repos with tasks seeded through `kebbie start`, `report`, `ask`, `tell` and `backlog add`. The `demo` scene and the `areas-page` clip add a vault index as notes-vault-mcp writes it (`vault-index.py`), and the `update-and-restarts` clip installs kebbie as a checkout at its release tag with a newer tag on a local origin, linked into herdr, so `U` really updates and restarts the sessions; that clip's world builds with your own cargo and rustup toolchain. `record.sh` stops the server when it is done; `demo/setup.sh --stop` does it by hand. Nothing reaches your own herdr, Claude or kebbie state.

`bin/claude` is a stand-in for Claude Code that never calls a model. It draws a session from `/tmp/kebbie-demo/screens/<folder>`, writes what it is sent to its transcript as Claude Code does (kebbie checks there that a prompt arrived), and runs `react/<task>` with the prompt: these scripts are what the lead and the tasks do, such as starting saved-carts, asking where carts are kept, or changing the code after a review comment. Asked by kebbie's rules check, it answers as the reviewer: `verdict: ok`, or the verdict in `verdicts/<task>` when the scene wrote one. `card.html` and `checkout.html` are the pages behind the sign-off cards' screenshots.

## The demo scene

`demo/setup.sh demo` builds the world the README film runs in, and it doubles as a live demo: no model is called and nothing leaves the machine, so it plays the same every time. Attach with `/tmp/kebbie-demo/env.sh herdr` and stop it with `demo/setup.sh --stop`. The board opens on:

| Row | What it shows |
| --- | --- |
| search-filters · shop | Needs you: a question with three options, one recommended |
| vat-rounding · api | Needs you: a preview of the new totals to check |
| rate-limit · api | Working, with a message from search-filters in its Space |
| checkout-copy · shop | Ready to push: the reviewer's rules check passed it, and its card has a screenshot |
| product-images · shop | Ready to push · rules: 1 break: the reviewer found a commit that carries a second change, and its Claude has the finding |
| request-ids · api | On local main · waiting for your push: put on the main checkout to try before it is pushed |
| invoice-archive · api | Queued to start at 06:00 tomorrow, with why it waits |
| price-alerts · shop | Queued after search-filters, with why it waits |

Beside them, the Backlog column reads the vault: the Ongoing note Order export shows as working on devbox, another machine on the same vault, and the backlog notes below it come from there too. Under the tasks, the board says kebbie fixed two of your issues, the ones its recheck found fixed after an update. The Areas page (`2`) reads the same vault. The film then starts saved-carts from the lead, answers its question, comments on its diff, and pushes it once the reviewer has checked its rules.
