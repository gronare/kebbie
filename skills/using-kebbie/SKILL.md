---
name: using-kebbie
description: Answer a question about kebbie itself from kebbie's own docs on this machine. Use when the user asks how to set up kebbie or something in it, what a kebbie command, board key or config key does, or how a kebbie feature works (tasks, the lead, the board, the push line, stage, the sandbox, profiles, Codex, routines, updates).
---

# Answer from kebbie's docs

`kebbie` is `${KEBBIE_BIN:-$HOME/.local/share/kebbie/current/target/release/kebbie}`.

kebbie carries its documentation with it: the install is a checkout of the kebbie repo, and its `docs/` holds a how-to for each setup, a reference for every command, key and file, and an explanation for each feature. That is the first source, and usually the only one.

1. Run `kebbie docs <words>` with the question's key words in English, the docs' language, whatever language the user asked in (`kebbie docs codex`, `kebbie docs push line`, `kebbie docs M` for a board key). It names the docs folder and the matching pages, best first, each with its path and first sentence. Without words it lists the index.
2. Read the pages it names, the how-to for a setup, the reference for an exact name or default.
3. For a command or option, `kebbie help` lists every one; `kebbie config` lists every config key with its value in effect.
4. Only when those leave the question open, search further: kebbie's source in the same checkout, then the user's notes or GitHub.

When `kebbie docs` says the docs are not on this machine, use `kebbie help` and the online docs URL it prints.

Done when the answer names the doc page (its path) or the command output it came from.
