#!/bin/sh
# Finds what a library names that only the kernel may hold, against the rule of
# docs/ARCHITECTURE.md, "The kernel decides, the agent side asks" (issue 516): a library builds no
# `SecurityContext`, asks no policy, holds no manager or controller of the kernel or of a plugin,
# nor the runtime of a participant, registers through its `LibraryBuilder` alone, and reads through
# the readers, not the views, but asks the kernel through the interface it is given
# (`ActorRuntime`).
#
#   library-names.sh [file...]
#
# The libraries are the crates that run an agent or bring components for its facets, and the
# components of the plugins that stand on both levels; their code is told apart from their tests,
# which may build what they test: a file below `channel_tests/` and a `testing.rs` are left out, as
# is the item a `#[cfg(test)]` stands on - an import, or an item to its closing brace - and every
# comment. Without files, it checks every Rust file of the libraries, as `import-blocks.sh` does
# every Rust file; with files, those of them that belong to a library, passes over the other
# Rust files and names any other file on stderr. Every name found is printed as
# `file:line: names Name`, and the script fails if there is one.
set -eu

libraries='caelumora-agents/src caelumora-mcp/src caelumora-a2a/src caelumora-ollama/src
caelumora-comfyui/src caelumora-workspace-tools/src caelumora-compaction/src
caelumora-skills/src/component.rs caelumora-artifacts/src/component.rs'
names='SecurityContext|ActionType|AccessPolicy|AccessQuestions|Enforcement|OwnershipPolicy'
names="$names|AllowAllPolicy|ConfidentialityPolicy|ConfidentialityQuestions|ConfidentialityRules"
names="$names|PrivacyPolicy|Policies|SecretManager|Kernel|ControllerManager"
names="$names|SessionManager|AgentManager|UserManager|GroupManager|ProjectManager|ActorManager"
names="$names|WorkspaceManager|AttachmentManager|SkillManager|ArtifactManager"
names="$names|Views|Viewer|SessionView|ChannelView|WorkspaceView|ParticipantView|SkillViews|SkillView"
names="$names|ParticipantRuntime|AgentRuntime|ChannelRuntime|AgentStateHandle|AgentController"
names="$names|PersonalityController|SessionController|ChannelController|UserController"
names="$names|KernelBuilder|Plugin|ComponentRegistry"

if [ $# -eq 0 ]; then
    cd "$(git rev-parse --show-toplevel)"
    set -- $(git ls-files -- $libraries | grep '\.rs$')
fi

found=0
for file in "$@"; do
    applies=no
    for library in $libraries; do
        case "$file" in
            "$library" | "$library"/*) applies=yes ;;
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
