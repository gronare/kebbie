# Contributing

[Develop kebbie](docs/how-to/develop-kebbie.md) shows how to run your checkout and run the checks CI runs.

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
