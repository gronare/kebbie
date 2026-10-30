# Develop kebbie

Run kebbie from your own checkout while you change it, and push changes that pass the same checks as CI.

## Run your checkout

1. Clone kebbie:

   ```bash
   git clone https://github.com/gronare/kebbie ~/code/kebbie
   ```

2. Point herdr at it:

   ```bash
   herdr plugin link ~/code/kebbie
   ```

3. Build it:

   ```bash
   cd ~/code/kebbie && cargo build --release
   ```

4. Restart herdr. The `current` link now points at your checkout.

## The Rust version is pinned

`rust-toolchain.toml` pins the Rust version, with clippy and rustfmt. rustup installs it on the first `cargo` in the checkout.

CI and the release builds read the same file, so `cargo clippy --locked --all-targets -- -D warnings` raises locally exactly what CI raises. To move to a newer Rust, change `channel` in that file and fix what clippy raises in the same commit.

## Check before you push

1. Once, install `cargo-audit` and `cargo-deny`, and the oldest Rust toolchain that `rust-version` in `Cargo.toml` allows:

   ```bash
   cargo install cargo-audit cargo-deny
   ```

2. Run the tests with a throwaway state and no herdr, so they touch nothing of yours:

   ```bash
   GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 HERDR_BIN_PATH=false KEBBIE_STATE_DIR=$(mktemp -d) cargo test
   ```

3. Before you push, run the gate:

   ```bash
   scripts/check.sh
   ```

   It runs the same checks CI runs: the tests as above, clippy, `cargo audit`, `cargo deny`, and a check on the oldest Rust that `rust-version` allows. It ends on `check: all green`.

The gate tests your working tree, and says so when that differs from the last commit.

## Keep the docs and the notes in step

1. Hold your tasks in kebbie to its docs. kebbie's documentation is part of the product, so turn on `docs.required` for the checkout, named after its folder:

   ```toml
   # ~/.config/kebbie/repos/kebbie.toml
   [docs]
   required = true
   ```

   A task that changes a command, a key or a notice then updates its page under `docs/`, and the reviewer holds one that does not. See [Keep docs in step with changes](keep-docs-in-step.md).

2. Add a line to `CHANGES.md` under `## Unreleased` for each change a user would notice, in the commit that makes it. Machines on the `main` channel show it after their next update, and the release takes it along. Write what got better or what was fixed, as you would tell someone who uses kebbie, with no commit ids, file names or backticks.

## Work against a dev hub

To work on the hub client against a kebbie-hub you run yourself:

1. Start the hub in its own repo in dev mode, on `localhost:8470`.

2. Set `KEBBIE_HUB_DEV=1` in the shell you run kebbie from, and connect:

   ```bash
   KEBBIE_HUB_DEV=1 kebbie hub connect
   ```

3. `kebbie hub status`, the board's `H` window and the connect card say `dev hub · localhost:8470`.

kebbie keeps the dev hub's keys in `hub-dev/` in the state directory. Your real hub connection in `hub/` and `[hub] url` stay as they are, and the keeper posts the board to both.

If your dev hub runs at another address, point kebbie at it with `KEBBIE_HUB_DEV_URL`. See [Environment variables](../reference/environment.md).

## Try commits before you push them

If you want to try a task's commits in the kebbie you run, turn on `stage_on_main` for the repo and stage the task: see [Try a task before pushing](try-a-task-before-pushing.md).

If you want to try your commits on a machine that follows a channel, see [Try unreleased code](try-unreleased-code.md).
