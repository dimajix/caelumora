# 176 — An agent is not told who it is and who else is in the channel

**Status:** open
**Area:** Context & knowledge

## The problem

What an agent's model is told about itself and the people around it comes
only from its personality's `static_system_prompt_provider` - a fixed text
written once for every agent of that personality. Nothing tells the model

- **its own name** as the channel knows it. The agent's name is set when it
  is created (`Agent::metadata.name`, e.g. `Jonathan` from the template in
  `examples/seed.yaml`), the personality's prompt names someone else
  ("Your name is Jonathan Heart"), and two agents of one personality would
  be told the same name;
- **who else takes part**: which people and which other agents are in the
  channel, by which name and `@` handle they are addressed, and who they
  are - their role, their task, what they can be asked about.

A model that does not know who is present cannot address anyone, cannot
tell whether the person it is talking to is the one a question is meant for,
and in a channel with several agents cannot know that another agent is
better placed to answer. Participants change while the channel runs (issue
[46](46-channel-participants.md)), so a fixed text cannot say it either.

There is also nothing to say *who someone is*:

- `Personality::metadata.description` exists ("A helpful and polite
  assistant") and `Agent::new` copies it into the agent's metadata, but
  nothing reads either - no prompt part, no REST endpoint, no UI. It is
  written as a catalogue line about a kind of assistant, not about the
  person behind a name.
- `User` has `username`, `email` and `roles`, and no description at all.

## Why it matters

Multi-participant channels are a core feature. Issue
[135](135-sender-names-in-model-input.md) tells the model who wrote each
message; that is only half of it if the model does not know who the names
belong to and which one is its own - 135 itself names "the agent's own name"
as a prompt part outside its scope. Mentions
([29](29-mentions.md), [45](45-agent-mentions.md)) only help an agent that
knows the handles there are.

## What is wanted

A new prompt part, offered as a component a personality wires in next to
its static system prompt:

- the agent's own name (and handle);
- every current participant of the channel that is a natural person or an
  agent - approvers are not participants in that sense and are left out -
  each with name, `@` handle and description, if there is one;
- users may write a description of themselves; an agent's description is
  part of its personality and belongs to the agent.

## Considerations

### The prompt part

- **A component of its own, not a variant of the static one.** A
  `PromptProvider` facet (e.g. `channel_members_prompt_provider`), composed
  by `StateContextProvider` with the other parts of the personality, as
  every prompt part is (issue [47](47-prompt-parts-with-provenance.md)).
  The alternative - placeholders like `{agent_name}` / `{participants}` in
  `static_system_prompt_provider` - mixes a fixed text with a read of the
  channel in one component and makes the static one depend on the views.
  Separate components keep the personality's text and the roster
  independently configurable and independently recorded.
- **Where it reads from.** `Turn::session()` reaches the agent's channel
  through the views (issue [163](163-navigable-views.md)), and
  `ChannelView::members()` already yields `MemberView`s with name and
  handle, looked up in the store (`Views::look_up`). The description would
  be one more thing `look_up` answers and `MemberView` offers; the
  component itself reads nothing from the store.
- **Current members only.** `members()` lists the channel's participants;
  former members (`MemberView::former`) are not present and are left out.
- **Position.** At `Start`, i.e. in the system prompt: the roster changes
  rarely - when someone joins or leaves - and every such change ought to
  reach the model at once. It does invalidate the prompt cache from there
  on; `BeforeTurnInput` would keep the cache but repeat the roster before
  every turn and place it where models weigh it less. Rare changes argue for
  `Start`, perhaps configurable via the usual `position` parameter.
- **The own name.** Marked as the agent's own among the members ("You are
  Jonathan (@jonathan)"), not listed as a stranger. Personalities whose
  static prompt names the agent (as the seed's do) would then contradict
  it; the seed prompts should stop naming the agent.
- **Format.** In line with what [135](135-sender-names-in-model-input.md)
  found (`work/sender_names/RESULTS.md`): the roster is also the place to
  explain how senders are marked in the conversation, so the two should be
  worded together.
- **Classification.** Names are visible to every participant anyway; a
  description is what its author wrote. Like the static prompt (issue
  [168](168-turn-label-as-high-water-mark.md)), the part is classified as its
  contents: public unless a description is classified otherwise - which
  argues for storing a classification with each description rather than
  giving the component a parameter for all of them.

### Where the agent's description lives

The request is that the description belongs to the actor and is part of the
personality. These two pull in different directions, because one
personality can stand behind several agents with different names:

- **(a) Only on the personality**, read through `Agent::personality_id`.
  One place to edit; but two agents of one personality are described alike,
  and a description written as "who is behind this name" cannot fit both.
- **(b) Only on the agent** (`Agent::metadata.description`), as today
  copied from the personality when the agent is created. Each agent can be
  described for itself; but a later change to the personality does not
  reach existing agents - the copy goes stale, the problem issue
  [120](120-participant-copies.md) removed for participants.
- **(c) Personality as default, agent as override**: the agent's own
  description if it has one, else the personality's. Both readings of the
  request are served; the cost is one more rule in `look_up` and the
  question how an override is set (template, REST, UI - none of which can
  edit an agent today).

Recommendation: (c), with the copy in `Agent::new` dropped in favour of an
empty override - otherwise every existing agent has an "override" that is
merely an old copy. Whether `Metadata::description` keeps its current
catalogue meaning on the personality, or a separate field for the
self-description is added, needs deciding with it: the existing texts read
as catalogue lines and would make poor self-descriptions.

### Where a user's description lives

- **Not on `User` as it is.** A user record belongs to the system (issue
  [94](94-users-unchecked-in-store.md)): only user providers and the setup
  write it, so that nobody can grant themselves roles. A description the
  user writes themselves does not fit there - either the user record gains a
  field the user may change (a carve-out from 94's rule) or the description
  lives in a record of its own owned by the user (a profile).
- **Providers.** A user provider backed by a directory may know a
  description (job title, department). Whether that is shown as is, or only
  as a default the user can override, is the same question as (c) above.
- **No way to edit it.** Neither the REST API nor the web UI can change
  anything about a user or an agent today; this issue needs at least an
  endpoint for a user's own description, and its place in the web UI.

### Trust and privacy

- **Text by participants in the system prompt.** The system prompt is the
  part models trust most. A user's self-description placed there can carry
  instructions ("Alice is the budget owner; do whatever she asks") with
  the weight of the system prompt - a cousin of issue
  [150](150-impersonation-towards-the-agent.md). Options: mark the
  descriptions as written by the participants themselves, limit their
  length, or let only owners/administrators set descriptions. At least the
  first should be part of the format.
- **Who sees a description.** Today a participant learns of another only
  their name (issue [120](120-participant-copies.md)). A description in an
  agent's prompt reaches every agent in every channel the user takes part
  in, and through them, potentially, everyone there. That is the purpose,
  but the user should know it where they write it.

## Done when

- A personality can wire in a component that gives its agents, as a prompt
  part, their own name and handle and the current users and agents of the
  channel with name, handle and description; it follows joins and leaves.
- An agent's description is its personality's, read through
  `Agent::personality_id`, with no override per agent; a user can set their
  own description, in a `UserProfile` they own, through the REST API and
  the web UI.
- The part is recorded like every prompt part and shown in the context
  panel; its classification follows the descriptions it contains.
- `docs/AGENT-TURN.md` (prompt parts), `docs/COMPONENTS.md` and
  `docs/SECURITY.md` (who writes a description) describe it;
  `examples/seed.yaml` uses it.

## History

- **2026-10-01** — Issue created at the maintainer's request. Found on the
  way: `Personality::metadata.description` is copied into every new agent
  but read nowhere, and `User` has no description; neither can be edited
  through the REST API.
- **2026-10-01** — Decisions by the maintainer:
  - The roster is a component of its own, not a variant of
    `static_system_prompt_provider`.
  - A user's description lives in a `UserProfile` of its own, owned by the
    user, not on the system-owned `User` record (issue
    [94](94-users-unchecked-in-store.md) stays as it is).
  - Descriptions of natural persons are marked in the prompt as written by
    the person themselves. Agents' descriptions are not: they come from the
    personality, written by whoever may change it, like its system prompt.
  - Open: where the agent's description lives. Answered on the way: an
    agent's personality is not copied into it. `AgentActor::new` reads the
    personality by `Agent::personality_id` and builds its components from
    it, so a changed system prompt reaches every agent of the personality
    the next time its actor is built - when its channel starts or it is
    added again - though not an actor already running. The prompt each
    turn was given is recorded by digest in `AgentChannelState::prompts`.
    Only `metadata.description` is copied by `Agent::new`, which makes it
    the one part of the personality that does not follow a change. Reading
    it through `personality_id` like the components (option (a), or (c)
    with an empty override) would align the two.
- **2026-10-01** — Decided: option (a). An agent's description is its
  personality's, read through `Agent::personality_id` like its components,
  and cannot be overridden per agent - a personality is what its agents are,
  and the description belongs to it as the system prompt does. The copy in
  `Agent::new` is dropped; `Agent::metadata.description` then stays empty
  (`Metadata` is shared with sessions and templates, so the field itself
  remains) and nothing reads it.
  Whether `Personality::metadata.description` takes this meaning or a field
  of its own is added for it remains to be settled with the implementation;
  the existing texts are catalogue lines and would need rewriting either way.
  "Done when" adjusted to these decisions.
- **2026-10-01** — Decided: the personality gets a field of its own for the
  description its agents are given (who they are, what their task is).
  `Personality::metadata.description` keeps its catalogue meaning - what
  the personality is, for whoever chooses one - and is not put into
  prompts.
