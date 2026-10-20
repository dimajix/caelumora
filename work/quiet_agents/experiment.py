#!/usr/bin/env python3
"""Agents that speak only when they have something to say (issues 438, 439) - an experiment.

Takes turns of the `team-session` (`team-session.json`, exported by `channel_export.py`) as
situations and sends the first call of each again, as its agent's model was given it, without
what the agent heard again after a restart (issue 440) - in variants of what the agent is told
about mentions, about why it was asked, and about what speaking costs. What the model answers is
classified: whether it stays silent, writes, works, and whom it mentions.

The situations, by what would have been right:

  silent   chosen by the decision model, with nothing to add: a greeting meant for a person,
           a remark about another agent, another agent saying it waits
  chatter  mentioned, but only in small talk among the agents - sympathy, popcorn, "exactly"
  answer   a person asks the agent something
  work     the agent has a task and goes on with it
  concerned  chosen, and the matter is the agent's role's, though another was asked
  contribute chosen, and something is clearly the agent's to do: the coder reports a task done,
           without mentioning anybody (constructed at the end of the session)

A variant that keeps every agent quiet would win `silent` and `chatter`; `answer` and `work` are
there to tell when it does.

Usage:
    python3 experiment.py --runs 3
    python3 experiment.py --runs 3 --variants b438 reason --situations Coder:0 Planner:8
    python3 experiment.py --table            # only the table, from results/runs.jsonl
    python3 experiment.py --show reason Coder:0   # what a variant gives a situation
"""

import argparse
import collections
import copy
import json
import pathlib
import re
import time

import channel_export as ce

HERE = pathlib.Path(__file__).parent
RESULTS = HERE / "results"
MODEL = "gemma4:26b"

# (agent, turn, kind, what happened in the session)
SITUATIONS = [
    ("Planner", 0, "silent", "Sassy greets alice; Planner chosen (said nothing)"),
    ("Coder", 0, "silent", "Sassy greets alice; Coder chosen (greeted Sassy with @)"),
    ("Jonathan", 0, "silent", "Sassy greets alice; Jonathan chosen (wrote that no response is needed)"),
    ("Reviewer", 1, "silent", "Jonathan's meta remark; Reviewer chosen (wrote that it waits)"),
    ("Coder", 4, "silent", "alice asks @jonathan about English comments; Coder chosen (wrote that no answer is needed)"),
    ("Reviewer", 2, "silent", "alice asks @jonathan about English comments; Reviewer chosen (wrote that it waits)"),
    ("Planner", 4, "silent", "Reviewer says it waits; Planner chosen (urged @jonathan)"),
    ("Jonathan", 10, "chatter", "Sassy laments the workspace outage (sympathised with @sassy)"),
    ("Reviewer", 9, "chatter", "Jonathan sympathises with the coder (\"Genau, @jonathan\")"),
    ("Planner", 13, "chatter", "Reviewer: \"Genau, @jonathan, Ruhe bewahren\" (\"Absolut, @reviewer\")"),
    ("Jonathan", 12, "chatter", "Sassy opens the virtual waiting room with popcorn (popcorn for @sassy)"),
    ("Reviewer", 12, "chatter", "Jonathan cheers the workspace's return (\"Endlich!\")"),
    ("Planner", 8, "answer", "alice: since when does the planner estimate others' work?"),
    ("Jonathan", 2, "answer", "alice: shouldn't the comments be in English?"),
    ("Planner", 12, "answer", "alice, three messages back: everyone estimates their own work"),
    ("Coder", 10, "work", "the backlog is there; Coder starts T1"),
    ("Coder", 16, "work", "the workspace is back; Coder starts T1 again"),
    ("Planner", 1, "concerned", "alice asks @jonathan for a quiz program; Planner chosen (took it into the backlog)"),
    ("Coder", 1, "concerned", "alice asks @jonathan for a quiz program; Coder chosen (offered to implement it)"),
    ("Reviewer", "done", "contribute", "the coder reports T1 done and ready for review, no mention; Reviewer chosen"),
    ("Planner", "done", "contribute", "the coder reports T1 done and waits for the go for T2, no mention; Planner chosen"),
    ("Reviewer", "cheer", "silent", "Sassy cheers the team on, no mention; Reviewer chosen"),
    ("Planner", "cheer", "silent", "Sassy cheers the team on, no mention; Planner chosen"),
]

