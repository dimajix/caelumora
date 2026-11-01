#!/bin/sh
# Finds what a library names that only the kernel may decide by and the model still offers, against
# the rule of docs/ARCHITECTURE.md, "The kernel decides, the agent side asks" (issue 516): a library
# builds no `SecurityContext`, asks no policy and holds no manager or store the model defines, but
# asks the kernel through the interface it is given (`ActorRuntime`). What is the kernel's - its
# managers, controllers, views and runtimes, its builder - the compiler refuses, as a library links
# the API, the model and other libraries, never the kernel (`library-deps.sh` checks that it does);
# the policies and the context stay in the model until issue 517 moves them.
#
#   library-names.sh [file...]
#
# The libraries are the crates that run an agent or bring components for its facets, and the library
# side of the crates that stand on both levels - their faces, their components and their libraries;
# their code is told apart from their tests, which may build what they test: a file below
# `channel_tests/` and a `testing.rs` are left out, as is the item a `#[cfg(test)]` stands on - an
# import, or an item to its closing brace - and every comment. Without files, it checks every Rust
# file of the libraries, as `import-blocks.sh` does every Rust file; with files, those of them that
# belong to a library, passes over the other Rust files and names any other file on stderr. Every
# name found is printed as `file:line: names Name`, and the script fails if there is one.
set -eu

. "$(dirname "$0")/libraries.sh"
paths=''
for library in $libraries; do
    paths="$paths $library/src"
done
for crate in $both_levels; do
    for file in $library_side; do
        paths="$paths $crate/src/$file"
    done
done
names='SecurityContext|ActionType|AccessPolicy|AccessQuestions|Enforcement|OwnershipPolicy'
names="$names|AllowAllPolicy|ConfidentialityPolicy|ConfidentialityQuestions|ConfidentialityRules"
names="$names|PrivacyPolicy|Policies|SecretManager|UserManager"
names="$names|EntityStore|EntityStores|StateStore|HistoryStore|TranscriptStore|MemoryStore"

if [ $# -eq 0 ]; then
    cd "$(git rev-parse --show-toplevel)"
    set -- $(git ls-files -- $paths | grep '\.rs$')
fi

found=0
for file in "$@"; do
    applies=no
    for path in $paths; do
        case "$file" in
            "$path" | "$path"/*) applies=yes ;;
        esac
    done
    case "$file" in
        */channel_tests/* | */testing.rs) applies=no ;;
    esac
    if [ "$applies" = no ]; then
        case "$file" in
            *.rs) ;;
            *) echo "library-names.sh: $file is no Rust file" >&2 ;;
        esac
        continue
    fi
    [ -f "$file" ] || continue
    if ! awk -v file="$file" -v names="$names" '
        # The item a `#[cfg(test)]` stands on: an import to its `;`, an item to its closing brace.
        skipping == 1 && /^[[:space:]]*#\[/ { next }
        skipping == 1 {
            line = $0
            if (line ~ /^[[:space:]]*(pub(\([a-z]+\))? )?use /) {
                skipping = (line ~ /;[[:space:]]*$/) ? 0 : 2
                next
            }
            depth = gsub(/\{/, "{", line) - gsub(/\}/, "}", line)
            opened = depth > 0
            skipping = (opened || line !~ /;[[:space:]]*$/) ? 3 : 0
            next
        }
        skipping == 2 { if ($0 ~ /;[[:space:]]*$/) skipping = 0; next }
        skipping == 3 {
            line = $0
            depth += gsub(/\{/, "{", line) - gsub(/\}/, "}", line)
            if (depth > 0) opened = 1
            if (opened && depth <= 0) skipping = 0
            next
        }
        /^[[:space:]]*#\[cfg\(test\)\]/ { skipping = 1; next }
        /^[[:space:]]*\/\// { next }
        {
            line = $0
            while (match(line, "(^|[^A-Za-z0-9_])(" names ")([^A-Za-z0-9_]|$)")) {
                name = substr(line, RSTART, RLENGTH)
                gsub(/[^A-Za-z]/, "", name)
                printf "%s:%d: names %s\n", file, NR, name
                failed = 1
                line = substr(line, RSTART + RLENGTH)
            }
        }
        END { exit failed }
    ' "$file"; then
        found=1
    fi
done
exit $found
