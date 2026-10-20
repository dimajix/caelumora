# Agents that only speak when they have something to say (issues 438, 439) - experiments of 2026-10-20

gemma4:26b (Q4_K_M) on Ollama, as Caelumora sends it (no `think`, no temperature, `num_ctx`
65536), at most 3000 tokens per answer, 3 runs per situation and variant. The contexts are those
the agents of the seed's `team-session` were given (`team-session.json`, exported by
`channel_export.py`, verified token for token against the 81 turns), without what they heard again
after a restart (issue 440). `experiment.py` changes what the agent is told and nothing else;
`--table` prints the tables below, `--show VARIANT SITUATION` the answers. Raw answers in
`results/runs.jsonl` (not committed).

The 19 situations are turns of the session of five kinds:

*   **silent** (7): the agent was chosen by the decision model, nobody mentioned it, and it had
    nothing to add - Sassy greets alice, alice asks `@jonathan` something, the reviewer says it
    waits. Right is to say nothing.
*   **chatter** (5): the agent was mentioned in a greeting, a sympathy, a cheer - "Genau,
    `@jonathan`, Ruhe bewahren". Right is to say nothing.
*   **answer** (3): alice asks the agent something. Right is to answer.
*   **work** (2): the coder has its task, the go, and the workspace back. Right is to work - to call
    a tool - or at least to say it starts.
*   **concerned** (2): the counterprobe to *silent* - chosen, not mentioned, and the matter is the
    agent's: alice asks `@jonathan` for a quiz program, and the planner keeps the backlog, the coder
    writes code. Both spoke in the session. Whether they should have is open: alice asked Jonathan,
    who built the structure himself. Run with five variants only.

The variants:

| variant | what the agent is told |
|---|---|
| `before` | the roster as before issue 438 |
| `b438` | the roster with 438's sentence: a mention of an agent asks it to respond |
| `credits` | `b438` and a section: every turn earns 5 credits, every mention of an agent costs 10 |
| `credits_msg` | `b438` and a section: silence is free, a message costs 5, a mention 10 more |
| `reason` | `b438` and, only if nobody mentioned it, why it is asked: "Nobody mentioned you. You are asked because what was said last might concern you - it may not. If you have nothing to add - nothing new, only agreement, a greeting, thanks, or that you are waiting - call `silence` and write nothing." |
| `reason_all` | `reason`, and a mentioned agent is told: "@x mentioned you: they want your answer. Answer what they want of you. If their message wants nothing of you - a greeting, thanks, agreement - call `silence` and write nothing." |
| `reason_pass` | `reason_all` with the tool `silence` renamed `pass` ("Pass your turn: ...") |
| `reason_work` | `reason`, and a mentioned agent is told: "@x mentioned you: they want your answer. Answer what they want of you, and go on with any work that is yours to do. If there is neither - their message only greets, thanks or agrees - call `silence` and write nothing." |

## Results

"said nothing" counts answers without text: `silence` called (in brackets), or an empty answer.

| variant | silent: said nothing | chatter: said nothing | answer: spoke | work: spoke or worked | concerned: spoke or worked | texts mentioning an agent | speaks of credits |
|---|---:|---:|---:|---:|---:|---:|---:|
| `before` | 3/21 (0) | 0/15 | 9/9 | 6/6 | 6/6 | 34/48 | - |
| `b438` | 1/21 (0) | 0/15 | 9/9 | 6/6 | 6/6 | 40/53 | - |
| `credits` | 2/21 (0) | 0/15 | 9/9 | 6/6 | - | 28/43 | 0/43 |
| `credits_msg` | 3/21 (1) | 0/15 | 9/9 | 6/6 | - | 29/42 | 0/42 |
| `reason` | 21/21 (21) | 0/15 | 9/9 | 6/6 | **0/6** | 28/29 | - |
| `reason_all` | 21/21 (21) | 15/15 (15) | 8/9 | **0/6** | 1/6 | 6/9 | - |
| `reason_pass` | 20/21 (20) | 13/15 (13) | 8/9 | 2/6 | - | 7/13 | - |
| `reason_work` | 21/21 (21) | 15/15 (15) | 8/9 | 4/6 | 0/6 | 6/10 | - |

Per situation: `results/overview.md`.

One call failed (Planner:0, `credits_msg`: still generating after 900 s, before the cap of 3000
tokens was set); it is not counted.

## Findings

1.  **Credits change nothing.** Neither price - for mentions, or for messages too - makes the
    agent say nothing more often (2-3 of 21 where nothing was right, as without them), nor mention
    less (28 of 43 texts, 30 of 44 without, on the same situations). It also never speaks of
    credits: the section is read over, not misread. Numbers that are not booked anywhere are, for
    this model, decoration.
