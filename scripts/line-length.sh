#!/bin/sh
# Finds lines of prose longer than 100 characters, the width at which Markdown documents and
# comments are wrapped (docs/CODING-GUIDELINES.md, "Comments and documentation").
#
#   line-length.sh [file...]
#
# Without files, it checks what is changed against HEAD in Markdown and Rust files: the lines added
# or changed, and untracked files whole - older lines are rewrapped when they are changed anyway,
# not on their own. With files, it checks them whole.
#
# In a Markdown file every line counts; in a Rust file only comments (`//`, `///`, `//!`), since
# rustfmt keeps the code. Characters are counted, not bytes. A line is not too long where it may not
# be wrapped: a heading, a table row, a line in a code block, a field line such as `**Summary:**`,
# and a line that has no space to break at within its first 100 characters - a long link, say.
# Every line too long is printed as `file:line: length`, and the script fails if there is one.
set -eu

max=100
checked=$(mktemp)
trap 'rm -f "$checked"' EXIT

# What is checked: `file:line` for one line, `file:*` for a whole file.
if [ $# -eq 0 ]; then
    cd "$(git rev-parse --show-toplevel)"
    git diff -U0 --no-color HEAD -- '*.md' '*.rs' | awk '
        /^\+\+\+ / { file = substr($0, 7); if ($2 == "/dev/null") file = ""; next }
        /^@@/ && file != "" {
            match($0, /\+[0-9]+(,[0-9]+)?/)
            n = split(substr($0, RSTART + 1, RLENGTH - 1), range, ",")
            count = n > 1 ? range[2] : 1
            for (i = 0; i < count; i++) print file ":" range[1] + i
        }' >"$checked"
    git ls-files --others --exclude-standard -- '*.md' '*.rs' | sed 's/$/:*/' >>"$checked"
else
    for file in "$@"; do echo "$file:*"; done >"$checked"
fi
[ -s "$checked" ] || exit 0
set -- $(sed 's/:[^:]*$//' "$checked" | sort -u)

# In the C locale every awk counts bytes; each UTF-8 sequence is replaced by one character before
# anything is counted, so that lengths and positions are characters.
LC_ALL=C awk -v max="$max" '
FILENAME == ARGV[1] { checked[$0] = 1; next }
FNR == 1 { fence = 0; rust = FILENAME ~ /\.rs$/ }
{
    line = $0
    sub(/\r$/, "", line)
    gsub(/[\300-\367][\200-\277]*/, "x", line)

    # What a line begins with that is no place to break: its indentation, a comment marker, a list
    # marker.
    if (rust) {
        if (!match(line, /^[ \t]*\/\/[\/!]?[ \t]*/)) next
    } else {
        match(line, /^[ \t]*/)
    }
    lead = RLENGTH
    text = substr(line, lead + 1)
    if (match(text, /^([*+-]|[0-9]+\.)[ \t]+/)) {
        lead += RLENGTH
        text = substr(text, RLENGTH + 1)
    }

    # Code blocks are followed through the whole file, also where only some lines are checked.
    if (text ~ /^(```|~~~)/) { fence = !fence; next }
    if (!((FILENAME ":*") in checked) && !((FILENAME ":" FNR) in checked)) next
    if (fence || text ~ /^#/ || text ~ /^\|/ || text ~ /^\*\*[A-Za-z]+:\*\*/) next
    if (length(line) <= max) next

    # A line is too long only if it could be broken: at a space after its lead that leaves at most
    # max characters before it.
    for (i = lead + 1; i <= max + 1; i++) {
        if (substr(line, i, 1) == " ") {
            printf "%s:%d: %d\n", FILENAME, FNR, length(line)
            found = 1
            next
        }
    }
}
END { exit found }
' "$checked" "$@"
