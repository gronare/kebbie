# Contributing

[Develop kebbie](docs/how-to/develop-kebbie.md) shows how to run your checkout and run the checks CI runs.

## Run the fault watcher

On the machine you develop kebbie on, every fault in kebbie goes one way: the fault watcher weighs it, names its root cause and queues the task that fixes it. Its cases are the user's words the lead reports, each kebbie issue the lead files with `kebbie issue add`, the reports other installs send, the user's corrections in a task's chat or on the board, and incidents seen twice. A case about an issue that is filed already gets its cause and fix on that issue. An issue closes as fixed once its fix task lands with green CI, and the lead is asked to confirm the fault is gone; when it comes back, the issue opens again and its next fix is a code barrier. The keeper's recheck after an update stays off on this machine. The watcher is off by default; turn it on in that machine's config.toml:

```toml
[kebbie_dev]
fault_watcher = true
```

When the user tells the lead something in kebbie is wrong, the lead hands their words to the watcher with `kebbie fault "<their words>"`: that case is weighed at once and alone.

Its code is in `src/faults`, its role in `roles/fault-watch.md`, and `kebbie faults` lists the faults it follows.

Without `[kebbie_dev]` nothing of it shows: `kebbie fault` and `kebbie faults` answer "Unknown command", the keeper registers neither of its routines and the lead is given neither command. Every developer-only feature asks the one gate in `src/dev.rs`: a command goes in `dev::COMMANDS`, a keeper routine is built with `Routine::dev`, and a module of kebbie's own development goes in `MODULES` in `src/dev/tests.rs`, whose tests fail when a command or routine runs such a module without the gate, or when anything else reads the flag.

## Sign off your commits

Contributions are made under the project's [MIT licence](LICENSE). Each commit in a pull request certifies the [Developer Certificate of Origin](https://developercertificate.org) with a `Signed-off-by:` line matching the commit's author:

```bash
git commit -s
```

To sign off commits already on your branch:

```bash
git rebase --signoff origin/main
```

A pull request with a commit that is not signed off fails the `dco` check.

## Report a vulnerability

Never in a public issue or pull request. See [SECURITY.md](SECURITY.md).
