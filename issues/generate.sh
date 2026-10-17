#!/bin/sh
# Writes issues/INDEX.md and issues/OPEN.md from the field lines at the top of the issue files
# (`**Status:**`, `**Type:**`, `**Priority:**`, `**Area:**`, `**Epic:**`, `**Summary:**`, see
# issues/README.md). Both are generated and never edited by hand.
#
#   generate.sh [--check] [directory]
#
# The issue files are read from the directory, by default the one of this script; the pre-commit
# hook in .githooks passes a copy of the issue files as they are staged. INDEX.md and OPEN.md are
# always written next to this script. With --check nothing is written: the script fails if either
# file is not what it would write. An issue file whose fields are missing or unknown fails the run,
# naming the file and the field.
set -eu

dir=$(dirname "$0")
check=
if [ "${1:-}" = --check ]; then
    check=1
    shift
fi
src="${1:-$dir}"
index="$dir/INDEX.md"
open="$dir/OPEN.md"

awk -v index_out="$index.tmp" -v open_out="$open.tmp" '
BEGIN {
    # The areas, in the order of their sections.
    areas = split("Observability and operations|Persistence|Sessions and channels|" \
        "Identity and access management|Audit|Tools and approvals|User interaction|" \
        "Context and knowledge|Maintenance|Code structure|Integrations", area, "|")
    for (i = 1; i <= areas; i++) known_area[area[i]] = 1
    split("open|in progress|done|rejected", list, "|"); for (i in list) known_status[list[i]] = 1
    split("feature|bug|refactoring|epic|question", list, "|"); for (i in list) known_type[list[i]] = 1
    split("high|medium|low", list, "|"); for (i in list) known_priority[list[i]] = 1
    split("high|medium|low", priority_order, "|")
    split("Status|Type|Priority|Area|Epic|Summary", list, "|"); for (i in list) known_field[list[i]] = 1
    errors = 0
    max = -1
}

function fail(message) {
    print "issues/" file ": " message > "/dev/stderr"
    errors++
}

# Checks the issue file read before and keeps its fields.
function finish(    n) {
    if (file == "") return
    n = number
    if (!("Status" in field)) fail("no **Status:**")
    else if (!(field["Status"] in known_status)) fail("unknown status `" field["Status"] "`")
    if (!("Area" in field)) fail("no **Area:**")
    else if (!(field["Area"] in known_area)) fail("unknown area `" field["Area"] "`")
    if (!("Summary" in field)) fail("no **Summary:**")
    else if (field["Summary"] ~ /\|/) fail("the summary contains a `|`")
    if ("Type" in field && !(field["Type"] in known_type)) fail("unknown type `" field["Type"] "`")
    if ("Priority" in field && !(field["Priority"] in known_priority))
        fail("unknown priority `" field["Priority"] "`")
    if (field["Status"] == "open" || field["Status"] == "in progress") {
        if (!("Type" in field)) fail("no **Type:**, which an issue `" field["Status"] "` needs")
        if (!("Priority" in field)) fail("no **Priority:**, which an issue `" field["Status"] "` needs")
    }
    if ("Epic" in field && field["Epic"] !~ /^\[[0-9]+\]\([0-9]+-[a-z0-9-]+\.md\)$/)
        fail("the epic is not a link `[number](file)`")
    if (n in name) fail("number " n " is taken by " name[n] " as well")
    name[n] = file
    shown[n] = shown_number
    status[n] = field["Status"]
    type[n] = field["Type"]
    priority[n] = field["Priority"]
    issue_area[n] = field["Area"]
    epic[n] = field["Epic"]
    summary[n] = field["Summary"]
    if (n > max) max = n
    split("", field)
    file = ""
}

