# What a model does once it started a sub-agent (issue 616) - experiment of 2026-11-27

`gemma4:26b` (local, `context_size` 65536, no `think`) on Ollama 0.40.18, driven through the
terminal program itself: `experiment.py` starts `caelumora --screen line` with the files of a first
start - the assistant with the component `subagents`, the personality `helper` to start - in a
directory of its own holding a small project with three TODOs, types a task, and approves what the
terminal asks to be approved. A proxy in front of Ollama records every exchange, so what is judged
is what Caelumora sent and what the model answered. Judged is the assistant's turn that started a
sub-agent, from the call of `start_subagent` to the end of that turn (the verdicts are in the
docstring of `experiment.py`). `--table` prints the table below, `--show RUN` a judged turn; the
exchanges of each run in `results/` (not committed). 10 runs per task and binary.

The binaries:

| label | built from | what the model is told |
|---|---|---|
| `before` | `main` at `ad75dc52`, before issue 616 | the texts of issue 558; the result of `start_subagent` is `{"thread"}` |
| `after` | the branch `issue-616-subagent-texts` at `5263c8bb` | the texts of issue 616; the result is `{"started", "thread"}`, `started` saying that nothing more is needed until the answer |

The tasks, both in German, as a person would type them:

*   **delegate**: start a sub-agent that finds the TODOs of the directory, then say what it found.
    Nothing else is to be done meanwhile: right is to end the turn.
*   **meanwhile**: start a sub-agent that collects the TODOs, and meanwhile read the README and say
    what the project does. Right is to read it - unless already read - and end the turn.

## Results

| label | task | runs | waits | polls | asks_early | asks_user | starts_more | no_start | unfinished |
|---|---|---|---|---|---|---|---|---|---|
| before | delegate | 10 | 1 | 0 | 9 | 0 | 0 | 0 | 0 |
| after | delegate | 10 | 7 | 1 | 2 | 0 | 0 | 0 | 0 |
| before | meanwhile | 10 | 9 | 0 | 0 | 1 | 0 | 0 | 0 |
| after | meanwhile | 10 | 9 | 0 | 0 | 0 | 0 | 1 | 0 |

## Findings

1.  **The observation of issue 616 is reproduced, and it is a question to the sub-agent.** With the
    texts before, `gemma4:26b` asked the sub-agent again in the turn that started it in 9 of 10
    `delegate` runs, 25 calls of `ask_subagent` in all, up to 6 in one turn, for what it was meant
    to wait for: "Please report your findings." (7 times), "Please provide your findings." (5),
    "Do you have the results yet?", "Any progress?", "I am waiting for your results.". Several of
    these runs then searched themselves and ended the sub-agent, one started a second. The model
    expects the answer within the turn.
2.  **The texts of issue 616 take most of it away.** With them, 2 of 10 `delegate` runs asked
    early, each once: one asked for the findings, the other gave as the thread "2ad4_...
    (placeholder, actually I should wait for the response in the next turn)" and no text, and
    ended the turn with the sentence that it would report once the sub-agent was done. One run
    looked for the answer instead: after the start, it called `read_history` and `run_command`
    23 times, and the turn ended without an answer.
3.  **With work of its own to do (`meanwhile`), the model waits under either texts** (9 of 10
    each). The early question is a matter of a turn with nothing else to do.
4.  **Outliers without bearing on the texts.** Under the texts before, one `meanwhile` run told the
    user what the project does and that the sub-agent was started through `ask_user`, as a
    "question" that asked nothing; under the new ones, one planned to start a sub-agent
    (`create_plan`) and never did.
5.  **A final answer that is a control token.** In 7 runs, under both binaries, the assistant's
    turn ended with an answer whose whole text was `<|tool_response>` - gemma4's token, given as
    text -, in one more beside a call. What the channel then shows as the assistant's answer was
    not looked at; not a matter of issue 616.
6.  **An earlier measurement was flawed and is not counted.** An exchange still open when a run's
    program was ended was recorded into the next run, which was then judged by it (`no_start`) and
    lost. `experiment.py` now drops what an earlier run opened; the flawed runs lie in
    `results/flawed-1/`.
