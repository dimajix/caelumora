# The libraries of the agent side (issue 516), for the scripts and the pre-commit hook that check
# them - sourced, not run: the crates that are libraries whole (`libraries`), the crates that stand
# on both levels (`both_levels`), and of those the files that are their library side
# (`library_side`): their faces, their components and their libraries.
libraries='caelumora-agents caelumora-mcp caelumora-a2a caelumora-ollama caelumora-comfyui
caelumora-workspace-tools caelumora-compaction'
both_levels='caelumora-skills caelumora-artifacts'
library_side='extension.rs component.rs library.rs'
