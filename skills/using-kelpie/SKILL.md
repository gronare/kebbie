---
name: using-kelpie
description: Answer a question about kelpie itself from kelpie's own docs on this machine. Use when the user asks how to set up kelpie or something in it, what a kelpie command, board key or config key does, or how a kelpie feature works (tasks, the lead, the board, the push line, stage, the sandbox, profiles, Codex, routines, updates).
---

# Answer from kelpie's docs

`kelpie` is `${KELPIE_BIN:-$HOME/.local/share/kelpie/current/target/release/kelpie}`.

kelpie carries its documentation with it: the install is a checkout of the kelpie repo, and its `docs/` holds a how-to for each setup, a reference for every command, key and file, and an explanation for each feature. That is the first source, and usually the only one.

1. Run `kelpie docs <words>` with the question's key words in English, the docs' language, whatever language the user asked in (`kelpie docs codex`, `kelpie docs push line`, `kelpie docs M` for a board key). It names the docs folder and the matching pages, best first, each with its path and first sentence. Without words it lists the index.
2. Read the pages it names, the how-to for a setup, the reference for an exact name or default.
3. For a command or option, `kelpie help` lists every one; `kelpie config` lists every config key with its value in effect.
4. Only when those leave the question open, search further: kelpie's source in the same checkout, then the user's notes or GitHub.

When `kelpie docs` says the docs are not on this machine, use `kelpie help` and the online docs URL it prints.

Done when the answer names the doc page (its path) or the command output it came from.
