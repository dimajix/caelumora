# Agents that mention others only to ask them something (issue 438) - experiments of 2026-10-20

gemma4:26b (Q4_K_M) on Ollama, as Caelumora sends it, at most 3000 tokens per answer, 2 runs per
situation and variant. The contexts are those of the second `team-session` of the seed
(`team-session-2.json`, exported by `channel_export.py` after issues 437 to 440): 62 turns, 59 of
them asked for by a mention, nearly all by another agent. `mentions.py` changes what the agent is
told and nothing else; `--table` prints the tables (`results/mentions-table.md`), `--show VARIANT
SITUATION` what a variant gives. Raw answers in `results/mentions.jsonl` (not committed).

What the agents were told in that session invites the mentions three times over: the roster says
"The others - mention one by their @handle to address them", one line above 438's sentence that
says to mention one only when its answer is wanted; every message of another is marked
`<message from="@planner" ...>`; and the session's briefing says "mention one with @ to ask them
directly". The planner's very first answer mentions `@coder` and `@reviewer` - the habit does not
wait for the history.

The 15 situations:

*   **social** (12): the agent answers praise, a greeting, an agreement, or says that it is ready
    - "Willkommen in der Arena, @sassy!", "Sehr gut, @reviewer". Every mention of an agent is
    wrong: nobody's answer is wanted.
*   **work** (1): alice gave the go, the planner is chosen and designs the project. A mention of
    the coder may be right.
*   **handover** (2): constructed at the end of the session - alice writes "@planner Fokus! Ihr
    solltet langsam mal Code produzieren!", or the same to `@jonathan`, whom she made the lead.
    Right is to hand the coder (or the planner) the next step with a mention, and to mention no
    one else. This is the counterprobe: a variant that only strips every @ fails it.

