# How a script that checks Rust code as one text reads its files - loaded with `awk -f`, not run.
# Each file is read whole before it is checked: as `text` of `size` characters, its line `n` in
# `lines[n]` beginning at `start[n]`, `count` lines in all, its name in `file`. Comments, strings
# and character literals are stepped over by `literal`.
#
# The script that loads it defines `check()`, called once for each file as soon as it is read, and
# sets `found` where `check` finds something, which makes awk fail. It runs awk in the C locale
# (`LC_ALL=C`), in which every awk counts bytes: each UTF-8 sequence is replaced by one character
# before anything is counted, so that lengths and positions are characters.

# Adds `line`, the next line of the file, to the text.
function take(line) {
    sub(/\r$/, "", line)
    gsub(/[\300-\367][\200-\277]*/, "x", line)
    lines[++count] = line
    start[count] = size + 1
    text = text line "\n"
    size += length(line) + 1
}

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
    if (c == "'" && substr(text, p, 3) ~ /^'[^\\']'$/) return p + 3
    if (c == "'" && substr(text, p, 4) ~ /^'\\.'$/) return p + 4
    return p
}

FNR == 1 { if (count) check(); file = FILENAME; count = 0; size = 0; text = "" }
{ take($0) }
END { if (count) check(); exit found }