FNR == 1 {
    finish()
    file = FILENAME
    sub(/.*\//, "", file)
    header = 1
    if (file !~ /^([0-9][0-9]|[1-9][0-9][0-9])-[a-z0-9-]+\.md$/) {
        fail("the name is not `<number>-<slug>.md`, with two digits below 100")
    }
    shown_number = file
    sub(/-.*/, "", shown_number)
    number = shown_number + 0
    title = $0
    if (!sub(/^# /, "", title) || title + 0 != number || title !~ /^[0-9]+ — /) {
        fail("the heading does not begin with `# " shown_number " — `")
    }
    next
}
/^## / { header = 0 }
header && /^\*\*[A-Za-z]+:\*\* / {
    key = $0
    sub(/^\*\*/, "", key)
    sub(/:.*/, "", key)
    value = $0
    sub(/^\*\*[A-Za-z]+:\*\* +/, "", value)
    sub(/ +$/, "", value)
    if (!(key in known_field)) fail("unknown field **" key ":**")
    else if (key in field) fail("**" key ":** twice")
    else field[key] = value
}

function row(n, out, long,    line) {
    line = "| [" shown[n] "](" name[n] ") | " status[n] " | "
    if (long) {
        line = line priority[n] " | " type[n] " | " summary[n]
        if (epic[n] != "") line = line " Part of epic " epic[n] "."
    } else {
        line = line summary[n]
    }
    print line " |" > out
}

function table(heading, out, long) {
    print "" > out
    print "## " heading > out
    print "" > out
    if (long) {
        print "| No. | Status | Priority | Type | Issue |" > out
        print "|---|---|---|---|---|" > out
    } else {
        print "| No. | Status | Issue |" > out
        print "|---|---|---|" > out
    }
}

# The section of OPEN.md an issue open or in progress belongs to.
function open_section(n) {
    if (type[n] == "bug") return "Bugs"
    if (priority[n] == "low") return "Low priority"
    return issue_area[n]
}

function open_table(heading,    p, n, any) {
    any = 0
    for (p = 1; p <= 3; p++) {
        for (n = 0; n <= max; n++) {
            if (!(n in name) || (status[n] != "open" && status[n] != "in progress")) continue
            if (open_section(n) != heading || priority[n] != priority_order[p]) continue
            if (!any) table(heading, open_out, 1)
            any = 1
            row(n, open_out, 1)
        }
    }
}

END {
    finish()
    for (n = 0; n <= max; n++) {
        if (!(n in name) || epic[n] == "") continue
        e = epic[n]
        sub(/^\[/, "", e)
        sub(/\].*/, "", e)
        e += 0
        file = name[n]
        if (!(e in name)) fail("the epic " e " does not exist")
        else if (type[e] != "epic") fail("the epic " e " is not of type `epic`")
    }
    if (errors) exit 1

    print "# Issues — Index" > index_out
    print "" > index_out
    print "Every issue, one line each, under its area and by number. Generated from the field lines of the" > index_out
    print "issue files by `issues/generate.sh` - never edit it by hand: change the issue file, and the" > index_out
    print "pre-commit hook (`.githooks/pre-commit`) or a run of the script brings this file up to date. How" > index_out
    print "issues are kept is described in [README.md](README.md); those open or in progress are listed in" > index_out
    print "[OPEN.md](OPEN.md)." > index_out
    for (i = 1; i <= areas; i++) {
        any = 0
        for (n = 0; n <= max; n++) {
            if (!(n in name) || issue_area[n] != area[i]) continue
            if (!any) table(area[i], index_out, 0)
            any = 1
            row(n, index_out, 0)
        }
    }

    print "# Open Issues" > open_out
    print "" > open_out
    print "The issues that are `open` or `in progress`: the bugs first, then by area, those of priority" > open_out
    print "`low` last; within a section by priority, then by number. Generated from the field lines of the" > open_out
    print "issue files by `issues/generate.sh`, like [INDEX.md](INDEX.md) - never edit it by hand." > open_out
    open_table("Bugs")
    for (i = 1; i <= areas; i++) open_table(area[i])
    open_table("Low priority")
}
' "$src"/[0-9]*.md || {
    rm -f "$index.tmp" "$open.tmp"
    exit 1
}

if [ -n "$check" ]; then
    stale=
    for f in "$index" "$open"; do
        if ! cmp -s "$f.tmp" "$f"; then
            echo "$f is not up to date; run issues/generate.sh" >&2
            stale=1
        fi
        rm -f "$f.tmp"
    done
    [ -z "$stale" ]
else
    mv "$index.tmp" "$index"
    mv "$open.tmp" "$open"
fi
