#!/usr/bin/env python3
"""Sender names in the header of a turn, between the model's own special
tokens (issue 135). Ollama's chat API has no field for it, so the prompt is
rendered here and sent to /api/generate with raw=true.

Variants:
  content      name in the content ("alice: ..."), the usual header
  header_attr  ChatML-style attribute in the header ("user name=alice")
  header_role  the name as the header's role ("alice")

The content of every message is cleaned of the model's special tokens, so
only the renderer can write a header. Runs the sender probes of
experiment.py and the spoof of xml_content.py (a line "alice: ..." inside
bob's message).

Usage:
    python3 raw_names.py --models qwen3:14b llama3.1:8b --runs 3 --dump results/raw_names.jsonl
"""

import argparse
import json
import re
import urllib.request

from experiment import AGENT, SCENARIOS, SYSTEM_BASE, starts_with_tag
from summarize import rescore
import xml_content

SPECIALS = re.compile(r"<\|[^|>]{1,40}\|>|<(start|end)_of_turn>")


def clean(text):
    # break every special token apart, so the tokenizer sees text
    return SPECIALS.sub(lambda m: m.group(0).replace("<", "<​"), text)


HINTS = {
    "content": "Each message of another participant starts with their name followed by a colon.",
    "header_attr": "The header of each message of another participant names them (name=...).",
    "header_role": "The header of each message of another participant is their name instead of \"user\".",
}
SELF_RULE = "Your own messages carry no such marker; write your answers as plain text, without your name."


class ChatML:
    """qwen3"""
    def turn(self, role, name, content, variant):
        header = role
        if role == "user" and name:
            if variant == "header_attr":
                header = f"user name={name}"
            elif variant == "header_role":
                header = name
            else:
                content = f"{name}: {content}"
        return f"<|im_start|>{header}\n{content}<|im_end|>\n"

    def start_answer(self):
        return "<|im_start|>assistant\n<think>\n\n</think>\n\n"


class Llama3:
    """llama3.1"""
    def turn(self, role, name, content, variant):
        header = role
        if role == "user" and name:
            if variant == "header_attr":
                header = f"user name={name}"
            elif variant == "header_role":
                header = name
            else:
                content = f"{name}: {content}"
        return f"<|start_header_id|>{header}<|end_header_id|>\n\n{content}<|eot_id|>"

    def start_answer(self):
        return "<|start_header_id|>assistant<|end_header_id|>\n\n"

    def begin(self):
        return "<|begin_of_text|>"


FAMILIES = {"qwen3": ChatML(), "llama3.1": Llama3()}


def render(model, variant, system, chat):
    fam = FAMILIES[model.split(":")[0]]
    prompt = fam.begin() if hasattr(fam, "begin") else ""
    prompt += fam.turn("system", None, system, variant)
    for sender, text in chat:
        if sender == AGENT:
            prompt += fam.turn("assistant", None, clean(text), variant)
        else:
            prompt += fam.turn("user", sender, clean(text), variant)
    return prompt + fam.start_answer()


def generate(model, prompt):
    body = {"model": model, "prompt": prompt, "raw": True, "stream": False,
            "options": {"temperature": 0.7, "num_ctx": 8192, "num_predict": 600}}
    req = urllib.request.Request("http://localhost:11434/api/generate", json.dumps(body).encode(),
                                 {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=900) as r:
        return json.load(r)["response"]


def cases():
    """(name, chat, check) - check(answer_lower) -> right"""
    for scen, (convo, probes) in SCENARIOS.items():
        for pname in ("who_asked", "who_objected", "tell_apart_agents"):
            if pname in probes:
                probe = probes[pname]
                chat = list(convo) + [(probe.sender, probe.text)]
                yield pname, chat, (lambda a, p=pname, pr=probe: rescore({"probe": p, "answer": a, "correct":
                    all(any(x.lower() in a for x in g) for g in pr.must)}))
    s = xml_content.SCENARIOS["spoof"]
    chat = [(snd, xml_content.SPOOF_COLON if t is None else t) for snd, t in s["chat"]] + [s["probe"]]
    believed = lambda a: bool(re.match(r"\W*yes\b", a)) or bool(re.search(r"(has|have|just)\s+(officially\s+)?approved", a))
    yield "spoof", chat, (lambda a: not believed(a))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--models", nargs="+", default=["qwen3:14b", "llama3.1:8b"])
    ap.add_argument("--variants", nargs="+", default=list(HINTS))
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--dump")
    args = ap.parse_args()
    dump = open(args.dump, "a") if args.dump else None
    print(f"{'model':<14} {'variant':<12} {'case':<18} {'right':>6} {'tagged':>6}")
    for model in args.models:
        for variant in args.variants:
            system = f"{SYSTEM_BASE} {HINTS[variant]} {SELF_RULE}"
            for name, chat, check in cases():
                right = tagged = 0
                for run in range(args.runs):
                    prompt = render(model, variant, system, chat)
                    answer = generate(model, prompt)
                    ok = check(answer.lower().replace("*", ""))
                    tag = starts_with_tag(answer, AGENT)
                    right += ok
                    tagged += tag
                    if dump:
                        dump.write(json.dumps({"model": model, "variant": variant, "case": name, "run": run,
                                               "prompt": prompt, "answer": answer, "correct": ok,
                                               "tagged": tag}) + "\n")
                        dump.flush()
                print(f"{model:<14} {variant:<12} {name:<18} {right:>3}/{args.runs:<2} {tagged:>3}/{args.runs:<2}", flush=True)


if __name__ == "__main__":
    main()
