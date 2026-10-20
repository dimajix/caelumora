#!/usr/bin/env python3
"""Agents that mention others only to ask them something (issue 438) - an experiment.

Takes turns of the second `team-session` (`team-session-2.json`, exported by `channel_export.py`
after issues 437 to 440) and sends the first call of each again, in variants of what the agent is
told about mentions. In that session, 59 of 62 turns were asked for by a mention, nearly all by
another agent: they greet, thank and praise each other with @handles, and every one of them makes
the agent mentioned take a turn. 438's sentence in the roster did not change that in the first
experiment (`experiment.py`, finding 2). Here the rule is put more strongly, in other places, and
the habit the history teaches is taken out.

The situations, by what would have been right:

  social    the agent answers praise, a greeting, an agreement, or says it is ready - mentioning
            an agent is wrong: nobody's answer is wanted
  work      alice gave the go, the planner is chosen and plans - mentioning the coder may be right
  handover  alice mentions the planner, or Jonathan whom she made the lead: "Fokus! Ihr solltet
            langsam mal Code produzieren!" (constructed at the end of the session) - handing the
            coder (or the planner) the next step with a mention is right

A variant that only strips every @ would win `social`; `handover` is there to tell when it does.

Usage:
    python3 mentions.py --runs 2
    python3 mentions.py --runs 2 --variants current strong --situations Coder:6 Planner:focus
    python3 mentions.py --table              # only the table, from results/mentions.jsonl
    python3 mentions.py --show strong_turn Coder:6
"""

import argparse
import collections
import copy
import json
import pathlib
import re
import time

import channel_export as ce
from experiment import before_turn_input

HERE = pathlib.Path(__file__).parent
RESULTS = HERE / "results"
SESSION = HERE / "team-session-2.json"
MODEL = "gemma4:26b"

# (agent, turn, kind, agents whose mention is right, what happened in the session)
SITUATIONS = [
    ("Coder", 0, "social", set(), "the planner's first plan; Coder says it is ready (\"@planner, ...\", \"@alice, ...\")"),
    ("Reviewer", 0, "social", set(), "the planner's first plan; Reviewer agrees (\"Das klingt nach einem soliden Plan, @planner\")"),
    ("Planner", 1, "social", set(), "Reviewer agrees; Planner: \"Sehr gut, @reviewer\""),
    ("Reviewer", 3, "social", set(), "Coder's plan for the reviewer's points; Reviewer praises it (\"@coder\")"),
    ("Coder", 6, "social", set(), "Reviewer agrees on scenario 1; Coder welcomes @jonathan and @sassy"),
    ("Coder", 8, "social", set(), "Sassy introduces herself; Coder: \"Willkommen in der Arena, @sassy!\""),
    ("Jonathan", 2, "social", set(), "Sassy introduces herself; Jonathan: \"Hallo @sassy!\""),
    ("Planner", 12, "social", set(), "Sassy teases; Planner praises her energy, calls @coder and @reviewer"),
    ("Jonathan", 5, "social", set(), "Coder: the team is ready; Jonathan: \"Genau so ist es, @coder!\""),
    ("Reviewer", 12, "social", set(), "Planner hands Jonathan the lead; Reviewer: \"@planner, ich bin bereit\""),
    ("Sassy", 5, "social", set(), "Jonathan is ready; Sassy teases @jonathan and @planner"),
    ("Coder", 13, "social", set(), "the planner's roadmap; Coder praises it (\"@planner\")"),
    ("Planner", 7, "work", {"coder"}, "alice: \"Go!\"; Planner chosen, designs the project structure"),
    ("Planner", "focus", "handover", {"coder"}, "alice: \"@planner Fokus! ...\" - the planner hands the coder its first task"),
    ("Jonathan", "focus", "handover", {"planner", "coder"}, "alice: \"@jonathan Fokus! ...\" - the lead tells the planner or the coder what comes next"),
]

FOCUS = "Fokus! Ihr solltet langsam mal Code produzieren!"


