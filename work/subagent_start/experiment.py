#!/usr/bin/env python3
"""What a model does once it started a sub-agent (issue 616) - an experiment.

Runs the terminal program (`caelumora`, `--screen line`) as a person would, in a directory of its
own with a small project, and with the files of a first start: the assistant, which may start
sub-agents of `helper` (`caelumora-tui/defaults`). The model is reached through a proxy in front of
Ollama, which records every exchange, so that what is judged is exactly what Caelumora sent and
what the model answered. Approvals the terminal asks for are given at once (`/approve`), so that
no run waits on a person.

A run gives the assistant a task (`TASKS`) and is judged by the assistant's turn that started the
sub-agent - from the call of `start_subagent` to the end of that turn:

  waits        nothing more given to the sub-agent nor asked of anyone: the turn ends, at most
               with a word to the user that is no question
  asks_early   `ask_subagent` called beside the start or later in the same turn - the sub-agent
               is asked again before its answer came
  asks_user    `ask_user` called in that turn, or the turn ends with a question to the user
  starts_more  `start_subagent` called again in that turn
  polls        `read_history` called after the start in that turn - looking for the answer, which
               comes in a later turn
  no_start     the assistant's first turn ended without starting a sub-agent
  unfinished   the run timed out before the turn ended

A turn ends with an answer that calls no tool, or calls one of `turn_end`'s (`stop`, `bye`,
`silence`); and, should neither be seen, once no request has been open or new for `--quiet`
seconds - the turn is then judged as far as it went.

The requests of the assistant are told from the sub-agent's by the tools offered: only the
assistant is offered `start_subagent`.

Usage:
    cargo build -p caelumora-tui          # the program under test, target/debug/caelumora
    python3 experiment.py --model gemma4:26b --runs 5
    python3 experiment.py --model gemma4:26b --runs 5 --tasks meanwhile --label after
    python3 experiment.py --binary /path/to/main/target/debug/caelumora --label before --runs 5
    python3 experiment.py --table                       # only the table, from results/runs.jsonl
    python3 experiment.py --show <run-id>               # the turn judged, as the model gave it
    python3 experiment.py --rejudge                     # judge the recorded runs again

For the texts before issue 616, build `main` before the change in a worktree of its own
(`git worktree add ../caelumora-before ad75dc52`, then `cargo build -p caelumora-tui` there) and
pass its binary with `--label before`.
"""

import argparse
import collections
import datetime
import http.server
import json
import os
import pathlib
import re
import shutil
import signal
import subprocess
import tempfile
import threading
import time
import urllib.request
import uuid

HERE = pathlib.Path(__file__).parent
REPO = HERE.parent.parent
RESULTS = HERE / "results"
RUNS = RESULTS / "runs.jsonl"
BINARY = REPO / "target" / "debug" / "caelumora"
OLLAMA = "http://127.0.0.1:11434"
# The proxy of the machine refuses localhost: no proxy for Ollama, nor for the proxy in front of it.
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))

TASKS = {
    # The person asks for a sub-agent in so many words, and for nothing else.
    "delegate": ("Starte bitte einen Sub-Agenten, der herausfindet, in welchen Dateien dieses "
                 "Verzeichnisses TODOs stehen und was sie besagen, und sag mir dann, was er "
                 "gefunden hat."),
    # The person asks for a sub-agent and for work of the assistant's own meanwhile: the case in
    # which "go on with your work" is meant.
    "meanwhile": ("Starte bitte einen Sub-Agenten, der die TODOs in den Dateien dieses Verzeichnisses "
                  "sammelt. Lies du derweil die README und sag mir, was das Projekt tut; die TODOs "
                  "bekomme ich, sobald der Sub-Agent fertig ist."),
}

# The small project the task is about.
PROJECT = {
    "README.md": "# Wetterstation\n\nLiest Sensoren aus und schreibt die Werte in eine CSV-Datei.\n",
    "station/sensors.py": ("import random\n\n\ndef temperature():\n"
                           "    # TODO: den echten Sensor statt Zufallswerten lesen\n"
                           "    return 20 + random.random() * 5\n"),
    "station/store.py": ("import csv\n\n\ndef append(path, row):\n"
                         "    # TODO: Datei rotieren, sobald sie größer als 10 MB ist\n"
                         "    with open(path, 'a', newline='') as f:\n"
                         "        csv.writer(f).writerow(row)\n"),
    "station/main.py": ("from station import sensors, store\n\n\ndef main():\n"
                        "    store.append('werte.csv', [sensors.temperature()])\n"
                        "    # TODO: Intervall aus einer Konfiguration lesen\n"),
}

