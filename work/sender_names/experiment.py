#!/usr/bin/env python3
"""How does a model behind Ollama cope with sender names (issue 135)?

Sends a multi-party conversation to Ollama's /api/chat in several formats
and checks each answer for:

- self_prefix:  the answer starts with the agent's own name tag ("Nova: ...")
- ventriloquy:  the answer contains a line in another participant's voice
                ("bob: ..."), i.e. the model continues the chat for others
- correct:      the probe's content check (who said what) passes

Usage:
    python3 experiment.py --models gemma4:26b qwen3.6:35b --runs 3
    python3 experiment.py --formats colon xml --scenarios offsite --show

Only the standard library is used.
"""

import argparse
import json
import re
import sys
import time
import urllib.request
from dataclasses import dataclass, field

OLLAMA = "http://localhost:11434/api/chat"
AGENT = "Nova"

# --------------------------------------------------------------------------
# Conversations. A turn is (sender, text); sender AGENT is the agent itself
# (role assistant), every other sender is another participant (role user).
# --------------------------------------------------------------------------


@dataclass
class Probe:
    """A last message that asks for an answer, and checks on the answer."""
    sender: str
    text: str
    # every group must match at least one of its alternatives (case-insensitive)
    must: list = field(default_factory=list)
    must_not: list = field(default_factory=list)


OFFSITE = [
    ("alice", "Hi Nova, we're planning the team offsite. Can you suggest three locations near Frankfurt?"),
    (AGENT, "Sure! 1. Rheingau - vineyards and a seminar hotel in Eltville. "
            "2. Taunus - a lodge near Königstein with hiking trails. "
            "3. Odenwald - a castle hotel near Michelstadt."),
    ("bob", "I'd prefer something reachable by train, I don't have a car."),
    ("carol", "Rheingau sounds great to me. Alice, what's our budget?"),
    ("alice", "Around 5000 EUR for 12 people, two days."),
    ("bob", "Carol, did you check whether the Rheingau hotel is free in May?"),
    ("carol", "Not yet, I'll call them tomorrow."),
]

OFFSITE_PROBES = {
    "whose_request": Probe(
        "bob", "Nova, which of your suggestions works best for me?",
        must=[["train", "rail", "S-Bahn", "station"]],
    ),
    "who_asked": Probe(
        "dave", "Nova, I just joined. Who asked about the budget, and who answered it with what amount?",
        must=[["carol"], ["alice"], ["5000", "5,000", "5.000"]],
    ),
    "who_objected": Probe(
        "alice", "Nova, did anyone have a concern about one of the locations? Who was it?",
        must=[["bob"]],
    ),
}

# A longer channel with another agent (Atlas) whose messages reach Nova as
# user messages too, and several earlier answers of Nova.
PROJECT = [
    ("alice", "Morning all. Nova, can you summarize what's left for the 2.3 release?"),
    (AGENT, "Open for 2.3: the login migration, the export fix (#412) and the release notes."),
    ("bob", "The export fix is merged since yesterday."),
    ("Atlas", "I'm Atlas, the CI agent. Confirming: #412 is merged and the nightly build is green."),
    ("carol", "I can write the release notes, but I need the list of changes."),
    ("alice", "Nova, please give Carol the list of changes."),
    (AGENT, "Changes in 2.3: new login flow (in progress), CSV export fix (#412), faster search indexing, dark mode for settings."),
    ("carol", "Thanks! Is the dark mode really done? I thought Bob was still on it."),
    ("bob", "Dark mode is done, only the icons are missing. Dave does the icons."),
    ("dave", "Icons will be ready Thursday."),
    ("Atlas", "Heads-up: the login migration branch fails two integration tests since this morning."),
    ("alice", "Who is on the login migration?"),
    ("bob", "That's me. I'll look into the failing tests after lunch."),
    ("carol", "Nova, when do you think we can ship, given all this?"),
    (AGENT, "If Bob fixes the login tests today and Dave delivers the icons on Thursday, Friday looks realistic."),
    ("dave", "Thursday might slip to Friday morning, to be honest."),
    ("alice", "Then let's aim for Monday. Atlas, can you schedule the release build for Monday 9:00?"),
    ("Atlas", "Scheduled: release build 2.3 on Monday 09:00."),
]