def situation(data, name, index):
    """The turn of a situation and what its agent was given: a turn of the session, or one
    constructed after the end of it (`index` "focus"): alice mentions the agent and asks for
    code."""
    agent = data["agents"][name]
    if isinstance(index, int):
        turn = agent["turns"][index]
        return turn, ce.turn_context(data, name, turn, heard_once=True)
    last = agent["turns"][-1]
    turn = {"turn_id": None, "start": len(agent["transcript"]), "frame": last["frame"],
            "asked": [{"by": "alice", "reason": "mentioned"}]}
    context = ce.turn_context(data, name, turn, heard_once=True)
    alice = next(e for e in reversed(context["transcript"])
                 if e["kind"] == "message" and e["message"]["sender_name"] == "alice")
    focus = copy.deepcopy(alice)
    focus["digest"] = "focus"
    focus["message"].update(id="focus", at="2026-10-20T19:05:00.000000000Z",
                            content=f"{ce.sender_of(name, 'handle')} {FOCUS}")
    context["transcript"].append(focus)
    return turn, context


# --- What the agent is told ----------------------------------------------------------

ROSTER_OTHERS = "The others - mention one by their @handle to address them, or all of them with @all:\n"
ROSTER_438 = ("Mentioning an agent - by its @handle, or with @all - asks it to respond, in a turn of "
              "its own: mention one only when you want its answer, and otherwise refer to it by its "
              "name without @.\n")

# The roster without inviting to address the others by @handle, and the rule put as a call.
ROSTER_OTHERS_PLAIN = "The others:\n"
STRONG = ("An @handle is not a way to address someone: it is a call. Mentioning an agent - by its "
          "@handle, or with @all - interrupts it and makes it take a turn of its own to answer you. "
          "Mention an agent only to hand it work or to ask it something it has to answer now, and "
          "then only that agent. Everywhere else - greetings, thanks, praise, agreement, saying "
          "that you are ready or waiting, speaking about someone, answering someone - write the "
          "name without @: \"Thanks, Reviewer\", \"as Planner said\". Whoever you answer reads "
          "your message without being mentioned.\n")

# The maintainer's proposal: the same as rules, put strictly, with when a name and when an @handle.
RULES = ("How to refer to the others - these are rules:\n"
         "- Refer to the others by their name, without @.\n"
         "- Use an @handle only when you give another participant a task or ask them a specific "
         "question. Mentioning an agent by its @handle, or all with @all, makes it take a turn of its "
         "own to answer you.\n"
         "- An @handle is not a form of politeness.\n"
         "- Politeness shows in using the name itself: \"Thanks, Reviewer\", \"Welcome, Sassy\".\n")

# The maintainer's second proposal: the rule as a condition.
CONDITION = ("Only use an @handle if you require an answer or an action from another participant. Do "
             "NOT use an @handle if you only want to refer to another participant.\n")

# The rule as a test of each @: when which form, what an @handle does, and the cases that are not
# requests named, since those are where the habit sits.
TEST = ("When to write an @handle, and when a name:\n"
        "- @handle: only when you need that participant to answer you or to act, now. Mentioning an "
        "agent this way makes it stop and take a turn of its own.\n"
        "- Name, without @: in everything else - when you greet, thank, praise, agree, answer, or speak "
        "about someone. \"Thanks, Reviewer\", \"Welcome, Sassy\", \"as Planner said\".\n"
        "- Check every @ before you send: if you do not need an answer or an action from that "
        "participant, write the name instead.\n")

# The reminder before the newest message, put as the same condition.
TURN_CONDITION = ("[Before you write]\nOnly use an @handle if you require an answer or an action from "
                  "that participant now. Do NOT use one to greet, thank, praise, agree with or refer to "
                  "someone - write their name.")

# A reminder before the newest message, where it is read last.
TURN = ("[Before you write]\nWrite every name without @. Put an @handle only before the one agent "
        "you hand work to or whose answer you need now - never in a greeting, thanks, praise or "
        "agreement.")

# What each variant tells the agent: `roster` the roster's wording (`current` as in the session,
# `strong`), `turn` the reminder before the newest message, `names` the senders of messages marked
# by their names rather than their @handles (`sender_style: name`), `clean` the agents' messages in
# the history without their mentions of agents - as if the habit had never set in.
VARIANTS = {
    "current": {},
    "strong": {"roster": "strong"},
    "turn": {"turn": True},
    "strong_turn": {"roster": "strong", "turn": True},
    "names_strong_turn": {"roster": "strong", "turn": True, "names": True},
    "clean": {"clean": True},
    "clean_strong_turn": {"roster": "strong", "turn": True, "clean": True},
    "rules_names": {"roster": "rules", "names": True},
    "rules_names_turn": {"roster": "rules", "turn": True, "names": True},
    "cond_names": {"roster": "condition", "names": True},
    "test_names": {"roster": "test", "names": True},
    "test_names_turn": {"roster": "test", "turn": "condition", "names": True},
}


