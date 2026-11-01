#!/bin/sh
# Finds a library that links the kernel, against the rule of docs/ARCHITECTURE.md, "The kernel
# decides, the agent side asks" (issue 516): a library - whole, or the library side of a crate that
# stands on both levels, built with its feature `library` alone - links `caelumora-api`, the model
# and other libraries, never `caelumora-kernel`; and `caelumora-api` links no crate of Caelumora but
# the model. Only normal dependencies count (`cargo tree -e normal`): a library's tests may build
# the kernel's world. Every crate found is named on stderr, and the script fails if there is one,
# or if `cargo tree` fails.
#
#   library-deps.sh
set -eu

cd "$(git rev-parse --show-toplevel)"
. scripts/crates.sh

# The names of the crates `cargo tree` finds under the package it is given: `$@` its arguments.
dependencies() {
    tree=$(cargo tree --quiet -e normal --prefix none --format '{p}' "$@")
    printf '%s\n' "$tree" | cut -d' ' -f1 | sort -u
}

found=0
for library in $libraries; do
    if dependencies -p "$library" | grep -qx 'caelumora-kernel'; then
        echo "library-deps.sh: $library depends on caelumora-kernel" >&2
        found=1
    fi
done
for crate in $both_levels; do
    if dependencies -p "$crate" --no-default-features --features library |
        grep -qx 'caelumora-kernel'; then
        echo "library-deps.sh: the library side of $crate depends on caelumora-kernel" >&2
        found=1
    fi
done
if dependencies -p caelumora-api | grep -vx 'caelumora-api\|caelumora-model' | grep -q '^caelumora-'
then
    echo "library-deps.sh: caelumora-api depends on a crate of Caelumora but the model" >&2
    found=1
fi
exit $found
