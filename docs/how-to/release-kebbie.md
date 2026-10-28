# Release kebbie

Release a new version of kebbie, for maintainers: one command does it, and only when you push a tag.

CI runs the tests on every push and releases nothing.

## Write the notes as you go

Each change a user would notice gets one line in `CHANGES.md`, under `## Unreleased`, in the commit that makes it:

```markdown
## Unreleased

### What's new
- The hub window shows when the board last went out

### Fixed
- Update no longer stops on a config key from a newer kebbie
```

- Write what got better or what was fixed, as you would tell someone who uses kebbie.
- No commit ids, file names, function names or backticks.
- A line that runs long may continue on an indented line.
- Leave out what nobody using kebbie would notice.

These lines are what the board shows after an update (on the `main` channel as soon as the commit lands) and what the GitHub Release says.

## Check the docs

kebbie.dev publishes `docs/` as it stands at the release tag, so the docs ship with the release.

1. Ask Claude to check them:

   ```text
   /kebbie:keep-docs before a release
   ```

   It lists the surface changes since the last tag and fixes each page that does not describe main yet.

2. Commit its fixes before you release.

## Release

1. Run the release script with the version and what the release brings:

   ```bash
   scripts/release.sh 1.4.0 "what the release brings"
   ```

It refuses unless:

- you are on `main`,
- your checkout is clean and equal to `origin/main`,
- the tag does not exist yet,
- CI is green on that commit.

Then it:

1. Moves the entries under `## Unreleased` in `CHANGES.md` under a new `## 1.4.0` heading, and leaves `## Unreleased` empty for the next. It refuses when `## Unreleased` holds no entry; write the notes first.
2. Sets the version in `Cargo.toml`, `herdr-plugin.toml` and `.claude-plugin/plugin.json`, and lets `cargo check` update `Cargo.lock`.
3. Commits `kebbie 1.4.0: what the release brings`, with `CHANGES.md`, tags it `v1.4.0`, and pushes `main` and the tag together.
4. Waits for `.github/workflows/release.yml`. The workflow fails when the tag and the three manifests disagree, runs the tests, builds `kebbie-<version>-<target>.tar.gz` for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, and writes `SHA256SUMS`. It publishes the GitHub Release once every file is attached, with the version's section of `CHANGES.md` as its notes (`scripts/release-notes.sh 1.4.0` prints them).
5. Checks that each binary downloads and prints the release's address.
6. Removes the build output under `target/`, everything but `target/release`, and says how much it freed.

## The build output

Every test run in the main checkout leaves build output under `target/`, and on macOS each incremental build keeps its own object files in `target/debug/deps`, so the folder grows by gigabytes between releases. The release script removes it once the release is out:

- `target/release` stays, since it holds the kebbie this checkout installs; the next test run builds `target/debug` again from scratch.
- A symlink is never followed: one inside `target/` goes as a link, and a `target/` that is a link is left alone.
- `KEBBIE_KEEP_BUILDS=1 scripts/release.sh 1.4.0 "…"` keeps everything.
- `scripts/clean-builds.sh` does the same at any time.

A task's worktree loses its `target/` when the task ends, by [`worktree.clean_on_release`](../reference/configuration.md), so the pooled worktrees do not grow.

## Who gets it when

| Channel | Gets it |
| --- | --- |
| `main` | Can try the commit as soon as it is on `main`. |
| `release` | Is offered the release once its binaries download, within 15 minutes of publishing. `kebbie update` installs it at once. |

A release whose binaries are not published is never offered, and `kebbie update` there says `kebbie 1.4.0 is not published yet; try again in a while`.

A release tag is exactly `v` and three numbers. `v1.4`, `v1.4.0-rc1` and other tags are never offered as releases.