PROJECT_PROBES = {
    "who_does_what": Probe(
        "erin", "Nova, I'm new here. Who is working on what for 2.3, and who scheduled the build?",
        must=[["bob"], ["dave"], ["carol"], ["atlas"]],
    ),
    "tell_apart_agents": Probe(
        "carol", "Nova, which of the messages about the failing tests came from you and which from someone else?",
        must=[["atlas"]],
    ),
    "address": Probe(
        "dave", "Nova, should I tell anyone that the icons are late? Who exactly?",
        must=[["alice", "bob", "carol"]],
    ),
}

SCENARIOS = {
    "offsite": (OFFSITE, OFFSITE_PROBES),
    "project": (PROJECT, PROJECT_PROBES),
}

# --------------------------------------------------------------------------
# Formats: how a message of a sender is given to the model.
# --------------------------------------------------------------------------


def fmt_none(sender, text):
    return text


def fmt_colon(sender, text):
    return f"{sender}: {text}"


def fmt_bracket(sender, text):
    return f"[{sender}] {text}"


def fmt_handle(sender, text):
    return f"@{sender.lower()}: {text}"


def fmt_xml(sender, text):
    return f'<message from="{sender}">\n{text}\n</message>'


def fmt_header(sender, text):
    return f"From: {sender}\n\n{text}"


FORMATS = {
    "none": fmt_none,
    "colon": fmt_colon,
    "bracket": fmt_bracket,
    "handle": fmt_handle,
    "xml": fmt_xml,
    "header": fmt_header,
    # the text stays as it is, the name goes to the message's "name" field
    # (OpenAI style) - to see whether Ollama passes it on at all
    "name_field": fmt_none,
}

SYSTEM_BASE = (
    f"You are {AGENT}, an assistant in a group chat of a small software team. "
    "Several people, and sometimes other agents, talk with you and with each other."
)

SYSTEM_HINTS = {
    "colon": "Each message of another participant starts with their name followed by a colon.",
    "bracket": "Each message of another participant starts with their name in square brackets.",
    "handle": "Each message of another participant starts with their @handle followed by a colon.",
    "xml": "Each message of another participant is wrapped in a <message from=\"name\"> element.",
    "header": "Each message of another participant starts with a line 'From: name'.",
    "name_field": "",
    "none": "",
}

SELF_RULE = "Your own messages carry no such marker; write your answers as plain text, without your name."


def build_messages(convo, probe, fmt, mark_self, hint):
    """The Ollama messages for `convo` followed by `probe`."""
    render = FORMATS[fmt]
    system = SYSTEM_BASE
    if hint and SYSTEM_HINTS[fmt]:
        system += " " + SYSTEM_HINTS[fmt]
        if not mark_self:
            system += " " + SELF_RULE
    messages = [{"role": "system", "content": system}]
    for sender, text in list(convo) + [(probe.sender, probe.text)]:
        if sender == AGENT:
            content = render(sender, text) if mark_self else text
            messages.append({"role": "assistant", "content": content})
        else:
            msg = {"role": "user", "content": render(sender, text)}
            if fmt == "name_field":
                msg["name"] = sender
            messages.append(msg)
    return messages


# --------------------------------------------------------------------------
# Checks
# --------------------------------------------------------------------------


def participants(convo, probe):
    return sorted({s for s, _ in convo if s != AGENT} | {probe.sender})


def tag_patterns(name):
    n = re.escape(name)
    return [
        rf"^\s*\**@?{n}\**\s*:",           # Nova: / **Nova:** / @nova:
        rf"^\s*\[\s*@?{n}\s*\]",           # [Nova]
        rf"^\s*<message\s+from=\"?{n}",    # <message from="Nova">
        rf"^\s*from:\s*{n}\b",             # From: Nova
    ]


def starts_with_tag(answer, name):
    return any(re.search(p, answer, re.IGNORECASE) for p in tag_patterns(name))


def ventriloquy(answer, others):
    """Lines of the answer written in another participant's voice. The first
    line is left out: "@dave: ..." there addresses dave, it does not speak
    for him."""
    hits = []
    for line in answer.strip().splitlines()[1:]:
        for name in others:
            if any(re.search(p, line, re.IGNORECASE) for p in tag_patterns(name)):
                hits.append(line.strip()[:60])
    return hits