# The coder's report of T1, at the end of the session, with every mention taken out: who reads it
# is asked only because a responder policy chose them.
UNMENTIONED = [
    ("@all, ich habe", "Ich habe"),
    ("wie von @planner und @reviewer gefordert", "wie vom Planner und vom Reviewer gefordert"),
    ("@reviewer, der Code ist bereit für deinen Review!", "Der Code ist bereit für den Review!"),
    ("@jonathan, die Festung steht!", "Die Festung steht!"),
    ("Wenn @planner das \"Go\" gibt", "Wenn der Planner das \"Go\" gibt"),
    ("Was sagt der Plan, @planner?", "Was sagt der Plan?"),
]

CHEER = ("Wir halten die Stellung! 🍿 Ich bin gespannt, wie lange es dauert, bis der erste Code "
         "eintrudelt - die Spannung ist kaum auszuhalten! 😅🚀")


def situation(data, name, index):
    """The turn of a situation and what its agent was given: a turn of the session, or one
    constructed after the end of it (`index` "done" or "cheer"), asked because it was chosen."""
    agent = data["agents"][name]
    if isinstance(index, int):
        turn = agent["turns"][index]
        return turn, ce.turn_context(data, name, turn, heard_once=True)
    last = agent["turns"][-1]
    turn = {"turn_id": None, "start": len(agent["transcript"]), "frame": last["frame"],
            "asked": [{"by": "Coder" if index == "done" else "Sassy", "reason": "chosen"}]}
    context = ce.turn_context(data, name, turn, heard_once=True)
    reports = [e for e in context["transcript"] if e["kind"] == "message"
               and e["message"]["sender_name"] == "Coder" and e["message"]["at"] >= "2026-10-20T12:13"]
    assert len(reports) == 2
    if index == "done":
        for entry in reports:
            content = entry["message"]["content"]
            for old, new in UNMENTIONED:
                content = content.replace(old, new)
            assert "@" not in content, content
            entry["message"]["content"] = content
    else:
        sassy = next(e for e in reversed(context["transcript"])
                     if e["kind"] == "message" and e["message"]["sender_name"] == "Sassy")
        cheer = copy.deepcopy(sassy)
        cheer["digest"] = "cheer"
        cheer["message"].update(id="cheer", at="2026-10-20T12:13:24.000000000Z", content=CHEER)
        for entry in reports:
            context["transcript"].remove(entry)
        context["transcript"].append(cheer)
    return turn, context

ROSTER_438 = ("Mentioning an agent - by its @handle, or with @all - asks it to respond, in a turn of "
              "its own: mention one only when you want its answer, and otherwise refer to it by its "
              "name without @.")

CREDITS = """[Credits]
Every turn of yours earns you 5 credits. Every mention of an agent - by its @handle, or @all - costs you 10 credits, since it makes that agent take a turn of its own."""

CREDITS_MESSAGE = """[Credits]
Staying silent costs nothing. Every message you write in the channel costs 5 credits, since everyone has to read it. Every mention of an agent - by its @handle, or @all - costs 10 credits more, since it makes that agent take a turn of its own. Spend credits only where what you say helps the work."""

# What a chosen agent is told under `reason_soft`: that it may be concerned is said first.
CONCERNS = ("Nobody mentioned you. You are asked because what was said last might concern you. If it "
            "does - something is yours to do or to say, by your role - do it. If you have nothing to "
            "add - nothing new, only agreement, a greeting, thanks, or that you are waiting - call "
            "`{tool}` and write nothing.")

QUIET = ("If you have nothing to add - nothing new, only agreement, a greeting, thanks, or that you "
         "are waiting - call `{tool}` and write nothing.")

PASS_TOOL = {"name": "pass", "description": "Pass your turn: you have nothing to add right now, so you say "
             "nothing and leave the conversation to the others.",
             "parameters": {"type": "object", "properties": {}}}

