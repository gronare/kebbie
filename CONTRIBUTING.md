# Contributing

[Develop kebbie](docs/how-to/develop-kebbie.md) shows how to run your checkout and run the checks CI runs.

## Run the fault watcher

On the machine you develop kebbie on, the fault watcher turns the user's corrections, in a chat or on the board, and incidents seen twice into kebbie issues with their root cause and a queued fix task. It is off by default; turn it on in that machine's config.toml:

```toml
[kebbie_dev]
fault_watcher = true
```

Its code is in `src/faults`, its role in `roles/fault-watch.md`, and `kebbie faults` lists the faults it follows.

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
