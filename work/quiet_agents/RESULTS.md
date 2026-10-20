# Agents that only speak when they have something to say (issues 438, 439) - experiments of 2026-10-20

gemma4:26b (Q4_K_M) on Ollama, as Caelumora sends it (no `think`, no temperature, `num_ctx`
65536), at most 3000 tokens per answer, 3 runs per situation and variant. The contexts are those
the agents of the seed's `team-session` were given (`team-session.json`, exported by
`channel_export.py`, verified token for token against the 81 turns), without what they heard again
after a restart (issue 440). `experiment.py` changes what the agent is told and nothing else;
`--table` prints the tables below, `--show VARIANT SITUATION` the answers. Raw answers in
`results/runs.jsonl` (not committed).

The 21 situations are turns of the session of six kinds, two of them constructed after its end:

*   **silent** (9): the agent was chosen by the decision model, nobody mentioned it, and it had
    nothing to add - Sassy greets alice, alice asks `@jonathan` something, the reviewer says it
    waits. Right is to say nothing. Two of them are constructed at the end of the session, as a
    control for *contribute*: Sassy cheers the team on, the reviewer or the planner is chosen.
*   **chatter** (5): the agent was mentioned in a greeting, a sympathy, a cheer - "Genau,
    `@jonathan`, Ruhe bewahren". Right is to say nothing.
*   **answer** (3): alice asks the agent something. Right is to answer.
*   **work** (2): the coder has its task, the go, and the workspace back. Right is to work - to call
    a tool - or at least to say it starts.
*   **concerned** (2): the counterprobe to *silent* - chosen, not mentioned, and the matter is the
    agent's: alice asks `@jonathan` for a quiz program, and the planner keeps the backlog, the coder
    writes code. Both spoke in the session. Whether they should have is open: alice asked Jonathan,
    who built the structure himself. Run with six variants only.
*   **contribute** (2): the counterprobe the session does not hold, constructed at its end. The
    coder reports T1 done - "Der Code ist bereit für den Review!", "Wenn der Planner das Go gibt
    ..." - its two messages as in the session, but with every mention taken out; the reviewer or
    the planner is chosen. Right is that the reviewer reviews and the planner decides about T2.
    Run with four variants.

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
| `reason_soft` | as `reason`, but put the other way round: "Nobody mentioned you. You are asked because what was said last might concern you. If it does - something is yours to do or to say, by your role - do it. If you have nothing to add - ... - call `silence` and write nothing." |
| `reason_before` | `reason`, but the part placed where a `before_turn_input` part goes: joined with the current time, before the newest message |
| `reason_work` | `reason`, and a mentioned agent is told: "@x mentioned you: they want your answer. Answer what they want of you, and go on with any work that is yours to do. If there is neither - their message only greets, thanks or agrees - call `silence` and write nothing." |

## Results

"said nothing" counts answers without text: `silence` called (in brackets), or an empty answer.

| variant | silent: said nothing | chatter: said nothing | answer: spoke | work: spoke or worked | concerned: spoke or worked | contribute: spoke or worked | texts mentioning an agent | speaks of credits |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `before` | 3/27 (0) | 0/15 | 9/9 | 6/6 | 6/6 | 6/6 | 45/59 | - |
| `b438` | 1/27 (0) | 0/15 | 9/9 | 6/6 | 6/6 | 6/6 | 49/62 | - |
| `credits` | 2/21 (0) | 0/15 | 9/9 | 6/6 | - | - | 28/43 | 0/43 |
| `credits_msg` | 3/21 (1) | 0/15 | 9/9 | 6/6 | - | - | 29/42 | 0/42 |
| `reason` | 27/27 (27) | 0/15 | 9/9 | 6/6 | **0/6** | **6/6** | 31/32 | - |
| `reason_soft` | 27/27 (27) | - | - | - | 2/6 | 6/6 | 4/4 | - |
| `reason_before` | 27/27 (27) | - | - | - | 1/6 | 6/6 | 5/5 | - |
| `reason_all` | 21/21 (21) | 15/15 (15) | 8/9 | **0/6** | 1/6 | - | 6/9 | - |
| `reason_pass` | 20/21 (20) | 13/15 (13) | 8/9 | 2/6 | - | - | 7/13 | - |
| `reason_work` | 21/21 (21) | 15/15 (15) | 8/9 | 4/6 | 0/6 | - | 6/10 | - |

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
    respond, 49 of 62 texts still mention an agent (45 of 59 without it); the greetings stay "Hallo
    `@sassy`!". The history is full of such mentions, and the model writes as the channel writes.
    In the first turns, before the habit has set in (Coder:0, Planner:0, Jonathan:0, Coder:1,
    Planner:1), it is no different: "Das klingt nach einem großartigen Projekt! Wenn `@jonathan`
    ..." with the sentence as without it.
