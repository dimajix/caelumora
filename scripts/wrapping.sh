#!/bin/sh
# Finds lines of prose wrapped against the rules of docs/CODING-GUIDELINES.md, "Comments and
# documentation": longer than 100 characters, or beginning with a dash that goes on with the
# sentence of the line before.
#
#   wrapping.sh [file...]
#
# It checks the files the rules cover - the Markdown documents in docs/, issues/ and
# .claude/agents/, AGENTS.md, and the comments of Rust files and of the web UI's TypeScript -, and
# names on stderr a file it is given that the rules do not cover. Without files, it checks what is
# changed against HEAD: the lines added or changed, and untracked files whole. With files, it checks
# them whole.
#
# In a Markdown document every line counts; in a Rust file only comments (`//`, `///`, `//!`), since
# rustfmt keeps the code, but for the inside of a macro call it cannot parse, which nothing checks;
# and in TypeScript only comments too (`//`, and `/**`, `/*` and `*` of a block). Characters are
# counted, not bytes. A line is not too long where it may not be wrapped: a heading, a table row, a
# line in a code block, a field line such as `**Summary:**`, and a line that has no space to break
# at within its first 100 characters outside a link - a link is not broken, not even in its text. A
# line that begins with `- ` goes on with the sentence of the line before where that line belongs to
# the same comment, its prose begins where the dash stands - after the list marker, if it is a list
# item -, is no heading or table row, and does not end in `.`, `:`, `?`, `!` or `**`; otherwise the
# dash begins a list item. Every line found is printed as `file:line: 112 characters` or `file:line:
# begins with a dash`, and the script fails if there is one.
set -eu

max=100
top=$(cd "$(git rev-parse --show-toplevel)" && pwd -P)
checked=$(mktemp)
trap 'rm -f "$checked"' EXIT

# Whether the rules cover `path`, relative to the top of the repository.
is_covered() {
    case "$1" in
        *.rs | caelumora-web-ui/src/*.ts | caelumora-web-ui/src/*.tsx) return 0 ;;
        docs/*.md | issues/*.md | .claude/agents/*.md | AGENTS.md) return 0 ;;
        *) return 1 ;;
    esac
}

# The entries `file:…` read whose file the rules cover.
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
            echo "wrapping.sh: $file is not covered by the wrapping rules" >&2
        fi
    done >"$checked"
fi
[ -s "$checked" ] || exit 0
set -- $(sed 's/:[^:]*$//' "$checked" | sort -u)

# In the C locale every awk counts bytes; each UTF-8 sequence is replaced by one character before
# anything is counted, so that lengths and positions are characters.
LC_ALL=C awk -v max="$max" '
# Whether `text`, the prose of a line after its indentation and list marker, ends in the middle of
# a sentence: it is no heading or table row, and does not end a sentence, introduce a list or end a
# title in bold.
function goes_on(text) {
    return text !~ /^($|#|\|)/ && text !~ /([.:?!]|\*\*)[ \t]*$/
}

FILENAME == ARGV[1] { checked[$0] = 1; next }
FNR == 1 { fence = 0; prior = ""; prior_comment = ""; rust = FILENAME ~ /\.rs$/; typescript = FILENAME ~ /\.tsx?$/ }
{
    line = $0
    sub(/\r$/, "", line)
    gsub(/[\300-\367][\200-\277]*/, "x", line)

    # The comment marker, and what follows it: the line itself in a Markdown document. The marker
    # of a block in TypeScript is `*` on every line, so that its lines are one comment.
    mark = 0
    if (rust) {
        if (!match(line, /^[ \t]*\/\/[\/!]?/)) { prior = prior_comment = ""; next }
        mark = RLENGTH
    } else if (typescript) {
        if (!match(line, /^[ \t]*(\/\/|\/\*\*?|\*)/)) { prior = prior_comment = ""; next }
        mark = RLENGTH
    }
    comment = substr(line, 1, mark)
    gsub(/[ \t]/, "", comment)
    sub(/^\/\*\*?$/, "*", comment)

    # What a line begins with that is no place to break: its indentation, a comment marker, a list
    # marker. Where its list marker and its prose begin is counted from the comment marker.
    text = substr(line, mark + 1)
    sub(/^[ \t]+/, "", text)
    lead = length(line) - length(text)
    indent = lead - mark
    dash = text ~ /^- /
    if (match(text, /^([*+-]|[0-9]+\.)[ \t]+/)) {
        lead += RLENGTH
        text = substr(text, RLENGTH + 1)
    }

    # Code blocks and the line before are followed through the whole file, also where only some
    # lines are checked.
    if (text ~ /^(```|~~~)/) { fence = !fence; prior = prior_comment = ""; next }
    before = prior
    before_comment = prior_comment
    before_column = prior_column
    prior = fence ? "" : text
    prior_comment = comment
    prior_column = lead - mark
    if (!((FILENAME ":*") in checked) && !((FILENAME ":" FNR) in checked)) next
    if (fence) next

    # A dash where the prose of the line before begins goes on with its sentence: to rustdoc and to
    # Markdown, it begins a list. A dash left of it is the next item of the list the line before is
    # in.
    if (dash && comment == before_comment && indent == before_column && goes_on(before)) {
        printf "%s:%d: begins with a dash\n", FILENAME, FNR
        found = 1
    }

    if (text ~ /^#/ || text ~ /^\|/ || text ~ /^\*\*[A-Za-z]+:\*\*/) next
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
            printf "%s:%d: %d characters\n", FILENAME, FNR, length(line)
            found = 1
            next
        }
    }
}
END { exit found }
' "$checked" "$@"
