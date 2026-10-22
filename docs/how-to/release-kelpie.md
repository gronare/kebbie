# Release kelpie

For maintainers. A release happens only when you push a tag, and one command does it. CI runs the tests on every push and releases nothing.

```bash
scripts/release.sh 1.4.0 "what the release brings"
```

It refuses unless you are on `main`, your checkout is clean and equal to `origin/main`, the tag does not exist yet, and CI is green on that commit. Then it:

1. Sets the version in `Cargo.toml`, `herdr-plugin.toml` and `.claude-plugin/plugin.json`, and lets `cargo check` update `Cargo.lock`.
2. Commits `kelpie 1.4.0: what the release brings`, tags it `v1.4.0`, and pushes `main` and the tag together.
3. Waits for `.github/workflows/release.yml`, which fails when the tag and the three manifests disagree, runs the tests, builds `kelpie-<version>-<target>.tar.gz` for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, writes `SHA256SUMS`, and publishes the GitHub Release with notes generated from the commits since the previous tag once every file is attached.
4. Checks that each binary downloads and prints the release's address.

Machines on the `main` channel can try the commit as soon as it is on `main`. Machines on the `release` channel are offered the release once its binaries download, within 15 minutes of publishing, and `kelpie update` installs it at once. A release whose binaries are not published is never offered, and `kelpie update` there says `kelpie 1.4.0 is not published yet; try again in a while`.

A release tag is exactly `v` and three numbers. `v1.4`, `v1.4.0-rc1` and other tags are never offered as releases.
