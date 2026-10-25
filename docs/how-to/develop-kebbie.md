# Develop kebbie

Run kebbie from your own checkout while you change it.

1. Clone kebbie and point herdr at it:

   ```bash
   git clone https://github.com/gronare/kebbie ~/code/kebbie
   herdr plugin link ~/code/kebbie
   cd ~/code/kebbie && cargo build --release
   ```

   `rust-toolchain.toml` pins the Rust version, with clippy and rustfmt. rustup installs it on the first `cargo` in the checkout, and CI and the release builds read the same file, so `cargo clippy --locked --all-targets -- -D warnings` raises locally exactly what CI raises. To move to a newer Rust, change `channel` in that file and fix what clippy raises in the same commit.

2. Restart herdr. The `current` link now points at your checkout.

3. Run the tests with a throwaway state and no herdr, so they touch nothing of yours:

   ```bash
   GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 HERDR_BIN_PATH=false KEBBIE_STATE_DIR=$(mktemp -d) cargo test
   ```

4. Before you push, run the gate:

   ```bash
   scripts/check.sh
   ```

   It runs the tests as above, clippy, `cargo audit`, `cargo deny`, and a check on the oldest Rust that `rust-version` in `Cargo.toml` allows (install that toolchain, and `cargo install cargo-audit cargo-deny`, once): the same checks CI runs. It tests your working tree, and says so when that differs from the last commit.

5. Hold your tasks in kebbie to its docs. kebbie's documentation is part of the product, so turn on `docs.required` for the checkout, named after its folder:

   ```toml
   # ~/.config/kebbie/repos/kebbie.toml
   [docs]
   required = true
   ```

   A task that changes a command, a key or a notice then updates its page under `docs/`, and the reviewer holds one that does not. See [Keep docs in step with changes](keep-docs-in-step.md).

6. Add a line to `CHANGES.md` under `## Unreleased` for each change a user would notice, in the commit that makes it. Machines on the `main` channel show it after their next update, and the release takes it along. See [Write the notes as you go](release-kebbie.md#write-the-notes-as-you-go).

To work on the hub client against a kebbie-hub you run yourself, start the hub in its own repo in dev mode, on `localhost:8470`, and set `KEBBIE_HUB_DEV=1` in the shell you run kebbie from:

```bash
KEBBIE_HUB_DEV=1 kebbie hub connect
```

kebbie then connects to that hub and keeps its keys in `hub-dev/` in the state directory, and `kebbie hub status`, the board's `H` window and the connect card say `dev hub · localhost:8470`; your real hub connection in `hub/` and `[hub] url` stay as they are, and the keeper posts the board to both. `KEBBIE_HUB_DEV_URL` points it at another address. See [Environment variables](../reference/environment.md).

To try a task's commits in the kebbie you run before pushing them, turn on `stage_on_main` for the repo and stage the task: see [Try a task before pushing](try-a-task-before-pushing.md).

To try your commits on a machine that follows a channel instead, see [Try unreleased code](try-unreleased-code.md).