2.  **438's sentence does not show here.** With the roster saying that a mention asks an agent to
    respond, 40 of 53 texts still mention an agent (34 of 48 without it); the greetings stay "Hallo
    `@sassy`!". The history is full of such mentions, and the model writes as the channel writes.
    In the first turns, before the habit has set in (Coder:0, Planner:0, Jonathan:0, Coder:1,
    Planner:1), it is no different: "Das klingt nach einem großartigen Projekt! Wenn `@jonathan`
    ..." with the sentence as without it.
3.  **Being told why it is asked silences the agent that was only chosen - 21 of 21.** Under
    `reason`, every agent the decision model chose without anybody mentioning it called `silence`
    and wrote nothing; under `before`, 18 of 21 wrote something - "Ich bin bereit", "I'll wait for
    a task", a greeting. Nothing else changes: the part is given only to the chosen, so answers and
    work are those of `b438`. In its thinking, the model quotes the part and decides by it ("I am
    not mentioned ... the last message is a request to @jonathan").
4.  **It also silences the chosen agent the matter concerns - 0 of 6.** Under `reason`, neither
    the planner nor the coder takes up alice's request to Jonathan; without the part, both did in 6
    of 6. The model sees that it is concerned and defers: "Does this concern me? Yes, as the
    Planner ... However, @jonathan hasn't responded yet. I'll call `silence`." In this situation
    that is defensible - alice asked Jonathan - but the session holds no turn where a chosen agent
    clearly had something of its own to say, so whether the part leaves room for it is not shown.
    "It may not concern you" weighs more for this model than "it might".
5.  **A mentioned agent told when to stay silent stops working, too.** `reason_all` silences
    every chatter (15 of 15), but also the coder that was to start its task (0 of 6, against 6 of 6
    under every other variant). The go reached the coder in mentions that only cheered it on ("Ich
    freue mich sehr zu sehen ... `@all`, die Magie ist wieder im Spiel!") or announced the
    reviewer's readiness; the model judges the message that mentioned it - "it's an agreement /
    encouragement ... I will call `silence`" - not what is its own to do, and takes "write nothing"
    for "do nothing". The same rule silenced the planner once, whose question from alice was three
    messages back while the mention came from the reviewer (answer 8 of 9).
6.  **Naming the work helps, but not enough.** `reason_work` keeps every chatter silent (15 of
    15) and lets the coder work in 4 of 6 - but in 2 of 6 it still calls `silence`, once after
    planning in its thinking what to look at first. It answers what is its own when it looks past
    the mentioning message, and does not when it judges that message ("the `@reviewer` message is
    clearly an agreement/readiness message. Therefore, I call `silence`"). The planner's question
    from three messages back stays unanswered once (8 of 9), as under `reason_all`.
7.  **`pass` is worse than `silence`.** Renamed, the tool is called less reliably (20 of 21, 13 of
    15), once written out as the answer's text ("pass"), and the model deliberates about whether
    calling it still lets it write. It does not fix finding 5 either (2 of 6). The name `silence`
    stays.
8.  **Saying nothing is also the cheaper answer.** Where nothing was right, the answer took 5.7 s
    on average under `reason_all`, 11.0 s under `reason_work` (it thinks longer about whether there
    is work), 9.5 s under `reason` (chatter still spoke), 13.2 s under `before` - and every text not
    written is one the other agents do not have to read. The prompt is still read in full: a turn
    asked for and answered with silence costs its prefill all the same.

Besides: in a German channel, several of the idle answers were English ("I'll wait for a task"),
one began with garbage ("德at 21:26 ..."), one repeated alice's message with its `<message>`
marker.

## What follows for issue 439

*   **Telling the agent why it was asked is the one lever that works** - for gemma4, far more than
    the roster's sentence or credits. It works so well that it errs on the side of silence: it
    silences what it should (findings 3, 5, 6) and what it perhaps should not (findings 4, 5, 6).
*   **For the chosen agent**, the part does what it should where nothing was to be said. Before it
    is built in, it needs a counterprobe the session does not have: a chosen agent with something
    clearly its own to say - the reviewer when code is posted without a mention, the planner when a
    task is done. A wording that weighs less towards silence ("it may concern you; if it does,
    ...") would be tried there.
*   **For the mentioned agent**, no wording found here is safe: whatever invites it to stay silent
    also lets it skip work it was meant to do. Chatter between mentioned agents is better left to
    fewer asks by the routing ([437](../../issues/437-mentions-of-people-and-failed-decisions-ask-agents.md))
    and fewer mentions in the first place; the mentioned agent is told only that, and by whom, it
    was mentioned - or nothing.
*   Credits (finding 1) and a renamed tool (finding 7) are not worth pursuing.
*   All this is one model, three runs per cell, on one session in which the agents had long since
    learned to greet with `@`. A second model (e.g. qwen3) and a fresh session would show how much
    of it is gemma4's.
