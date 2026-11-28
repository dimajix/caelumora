#!/bin/sh
# Finds lines of Rust code longer than 100 characters that rustfmt leaves to their author, against
# the rule of docs/CODING-GUIDELINES.md, "Code is laid out by rustfmt": what rustfmt leaves as
# written - the inside of a macro call it cannot parse, a statement around a string literal it
# cannot fit - is broken by hand within 100 characters, as rustfmt would break it.
#
#   code-width.sh [file...]
#
# Without files, it checks every Rust file of the repository, as `scripts/tuple-bindings.sh` does;
# with files, those of them that are Rust files, and names any other on stderr. A file is taken to
# be laid out by rustfmt already, as the pre-commit hook makes sure before. A line is looked at if
# it is longer than 100 characters, has no string literal on it or running across it, and has a
# space in its code past its indent - outside a comment and a character literal, and not before a
# comment that ends the line, which a break would move to a line of its own, where rustfmt keeps
# it: a comment line is `scripts/wrapping.sh`'s, a line of a single word cannot be broken. Whose a
# line is, rustfmt is asked: in a copy of the file, the line is broken at every such space, and
# rustfmt lays out the copy. A line rustfmt puts back together is its own - a test's long name, a
# return type -, and stays; a line it leaves broken is the author's. Rustfmt and its settings are
# those of the repository the script lies in - the edition of its `Cargo.toml`, its
# `rustfmt.toml` -, wherever it is called from. Characters are counted, not bytes. Every line found
# is printed as `file:line: 112 characters`, and the script fails if there is one; a file rustfmt
# cannot read, or whose layout cannot be compared, is named as `file: not read by rustfmt` or
# `file: not compared`.
set -eu

max=100
top=$(cd "$(dirname "$0")/.." && pwd -P)
edition=$(sed -n 's/^edition *= *"\([0-9]*\)".*/\1/p' "$top/Cargo.toml" | head -n 1)
probe=$(mktemp)
formatted=$(mktemp)
trap 'rm -f "$probe" "$formatted"' EXIT

if [ $# -eq 0 ]; then
    cd "$top"
    set -- $(git ls-files -- '*.rs')
else
    for file in "$@"; do
        shift
        case "$file" in
            *.rs) set -- "$@" "$file" ;;
            *) echo "code-width.sh: $file is no Rust file" >&2 ;;
        esac
    done
fi
[ $# -gt 0 ] || exit 0

# Each file is read as `scripts/rust-text.awk` describes, which calls `check`.
# What is particular to the width of code follows on the standard input as a second program file
# (`-f -`): once awk is given one with `-f`, it takes no program as an argument.
LC_ALL=C awk -v max="$max" -v probe="$probe" -v formatted="$formatted" \
    -v rustfmt="rustfmt --edition $edition --config-path '$top/rustfmt.toml'" \
    -f "$top/scripts/rust-text.awk" -f - "$@" <<'EOF'
# Finds the comments, strings and character literals of the text: each as the characters from
# `from[i]` to before `to[i]`, `literals` of them in order, `comment[i]` set for a comment;
# `quoted[n]` is set for a line that a string runs on.
function find_literals(    n, p, q, c, s) {
    literals = 0
    split("", quoted)
    start[count + 1] = size + 1
    n = 1
    p = 1
    while (p <= size) {
        while (start[n + 1] <= p) n++
        s = substr(lines[n], p - start[n] + 1)
        if (!match(s, /\/[\/*]|["'r]/)) {
            p = start[n + 1]
            continue
        }
        p += RSTART - 1
        q = literal(p)
        if (q == p) {
            p++
            continue
        }
        c = substr(text, p, 1)
        from[++literals] = p
        to[literals] = q
        comment[literals] = c == "/"
        for (; n <= count && start[n] < q; n++) if (c == "\"" || c == "r") quoted[n] = 1
        n--
        p = q
    }
}

# The spaces of line `n` to break it at, by their order among its spaces - `1 3 4` -, or "" if
# it has none: those in its code past its indent, outside a comment and a character literal, and
# before its last character of code - not one before a comment that ends the line, which a break
# would move to a line of its own.
function breaks(n,    line, i, j, k, p, at, last, indent) {
    line = lines[n]
    match(line, /^[ \t]*/)
    indent = RLENGTH
    last = 0
    i = 1
    for (j = indent + 1; j <= length(line); j++) {
        p = start[n] + j - 1
        while (i <= literals && to[i] <= p) i++
        if (i <= literals && from[i] <= p && comment[i]) continue
        if (substr(line, j, 1) != " ") last = j
    }
    at = ""
    k = 0
    i = 1
    for (j = 1; j < last; j++) {
        if (substr(line, j, 1) != " ") continue
        k++
        if (j <= indent) continue
        p = start[n] + j - 1
        while (i <= literals && to[i] <= p) i++
        if (i <= literals && from[i] <= p) continue
        at = at " " k
    }
    return substr(at, 2)
}

# Writes the file to `probe`, its line `n` broken where `cut[n]` names, each part at its indent.
function write_probe(    n, raw, indent, k, ordinals, chosen, part, out) {
    printf "" > probe
    n = 0
    while ((getline raw < file) > 0) {
        n++
        if (!(n in cut)) {
            print raw > probe
            continue
        }
        match(raw, /^[ \t]*/)
        indent = substr(raw, 1, RLENGTH)
        split("", chosen)
        split(cut[n], ordinals, " ")
        for (k in ordinals) chosen[ordinals[k]] = 1
        out = ""
        k = 0
        while (match(raw, / /)) {
            part = substr(raw, 1, RSTART - 1)
            raw = substr(raw, RSTART + 1)
            out = out part ((++k in chosen) ? "\n" indent : " ")
        }
        print out raw > probe
    }
    close(file)
    close(probe)
}

# Marks in `changed` the lines of the file that rustfmt does not lay out as they stand in it;
# whether `diff` could compare them.
function compare(    command, row, range, ends, n) {
    split("", changed)
    command = "diff '" file "' '" formatted "'"
    while ((command | getline row) > 0) {
        if (!match(row, /^[0-9]+(,[0-9]+)?[cd]/)) continue
        range = substr(row, 1, RLENGTH - 1)
        if (split(range, ends, ",") == 1) ends[2] = ends[1]
        for (n = ends[1]; n <= ends[2]; n++) changed[n] = 1
    }
    return close(command) <= 1
}

function check(    n, at, any) {
    for (n = 1; n <= count; n++) if (length(lines[n]) > max) any = 1
    if (!any) return
    find_literals()
    split("", cut)
    any = 0
    for (n = 1; n <= count; n++) {
        if (length(lines[n]) <= max || (n in quoted)) continue
        at = breaks(n)
        if (at != "") { cut[n] = at; any = 1 }
    }
    if (!any) return
    write_probe()
    if (system(rustfmt " < '" probe "' > '" formatted "' 2>/dev/null") != 0) {
        printf "%s: not read by rustfmt\n", file
        found = 1
        return
    }
    if (!compare()) {
        printf "%s: not compared\n", file
        found = 1
        return
    }
    for (n = 1; n <= count; n++) {
        if ((n in cut) && (n in changed)) {
            printf "%s:%d: %d characters\n", file, n, length(lines[n])
            found = 1
        }
    }
}
EOF
