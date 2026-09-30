#!/bin/bash
# Runs experiment.py for each model into its own output files.
# Usage: ./run_models.sh model[=think] ...   (think: default|on|off, default "default")
cd "$(dirname "$0")"
for spec in "$@"; do
  model="${spec%%=*}"; think="default"; [[ "$spec" == *=* ]] && think="${spec#*=}"
  out="results/$(echo "$model" | tr ':/' '__')"
  mkdir -p results
  rm -f "$out.jsonl"
  echo "=== $model (think=$think) $(date +%T)"
  python3 experiment.py --models "$model" --runs 3 --hint both --think "$think" --dump "$out.jsonl" > "$out.txt" 2>&1
  echo "=== $model done $(date +%T) exit=$?"
done
