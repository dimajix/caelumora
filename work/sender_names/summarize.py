#!/usr/bin/env python3
"""Overview over all results/*.jsonl of experiment.py (issue 135).

Per model and variant (format, own messages marked, hint): the share of
answers tagged with the agent's own name, the share speaking for others, and
the share passing the probe's check - overall and for the probes that can
only be answered by knowing the sender.
"""

import collections
import glob
import json
import re
import sys

# probes that cannot be answered from the text alone
SENDER_PROBES = {"who_asked", "who_objected", "tell_apart_agents"}


def rescore(r):
    """A stricter check than experiment.py's keyword test where that one
    lets a wrong attribution pass: for who_asked, naming Alice and Carol is
    not enough - Carol must be the one who asked."""
    if r["probe"] != "who_asked":
        return r["correct"]
    a = r["answer"].lower().replace("*", "")
    carol_asked = re.search(r"carol\W+(\w+\W+){0,3}ask", a) or re.search(r"ask\w*\W+(\w+\W+){0,6}by\W+carol", a) \
        or re.search(r"who asked[^?]*\?\W*carol", a)
    alice_asked = re.search(r"alice\W+(?!was\b)(\w+\W+){0,2}asked", a) or re.search(r"asked\W+(\w+\W+){0,6}by\W+alice", a)
    amount = any(x in a for x in ("5000", "5,000", "5.000"))
    return bool(carol_asked and not alice_asked and amount)


def pct(a, n):
    return f"{100 * a / n:3.0f}%" if n else "   -"


def main(paths):
    stats = collections.defaultdict(lambda: collections.Counter())
    models = []
    for path in paths:
        for line in open(path):
            r = json.loads(line)
            if r["model"] not in models:
                models.append(r["model"])
            key = (r["model"], r["format"], r["mark_self"], r["hint"])
            s = stats[key]
            s["n"] += 1
            s["prefix"] += r["prefix"]
            s["voice"] += bool(r["ventriloquy"])
            ok = rescore(r)
            s["right"] += ok
            if r["probe"] in SENDER_PROBES:
                s["sn"] += 1
                s["sright"] += ok
            s["empty"] += not r["answer"].strip()

    print(f"| model | format | own marked | hint | own name tag | speaks for others | correct | correct (sender probes) | empty |")
    print("|---|---|---|---|---:|---:|---:|---:|---:|")
    for model in models:
        for (m, fmt, self_, hint), s in stats.items():
            if m != model:
                continue
            print(f"| {m} | {fmt} | {'yes' if self_ else 'no'} | {'yes' if hint else 'no'} | "
                  f"{pct(s['prefix'], s['n'])} | {pct(s['voice'], s['n'])} | {pct(s['right'], s['n'])} | "
                  f"{pct(s['sright'], s['sn'])} | {s['empty'] or ''} |")


if __name__ == "__main__":
    main(sys.argv[1:] or sorted(glob.glob("results/*.jsonl")))
