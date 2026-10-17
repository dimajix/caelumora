# Asks each decision model named on the command line which of six agents should
# respond to 16 messages with a known right answer, as `decision_policy` asks
# (issue 403), and prints how many it got right, how probable it made the right
# answer, and how long a decision took once the model was loaded.
# Usage: python3 -I probe.py nimble:9b tev1:4b ...
import json, sys, time, urllib.request

URL = "http://127.0.0.1:11434/v1/systemone"
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))
CRITERIA = {
    "coder": "Coder: Writes and fixes application code in Rust and TypeScript.",
    "tester": "Tester: Writes and runs tests, reproduces bugs, reports what fails.",
    "reviewer": "Reviewer: Reviews changes for correctness, style and security before they are merged.",
    "ops": "Ops: Runs the servers, deployments, Docker and CI pipelines; watches logs and alerts.",
    "researcher": "Researcher: Looks things up - documentation, papers, prices, facts from the web - and summarises them.",
    "planner": "Planner: Keeps the backlog, splits work into tasks, estimates and schedules them.",
    "nobody": "Nobody: the message needs no answer from any of them.",
}
INSTRUCTIONS = "Which participant of the conversation should respond to the last message? Choose nobody if it needs no answer."
CASES = [
    ("The deployment to staging failed, the container keeps restarting with exit code 137.", "ops"),
    ("Kannst du einen Testfall schreiben, der den Bug mit leeren Namen reproduziert?", "tester"),
    ("Is it safe to store the session token in localStorage like I did in this change?", "reviewer"),
    ("Wir brauchen bis Freitag die Exportfunktion - wie teilen wir das am besten auf?", "planner"),
    ("Welche Lizenz hat eigentlich die Bibliothek `rmcp`?", "researcher"),
    ("The CI pipeline has been red for an hour, the runner seems to be out of disk space.", "ops"),
    ("Please add a `--verbose` flag to the CLI that prints every request.", "coder"),
    ("I merged it, thanks everyone.", "nobody"),
    ("Gute Nacht zusammen!", "nobody"),
    ("👍", "nobody"),
    ("Does the bug also happen on Firefox? Someone should try it.", "tester"),
    ("How long would it take to migrate the store to Postgres, roughly, and in which order?", "planner"),
    ("Die Funktion wirft bei ungültigem Input eine Panic - bitte so ändern, dass sie einen Fehler zurückgibt.", "coder"),
    ("Was sagt die offizielle Doku von Tokio zu `spawn_blocking`?", "researcher"),
    ("Before I merge: does this diff follow our error-handling guidelines?", "reviewer"),
    ("Prometheus alarmiert seit 10 Minuten wegen hoher Latenz auf dem API-Server.", "ops"),
]

def ask(model, state):
    body = {"model": model, "state": state, "keep_alive": "10m",
            "questions": {"respondent": {"type": "choice", "instructions": INSTRUCTIONS, "criteria": CRITERIA}}}
    req = urllib.request.Request(URL, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    start = time.time()
    with OPENER.open(req, timeout=300) as r:
        answer = json.load(r)
    return answer["answers"]["respondent"], time.time() - start

for model in sys.argv[1:]:
    ask(model, "warm up")
    right, times, wrong, p_right = 0, [], [], []
    for state, expected in CASES:
        a, t = ask(model, state)
        times.append(t)
        p_right.append(a["probabilities"].get(expected, 0.0))
        if a["choice"] == expected:
            right += 1
        else:
            wrong.append(f"{expected}->{a['choice']}")
    times.sort()
    print(f"{model}: {right}/{len(CASES)}, p(right) mean {sum(p_right)/len(p_right):.2f} min {min(p_right):.2f}, median {times[len(times)//2]*1000:.0f} ms; wrong: {', '.join(wrong) or '-'}", flush=True)
