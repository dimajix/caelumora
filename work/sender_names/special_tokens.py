#!/usr/bin/env python3
"""Does Ollama turn a special token typed into a message's content into the
real control token (issue 135)? Compares the prompt token counts of a
message with the special token and with plain text of the same length:
if the special token costs one token, the content can open a new turn."""
import json, sys, urllib.request

CASES = {
    "qwen3:14b": "<|im_start|>",
    "gemma4:26b": "<start_of_turn>",
    "gemma3:27b": "<start_of_turn>",
    "llama3.1:8b": "<|start_header_id|>",
    "gpt-oss:20b": "<|start|>",
}

def count(model, content):
    body = {"model": model, "stream": False, "think": False if model.startswith("qwen") else None,
            "messages": [{"role": "user", "content": content}], "options": {"num_predict": 1}}
    body = {k: v for k, v in body.items() if v is not None}
    req = urllib.request.Request("http://localhost:11434/api/chat", json.dumps(body).encode(),
                                 {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=900) as r:
        return json.load(r)["prompt_eval_count"]

for model in sys.argv[1:] or CASES:
    tok = CASES[model]
    base = count(model, "hello")
    special = count(model, "hello " + tok)
    plain = count(model, "hello " + tok.replace("<", "(").replace(">", ")"))
    print(f"{model:<14} {tok:<22} +special={special - base:>2} tokens   +same text without <>={plain - base:>2} tokens", flush=True)