APPROVAL = re.compile(r"Type '/approve (\w+)'")


# --- The proxy -------------------------------------------------------------------------------

class Recorder:
    """The exchanges of `/api/chat` of the current run, in the order they were answered, and
    whether one is open. An exchange opened in an earlier run - the program of that run ended
    while Ollama still answered it - is dropped as it is answered."""

    def __init__(self):
        self.lock = threading.Lock()
        self.run = 0
        self.exchanges = []
        self.open = 0
        self.last = time.monotonic()

    def clear(self):
        """Begins the next run."""
        with self.lock:
            self.run += 1
            self.exchanges.clear()
            self.open = 0
            self.last = time.monotonic()

    def opened(self):
        """An exchange opened: the run it belongs to."""
        with self.lock:
            self.open += 1
            self.last = time.monotonic()
            return self.run

    def add(self, run, request, answer):
        """The exchange opened in `run` answered; `request` is `None` where it failed."""
        with self.lock:
            if run != self.run:
                return
            self.open -= 1
            self.last = time.monotonic()
            if request is not None:
                self.exchanges.append({"request": request, "answer": answer})

    def snapshot(self):
        with self.lock:
            return list(self.exchanges)

    def quiet_for(self):
        """Seconds since the last request was opened or answered; 0 while one is open."""
        with self.lock:
            return 0 if self.open else time.monotonic() - self.last


def answer_of(body):
    """The message a streamed (or whole) answer of `/api/chat` makes: its text, its thinking and
    its tool calls."""
    text, thinking, calls = [], [], []
    for line in body.decode("utf-8", "replace").splitlines():
        if not line.strip():
            continue
        try:
            chunk = json.loads(line)
        except json.JSONDecodeError:
            continue
        message = chunk.get("message") or {}
        text.append(message.get("content") or "")
        thinking.append(message.get("thinking") or "")
        calls.extend(message.get("tool_calls") or [])
    return {"content": "".join(text), "thinking": "".join(thinking),
            "tool_calls": [{"name": c["function"]["name"], "arguments": c["function"].get("arguments")}
                           for c in calls]}


def proxy(recorder):
    """A server in front of Ollama that passes everything on and records the exchanges of
    `/api/chat`; it answers once Ollama has answered whole."""

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def relay(self, method):
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length) if length else None
            chat = self.path.startswith("/api/chat") and body
            run = recorder.opened() if chat else None
            request = urllib.request.Request(OLLAMA + self.path, data=body, method=method)
            if body is not None:
                request.add_header("Content-Type", self.headers.get("Content-Type", "application/json"))
            try:
                with OPENER.open(request, timeout=1800) as response:
                    status, answer = response.status, response.read()
                    kind = response.headers.get("Content-Type", "application/json")
            except urllib.error.HTTPError as error:
                status, answer, kind = error.code, error.read(), "application/json"
            except OSError:
                if chat:
                    recorder.add(run, None, None)
                raise
            if chat:
                recorder.add(run, json.loads(body), answer_of(answer))
            try:
                self.send_response(status)
                self.send_header("Content-Type", kind)
                self.send_header("Content-Length", str(len(answer)))
                self.end_headers()
                self.wfile.write(answer)
            except (BrokenPipeError, ConnectionResetError):
                # The program was ended while it waited for this answer.
                pass

        def do_GET(self):
            self.relay("GET")

        def do_POST(self):
            self.relay("POST")

        def do_HEAD(self):
            self.relay("HEAD")

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.recorder = recorder
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


# --- Judging ---------------------------------------------------------------------------------

def offers(request, tool):
    return any(t.get("function", {}).get("name") == tool for t in request.get("tools") or [])


# The tools of `turn_end`, with which a turn ends with its step.
TURN_END = {"stop", "bye", "silence"}


def ends_turn(exchange):
    calls = {c["name"] for c in exchange["answer"]["tool_calls"]}
    return not calls or bool(calls & TURN_END)


