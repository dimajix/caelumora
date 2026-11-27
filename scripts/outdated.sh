#!/bin/sh
# Says which dependencies are behind their newest versions, for the update made once a month
# (docs/CRATES.md, "Keeping the dependencies up to date", issue 620). It changes nothing.
#
#   outdated.sh
#
# Of the Rust crates, a dry run of `cargo update`: `Updating` names a crate the update raises
# within the ranges of the manifests, `Unchanged` one whose newer version lies outside them - a new
# major version of a crate the workspace declares, or one another crate holds back. Of the web UI,
# `npm outdated`: `Wanted` is what `npm update` installs, `Latest` the newest version. Both ask
# their registries, so it needs network access. The web UI's packages are read as they are
# installed (`npm ci` in caelumora-web-ui).
set -eu

cd "$(git rev-parse --show-toplevel)"

echo "Rust crates (cargo update --dry-run --verbose):"
cargo update --dry-run --verbose 2>&1 | grep -E '^ *(Updating|Unchanged|Adding|Removing) [^ ]+ v' \
    || echo "    none behind"

echo
echo "npm packages of the web UI (npm outdated):"
# `npm outdated` fails when it finds a package behind; it found nothing when it prints nothing.
outdated=$(cd caelumora-web-ui && npm outdated || true)
if [ -n "$outdated" ]; then
    echo "$outdated" | sed 's/^/    /'
else
    echo "    none behind"
fi
