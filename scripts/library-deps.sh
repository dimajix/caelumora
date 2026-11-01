#!/bin/sh
# Finds a library that links what decides, against the rule of docs/ARCHITECTURE.md, "The kernel
# decides, the agent side asks" (issues 516, 517): a library - whole, or the library side of a crate
# that stands on both levels, built with its feature `library` alone - links `caelumora-api`, the
# model and other libraries, never `caelumora-kernel`, `caelumora-security` or `caelumora-store`; and
# `caelumora-api` links no crate of Caelumora but the model. Only normal dependencies count
# (`cargo tree -e normal`): a library's tests may build the kernel's world. Every crate found is
# named on stderr, with what it links, and the script fails if there is one, or if `cargo tree`
# fails.
#
#   library-deps.sh
set -eu

cd "$(git rev-parse --show-toplevel)"
. scripts/crates.sh
forbidden='caelumora-kernel caelumora-security caelumora-store'

# The names of the crates `cargo tree` finds under the package it is given: `$@` its arguments.
dependencies() {
    tree=$(cargo tree --quiet -e normal --prefix none --format '{p}' "$@")
    printf '%s\n' "$tree" | cut -d' ' -f1 | sort -u
}

# Names on stderr each crate of `forbidden` that `found`, the dependencies of `what`, holds.
refuse() {
    what=$1
    found=$2
    for crate in $forbidden; do
        if printf '%s\n' "$found" | grep -qx "$crate"; then
            echo "library-deps.sh: $what depends on $crate" >&2
            failed=1
        fi
    done
}

failed=0
for library in $libraries; do
    refuse "$library" "$(dependencies -p "$library")"
done
for crate in $both_levels; do
    refuse "the library side of $crate" \
        "$(dependencies -p "$crate" --no-default-features --features library)"
done
if dependencies -p caelumora-api | grep -vx 'caelumora-api\|caelumora-model' | grep -q '^caelumora-'
then
    echo "library-deps.sh: caelumora-api depends on a crate of Caelumora but the model" >&2
    failed=1
fi
exit $failed