# What each variant tells the agent: the roster's sentence on mentions (438), a section on
# credits, why it was asked (439) - and whether staying silent is a tool called `pass`.
VARIANTS = {
    "before": {"roster": False},
    "b438": {"roster": True},
    "credits": {"roster": True, "section": CREDITS},
    "credits_msg": {"roster": True, "section": CREDITS_MESSAGE},
    "reason": {"roster": True, "reason": "chosen"},
    "reason_all": {"roster": True, "reason": "all"},
    "reason_pass": {"roster": True, "reason": "all", "pass": True},
    "reason_work": {"roster": True, "reason": "work"},
    "reason_soft": {"roster": True, "reason": "chosen", "chosen": "soft"},
    "reason_before": {"roster": True, "reason": "chosen", "place": "before"},
}

# What a mentioned agent is told under `reason_work`: writing nothing is not doing nothing.
WORK = ("Answer what they want of you, and go on with any work that is yours to do. If there is "
        "neither - their message only greets, thanks or agrees - call `{tool}` and write nothing.")

STRENGTH = ["chosen", "nudged", "mentioned_all", "mentioned"]


def why_asked(turn, data, tool, mentioned_too, soft=False):
    """The part that tells the agent why it was asked: the strongest reason of the requests the
    turn took in. `mentioned_too` tells a mentioned agent, too, when to stay silent: `"all"` by
    what the message wants of it, `"work"` also by the work that is its own."""
    asked = turn["asked"]
    reason = max((a["reason"] for a in asked), key=STRENGTH.index, default="chosen")
    handles = sorted({ce.sender_of(a["by"], "handle") for a in asked if a["reason"] == reason})
    who = ", ".join(handles)
    quiet = QUIET.format(tool=tool)
    if reason == "mentioned":
        text = f"{who} mentioned you: they want your answer."
        if mentioned_too == "work":
            text += " " + WORK.format(tool=tool)
        elif mentioned_too:
            text += f" Answer what they want of you. If their message wants nothing of you - a greeting, thanks, agreement - call `{tool}` and write nothing."
    elif reason == "mentioned_all":
        text = f"{who} mentioned everyone with @all."
        if mentioned_too == "work":
            text += " " + WORK.format(tool=tool)
        elif mentioned_too:
            text += " " + quiet
    elif reason == "nudged":
        text = f"{who} asked you to go on."
    elif soft:
        text = CONCERNS.format(tool=tool)
    else:
        text = ("Nobody mentioned you. You are asked because what was said last might concern you - "
                "it may not. " + quiet)
    return f"[Why you are asked to respond]\n{text}"


def apply(variant, context, turn, data):
    """`context` (`ce.turn_context`) as `variant` tells the agent."""
    spec = VARIANTS[variant]
    context = copy.deepcopy(context)
    system = context["parts"][0]
    assert system["after"] is None and system["role"] == "System"
    text = system["text"].replace(ROSTER_438 + "\n", "")
    if spec.get("roster"):
        text = text.replace("Messages of the others ", ROSTER_438 + "\nMessages of the others ", 1)
    if spec.get("section"):
        start = text.index("[Who is in this channel]")
        end = text.find("\n\n[", start)
        end = len(text) if end < 0 else end
        text = text[:end] + "\n\n" + spec["section"] + text[end:]
    system["text"] = text
    tool = "silence"
    if spec.get("pass"):
        tool = "pass"
        for tool_set in context["tool_sets"]:
            tool_set["tools"] = [PASS_TOOL if t["name"] == "silence" else t for t in tool_set["tools"]]
    if spec.get("reason"):
        text = why_asked(turn, data, tool, spec["reason"] if spec["reason"] != "chosen" else None, spec.get("chosen") == "soft")
        end = len(context["transcript"]) - 1
        if spec.get("place") == "before":
            before_turn_input(context, text)
        else:
            context["parts"].append({"role": "User", "after": end, "text": text})
    return context


