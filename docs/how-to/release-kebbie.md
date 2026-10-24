# Release kebbie

For maintainers. A release happens only when you push a tag, and one command does it. CI runs the tests on every push and releases nothing.

## Write the notes as you go

Each change a user would notice gets one line in `CHANGES.md`, under `## Unreleased`, in the commit that makes it:

```markdown
## Unreleased

### What's new
- The hub window shows your devices by name, and r removes one

### Fixed
- Update no longer stops on a config key from a newer kebbie
```

Write what got better or what was fixed, as you would tell someone who uses kebbie: no commit ids, file names, function names or backticks. A line that runs long may continue on an indented line. Leave out what nobody using kebbie would notice. These lines are what the board shows after an update (on the `main` channel as soon as the commit lands) and what the GitHub Release says.

## Release

```bash
scripts/release.sh 1.4.0 "what the release brings"
```

It refuses unless you are on `main`, your checkout is clean and equal to `origin/main`, the tag does not exist yet, and CI is green on that commit. Then it:

1. Moves the entries under `## Unreleased` in `CHANGES.md` under a new `## 1.4.0` heading, and leaves `## Unreleased` empty for the next. It refuses when `## Unreleased` holds no entry; write the notes first.
2. Sets the version in `Cargo.toml`, `herdr-plugin.toml` and `.claude-plugin/plugin.json`, and lets `cargo check` update `Cargo.lock`.
3. Commits `kebbie 1.4.0: what the release brings`, with `CHANGES.md`, tags it `v1.4.0`, and pushes `main` and the tag together.
4. Waits for `.github/workflows/release.yml`, which fails when the tag and the three manifests disagree, runs the tests, builds `kebbie-<version>-<target>.tar.gz` for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, writes `SHA256SUMS`, and publishes the GitHub Release once every file is attached, with the version's section of `CHANGES.md` as its notes (`scripts/release-notes.sh 1.4.0` prints them).
5. Checks that each binary downloads and prints the release's address.

Machines on the `main` channel can try the commit as soon as it is on `main`. Machines on the `release` channel are offered the release once its binaries download, within 15 minutes of publishing, and `kebbie update` installs it at once. A release whose binaries are not published is never offered, and `kebbie update` there says `kebbie 1.4.0 is not published yet; try again in a while`.

A release tag is exactly `v` and three numbers. `v1.4`, `v1.4.0-rc1` and other tags are never offered as releases.