3.  **Being told why it is asked silences the agent that was only chosen - 27 of 27.** Under
    `reason`, every agent the decision model chose without anybody mentioning it called `silence`
    and wrote nothing; under `before`, 24 of 27 wrote something - "Ich bin bereit", "I'll wait for
    a task", a greeting. Nothing else changes: the part is given only to the chosen, so answers and
    work are those of `b438`. In its thinking, the model quotes the part and decides by it ("I am
    not mentioned ... the last message is a request to @jonathan").
4.  **It also silences the chosen agent the matter concerns - 0 of 6.** Under `reason`, neither
    the planner nor the coder takes up alice's request to Jonathan; without the part, both did in 6
    of 6. The model sees that it is concerned and defers: "Does this concern me? Yes, as the
    Planner ... However, @jonathan hasn't responded yet. I'll call `silence`." In this situation
    that is defensible - alice asked Jonathan - but the session holds no turn where a chosen agent
    clearly had something of its own to say; finding 5 constructs one.
5.  **Where something is clearly its own, the chosen agent does it - 6 of 6.** With the coder's
    report of T1 freed of every mention, the reviewer chosen under `reason` looks for `loader.py`
    and its tests in the workspace (3 of 3, a tool call each), the planner answers (3 of 3) - and
    holds T2 back until the review is done: "Bevor wir das Go für T2 geben, müssen wir
    sicherstellen ...". In the control at the same place, Sassy cheering, both stay silent (6 of
    6). So the part tells a report that is meant for the agent from small talk that is not.
    Putting the chance that it concerns the agent first (`reason_soft`) changes neither: silent 27
    of 27, contribute 6 of 6; only in the *concerned* situations does the agent look around now
    and then (2 of 6: the coder lists the workspace, the planner reads its notes). Nor does its
    place: before the newest message, with the current time (`reason_before`), it does what it does
    after everything heard - silent 27 of 27, contribute 6 of 6, concerned 1 of 6.
6.  **A mentioned agent told when to stay silent stops working, too.** `reason_all` silences
    every chatter (15 of 15), but also the coder that was to start its task (0 of 6, against 6 of 6
    under every other variant). The go reached the coder in mentions that only cheered it on ("Ich
    freue mich sehr zu sehen ... `@all`, die Magie ist wieder im Spiel!") or announced the
    reviewer's readiness; the model judges the message that mentioned it - "it's an agreement /
    encouragement ... I will call `silence`" - not what is its own to do, and takes "write nothing"
    for "do nothing". The same rule silenced the planner once, whose question from alice was three
    messages back while the mention came from the reviewer (answer 8 of 9).
7.  **Naming the work helps, but not enough.** `reason_work` keeps every chatter silent (15 of
    15) and lets the coder work in 4 of 6 - but in 2 of 6 it still calls `silence`, once after
    planning in its thinking what to look at first. It answers what is its own when it looks past
    the mentioning message, and does not when it judges that message ("the `@reviewer` message is
    clearly an agreement/readiness message. Therefore, I call `silence`"). The planner's question
    from three messages back stays unanswered once (8 of 9), as under `reason_all`.
8.  **`pass` is worse than `silence`.** Renamed, the tool is called less reliably (20 of 21, 13 of
    15), once written out as the answer's text ("pass"), and the model deliberates about whether
    calling it still lets it write. It does not fix finding 6 either (2 of 6). The name `silence`
    stays.
9.  **Saying nothing is also the cheaper answer.** Where nothing was right, the answer took 5.7 s
    on average under `reason_all`, 11.0 s under `reason_work` (it thinks longer about whether there
    is work), 9.5 s under `reason` (chatter still spoke), 13.2 s under `before` - and every text not
    written is one the other agents do not have to read. The prompt is still read in full: a turn
    asked for and answered with silence costs its prefill all the same.

Besides: in a German channel, several of the idle answers were English ("I'll wait for a task"),
one began with garbage ("德at 21:26 ..."), one repeated alice's message with its `<message>`
marker.

## What follows for issue 439

*   **Telling the agent why it was asked is the one lever that works** - for gemma4, far more than
    the roster's sentence or credits.
*   **For the chosen agent** the part does what it should: silent where nothing was to be said
    (27 of 27), speaking and working where something was its own (6 of 6, finding 5). Issue 439
    gives it, worded as `reason`, as a prompt part before the turn's input (`reason_before`);
    `reason_soft` does as well and may be tried if chosen agents turn out too quiet in use.
*   **For the mentioned agent**, no wording found here is safe: whatever invites it to stay silent
    also lets it skip work it was meant to do (findings 6, 7). It is told nothing. Chatter between
    mentioned agents is left to the routing - fewer asks
    ([437](../../issues/437-mentions-of-people-and-failed-decisions-ask-agents.md)) and a limit to
    the asks one person's message can set off among the agents.
*   Credits (finding 1) and a renamed tool (finding 8) are not worth pursuing.
*   All this is one model, three runs per cell, on one session in which the agents had long since
    learned to greet with `@`.
