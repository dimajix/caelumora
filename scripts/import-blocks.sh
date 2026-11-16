#!/bin/sh
# Finds a module, a function or a block whose declarations and imports do not stand in their order,
# against the rule of docs/CODING-GUIDELINES.md, "Imports": a module's `mod` declarations come
# first, then its re-exports (`pub use`), then its private imports in one block, which rustfmt
# sorts; a function's or a block's imports stand in one block at its start.
#
#   import-blocks.sh [file...]
#
# Without files, it checks every Rust file of the repository, as `scripts/tuple-bindings.sh` does;
# with files, those of them that are Rust files, and names any other on stderr. A module is told by
# the indentation rustfmt gives its items: the imports of one level belong together until a line
# less indented ends the module, the function or the block they stand in. A private import that
# follows another of its level with a blank line or a comment between them begins a group of its
# own; a re-export or a module declared without a body (`mod name;`) that follows a private import
# of its level stands after the imports; an import or a re-export that follows anything of its
# level but a module declared without a body, an import or an attribute follows code, and is told
# so alone. A line of a string that begins with `use ` is taken for an import; none does. Every
# line found is printed as `file:line: the imports are split`, `file:line: a re-export after the
# imports`, `file:line: a module declared after the imports` or `file:line: an import after code`,
# and the script fails if there is one.
set -eu

if [ $# -eq 0 ]; then
    cd "$(git rev-parse --show-toplevel)"
    set -- $(git ls-files -- '*.rs')
else
    for file in "$@"; do
        shift
        case "$file" in
            *.rs) set -- "$@" "$file" ;;
            *) echo "import-blocks.sh: $file is no Rust file" >&2 ;;
        esac
    done
fi
[ $# -gt 0 ] || exit 0

# Per level of indentation, `imported` is whether a private import stands there, `apart` whether
# something else has followed it since, `coded` whether anything but a module declared without a
# body, an import or an attribute has stood there; `level` is the level of the last line that is
# not blank.
# An import or an attribute that rustfmt breaks over several lines ends where `rest` matches.
LC_ALL=C awk '
function report(message) {
    printf "%s:%d: %s\n", FILENAME, FNR, message
    found = 1
}

FNR == 1 { split("", imported); split("", apart); split("", coded); level = 0; rest = "" }

{ sub(/\r$/, ""); sub(/[ \t]*\/\/.*$/, "") }

rest != "" { if ($0 ~ rest) rest = ""; next }

/^[ \t]*$/ { apart[level] = 1; next }

{
    match($0, /^[ \t]*/)
    for (n = RLENGTH + 1; n <= level; n++) { imported[n] = 0; apart[n] = 0; coded[n] = 0 }
    level = RLENGTH
}

/^[ \t]*#!?\[/ { if ($0 ~ /\($/) rest = "^[ \t]*\\)+]$"; next }

/^[ \t]*(pub(\([^)]*\))? )?use / {
    if (coded[level]) report("an import after code")
    if ($0 ~ /^[ \t]*pub/) {
        if (imported[level]) report("a re-export after the imports")
    } else {
        if (imported[level] && apart[level] && !coded[level]) report("the imports are split")
        imported[level] = 1
        apart[level] = 0
    }
    if ($0 !~ /;$/) rest = ";$"
    next
}

/^[ \t]*(pub(\([^)]*\))? )?mod [A-Za-z0-9_]+;$/ {
    if (imported[level]) report("a module declared after the imports")
    apart[level] = 1
    next
}

{ apart[level] = 1; coded[level] = 1 }

END { exit found }
' "$@"
