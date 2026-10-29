#!/bin/sh
# Finds tuple bindings that do not fit on one line, against the rule of docs/CODING-GUIDELINES.md,
# "Small return values": a `let` that binds several values by a tuple made on the spot - `let (a,
# b) = (x, y);` - stays on one line of at most 100 characters, the indentation counted.
#
#   tuple-bindings.sh [file...]
#
# Without files, it checks every Rust file of the repository - unlike `scripts/wrapping.sh`, which
# checks what changed, since no binding of the repository breaks the rule and none is to begin to;
# with files, those of them that are Rust files, and names any other on stderr. A binding is a
# `let` at the beginning of its line whose pattern is a tuple, followed - after a type, if one is
# given - by a tuple of two parts or more and a `;`, on as many lines as it takes, comments and
# string literals - raw ones too - stepped over; a `let … else` matches a pattern instead, and is
# left alone. Whether the values belong together is no script's to judge: only the length is
# checked. Characters are counted, not bytes. Every binding found is printed as `file:line: 112
# characters` or `file:line: across 3 lines`, and the script fails if there is one.
set -eu

max=100

if [ $# -eq 0 ]; then
    cd "$(git rev-parse --show-toplevel)"
    set -- $(git ls-files -- '*.rs')
else
    for file in "$@"; do
        shift
        case "$file" in
            *.rs) set -- "$@" "$file" ;;
            *) echo "tuple-bindings.sh: $file is no Rust file" >&2 ;;
        esac
    done
fi
[ $# -gt 0 ] || exit 0

# In the C locale every awk counts bytes; each UTF-8 sequence is replaced by one character before
# anything is counted, so that lengths and positions are characters. A file is read as one text,
# `text` of `size` characters, its line `n` beginning at `start[n]`.
LC_ALL=C awk -v max="$max" '
# The position after a comment, a string or a character literal at `p`, or `p` itself if none
# begins there - a quote that begins a lifetime.
function literal(p,    c, q, hashes) {
    c = substr(text, p, 1)
    if (c == "/" && substr(text, p + 1, 1) == "/") {
        q = index(substr(text, p), "\n")
        return q ? p + q - 1 : size + 1
    }
    if (c == "/" && substr(text, p + 1, 1) == "*") {
        q = index(substr(text, p + 2), "*/")
        return q ? p + q + 3 : size + 1
    }
    if (c == "r" && substr(text, p - 1, 1) !~ /[A-Za-z0-9_]/ && match(substr(text, p), /^r#*"/)) {
        hashes = substr(text, p + 1, RLENGTH - 2)
        q = index(substr(text, p + RLENGTH), "\"" hashes)
        return q ? p + RLENGTH + q + length(hashes) : size + 1
    }
    if (c == "\"") {
        for (p++; p <= size; p++) {
            c = substr(text, p, 1)
            if (c == "\\") p++
            else if (c == "\"") return p + 1
        }
        return p
    }
    if (c == "'\''" && substr(text, p, 3) ~ /^'\''[^\\'\'']'\''$/) return p + 3
    if (c == "'\''" && substr(text, p, 4) ~ /^'\''\\.'\''$/) return p + 4
    return p
}

# The position of the first character from `p` on that is neither white space nor in a comment.
function skip(p,    q) {
    while (p <= size) {
        if (substr(text, p, 1) ~ /[ \t\n]/) { p++; continue }
        if (substr(text, p, 2) !~ /^\/[\/*]/) break
        p = literal(p)
    }
    return p
}

# The position of the bracket that closes the one at `p`, or 0 if the text ends before it.
# `parts` is set to whether a comma between them, outside a nested bracket, is followed by another
# part - not by the closing bracket, as the comma of a tuple of one is.
function closing(p,    depth, c, q) {
    depth = 0
    parts = 0
    while (p <= size) {
        q = literal(p)
        if (q != p) { p = q; continue }
        c = substr(text, p, 1)
        if (c ~ /[([{]/) depth++
        else if (c ~ /[])}]/) { if (--depth == 0) return p }
        else if (c == "," && depth == 1 && substr(text, skip(p + 1), 1) !~ /[])}]/) parts = 1
        p++
    }
    return 0
}

# Whether the `=` at `p` assigns: it is no part of `==`, `=>`, `!=`, `<=` or `>=`.
function alone(p) {
    return substr(text, p + 1, 1) !~ /[=>]/ && substr(text, p - 1, 1) !~ /[!<>=]/
}

# The position of the `=` that ends the type of a binding, from `p` on, or 0.
function assignment(p,    depth, c, q) {
    depth = 0
    while (p <= size) {
        q = literal(p)
        if (q != p) { p = q; continue }
        c = substr(text, p, 1)
        if (c ~ /[([{]/) depth++
        else if (c ~ /[])}]/) depth--
        else if (c == ";" && depth == 0) return 0
        else if (c == "=" && depth == 0 && alone(p)) return p
        p++
    }
    return 0
}

# How many lines the tuple binding beginning at line `n` takes, or 0 if no tuple binding begins
# there.
function binding(n,    p, q, r, taken) {
    p = start[n] + index(lines[n], "let (") + 3
    q = closing(p)
    if (!q) return 0
    r = skip(q + 1)
    if (substr(text, r, 1) == ":") r = assignment(r + 1)
    else if (substr(text, r, 1) != "=" || !alone(r)) r = 0
    if (!r) return 0
    p = skip(r + 1)
    if (substr(text, p, 1) != "(") return 0
    q = closing(p)
    if (!q || !parts) return 0
    r = skip(q + 1)
    if (substr(text, r, 1) != ";") return 0
    for (taken = 1; n + taken <= count && start[n + taken] <= r; taken++);
    return taken
}

function check(    n, taken) {
    size = length(text)
    for (n = 1; n <= count; n++) {
        if (lines[n] !~ /^[ \t]*let \(/) continue
        taken = binding(n)
        if (taken > 1) {
            printf "%s:%d: across %d lines\n", file, n, taken
            found = 1
        } else if (taken == 1 && length(lines[n]) > max) {
            printf "%s:%d: %d characters\n", file, n, length(lines[n])
            found = 1
        }
    }
}

FNR == 1 { if (count) check(); file = FILENAME; count = 0; text = "" }
{
    line = $0
    sub(/\r$/, "", line)
    gsub(/[\300-\367][\200-\277]*/, "x", line)
    lines[++count] = line
    start[count] = length(text) + 1
    text = text line "\n"
}
END { if (count) check(); exit found }
' "$@"