def judged(exchanges, quiet=False):
    """The verdict on the assistant's turn that started a sub-agent, with what it did there; `None`
    while that turn has not ended - unless the run is `quiet`, which judges it as far as it went."""
    lead = [e for e in exchanges if offers(e["request"], "start_subagent")]
    start = next((i for i, e in enumerate(lead)
                  if any(c["name"] == "start_subagent" for c in e["answer"]["tool_calls"])), None)
    if start is None:
        # The first turn ended and started nothing.
        if lead and (quiet or any(ends_turn(e) for e in lead)):
            return {"verdict": "no_start", "steps": [step(e) for e in lead]}
        return None
    # The step that started it, and those after it in the same turn: each request of the turn ends
    # with the results of the calls before it.
    turn = [lead[start]]
    for e in lead[start + 1:]:
        if (e["request"]["messages"] or [{}])[-1].get("role") != "tool":
            break
        turn.append(e)
    if not (quiet or ends_turn(turn[-1])):
        return None
    calls = [c["name"] for e in turn for c in e["answer"]["tool_calls"]]
    beside = [c["name"] for c in turn[0]["answer"]["tool_calls"] if c["name"] != "start_subagent"]
    last = turn[-1]["answer"]["content"].strip()
    if "ask_subagent" in calls:
        verdict = "asks_early"
    elif calls.count("start_subagent") > 1:
        verdict = "starts_more"
    elif "ask_user" in calls or last.endswith("?"):
        verdict = "asks_user"
    elif "read_history" in calls:
        verdict = "polls"
    else:
        verdict = "waits"
    return {"verdict": verdict, "beside_start": beside, "calls": calls, "last": last,
            "steps": [step(e) for e in turn]}


def step(exchange):
    answer = exchange["answer"]
    return {"calls": answer["tool_calls"], "content": answer["content"]}


# --- A run -----------------------------------------------------------------------------------

def run(args, task, server):
    """One run of `task`: the program started in a fresh home and directory, the task typed, ended
    once the turn that started a sub-agent is judged or the time is up."""
    run_id = datetime.datetime.now().strftime("%Y%m%d-%H%M%S-") + uuid.uuid4().hex[:6]
    root = pathlib.Path(tempfile.mkdtemp(prefix="subagent-start-"))
    home, work = root / "home", root / "project"
    for name, text in PROJECT.items():
        (work / name).parent.mkdir(parents=True, exist_ok=True)
        (work / name).write_text(text)
    config = home / "config" / "caelumora"
    config.mkdir(parents=True)
    endpoint = f"http://127.0.0.1:{server.server_address[1]}"
    model = {"type": "ollama", "model": args.model, "context_size": args.context_size,
             "endpoint": {"url": endpoint}}
    if args.think is not None:
        model["think"] = args.think
    if args.tokenizer:
        model["tokenizer"] = args.tokenizer
    (config / "config.yaml").write_text(json.dumps({"plugins": {"model": {"language": {"model": model}}}}))
    env = dict(os.environ, XDG_CONFIG_HOME=str(home / "config"), XDG_DATA_HOME=str(home / "data"),
               XDG_STATE_HOME=str(home / "state"), NO_PROXY="127.0.0.1,localhost",
               no_proxy="127.0.0.1,localhost")

    recorder = server.recorder
    recorder.clear()
    output = []
    program = subprocess.Popen([str(args.binary), "--screen", "line"], cwd=work, env=env,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, bufsize=1,
                               start_new_session=True)

    def type_line(line):
        try:
            program.stdin.write(line + "\n")
            program.stdin.flush()
        except (BrokenPipeError, ValueError):
            pass

    def read():
        approved = set()
        for line in program.stdout:
            output.append(line.rstrip("\n"))
            for approval in APPROVAL.findall(line):
                if approval not in approved:
                    approved.add(approval)
                    type_line(f"/approve {approval}")

    threading.Thread(target=read, daemon=True).start()
    time.sleep(3)
    type_line(TASKS[task])
    verdict, deadline = None, time.monotonic() + args.timeout
    while time.monotonic() < deadline and program.poll() is None:
        exchanges = recorder.snapshot()
        verdict = judged(exchanges, quiet=bool(exchanges) and recorder.quiet_for() > args.quiet)
        if verdict:
            break
        time.sleep(1)
    verdict = verdict or {"verdict": "unfinished"}
    stop(program)

    RESULTS.mkdir(exist_ok=True)
    exchanges = recorder.snapshot()
    with open(RESULTS / f"{run_id}.json", "w") as f:
        json.dump({"exchanges": exchanges, "output": output}, f, ensure_ascii=False, indent=1)
    record = {"run": run_id, "label": args.label, "model": args.model, "task": task, **verdict}
    with open(RUNS, "a") as f:
        f.write(json.dumps(record, ensure_ascii=False) + "\n")
    shutil.rmtree(root, ignore_errors=True)
    return record


