# 442 — Agents keep mentioning each other although the roster says what a mention does

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** Agents greet, thank and praise each other with `@handle` although the roster says since issue 438 that a mention asks the agent to respond, because the rule stands once in the system prompt next to an invitation to address others by `@handle`, so nearly every turn in a channel of several agents is asked for by another agent's courtesy.

## The problem

Since [438](438-agents-mention-without-knowing-it-asks.md) the roster of
`channel_participants_prompt_provider` says that mentioning an agent asks it to respond, and to
mention one only when its answer is wanted. It changed nothing. In the second `team-session` of
the seed, after issues 437 to 440, 59 of 62 turns were asked for by a mention, nearly all by
another agent: "Willkommen in der Arena, @sassy!", "Sehr gut, @reviewer", "Das ist eine
hervorragende Roadmap, @planner". Each makes the agent mentioned take a turn, in which it does the
same; `max_agent_hops` limits how deep such a chain goes, not how wide.

What the agents are told invites the mentions, three times over:

*   The roster opens the list of the others with "mention one by their @handle to address them,
    or all of them with @all" - one line above 438's sentence, which says the opposite.
*   The `team-session`'s briefing in the seed says "mention one with @ to ask them directly".
*   Every message of another is marked `<message from="@planner" ...>`.

And the habit does not wait for the history: the planner's first answer, before any agent had
written, mentioned `@coder` and `@reviewer`.

## Considerations

### What the experiments found

`work/quiet_agents/mentions.py` sent 15 turns of that session again in 12 variants of what the
agent is told (gemma4:26b, two runs each; [MENTIONS.md](../work/quiet_agents/MENTIONS.md)):
12 social turns, where every mention of an agent is wrong, and as the counterprobe two handovers
constructed at the session's end ("@planner Fokus! Ihr solltet langsam mal Code produzieren!"),
where mentioning the coder is right and mentioning anyone else is not.

*   **A rule in the system prompt alone does little, however it is put** - as prose, as a list of
    rules, as a condition, with the roster's invitation taken out: 14 to 23 of 23 social answers
    still mention an agent, against 23 of 24 as the session was.
*   **The rule repeated before the newest message works.** Put as a condition - "Only use an
    @handle if you require an answer or an action from that participant now. Do NOT use one to
    greet, thank, praise, agree with or refer to someone - write their name." - and with the
    roster saying when which form is used, 3 of 24 social answers mention an agent, 0.2 mentions
    per answer against 2.8. The greeting that held out everywhere else is written without @.
*   **Handovers survive**: after "@planner Fokus!" the planner hands the coder its tasks with a
    mention, and mentions no one else (without the rule it added the reviewer and Sassy). Once, the
    lead wrote that the team waits for the planner's tasks without mentioning the planner.
*   **The rule needs a bound, not only a condition.** Told that an @handle is for giving a task,
    the planner refocusing the team gives everybody one and mentions everybody (4 of 4); "now" and
    "that participant" bound it.
*   **Senders marked by name instead of `@handle` change nothing**; `sender_style` stays.
*   **A history without the mentions supports the rule** (1 of 23 with both): where it holds from
    the start, a session should grow quieter, not louder.

### Where the reminder comes from

A prompt provider has one position; the roster stands at the start, the reminder belongs before
the turn's input (`PromptPosition::BeforeTurnInput`).

*   **A component of its own** (decided): a prompt provider - for instance
    `channel_mentions_prompt_provider`, beside the other `channel_…` providers - whose part, before
    the turn's input, tells the agent when to write an @handle and when a name. It gives the part
    only where there is another agent in the channel to mention: a mention of a person asks no
    agent, and an agent alone has nobody to ask. The text is a constant of the provider until the
    prompt catalogue ([57](57-prompt-catalogue.md)) takes it over, with the roster's. The seed's
    `channel-context` includes it.
*   **`static_system_prompt_provider` at `before_turn_input`, the text in the seed** - set aside:
    no code, but every configuration would carry the text itself, it would be given in channels
    without another agent, and what a mention does in a channel is the runtime's to say, not each
    configuration's.

### The roster and the seed

The roster stops inviting: the list of the others opens without "mention one by their @handle to
address them", and 438's sentence becomes the rule of when which form is used - an @handle only
when that participant's answer or action is needed now, which makes an agent take a turn of its
own; the name, without @, to greet, thank, praise, agree, answer or speak about someone. The
`team-session`'s briefing in the seed no longer tells agents to mention.

## Done when

*   The roster says when an @handle is written and when a name, and invites no mention.
*   A prompt provider gives, before the turn's input, the rule on mentions where another agent is
    in the channel; the seed's `channel-context` includes it, and the `team-session`'s briefing
    tells no agent to mention.
*   [AGENT-TURN.md](../docs/AGENT-TURN.md), [CHANNELS.md](../docs/CHANNELS.md) and
    [COMPONENTS.md](../docs/COMPONENTS.md) describe it.

## History

- **2026-10-20** — Opened after a look at the second `team-session` of the seed, where 438's
  sentence left nearly every turn asked for by another agent's mention, and the experiments in
  `work/quiet_agents/mentions.py`. The maintainer decided on a prompt provider of its own for the
  reminder before the turn's input.
