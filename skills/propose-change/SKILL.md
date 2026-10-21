---
name: propose-change
description: Propose saving a kelpie issue when kelpie itself cost something in this session (an error, a workaround, the user's frustration, a missing capability), and save, list or send kelpie issues on the user's word. Use when kelpie misbehaved or lacked something, when the user asks to report a problem with kelpie or to wish for a change in it, or when a task tells the lead that kelpie got in its way.
---

# Propose a change to kelpie

`kelpie` is `${KELPIE_BIN:-$HOME/.local/share/kelpie/current/target/release/kelpie}`.

kelpie issues are problems and wishes about kelpie itself, kept on this machine so they survive the session that found them: after `kelpie update` the lead learns which ones the new commits may fix, and the user can send one to kelpie's developers on GitHub. A proposal costs the user a moment, so it has to be worth one.

## When to propose

Propose when kelpie cost something real: a command failed or hid an error, you had to work around kelpie, the user was frustrated with it, or kelpie could not do what the work needed. Never for small friction you got past at once, and never for a problem in the project you work on: the issue is about kelpie, not the project.

**A task never proposes to the user.** It tells the lead in one message, `kelpie tell lead "kelpie <what it did>; expected <what>; <how to reproduce>"`, and goes on with its work. The lead weighs it and proposes.

## Propose

1. Run `kelpie issues` and read the titles. When one already covers it, say so in one line and stop.
2. At most one proposal per turn, as your last line: one sentence on what kelpie did and a yes/no question, such as "kelpie push hid git's error. Save as a kelpie issue?"
3. Wait for the answer. Only a yes saves anything.

## Save on yes

Write the text, then save it:

```bash
kelpie issue add --kind bug --title "push hides git's error when the rebase fails" - <<'EOF'
What happened: …
What I expected: …
How to reproduce: 1. … 2. …
EOF
```

- `--kind bug` for something broken, `wish` for something missing.
- The title says what kelpie does or lacks, in a dozen words.
- The text describes kelpie and nothing else. Leave out project and repo names, branch and task names, code, file paths, hostnames, people and secrets; write "a repo", "a task", "a file". The kelpie repo is public.
- kelpie records the version and commit itself.

Done when `kelpie issues` lists it. Tell the user its id in one line.

## Send on the user's ask

Only when the user asks to send one: run `kelpie issue send <id>`. It prints the text, opens a prefilled GitHub issue in the browser, and the user reads it and presses Submit themselves. Nothing reaches GitHub without that. Point them at the text before they submit, so they check it for anything about their project.

## After an update

`[from kelpie] kelpie was updated to …` lists the open issues filed on older code and the commits since. Judge from the subjects which issues are likely fixed and tell the user which to retry. On their word, `kelpie issue retry <id>` marks one worth retrying, and `kelpie issue close <id> fixed|wontfix|duplicate` closes one they confirm.