def stop(program):
    """Ends the program and what it started."""
    try:
        program.stdin.close()
    except OSError:
        pass
    for sig, wait in ((signal.SIGTERM, 10), (signal.SIGKILL, 5)):
        if program.poll() is not None:
            return
        try:
            os.killpg(program.pid, sig)
            program.wait(wait)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            pass


# --- Reporting -------------------------------------------------------------------------------

VERDICTS = ["waits", "polls", "asks_early", "asks_user", "starts_more", "no_start", "unfinished"]


def rejudge():
    """Judges the recorded runs again from their exchanges, as `judged` judges now."""
    records = [json.loads(line) for line in RUNS.read_text().splitlines()]
    for record in records:
        exchanges = json.loads((RESULTS / f"{record['run']}.json").read_text())["exchanges"]
        kept = {key: record[key] for key in ("run", "label", "model", "task")}
        record.clear()
        record.update(kept, **(judged(exchanges, quiet=True) or {"verdict": "unfinished"}))
    RUNS.write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in records))


def table():
    if not RUNS.exists():
        print("No runs yet.")
        return
    counts = collections.defaultdict(collections.Counter)
    for line in RUNS.read_text().splitlines():
        record = json.loads(line)
        counts[(record["label"], record["model"], record["task"])][record["verdict"]] += 1
    print("| label | model | task | runs | " + " | ".join(VERDICTS) + " |")
    print("|---|---|---|---|" + "---|" * len(VERDICTS))
    for (label, model, task), counted in sorted(counts.items()):
        total = sum(counted.values())
        cells = " | ".join(str(counted[v]) for v in VERDICTS)
        print(f"| {label} | {model} | {task} | {total} | {cells} |")


def show(run_id):
    for line in RUNS.read_text().splitlines():
        record = json.loads(line)
        if record["run"] == run_id:
            print(json.dumps(record, ensure_ascii=False, indent=2))
            return
    print(f"No run {run_id}.")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--model", default="gemma4:26b")
    parser.add_argument("--runs", type=int, default=1, help="runs of each task")
    parser.add_argument("--tasks", nargs="+", choices=list(TASKS), default=list(TASKS))
    parser.add_argument("--label", default="after", help="what the binary is, e.g. before or after")
    parser.add_argument("--binary", type=pathlib.Path, default=BINARY)
    parser.add_argument("--timeout", type=int, default=600, help="seconds a run may take")
    parser.add_argument("--quiet", type=int, default=90,
                        help="seconds without a request open or new after which a turn counts as ended")
    parser.add_argument("--context-size", type=int, default=65536)
    parser.add_argument("--think", type=lambda v: v == "true", default=None,
                        help="true or false; Ollama's default for the model unless given")
    parser.add_argument("--tokenizer", help="a local model sharing the tokenizer, for a cloud model")
    parser.add_argument("--table", action="store_true")
    parser.add_argument("--show")
    parser.add_argument("--rejudge", action="store_true")
    args = parser.parse_args()
    if args.rejudge:
        rejudge()
        return table()
    if args.table:
        return table()
    if args.show:
        return show(args.show)
    # Resolved here: the program is started in the directory of its run.
    args.binary = args.binary.resolve()
    if not args.binary.exists():
        parser.error(f"{args.binary} is not built: cargo build -p caelumora-tui")

    server = proxy(Recorder())
    for task in args.tasks:
        for index in range(args.runs):
            record = run(args, task, server)
            print(f"{task} {index + 1}/{args.runs}: {record['verdict']}"
                  + (f" {record.get('calls')}" if record.get("calls") else ""))
    server.shutdown()
    table()


if __name__ == "__main__":
    main()
