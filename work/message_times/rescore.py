#!/usr/bin/env python3
"""Scores the answers in results/<experiment>.jsonl again with the checks of
experiment.py as they stand, and prints the table - for checks corrected
after a run."""
import collections, json, sys
from experiment import PROBES, NOW_PROBES, imitates

def main(name):
    rows = [json.loads(l) for l in open(f"results/{name}.jsonl")]
    probes = PROBES if rows[0]["experiment"] == "probes" else NOW_PROBES
    key = "style" if rows[0]["experiment"] == "probes" else "position"
    table = collections.defaultdict(lambda: [0, 0, 0])
    for r in rows:
        t = table[(r[key], r["probe"])]
        t[0] += probes[r["probe"]][1](r["answer"])
        t[1] += imitates(r["answer"])
        t[2] += 1
    names = list(dict.fromkeys(r["probe"] for r in rows))
    variants = list(dict.fromkeys(r[key] for r in rows))
    print(f"| {key} | " + " | ".join(names) + " | imitates |\n|---|" + "---:|" * (len(names) + 1))
    for v in variants:
        cells = [f"{table[(v, p)][0]}/{table[(v, p)][2]}" for p in names]
        print(f"| {v} | " + " | ".join(cells) + f" | {sum(table[(v, p)][1] for p in names)}/{sum(table[(v, p)][2] for p in names)} |")

if __name__ == "__main__":
    main(sys.argv[1])
