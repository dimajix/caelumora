#!/bin/sh
# Finds lines of prose longer than 100 characters, the width at which Markdown documents and
# comments are wrapped (docs/CODING-GUIDELINES.md, "Comments and documentation").
#
#   line-length.sh [file...]
#
# It checks the files the rule covers - the Markdown documents in docs/, issues/ and
# .claude/agents/, AGENTS.md, and the comments of Rust files and of the web UI's TypeScript -, and
# names on stderr a file it is given that the rule does not cover. Without files, it checks what is
# changed against HEAD: the lines added or changed, and untracked files whole. With files, it checks
# them whole.
#
# In a Markdown document every line counts; in a Rust file only comments (`//`, `///`, `//!`), since
# rustfmt keeps the code, and in TypeScript only comments too (`//`, and `/**`, `/*` and `*` of a
# block). Characters are counted, not bytes. A line is not too long where it may not be wrapped: a
# heading, a table row, a line in a code block, a field line such as `**Summary:**`, and a line that
# has no space to break at within its first 100 characters outside a link - a link is not broken,
# not even in its text. Every line too long is printed as `file:line: length`, and the script fails
# if there is one.
set -eu

max=100
top=$(cd "$(git rev-parse --show-toplevel)" && pwd -P)
checked=$(mktemp)
trap 'rm -f "$checked"' EXIT

# Whether the rule covers `path`, relative to the top of the repository.
is_covered() {
    case "$1" in
        *.rs | caelumora-web-ui/src/*.ts | caelumora-web-ui/src/*.tsx) return 0 ;;
        docs/*.md | issues/*.md | .claude/agents/*.md | AGENTS.md) return 0 ;;
        *) return 1 ;;
    esac
}

# The entries `file:…` read whose file the rule covers.
covered_only() {
    while IFS= read -r entry; do
        if is_covered "${entry%:*}"; then echo "$entry"; fi
    done
}

# What is checked: `file:line` for one line, `file:*` for a whole file.
if [ $# -eq 0 ]; then
    cd "$top"
    git diff -U0 --no-color HEAD -- '*.md' '*.rs' '*.ts' '*.tsx' | awk '
        /^\+\+\+ / { file = substr($0, 7); if ($2 == "/dev/null") file = ""; next }
        /^@@/ && file != "" {
            match($0, /\+[0-9]+(,[0-9]+)?/)
            n = split(substr($0, RSTART + 1, RLENGTH - 1), range, ",")
            count = n > 1 ? range[2] : 1
            for (i = 0; i < count; i++) print file ":" range[1] + i
        }' | covered_only >"$checked"
    git ls-files --others --exclude-standard -- '*.md' '*.rs' '*.ts' '*.tsx' | sed 's/$/:*/' \
        | covered_only >>"$checked"
else
    for file in "$@"; do
        dir=$(cd "$(dirname "$file")" && pwd -P)
        relative=${dir#"$top"}/$(basename "$file")
        if is_covered "${relative#/}"; then
            echo "$file:*"
        else
            echo "line-length.sh: $file is not covered by the wrapping rule" >&2
        fi
    done >"$checked"
fi
[ -s "$checked" ] || exit 0
set -- $(sed 's/:[^:]*$//' "$checked" | sort -u)

# In the C locale every awk counts bytes; each UTF-8 sequence is replaced by one character before
# anything is counted, so that lengths and positions are characters.
LC_ALL=C awk -v max="$max" '
FILENAME == ARGV[1] { checked[$0] = 1; next }
FNR == 1 { fence = 0; rust = FILENAME ~ /\.rs$/; typescript = FILENAME ~ /\.tsx?$/ }
{
    line = $0
    sub(/\r$/, "", line)
    gsub(/[\300-\367][\200-\277]*/, "x", line)

    # What a line begins with that is no place to break: its indentation, a comment marker, a list
    # marker.
    if (rust) {
        if (!match(line, /^[ \t]*\/\/[\/!]?[ \t]*/)) next
    } else if (typescript) {
        if (!match(line, /^[ \t]*(\/\/|\/\*\*?|\*)[ \t]*/)) next
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

    # A link is not broken: the spaces in it are no place to break.
    rest = line
    line = ""
    while (match(rest, /\[[^]]*\]\([^)]*\)/)) {
        link = substr(rest, RSTART, RLENGTH)
        gsub(/ /, "_", link)
        line = line substr(rest, 1, RSTART - 1) link
        rest = substr(rest, RSTART + RLENGTH)
    }
    line = line rest

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
