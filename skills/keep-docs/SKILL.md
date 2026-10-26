---
name: keep-docs
description: Keep a repo's README and docs/ in step with a change to what a user sees or does, in the house page shapes. Use when kebbie's task role names it for a repo with docs.required, when a review finding ends "(keep-docs)", when the user asks to document a change or rewrite a docs page, and before a release.
---

# Keep the docs in step

The docs are the product's **surface** written down: everything a user types, reads or sets. A change that moves the surface moves its docs in the same task, so main never ships code its docs contradict.

## Steps

1. **Surface.** Read your branch with `git diff <default branch>...HEAD` and sort each hunk. Surface is what a user notices without reading the code: a command or its options, a config key or its default, text the program prints (a notice, an error, help, a prompt), a key binding, a file the program reads or writes, a behaviour a user relies on. A refactor, a test, an internal name, CI, a build script, a dependency file or a hook is internal. Done when every hunk is surface or internal, and each surface item has a one-line description of what changed.
2. **Pages.** Read the docs index (`docs/README.md`, or the README when there is none). Place each surface item by what its reader came for:
   - **Reference** for exact behaviour looked up by name: a key, an option, a command, an output, a default. Nearly every surface change lands here.
   - **How-to** for a goal the user can now reach, or reaches by other steps.
   - **Explanation** for why it works this way, when the change moves a design decision or a trade-off.
   - **Tutorial** only when the first run changes.

   Then grep the docs and the README for each old name, default and text: every page that shows one joins the list. Done when every surface item has its pages and every stale mention is on the list.
3. **Write.** Update each page in its quadrant's shape from **Page shapes** below. Write product text that sells: what the reader can do with it, in the present tense, as if it always worked this way. Every path, name and value is an example any reader could have (`~/code/app`, `billing`), so the page describes the product rather than one machine or this task. A new page gets its line in the docs index under its quadrant. The README stays short (what the product is, how to start, links into docs/) and changes only when the first impression does. Done when each page on the list describes the code on your branch.
4. **Check.** Read your docs diff beside the code diff, and commit the docs with the change they describe, by the rules skill in effect. Done when every name, default and printed text in the docs matches the code character for character, every link you wrote resolves to a file, each surface item from step 1 has its line in the docs, and every page you touched holds its shape.

## Page shapes

A reader scans before they read, so every page is **scannable**: short blocks, each saying one thing.

- **Every page** opens with its `#` title and one sentence saying what the reader can do with the page or what it explains. Paragraphs run one to three sentences. A note the reader must not miss is a blockquote starting `**Note:**`, `**Tip:**` or `**Warning:**`, at most one per section. A clip or still from `docs/assets/` goes where it shows the thing faster than words.
- **How-to**: numbered steps from goal to done, one action each, with its command alone in a fenced block, no `$` prompt. The last step says how the reader sees it worked. Cases that apply to some readers follow as a short list of lines starting "If".
- **Reference**: one table per group of names (key, default, what it does), exact names in backticks, one line per row. Why it works that way is a link to the explanation.
- **Explanation**: sections whose `##` heading states the point, each under 150 words. Timeouts, keys and edge cases live in the reference and are linked.
- **Tutorial**: the how-to shape, for the first run only, ending on the reader's first landed task.

## Before a release

1. **Changes.** List the surface changes since the last tag from `CHANGES.md`'s `## Unreleased` and `git log <last tag>..HEAD`. Done when every Unreleased line and every commit that moved the surface is on the list.
2. **Docs.** For each item, find its page by Steps 2 and check it describes main, in its shape. Fix a page that lags, by Steps 3 and 4. Done when every item names a page that matches the code and every page changed since the last tag holds its shape.