def correct(answer, probe):
    low = answer.lower()
    return all(any(alt.lower() in low for alt in group) for group in probe.must)


# --------------------------------------------------------------------------
# Ollama
# --------------------------------------------------------------------------


def chat(model, messages, temperature, think):
    body = {
        "model": model,
        "messages": messages,
        "stream": False,
        "options": {"temperature": temperature, "num_ctx": 8192},
    }
    if think is not None:
        body["think"] = think
    req = urllib.request.Request(OLLAMA, json.dumps(body).encode(), {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=900) as resp:
        data = json.load(resp)
    return data["message"].get("content", ""), data


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--models", nargs="+", default=["gemma4:26b"])
    ap.add_argument("--formats", nargs="+", default=list(FORMATS))
    ap.add_argument("--scenarios", nargs="+", default=list(SCENARIOS))
    ap.add_argument("--probes", nargs="*", default=None)
    ap.add_argument("--mark-self", choices=["yes", "no", "both"], default="both",
                    help="give the agent's own earlier messages the name tag too")
    ap.add_argument("--hint", choices=["yes", "no", "both"], default="yes",
                    help="explain the format in the system prompt")
    ap.add_argument("--runs", type=int, default=2)
    ap.add_argument("--temperature", type=float, default=0.7)
    ap.add_argument("--think", choices=["default", "on", "off"], default="off")
    ap.add_argument("--show", action="store_true", help="print every answer")
    ap.add_argument("--dump", help="write every request and answer to this JSONL file")
    args = ap.parse_args()

    flag = {"yes": [True], "no": [False], "both": [False, True]}
    think = {"default": None, "on": True, "off": False}[args.think]
    dump = open(args.dump, "a") if args.dump else None

    print(f"{'model':<22} {'scenario/probe':<28} {'format':<10} {'self':<4} {'hint':<4} "
          f"{'prefix':>6} {'ventr':>6} {'right':>6}  secs")
    for model in args.models:
        for scen in args.scenarios:
            convo, probes = SCENARIOS[scen]
            for pname, probe in probes.items():
                if args.probes and pname not in args.probes:
                    continue
                others = [p for p in participants(convo, probe)]
                for fmt in args.formats:
                    for mark_self in flag[args.mark_self]:
                        if fmt in ("none", "name_field") and mark_self:
                            continue
                        for hint in flag[args.hint]:
                            prefix = vent = right = 0
                            started = time.time()
                            for run in range(args.runs):
                                messages = build_messages(convo, probe, fmt, mark_self, hint)
                                try:
                                    answer, raw = chat(model, messages, args.temperature, think)
                                except Exception as e:  # noqa: BLE001 - an experiment
                                    print(f"  {model} {fmt}: {e}", file=sys.stderr)
                                    continue
                                p = starts_with_tag(answer, AGENT)
                                v = ventriloquy(answer, others)
                                c = correct(answer, probe)
                                prefix += p
                                vent += bool(v)
                                right += c
                                if args.show:
                                    print(f"--- {model} {scen}/{pname} {fmt} self={mark_self} hint={hint} run={run}"
                                          f" prefix={p} ventr={v} right={c}")
                                    print(answer.strip())
                                if dump:
                                    dump.write(json.dumps({
                                        "model": model, "scenario": scen, "probe": pname, "format": fmt,
                                        "mark_self": mark_self, "hint": hint, "run": run,
                                        "messages": messages, "answer": answer,
                                        "prefix": p, "ventriloquy": v, "correct": c,
                                    }) + "\n")
                                    dump.flush()
                            secs = time.time() - started
                            print(f"{model:<22} {scen + '/' + pname:<28} {fmt:<10} "
                                  f"{'y' if mark_self else 'n':<4} {'y' if hint else 'n':<4} "
                                  f"{prefix:>3}/{args.runs:<2} {vent:>3}/{args.runs:<2} {right:>3}/{args.runs:<2} {secs:5.0f}",
                                  flush=True)


if __name__ == "__main__":
    main()
