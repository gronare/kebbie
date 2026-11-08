# The vault contract

What a vault command answers, so kebbie can run it as `vault.command`, whatever the package is called.

kebbie runs the command with its own arguments after the words you set, such as `uvx mindin about --json`. Every answer is JSON on stdout; a refusal exits non-zero with its reason on the last line of stderr.

A value kebbie cannot vouch for goes in the same word as its option, as `--line=<text>`, `--branch=<name>` and `--note=<path>`, so a log line that starts with `-` stays the line. The vault's parser takes `--option=value` as well as `--option value`.

## `about --json`

kebbie asks this first, at each `kebbie doctor` run and whenever its kept answer is older than 10 minutes.

```json
{
  "name": "mindin",
  "version": "0.14.0",
  "contract": 1,
  "capabilities": ["tasks", "claims", "log", "snapshot", "read"],
  "former_names": ["notes-vault-mcp"]
}
```

| Field | Type | What kebbie does with it |
| --- | --- | --- |
| `name` | string | The package's name, in `kebbie doctor` and in `kebbie update --check`. |
| `version` | string | The package's version, beside its name. |
| `contract` | number | `1` is this page. A higher number keeps the vault off until kebbie is updated, and `kebbie doctor` says so. |
| `capabilities` | list of strings | The calls kebbie makes. A call whose capability is not listed is never run. |
| `former_names` | list of strings, optional | Names the package had before. A `vault.command` word that is one of them makes `kebbie doctor` suggest the line with the new name. |

## Capabilities

| Capability | Calls kebbie makes | Without it |
| --- | --- | --- |
| `tasks` | `task open`, `task check`, `task close`, `task backlog` | The vault is off for tasks: no notes, no Problem gate. |
| `claims` | `task claim`, `task release` | Notes are kept, but claimed by no machine. |
| `log` | `log append`, `log path` | Pushes write no log lines. |
| `snapshot` | `snapshot --json [--since <etag>]` | The board reads the vault's local index instead. |
| `read` | `read <note> --body` | A plan in a vault note shows only from the local index. |

## `status`

kebbie runs `status` with every `about --json`, and `status --probe` from `kebbie doctor`, which may try a write.

| Field | Type | What kebbie does with it |
| --- | --- | --- |
| `configured` | bool | `false` keeps the vault off: no vault is set up for the command. |
| `writable` | bool or null | `false` keeps the vault off; `null` is unknown and keeps it on. |
| `backend` | string | Shown in `kebbie doctor`, such as `local`, `s3` or `server`. |
| `version` | string | Used when `about --json` gives none. |

## A vault without `about`

A vault that refuses `about --json`, as notes-vault-mcp up to 0.13 does, is read from `status`, `task --help` and `--help`: the name from the usage line, `tasks` and `log` when `task --help` answers, `claims` when it lists `claim`, and `snapshot` and `read` when `--help` lists them. `kebbie doctor` names it as read without `about --json`.