The variants (`current` as in the session; every other replaces the roster's invitation by "The
others:" and 438's sentence by its rule, unless it only adds the reminder):

| variant | what changes |
|---|---|
| `current` | nothing |
| `strong` | the rule as prose: an @handle is a call, not a way to address someone; it interrupts the agent; only to hand it work or ask what it has to answer now, and then only that agent; everywhere else the name - "Thanks, Reviewer", "as Planner said" |
| `turn` | `current`, and before the newest message: "[Before you write] Write every name without @. Put an @handle only before the one agent you hand work to or whose answer you need now - never in a greeting, thanks, praise or agreement." |
| `strong_turn` | `strong` and `turn` |
| `names_strong_turn` | `strong_turn`, senders marked by name (`sender_style: name`) |
| `clean` | `current`, the agents' messages in the history with every mention of an agent written as its name |
| `clean_strong_turn` | `strong_turn` on the cleaned history - as if the rule had held from the start |
| `rules_names` | the maintainer's rules as a list: refer to others by name; an @handle only to give a task or ask a specific question, which makes the agent take a turn; an @handle is not a form of politeness; politeness shows in the name. Senders by name |
| `rules_names_turn` | `rules_names` and `turn` |
| `cond_names` | the maintainer's condition alone: "Only use an @handle if you require an answer or an action from another participant. Do NOT use an @handle if you only want to refer to another participant." Senders by name |
| `test_names` | when which form, as a list: @handle only when you need that participant to answer or to act, now - it makes the agent stop and take a turn; the name in everything else - greet, thank, praise, agree, answer, speak about; check every @ before you send. Senders by name |
| `test_names_turn` | `test_names`, and before the newest message the condition: "Only use an @handle if you require an answer or an action from that participant now. Do NOT use one to greet, thank, praise, agree with or refer to someone - write their name." |

## Results

| variant | social: texts with a mention | social: mentions per text | social: said nothing | work: wrong mention | handover: mentions whom it hands to | handover: wrong mention |
|---|---:|---:|---:|---:|---:|---:|
| `current` | 23/24 | 2.8 | 0/24 | 1/2 | 3/4 | 3/4 |
| `strong` | 14/23 | 2.0 | 1/24 | 0/2 | 4/4 | 2/4 |
| `turn` | 9/23 | 0.8 | 1/24 | 0/2 | 4/4 | 2/4 |
| `strong_turn` | 7/24 | 0.5 | 0/24 | 0/2 | 4/4 | 0/4 |
| `names_strong_turn` | 6/24 | 0.8 | 0/24 | 1/2 | 4/4 | 1/4 |
| `clean` | 19/23 | 1.6 | 1/24 | 0/2 | 4/4 | 3/4 |
| `clean_strong_turn` | 1/23 | 0.0 | 1/24 | 0/2 | 4/4 | 0/4 |
| `rules_names` | 16/23 | 1.9 | 0/24 | 0/2 | 4/4 | 4/4 |
| `rules_names_turn` | 8/24 | 0.7 | 0/24 | 0/2 | 4/4 | 1/4 |
| `cond_names` | 23/24 | 2.4 | 0/24 | 0/2 | 4/4 | 3/4 |
| `test_names` | 16/23 | 2.3 | 1/24 | 0/2 | 4/4 | 4/4 |
| `test_names_turn` | **3/24** | **0.2** | 0/24 | 0/2 | 3/4 | **0/4** |

"texts" are answers with text; a social situation whose answer was a tool call alone is not
counted among them. One answer (Coder:13, `names_strong_turn`) did not end before the limit.

## Findings

1.  **A rule in the system prompt alone does little, however it is put.** Prose (`strong`, 14 of
    23 texts still mention an agent), a list of rules (`rules_names`, 16 of 23), a test
    (`test_names`, 16 of 23), a bare condition (`cond_names`, 23 of 23 - no change). The roster's
    invitation taken out with them changes nothing either. The model has read the rule; by the
    time it writes, 40 messages full of @handles stand between.
2.  **The same rule before the newest message works.** A reminder of two sentences, read last,
    roughly halves what the rule in the system prompt leaves (`turn` 9 of 23 alone, `strong_turn`
    7 of 24, `rules_names_turn` 8 of 24) - and put as the maintainer's condition, with the cases
    that are no request named, it does best: `test_names_turn`, 3 of 24, 0.2 mentions per text
    against 2.8. The greeting that held out in every other variant (Coder:8, "Willkommen in der
    Arena, @sassy!") is written without @ in both runs. People are named, too: "Du hast recht,
    Alice."
3.  **Handovers survive.** After "@planner Fokus!" the planner mentions `@coder` and hands it the
    first tasks in every run of every variant but one of `current`. What the rules take away are
    the others: `current` adds `@reviewer` and `@sassy`, `test_names_turn` and `strong_turn`
    mention the coder alone. Once (Jonathan:focus, `test_names_turn`) the lead wrote "Wir warten
    jetzt nur noch auf die konkreten Tasks von Planner für Coder" without @ - the planner would
    not have been asked. Waiting for someone is the case the rule leaves to the model's judgement.
4.  **A rule that allows an @ for every task licenses one for everybody.** `rules_names` and
    `test_names` alone made the handovers worse (4 of 4 with a wrong mention): told that an
    @handle is for giving a task, the planner refocusing the team gives everybody one - `@coder`,
    `@jonathan`, `@reviewer`, `@sassy`. What bounds it is "now" and "only that agent"
    (`strong`), and the reminder.
5.  **The history teaches the habit, but taking it out alone is not enough.** Without the agents'
    mentions in the history (`clean`), 19 of 23 texts still mention an agent - the model mentions
    from its own habit, as the planner's first answer shows. With the rule, a clean history leaves
    1 of 23 (`clean_strong_turn`): where the rule holds from the start, the history it makes
    supports it, and a session should become quieter as it goes, not louder.
6.  **Senders marked by name instead of @handle change nothing measurable** (`names_strong_turn`
    against `strong_turn`). The roster still names every handle.
7.  **Nobody falls silent.** In every variant the agents answer as before (said nothing: 0 or 1 of
    24); they only write names instead of handles. Whether they should answer at all is issue 439's
    and the routing's question, not this one's.

All this is one model, two runs per cell, on one session. The differences between the variants
with a reminder (3, 7, 8 of 24) are within what two runs can tell apart; the difference between
those with and those without one is not.

## What follows for issue 438

*   The rule belongs **before the newest message**, as a part of the turn, not (only) in the
    roster: worded as `test_names_turn`'s condition - an @handle only for an answer or an action
    needed from that participant now; never to greet, thank, praise, agree with or refer to
    someone; the name instead.
*   The roster stops inviting mentions ("mention one by their @handle to address them") and says
    when which form is used, as `test_names` does; the seed's briefing of the `team-session` stops
    telling agents to mention.
*   `sender_style` stays `handle`.
