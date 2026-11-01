#!/bin/sh
# Writes the generated parts of docs/CRATES.md from the manifests of the workspace, as
# `cargo metadata` reports them: the graph - the normal dependencies between its crates, as a
# Mermaid flowchart - and the table of the programs. Each lies between its mark and the mark
# `<!-- crate-graph.sh: end -->`, the graph after `<!-- crate-graph.sh: graph -->`, the table after
# `<!-- crate-graph.sh: programs -->`, and is never edited by hand; the prose around them is.
#
#   crate-graph.sh [--check] [document]
#
# The programs are the crates with a binary. They are left out of the graph, as their arrows would
# reach every crate; the table names what each depends on directly, said against the graph, and
# where it takes a crate with other features than its default ones, or only with a feature of its
# own. The libraries, the crates that stand on both levels and the plugins of `scripts/crates.sh`
# are drawn as boxes: an arrow from a box stands for the arrows from each crate in it, drawn so
# where every one of them has it. An optional dependency is dashed and named by the feature that
# brings it. The crates and the arrows are written in the order of their names, the crates in a box
# in that of `scripts/crates.sh`.
#
# The document is read from `document`, by default docs/CRATES.md, which is written; the pre-commit
# hook in .githooks passes a copy of the document as it is staged. With --check nothing is written:
# the script fails if the document is not what it would write. A mark missing, given twice or not
# ended fails the run. It needs `jq`.
set -eu

cd "$(git rev-parse --show-toplevel)"
. scripts/crates.sh
check=
if [ "${1:-}" = --check ]; then
    check=1
    shift
fi
document="${1:-docs/CRATES.md}"

metadata=$(cargo metadata --format-version 1 --no-deps)
groups=$(jq -n --arg both "$both_levels" --arg plugins "$plugins" --arg libraries "$libraries" '
    def crates: [splits("[[:space:]]+") | select(. != "")];
    [
        {id: "both", title: "Plugin and library", members: ($both | crates)},
        {id: "plugins", title: "Plugins", members: ($plugins | crates)},
        {id: "libraries", title: "Libraries", members: ($libraries | crates)}
    ]')

# What both parts read of the metadata: the programs, and the crates of the graph with their names.
common='
    [.packages[] | select(any(.targets[].kind[]; . == "bin")) | .name] as $programs
    | ([.packages[] | select(.name | IN($programs[]) | not)] | sort_by(.name)) as $crates
    | [$crates[].name] as $names
'

graph=$(printf '%s' "$metadata" | jq -r --argjson groups "$groups" "$common"'
    | def node: sub("^caelumora-"; "") | gsub("-"; "_");
    [$crates[] | . as $crate | .dependencies[]
        | select(.kind == null and (.name | IN($names[]))) | . as $dependency
        | {
            from: $crate.name,
            to: .name,
            feature: (if .optional then
                    ([$crate.features | to_entries[]
                        | select(any(.value[]; . == "dep:" + $dependency.name)) | .key] | first)
                    // ($dependency.rename // $dependency.name)
                else null end)
        }] | unique as $edges
    | ($groups | map(select(.members | length > 0))) as $groups
    | [$groups[] | .id as $id | .members as $members
        | [$edges[] | select((.from | IN($members[])) and (.to | IN($members[]) | not))]
        | group_by([.to, .feature])[]
        | select(length == ($members | length))
        | {from: [$id], to: .[0].to, feature: .[0].feature, members: $members}] as $boxed
    | [$edges[] | . as $edge
        | select(any($boxed[]; . as $box | $box.to == $edge.to and $box.feature == $edge.feature
            and ($edge.from | IN($box.members[]))) | not)] as $loose
    | [$groups[].id] as $box_ids
    | ($box_ids + $names) as $order
    | def position: . as $n | $order | index($n);
      def place: (.from[0] | if IN($box_ids[]) then . else (. as $c
        | first($groups[] | select(.members | index($c)) | .id) // $c) end | position);
      ([$loose | group_by([.to, .feature])[]
        | {from: map(.from), to: .[0].to, feature: .[0].feature}] + $boxed) as $arrows
    | "```mermaid",
      "flowchart TD",
      ($groups[] | "    subgraph \(.id)[\"\(.title)\"]", (.members[] | "        \(node)[\"\(.)\"]"),
        "    end", ""),
      ($names[] | select(IN($groups[].members[]) | not) | "    \(node)[\"\(.)\"]"),
      "",
      ($arrows | sort_by([place, (.to | position), .feature // ""])[]
        | (.from | map(if IN($box_ids[]) then . else node end) | join(" & ")) as $from
        | (.to | node) as $to
        | if .feature == null then "    \($from) --> \($to)"
          else "    \($from) -. \"\(.feature)\" .-> \($to)" end),
      "```"
')

programs=$(printf '%s' "$metadata" | jq -r "$common"'
    | def words: if length < 2 then join("") else (.[:-1] | join(", ")) + " and " + .[-1] end;
      def code: map("`" + . + "`");
      def taken: (if .uses_default_features | not then
              if (.features | length) > 0 then
                  " with " + (.features | sort | code | words) + " alone"
              else " without features" end
          elif (.features | length) > 0 then
              " with " + (.features | sort | code | words) + " beside its default features"
          else "" end)
          + (if .optional then " if its feature `" + (.rename // .name) + "` is on" else "" end);
    "| Program | Depends on |", "|---|---|",
    (.packages | sort_by(.name)[] | select(.name | IN($programs[])) | . as $program
    | [.dependencies[] | select(.kind == null and (.name | IN($names[])))] as $direct
    | [$names[] | select(IN($direct[].name) | not)] as $missing
    | [$direct[] | {name, taken: taken} | select(.taken != "")]
        | group_by(.taken) | map((map(.name) | sort | code | words) + .[0].taken) as $taken
    | "| `\($program.name)` | every crate of the graph"
      + (if ($missing | length) > 0 then " but " + ($missing | code | words) else "" end)
      + ($taken | map("; " + .) | join(""))
      + " |")
')

written=$(mktemp -d)
trap 'rm -rf "$written"' EXIT
printf '%s\n' "$graph" > "$written/graph"
printf '%s\n' "$programs" > "$written/programs"
awk -v parts="$written" '
    function fail(message) {
        print "crate-graph.sh: " FILENAME ": " message > "/dev/stderr"
        failed = 1
        exit 1
    }
    /^<!-- crate-graph\.sh: [a-z]+ -->$/ {
        part = $0
        sub(/^<!-- crate-graph\.sh: /, "", part)
        sub(/ -->$/, "", part)
        if (part == "end") {
            if (inside == "") fail("line " NR ": an end without a mark before it")
            inside = ""
            print
            next
        }
        if (part != "graph" && part != "programs") fail("line " NR ": the unknown mark " part)
        if (inside != "") fail("line " NR ": " part " begins before " inside " ends")
        if (seen[part]++) fail("line " NR ": " part " a second time")
        print
        while ((getline line < (parts "/" part)) > 0) print line
        close(parts "/" part)
        inside = part
        next
    }
    inside == "" { print }
    END {
        if (failed) exit 1
        if (inside != "") fail(inside " has no end")
        if (!("graph" in seen)) fail("no mark graph")
        if (!("programs" in seen)) fail("no mark programs")
    }
' "$document" > "$written/document"

if [ -n "$check" ]; then
    if ! cmp -s "$written/document" "$document"; then
        echo "crate-graph.sh: docs/CRATES.md is not up to date; run scripts/crate-graph.sh" >&2
        exit 1
    fi
else
    cp "$written/document" "$document"
fi
