---
name: write-jobs
description: Turn the commands tasks keep running on another machine with kebbie run --on into named jobs in the repo's kebbie config, once the user said yes on the board, and run each once to check it passes. Use after a task ran kebbie run --on for the same commands more than once (a build, a simulator boot, a UI test), when kebbie job suggest lists commands, or when the user asks to make jobs of what tasks run often.
---

# Write jobs from repeated runs

`kebbie` is the absolute path your kebbie role names for it, written out in full so your allow rules match the command; without a kebbie role, `${KEBBIE_BIN:-$HOME/.local/share/kebbie/current/target/release/kebbie}`. Run every `kebbie` command below in the repo's main checkout or one of its worktrees; the main checkout is the parent of `git rev-parse --path-format=absolute --git-common-dir`.

kebbie keeps the last 200 commands `kebbie run --on` ran per repo, with where each ran, its exit code and how long it took. `kebbie job suggest` groups them by command and `--on`. A job is a `[jobs.<name>]` table in the repo config that `kebbie job run <name>` sends to a worker through the hub; docs/how-to/run-a-job-on-another-machine.md and the `[jobs]` section of docs/reference/configuration.md describe it.

1. Read the history: `kebbie job suggest`, and `kebbie job suggest --all` when the user named a command that ran once. Each group gives its run count, how many passed, the longest run, the last machine and the command line as it was sent, plus the jobs that already run some of them. Done when you have the groups worth a job: ones that passed at least once and that the user would run again. A command that never passed, or one that only looked something up (`ls`, `xcrun simctl list`), is left out.
2. Propose the jobs. For each, a short kebab-case name for what it does (`build-dev`, `boot`, `launch-dev`, `ui-test`, `shutdown`), `run` the command line from the history, `on` the system or machine it ran with, and `timeout_minutes` only where `kebbie job suggest` names one or the longest run nears 60 minutes. Then fix what a job folder changes:
   - A job's folder on the worker is a fresh checkout for its task: a folder the repo does not track is missing in its first job. A command that writes into one (`build/foo.log`, `-derivedDataPath build/dd`, `> out/screen.png`) starts with `mkdir -p <folder> && `.
   - Each job runs on its own, so one that needs another's state (a launch after a boot, an install after a build) either says so in its name and is run in order, or does both steps in one `run` joined by `&&`.
   - A command with a value of one session in it, such as a simulator UDID or a temp path, is kept only when the value is stable on that machine, or written in its stable form, such as the simulator's name; one you cannot make stable is left out and named in your report.

   Done when every proposed job has a name, `run` and `on`, and none writes into a folder it did not create.
3. Write the TOML you add to the repo config, the `repo config:` line of `kebbie config --repo <main checkout>`, as one block. The user's yes on the board, or their own ask, is the go-ahead for the jobs step 2 proposed:

   ```toml
   [jobs.build-dev]
   run = "mkdir -p build && xcodebuild -scheme App -derivedDataPath build/dd -destination 'platform=iOS Simulator,name=iPhone 16' build"
   on = "macos"
   timeout_minutes = 90

   [jobs.boot]
   run = "xcrun simctl boot 'iPhone 16' || true; xcrun simctl bootstatus 'iPhone 16'"
   on = "macos"
   ```

   Done when every proposed job is a table in that block.
4. Write them. Add the tables to the repo config, keeping every other line, creating the file and its folder with `mkdir -p` when missing. A name that is already a job is the user's: keep it, and give the new job another name. Done when `kebbie doctor` reads `ok` and `parses` on that file and `kebbie job run` lists the new names.
5. Run each new job once from a worktree of the repo, in the order step 2 gave: `kebbie job run <name>`. It exits 0 when the job exited 0 and 1 otherwise, with the reason in its last line. A job that fails where its `kebbie run --on` passed usually lacks a folder (step 2's `mkdir -p`) or something the run's folder kept from an earlier run; fix its `run` and run it again, at most twice. Done when every job has a pass or a failure with its last line.
6. Report in one line per job: its name, pass or fail, and for a failure the last line. Say which jobs you wrote and where.
