#!/bin/sh
# Writes issues/OPEN.md: the issues of issues/INDEX.md that are open or in
# progress, under the same sections. INDEX.md stays the one place a status is
# kept; OPEN.md is generated from it and never edited by hand. Run from
# anywhere; the pre-commit hook in .githooks runs it whenever INDEX.md is
# committed, on the index as it is staged, which it passes as the argument.
set -eu

dir=$(dirname "$0")
index="${1:-$dir/INDEX.md}"
open="$dir/OPEN.md"

awk '
BEGIN {
    print "# Open Issues"
    print ""
    print "The issues of [INDEX.md](INDEX.md) that are `open` or `in progress`, under the same sections."
    print "Generated from the index by `issues/open.sh` - never edit it by hand: change the index, and"
    print "the pre-commit hook (`.githooks/pre-commit`) or a run of the script brings this file up to date."
}
# Only what follows the rule below the maintenance notes are sections.
/^---$/ { sections = 1; next }
!sections { next }
/^## / { section = $0; header = ""; shown = 0; next }
/^\| No\. / { header = $0; next }
/^\|---/ { rule = $0; next }
/^\| \[/ {
    split($0, cells, "|")
    status = cells[3]
    gsub(/^ +| +$/, "", status)
    if (status != "open" && status != "in progress") next
    if (!shown) {
        print ""
        print section
        print ""
        print header
        print rule
        shown = 1
    }
    print
}
' "$index" > "$open.tmp"
mv "$open.tmp" "$open"
