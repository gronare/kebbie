---
name: keep-docs
description: Keep a repo's README and docs/ in step with a change to what a user sees or does. Use when kebbie's task role names it for a repo with docs.required, when a review finding ends "(keep-docs)", or when the user asks to document a change.
---

# Keep the docs in step

The docs are the product's **surface** written down: everything a user types, reads or sets. A change that moves the surface moves its docs in the same task, so main never ships code its docs contradict.

## Steps

1. **Surface.** Read your branch with `git diff <default branch>...HEAD` and sort each hunk. Surface is what a user notices without reading the code: a command or its options, a config key or its default, text the program prints (a notice, an error, help, a prompt), a key binding, a file the program reads or writes, a behaviour a user relies on. A refactor, a test, an internal name, CI, a build script, a dependency file or a hook is internal. Done when every hunk is surface or internal, and each surface item has a one-line description of what changed.
2. **Pages.** Read the docs index (`docs/README.md`, or the README when there is none) and a page beside the one you will touch, to take the house style. Place each surface item by what its reader came for:
   - **Reference** for exact behaviour looked up by name: a key, an option, a command, an output, a default. Nearly every surface change lands here.
   - **How-to** for a goal the user can now reach, or reaches by other steps.
   - **Explanation** for why it works this way, when the change moves a design decision or a trade-off.
   - **Tutorial** only when the first run changes.

   Then grep the docs and the README for each old name, default and text: every page that shows one joins the list. Done when every surface item has its pages and every stale mention is on the list.
3. **Write.** Update each page in its quadrant's shape and the house style: reference as tables with exact names, how-to as numbered steps from goal to done, explanation as prose about why. Write product text that sells: what the reader can do with it, in the present tense, as if it always worked this way. Every path, name and value is an example any reader could have (`~/code/app`, `billing`), so the page describes the product rather than one machine or this task. A new page gets its line in the docs index under its quadrant. The README stays short (what the product is, how to start, links into docs/) and changes only when the first impression does. Done when each page on the list describes the code on your branch.
4. **Check.** Read your docs diff beside the code diff, and commit the docs with the change they describe, by the rules skill in effect. Done when every name, default and printed text in the docs matches the code character for character, every link you wrote resolves to a file, and each surface item from step 1 has its line in the docs.
