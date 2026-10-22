# Develop kelpie

Run kelpie from your own checkout while you change it.

1. Clone kelpie and point herdr at it:

   ```bash
   git clone https://github.com/gronare/kelpie ~/code/kelpie
   herdr plugin link ~/code/kelpie
   cd ~/code/kelpie && cargo build --release
   ```

   `rust-toolchain.toml` pins the Rust version, with clippy and rustfmt. rustup installs it on the first `cargo` in the checkout, and CI and the release builds read the same file, so `cargo clippy --locked --all-targets -- -D warnings` raises locally exactly what CI raises. To move to a newer Rust, change `channel` in that file and fix what clippy raises in the same commit.

2. Restart herdr. The `current` link now points at your checkout.

3. Run the tests with a throwaway state and no herdr, so they touch nothing of yours:

   ```bash
   GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 HERDR_BIN_PATH=false KELPIE_STATE_DIR=$(mktemp -d) cargo test
   ```

To try your commits on a machine that follows a channel instead, see [Try unreleased code](try-unreleased-code.md).
