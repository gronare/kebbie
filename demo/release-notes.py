#!/usr/bin/env python3
# Cuts the demo's next release in CHANGES.md as scripts/release.sh does, moving its shortest Unreleased entries under it so the update window shows them whole: release-notes.py <CHANGES.md> <version>; --drop leaves those entries out instead.
import re
import sys

path, version = sys.argv[1], sys.argv[2]
TAKEN = {"What's new": 3, "Fixed": 2}
text = open(path).read()
head, rest = text.split("## Unreleased\n", 1)
unreleased, _, older = rest.partition("\n## ")
moved, kept = [], []
for heading, body in re.findall(r"### (.+?)\n(.*?)(?=\n### |\Z)", unreleased, re.S):
    entries = re.findall(r"^- .*(?:\n  .*)*", body, re.M)
    taken = sorted(entries, key=len)[:TAKEN.get(heading, 0)]
    if taken:
        moved.append(f"### {heading}\n" + "\n".join(entry for entry in entries if entry in taken))
    if len(taken) < len(entries):
        kept.append(f"### {heading}\n" + "\n".join(entry for entry in entries if entry not in taken))
sections = [f"{head}## Unreleased\n", *kept, *([] if version == "--drop" else [f"## {version}\n", *moved]), *([f"## {older}"] if older else [])]
open(path, "w").write("\n\n".join(part.rstrip("\n") for part in sections) + "\n")
