#!/bin/sh
# The highest vX.Y.Z among the tag names on stdin, by version, not by date: scripts/latest-tag.sh < tags
set -eu
grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sed 's/^v//' | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1 | sed 's/^/v/'