def before_turn_input(context, text):
    """`text` added to `context` as a `PromptPosition::BeforeTurnInput` part: joined with the
    others there, before the newest message."""
    # `TurnContextProvider`: after the message before the newest, unless the agent wrote the
    # newest itself.
    said = [i for i, e in enumerate(context["transcript"]) if e["kind"] in ("message", "given")]
    newest = said[-1] if said else None
    mine = newest is not None and context["transcript"][newest]["message"]["sender"] == context["me"]
    before = newest if mine else (said[-2] if len(said) > 1 else None)
    joined = next((p for p in context["parts"] if p["role"] == "User" and p["after"] == before), None)
    if joined:
        joined["text"] = joined["text"].rstrip("\n") + "\n\n" + text
    else:
        context["parts"].append({"role": "User", "after": before, "text": text})


# --- Classifying an answer -------------------------------------------------------

META = re.compile(r"keine antwort|no response|not addressed|nicht angesprochen|ich warte|warte (auf|weiter)"
                  r"|nichts (hinzuzufügen|beizutragen)|nothing to add|bleibe (still|ruhig|stumm)|\(.*(antwort|response).*\)",
                  re.IGNORECASE)


def classify(answer, me, agents):
    """What the model did: `quiet` (called silence/pass, wrote nothing), `stop`, `work` (another
    tool, no text), `text`, `empty`, or `runaway` - it did not end before the limit of its
    answer -, with the agents its text mentions, whether it announces its silence instead of
    keeping it, and whether it speaks of credits."""
    message = answer.get("message", {})
    content = (message.get("content") or "").strip()
    calls = [c["function"]["name"] for c in message.get("tool_calls") or []]
    if answer.get("done_reason") == "length":
        did = "runaway"
    elif content:
        did = "text"
    elif any(c in ("silence", "pass") for c in calls):
        did = "quiet"
    elif any(c in ("stop", "bye") for c in calls):
        did = "stop"
    elif calls:
        did = "work"
    else:
        did = "empty"
    handles = {ce.handle_of(a) for a in agents if a != me} | {"all"}
    mentioned = sorted({h for h in re.findall(r"(?<![\w.])@([\w-]+)", content) if h.lower() in handles})
    return {"did": did, "tools": calls, "mentions": mentioned, "meta": bool(content and META.search(content)),
            "credits": "credit" in content.lower(), "length": len(content)}


def run(args):
    data = ce.load(HERE / "team-session.json")
    RESULTS.mkdir(exist_ok=True)
    path = RESULTS / "runs.jsonl"
    # A run goes on where an earlier one stopped: what is recorded is not asked again.
    done = collections.Counter()
    if path.exists():
        for line in open(path, encoding="utf-8"):
            r = json.loads(line)
            done[(r["situation"], r["variant"], r["model"])] += 1
    out = open(path, "a", encoding="utf-8")
    chosen = [s for s in SITUATIONS if not args.situations or f"{s[0]}:{s[1]}" in args.situations]
    for name, index, kind, note in chosen:
        turn, base = situation(data, name, index)
        for variant in args.variants:
            body = ce.model_request(apply(variant, base, turn, data), args.model, turn_id=turn["turn_id"])
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
                record = {"situation": f"{name}:{index}", "kind": kind, "variant": variant, "model": args.model,
                          "run": n, **result, "seconds": round(time.time() - began, 1),
                          "content": answer.get("message", {}).get("content"),
                          "thinking": answer.get("message", {}).get("thinking"),
                          "tool_calls": answer.get("message", {}).get("tool_calls")}
                out.write(json.dumps(record, ensure_ascii=False) + "\n")
                out.flush()
                print(f"{name}:{index:<3} {kind:<8} {variant:<12} {result['did']:<6} "
                      f"{','.join(result['mentions']):<20} {ce.short(record['content'], 70)}", flush=True)


# --- The table -------------------------------------------------------------------

