# Demo recording

```bash
demo/record.sh                      # the README film and every clip
demo/record.sh demo                 # only the README film
demo/record.sh review-in-nvim       # one clip, named after its tape in demo/clips
```

Recorded with [vhs](https://github.com/charmbracelet/vhs) (`brew install vhs`); the card images are rendered with headless Chrome. kelpie is built from the committed HEAD, so uncommitted changes stay out of the recording.

| Tape | Writes |
| --- | --- |
| `demo.tape` | `docs/assets/demo.gif`, the README film, and the stills `board.png`, `options.png`, `question.png`, `review.png`, `card.png`, `push.png`, `pushed.png` |
| `clips/<name>.tape` | `docs/assets/clips/<name>.gif`, shown at the top of its docs page |

Each tape gets a fresh world from `setup.sh <name>` under `/tmp/kelpie-demo`: its own `HOME` and XDG dirs, its own herdr server on a socket there, two small repos with tasks seeded through `kelpie start`, `report`, `ask`, `tell`, `approve` and `backlog add`. Two clips add to it: `areas-page` writes a vault index for the Areas page (`vault-index.py`), and `update-and-restarts` installs kelpie as a checkout at its release tag with a newer tag on a local origin, linked into herdr, so `U` really updates and restarts the sessions; that clip's world builds with your own cargo and rustup toolchain. `record.sh` stops the server when it is done; `demo/setup.sh --stop` does it by hand. Nothing reaches your own herdr, Claude or kelpie state.

`bin/claude` is a stand-in for Claude Code that never calls a model. It draws a session from `/tmp/kelpie-demo/screens/<folder>`, writes what it is sent to its transcript as Claude Code does (kelpie checks there that a prompt arrived), and runs `react/<task>` with the prompt: these scripts are what the lead and the tasks do, such as starting saved-carts, asking where carts are kept, or changing the code after a review comment. `card.html` and `checkout.html` are the pages behind the sign-off cards' screenshots.
