# The kinds of crates (issue 516), for the scripts and the pre-commit hook that check them -
# sourced, not run: the crates that are libraries of the agent side whole (`libraries`), the crates
# that stand on both levels (`both_levels`), and of those the files that are their library side
# (`library_side`): their faces, their components and their libraries; and the crates that are
# plugins whole (`plugins`). The programs are not listed: they are the crates with a binary.
libraries='caelumora-agents caelumora-mcp caelumora-a2a caelumora-comfyui
caelumora-workspace-tools caelumora-compaction'
both_levels='caelumora-skills caelumora-artifacts'
library_side='extension.rs component.rs library.rs'
plugins='caelumora-decision caelumora-model caelumora-model-ollama caelumora-open-terminal
caelumora-store-json caelumora-store-postgres'
