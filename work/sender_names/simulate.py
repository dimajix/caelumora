#!/usr/bin/env python3
"""A longer multi-party chat in which the model writes the agent's answers
itself, round by round (issue 135).

Scripted participants (people and another agent) talk; after each round the
model answers as Nova, and its answer becomes part of the history for the
next round. Watches whether the model starts to tag its own answers with its
name, or to write lines for other participants, as the chat goes on.

--own decides how Nova's answers go back into the history:
  plain   stored as the model wrote it, minus a leading name tag (the tag is
          stripped, as a runtime would)
  raw     stored exactly as the model wrote it
  marked  stored with the same name tag the others get

Usage:
    python3 simulate.py --model gemma4:26b --format colon --own plain
"""

import argparse
import json
import re

from experiment import AGENT, FORMATS, SYSTEM_BASE, SYSTEM_HINTS, SELF_RULE, chat, starts_with_tag, tag_patterns

ROUNDS = [
    [("alice", "Hi Nova! We need to plan the Q3 hackathon. Can you propose a two-day agenda?")],
    [("bob", "Looks good, but day 1 starts too early. Most of us commute."),
     ("carol", "Agreed with Bob. Also, we need a slot for the security talk.")],
    [("alice", "Nova, please update the agenda with Bob's and Carol's points.")],
    [("Atlas", "I'm Atlas, the facilities agent. Room 4.12 is free both days; the auditorium only on day 2."),
     ("dave", "Then the kick-off should be in the auditorium on day 2? That feels backwards.")],
    [("carol", "Nova, what do you think about Dave's concern?")],
    [("bob", "Can someone summarize who is organizing what so far? I lost track.")],
    [("alice", "I'll do catering. Carol does the security talk. Dave, can you take the prizes?"),
     ("dave", "Sure, I'll handle the prizes."),
     ("erin", "Hi, I'm new. Nova, what did I miss? Keep it short.")],
    [("Atlas", "Correction: the auditorium is now also free on day 1 afternoon."),
     ("bob", "Nova, does that change your agenda?")],
    [("carol", "The security talk needs 45 minutes, not 30. Can you fix that?")],
    [("erin", "I could run a workshop on the new API. Where would it fit?")],
    [("alice", "Nova, please post the final agenda, with who owns which slot.")],
    [("dave", "Nova, who asked for the security slot originally, and who raised the room problem?")],
]


def strip_own_tag(answer):
    for p in tag_patterns(AGENT):
        m = re.match(p, answer, re.IGNORECASE)
        if m:
            rest = answer[m.end():]
            if p.startswith(r"^\s*<message"):
                rest = re.sub(r"^[^>]*>\s*", "", rest)
                rest = re.sub(r"\s*</message>\s*$", "", rest)
            return rest.lstrip()
    return answer


def others_lines(answer, names):
    hits = []
    for line in answer.splitlines()[1:] if answer.splitlines() else []:
        for n in names:
            if any(re.search(p, line, re.IGNORECASE) for p in tag_patterns(n)):
                hits.append(line.strip()[:70])
    # a first line addressing someone ("@dave: ...") is an address, not a voice;
    # report it separately
    return hits


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="gemma4:26b")
    ap.add_argument("--format", default="colon", choices=list(FORMATS))
    ap.add_argument("--own", default="plain", choices=["plain", "raw", "marked"])
    ap.add_argument("--hint", action=argparse.BooleanOptionalAction, default=True)
    ap.add_argument("--temperature", type=float, default=0.7)
    ap.add_argument("--think", choices=["default", "on", "off"], default="off")
    ap.add_argument("--quiet", action="store_true", help="print only the per-round verdicts")
    ap.add_argument("--dump", help="append the final history to this JSONL file")
    args = ap.parse_args()

    render = FORMATS[args.format]
    think = {"default": None, "on": True, "off": False}[args.think]
    names = sorted({s for r in ROUNDS for s, _ in r})

    system = SYSTEM_BASE
    if args.hint and SYSTEM_HINTS[args.format]:
        system += " " + SYSTEM_HINTS[args.format]
        if args.own != "marked":
            system += " " + SELF_RULE
    messages = [{"role": "system", "content": system}]

    tagged = voiced = 0
    for i, rnd in enumerate(ROUNDS, 1):
        for sender, text in rnd:
            msg = {"role": "user", "content": render(sender, text)}
            if args.format == "name_field":
                msg["name"] = sender
            messages.append(msg)
        answer, _ = chat(args.model, messages, args.temperature, think)
        tag = starts_with_tag(answer, AGENT)
        voices = others_lines(answer, [n for n in names])
        first = answer.strip().splitlines()[0] if answer.strip() else ""
        address = next((n for n in names if re.match(rf"^\s*\**@?{re.escape(n)}\**\s*[:,]", first, re.IGNORECASE)), None)
        tagged += tag
        voiced += bool(voices)
        print(f"[{args.model} {args.format} own={args.own}] round {i:>2}: own_tag={int(tag)} "
              f"address={address or '-'} other_voices={voices}", flush=True)
        if not args.quiet:
            print("    " + answer.strip().replace("\n", "\n    ")[:1200])
        if args.own == "plain":
            stored = strip_own_tag(answer)
        elif args.own == "raw":
            stored = answer
        else:
            stored = render(AGENT, strip_own_tag(answer))
        messages.append({"role": "assistant", "content": stored})

    print(f"SUMMARY {args.model} format={args.format} own={args.own} hint={args.hint}: "
          f"own_tag {tagged}/{len(ROUNDS)}, other_voices {voiced}/{len(ROUNDS)}", flush=True)
    if args.dump:
        with open(args.dump, "a") as f:
            f.write(json.dumps({"model": args.model, "format": args.format, "own": args.own,
                                "hint": args.hint, "messages": messages}) + "\n")


if __name__ == "__main__":
    main()