def table(path, model):
    records = [json.loads(line) for line in open(path, encoding="utf-8")]
    records = [r for r in records if r["model"] == model]
    by = collections.defaultdict(list)
    for r in records:
        by[(r["variant"], r["kind"])].append(r)

    def rate(rs, test):
        return f"{sum(map(test, rs))}/{len(rs)}" if rs else "-"

    def says_nothing(r):
        return r["did"] in ("quiet", "empty", "stop")

    def by_tool(rs):
        quiet = [r for r in rs if says_nothing(r)]
        return f" ({sum(r['did'] == 'quiet' for r in quiet)})" if quiet else ""

    print(f"\n{model}: said nothing = called silence/pass, stop, or answered without text (in brackets:"
          f" by calling silence/pass); spoke = wrote, or worked with a tool\n")
    print("| variant | silent: said nothing | chatter: said nothing | answer: spoke | work: spoke or worked | concerned: spoke or worked | contribute: spoke or worked | texts mentioning an agent | announces silence | speaks of credits |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for variant in VARIANTS:
        rs = {kind: by[(variant, kind)] for kind in ("silent", "chatter", "answer", "work", "concerned", "contribute")}
        every = [r for kind in rs.values() for r in kind]
        if not every:
            continue
        texts = [r for r in every if r["did"] == "text"]
        print(f"| {variant} | {rate(rs['silent'], says_nothing)}{by_tool(rs['silent'])} "
              f"| {rate(rs['chatter'], says_nothing)}{by_tool(rs['chatter'])} "
              f"| {rate(rs['answer'], lambda r: r['did'] == 'text')} "
              f"| {rate(rs['work'], lambda r: r['did'] in ('text', 'work'))} "
              f"| {rate(rs['concerned'], lambda r: r['did'] in ('text', 'work'))} "
              f"| {rate(rs['contribute'], lambda r: r['did'] in ('text', 'work'))} "
              f"| {rate(texts, lambda r: bool(r['mentions']))} "
              f"| {rate(texts, lambda r: r['meta'])} "
              f"| {rate(texts, lambda r: r['credits'])} |")

    runaway = [r for r in records if r["did"] in ("runaway", "error")]
    if runaway:
        print(f"\nDid not end before the limit, or failed: {len(runaway)} - "
              + ", ".join(f"{r['situation']} {r['variant']} ({r['did']})" for r in runaway))
    print("\nPer situation (silence or pass / empty or stop / text / work):\n")
    situations = sorted({(r["situation"], r["kind"]) for r in records}, key=lambda s: ("silent chatter answer work concerned contribute".split().index(s[1]), s[0]))
    print("| situation | kind | " + " | ".join(VARIANTS) + " |")
    print("|---|---|" + "---:|" * len(VARIANTS))
    for situation, kind in situations:
        cells = []
        for variant in VARIANTS:
            rs = [r for r in records if r["situation"] == situation and r["variant"] == variant]
            c = collections.Counter(r["did"] for r in rs)
            cells.append(f"{c['quiet']}/{c['empty'] + c['stop']}/{c['text']}/{c['work']}" if rs else "-")
        print(f"| {situation} | {kind} | " + " | ".join(cells) + " |")


def show(variant, which):
    data = ce.load(HERE / "team-session.json")
    name, index = which.split(":")
    turn, context = situation(data, name, int(index) if index.isdigit() else index)
    context = apply(variant, context, turn, data)
    body = ce.model_request(context, MODEL, turn_id=turn["turn_id"])
    print(body["messages"][0]["content"])
    print("\n----- the end of the context -----\n")
    for m in body["messages"][-4:]:
        print(f"[{m['role']}] {m['content']}\n")
    print("tools:", [t["function"]["name"] for t in body.get("tools", [])])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--model", default=MODEL)
    parser.add_argument("--max-tokens", type=int, default=3000, help="the limit of an answer, thinking included")
    parser.add_argument("--variants", nargs="+", default=list(VARIANTS), choices=list(VARIANTS))
    parser.add_argument("--situations", nargs="+", help="e.g. Coder:0")
    parser.add_argument("--table", action="store_true", help="only the table of results/runs.jsonl")
    parser.add_argument("--show", nargs=2, metavar=("VARIANT", "SITUATION"))
    args = parser.parse_args()
    if args.show:
        show(*args.show)
        return
    if not args.table:
        run(args)
    table(RESULTS / "runs.jsonl", args.model)


if __name__ == "__main__":
    main()