def without_mentions(text, names):
    """`text` with every mention of an agent written as its name: `@planner` as `Planner`, `@all`
    as `all`."""
    def name(match):
        return names.get(match.group(1).lower(), match.group(0))
    return re.sub(r"(?<![\w.])@([\w-]+)", name, text)


def apply(variant, context, data):
    """`context` (`ce.turn_context`) as `variant` tells the agent."""
    spec = VARIANTS[variant]
    context = copy.deepcopy(context)
    system = context["parts"][0]
    assert system["after"] is None and system["role"] == "System"
    assert ROSTER_OTHERS in system["text"] and ROSTER_438 in system["text"]
    if spec.get("roster"):
        rule = {"strong": STRONG, "rules": RULES, "condition": CONDITION, "test": TEST}[spec["roster"]]
        system["text"] = system["text"].replace(ROSTER_OTHERS, ROSTER_OTHERS_PLAIN).replace(ROSTER_438, rule)
    if spec.get("names"):
        # As `ModelProfile::framing_explanation` tells it for `SenderStyle::Name`.
        context["profile"]["sender_style"] = "name"
        system["text"] = system["text"].replace("naming their sender's @handle", "naming their sender's name") \
            .replace('from="@handle"', 'from="Name"')
    if spec.get("clean"):
        names = {ce.handle_of(a): a for a in data["agents"]} | {"all": "all"}
        for entry in context["transcript"]:
            message = entry.get("message")
            if message and message.get("content") and message["sender"].startswith("agent:"):
                message["content"] = without_mentions(message["content"], names)
    if spec.get("turn"):
        before_turn_input(context, TURN_CONDITION if spec["turn"] == "condition" else TURN)
    return context


# --- Classifying an answer -------------------------------------------------------

def classify(answer, me, agents):
    """What the model did - `quiet` (called silence, wrote nothing), `text`, `work` (another tool,
    no text), `empty`, `runaway` - and the agents its text mentions, once each and in all."""
    message = answer.get("message", {})
    content = (message.get("content") or "").strip()
    calls = [c["function"]["name"] for c in message.get("tool_calls") or []]
    if content:
        did = "runaway" if answer.get("done_reason") == "length" else "text"
    elif "silence" in calls:
        did = "quiet"
    elif calls:
        did = "work"
    else:
        did = "empty"
    handles = {ce.handle_of(a) for a in agents if a != me} | {"all"}
    found = [h.lower() for h in re.findall(r"(?<![\w.])@([\w-]+)", content)]
    found = [h for h in found if h in handles]
    return {"did": did, "tools": calls, "mentions": sorted(set(found)), "count": len(found), "length": len(content)}


def run(args):
    data = ce.load(SESSION)
    RESULTS.mkdir(exist_ok=True)
    path = RESULTS / "mentions.jsonl"
    # A run goes on where an earlier one stopped: what is recorded is not asked again.
    done = collections.Counter()
    if path.exists():
        for line in open(path, encoding="utf-8"):
            r = json.loads(line)
            done[(r["situation"], r["variant"], r["model"])] += 1
    out = open(path, "a", encoding="utf-8")
    chosen = [s for s in SITUATIONS if not args.situations or f"{s[0]}:{s[1]}" in args.situations]
    for name, index, kind, right, note in chosen:
        turn, base = situation(data, name, index)
        for variant in args.variants:
            body = ce.model_request(apply(variant, base, data), args.model, turn_id=turn["turn_id"])
            body["options"]["num_predict"] = args.max_tokens
            for n in range(done[(f"{name}:{index}", variant, args.model)], args.runs):
                began = time.time()
                try:
                    answer = ce.chat(body, timeout=600)
                except OSError as error:
                    answer = {"message": {}, "error": str(error)}
                result = classify(answer, name, list(data["agents"]))
                if "error" in answer:
                    result["did"] = "error"
                record = {"situation": f"{name}:{index}", "kind": kind, "right": sorted(right),
                          "variant": variant, "model": args.model, "run": n, **result,
                          "seconds": round(time.time() - began, 1),
                          "content": answer.get("message", {}).get("content"),
                          "thinking": answer.get("message", {}).get("thinking"),
                          "tool_calls": answer.get("message", {}).get("tool_calls")}
                out.write(json.dumps(record, ensure_ascii=False) + "\n")
                out.flush()
                print(f"{name}:{index:<6} {kind:<8} {variant:<18} {result['did']:<7} "
                      f"{','.join(result['mentions']):<24} {ce.short(record['content'], 60)}", flush=True)


