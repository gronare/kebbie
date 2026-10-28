# Receive bug reports as a maintainer

On a maintainer's machine, kebbie fetches what other installs sent to the issues endpoint and files it as kebbie issues on this machine: each bug report a user chose to send, and each error signal that enough installs share.

- Nothing is fetched until you give kebbie the endpoint's inbox token.
- A filed report or signal adds one line under messages on the board: `New bug report: <title>`. It is for you alone: it starts no turn of the lead's and is not in its notices.
- Everything filed this way is another install's data, so kebbie marks it untrusted, never rechecks it after an update and never offers it for sending.

## Turn it on

Put the token the issues endpoint issued in config.toml by hand:

```toml
[report]
inbox_token = "kha_…"
```

The keeper then fetches every 10 minutes, or an open board does when the keeper is off. `kebbie config` shows only whether the token is set.

## Bug reports

A report the endpoint lets through becomes one bug, with the id `r<report id>`, the report's title and text, the sender's version and OS, and `from report <id>` in `kebbie issue show`.

A report from an install the endpoint has not seen before waits in quarantine there until you allow or block its sender. `kebbie doctor`'s `reports` line counts those waiting.

## Error signals

An error signal is a count of one of kebbie's own failures, with no text: the version, a component such as `board.render`, a kind (`panic`, `internal-error` or `corrupt-state`) and how many times. See [Error signals](report-a-problem-with-kebbie.md#error-signals) for what an install sends.

The endpoint passes a signal on once at least three installs report the same version, component and kind on one UTC day. It counts installs, not failures: one install that crashes a hundred times is one. kebbie then files it as one bug:

```
board.render: panic in 0.28.0 (4 installs on 2026-10-28)
```

| When | What happens |
| --- | --- |
| The first day a signal reaches three installs | A new bug, filed by `signals`. Its text lists the installs per UTC day. A line goes on the board. |
| A later day, or more installs on the same day | The same bug: its title takes the latest day and count, and the day is added to its list. No line on the board. |
| A new version reports the same component and kind | A new bug, since a release may have fixed or changed it. |

The issue holds only the version, component, kind, the counts and the dates. The endpoint counts an install by a digest of its signing key that changes every day, so neither it nor the issue can follow one install from day to day.

## Check it

```bash
kebbie doctor            # the reports line: the endpoint, what it filed, the last fetch
kebbie issues            # the filed bugs, with [report <id>, untrusted] or [error signals, untrusted]
```

If a fetch fails, `kebbie doctor` shows the error on the `reports` line and the keeper tries again 10 minutes later. An endpoint with no signal inbox, such as a kebbie hub, is fetched for reports only.

See also: [`inbox.json`](../reference/files.md#reports-to-kebbies-maintainers), [`report.inbox_token`](../reference/configuration.md#reports-and-issues), [The issue file](../reference/files.md#the-issue-file).