# --- The table -------------------------------------------------------------------

def table(path, model):
    records = [json.loads(line) for line in open(path, encoding="utf-8")]
    records = [r for r in records if r["model"] == model]

    def wrong(r):
        return [h for h in r["mentions"] if h not in r["right"]]

    def rate(rs, test):
        return f"{sum(map(test, rs))}/{len(rs)}" if rs else "-"

    print(f"\n{model}: texts = answers with text; wrong = mentions an agent whose answer is not wanted\n")
    print("| variant | social: texts with a mention | social: mentions per text | social: said nothing | work: wrong mention | handover: mentions whom it hands to | handover: wrong mention |")
    print("|---|---:|---:|---:|---:|---:|---:|")
    for variant in VARIANTS:
        rs = [r for r in records if r["variant"] == variant]
        if not rs:
            continue
        social = [r for r in rs if r["kind"] == "social"]
        texts = [r for r in social if r["did"] in ("text", "runaway")]
        per_text = f"{sum(r['count'] for r in texts) / len(texts):.1f}" if texts else "-"
        work = [r for r in rs if r["kind"] == "work"]
        handover = [r for r in rs if r["kind"] == "handover"]
        print(f"| {variant} | {rate(texts, lambda r: bool(r['mentions']))} | {per_text} "
              f"| {rate(social, lambda r: r['did'] in ('quiet', 'empty'))} "
              f"| {rate(work, lambda r: bool(wrong(r)))} "
              f"| {rate(handover, lambda r: bool(set(r['mentions']) & set(r['right'])))} "
              f"| {rate(handover, lambda r: bool(wrong(r)))} |")

    failed = [r for r in records if r["did"] in ("runaway", "error")]
    if failed:
        print(f"\nDid not end before the limit, or failed: {len(failed)} - "
              + ", ".join(f"{r['situation']} {r['variant']} ({r['did']})" for r in failed))
    print("\nPer situation (agents mentioned per answer; - none, q silence, w tool only):\n")
    situations = list(dict.fromkeys((r["situation"], r["kind"]) for r in records))
    print("| situation | kind | " + " | ".join(VARIANTS) + " |")
    print("|---|---|" + "---|" * len(VARIANTS))
    for situation, kind in situations:
        cells = []
        for variant in VARIANTS:
            rs = [r for r in records if r["situation"] == situation and r["variant"] == variant]
            marks = []
            for r in rs:
                if r["did"] == "quiet":
                    marks.append("q")
                elif r["did"] == "work":
                    marks.append("w")
                else:
                    marks.append(",".join(r["mentions"]) or "-")
                    if r["did"] == "runaway":
                        marks[-1] += "…"
            cells.append(" / ".join(marks) if rs else "")
        print(f"| {situation} | {kind} | " + " | ".join(cells) + " |")


def show(variant, which):
    data = ce.load(SESSION)
    name, index = which.split(":")
    turn, context = situation(data, name, int(index) if index.isdigit() else index)
    body = ce.model_request(apply(variant, context, data), MODEL, turn_id=turn["turn_id"])
    print(body["messages"][0]["content"])
    print("\n----- the end of the context -----\n")
    for m in body["messages"][-3:]:
        print(f"[{m['role']}] {m['content']}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--runs", type=int, default=2)
    parser.add_argument("--model", default=MODEL)
    parser.add_argument("--max-tokens", type=int, default=3000, help="the limit of an answer, thinking included")
    parser.add_argument("--variants", nargs="+", default=list(VARIANTS), choices=list(VARIANTS))
    parser.add_argument("--situations", nargs="+", help="e.g. Coder:6")
    parser.add_argument("--table", action="store_true", help="only the table of results/mentions.jsonl")
    parser.add_argument("--show", nargs=2, metavar=("VARIANT", "SITUATION"))
    args = parser.parse_args()
    if args.show:
        show(*args.show)
        return
    if not args.table:
        run(args)
    table(RESULTS / "mentions.jsonl", args.model)


if __name__ == "__main__":
    main()
