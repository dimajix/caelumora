# Caelumora Architecture

Caelumora is a multi-agent system: people and AI agents talk to each other in
channels, and agents act through tools. This document draws the overall
picture - what the parts are, how a message travels through them, and the
principles the design follows. Every section ends with the document that
holds the details; the list at the end names them all.

## Crates

The project is a Rust workspace, with a web UI next to it:

*   **`caelumora-model`** - the domain: entities (`Session`, `Channel`,
    `Agent`, `Personality`, `User`, `SessionTemplate`), messages and events,
    the traits of the stores, the security and confidentiality model, and the
    configuration (`AppConfig`). It holds data and rules, but runs nothing.
*   **`caelumora-runtime`** - everything that runs: sessions, channels and
    their actors, the agent's cognitive cycle, components, the event bus,
    the controllers that keep entities live, and the store implementations.
*   **`caelumora-rest-server`** - the REST API and the server-sent event
    streams, with authentication. It maps what the runtime answers to
    responses and decides nothing itself. See
    [REST-SERVER.md](REST-SERVER.md).
*   **`caelumora-web-ui`** - the browser frontend (React, TypeScript), built
    on the REST server. See [WEB-UI.md](WEB-UI.md).
*   **`caelumora-tui`** - a terminal frontend that runs the runtime in its
    own process.
*   **`caelumora-client-api`**, **`caelumora-local-client`**,
    **`caelumora-rest-client`** - reserved for a client interface and its
    in-process and REST implementations; empty so far.

## The domain

A **session** is one piece of work, such as a conversation about a topic. It
has participants - users and agents - and one or more **channels**, in which
some of them talk. A channel is a chat room with its own history, its own
classification and its own **moderator**, which decides who is asked to
respond to a message.

Everything that takes part in a channel is an **actor**: a user (`UserActor`),
an agent (`AgentActor`), or the channel's approver (`ApproverActor`), which
collects the users' decisions on tool calls that need approval. Actors
communicate only through **envelopes** - an event with who sent it, on whose
authority (`SecurityContext`) and in reply to what (`EventContext`).

An **agent** is an identity with a **personality**. The personality says
what the agent consists of: a list of components (its tools, its system
prompt, its notes, ...), how its agents are introduced to the others
(issue 176) and how confidential its reasoning is. An agent is read through
its personality, not copied from it. One agent can take part in many
sessions and channels at the same time; it has one actor in each channel
and one state across them.

Sessions are made from **session templates**. A template names every
participant of the session - users, agents with their personality and scope,
approvers with their strategy - and the channels, which refer to the
participants by name. Creating a session from a template has two steps
(issue 88):

1.  The model says what each participant is for the session
    (`ParticipantTemplate::request`: "user" is the session's owner, an agent
    is looked up by name in its scope, an approver is made), and builds the
    session and its channels from the participants it is given
    (`SessionTemplate::build`), without touching a store.
2.  `SessionManager::create_from_template(ctx, template_id, name)` does the
    rest: it reads the template, finds the owner and the agents, makes the
    agents that are missing (only when the lookup finds none, never because
    it failed), and builds, stores and starts the session. REST
    (`POST /sessions`) and the TUI both call it.

Sessions and channels hold their users and agents by reference -
`Participant::User(UserId)`, `Participant::Agent(AgentId)` - not as copies
(issue 120). The records live in the `EntityStore`; a channel leaves out,
with a warning, a participant whose record no longer exists, and runs
without it. An approver has no record of its own and is held whole.

Details: [CHANNELS.md](CHANNELS.md).

## How a message travels

The path of one message shows how the parts fit together:

1.  **A user writes.** The frontend (REST or TUI) hands the text to *that
    user's* actor in the channel. There is no way to write into a channel
    other than through one's own actor, so sender and authority always
    follow from who is writing.
2.  **The channel routes it.** The `ChannelLoop` resolves `@`-mentions,
    appends the message to the channel's history and passes it through the
    moderator, which delivers it to every other participant and asks some
    agents to respond.
3.  **An agent takes a turn.** The `AgentActor` begins a **turn**: its
    context provider assembles what the model is given (prompt parts, the
    agent's transcript), the model answers, and tool calls in the answer are
    run - after approval, where a tool requires it - and their results fed
    back, until the model answers without calling a tool.
4.  **The answer is published.** What the agent sends is stripped of what
    only its owner may see, delivered to the others through the channel,
    and the turn is committed: the messages go into the agent's transcript
    and the history, staged changes to the agent's state are applied, and
    everything is stored.
5.  **Observers are told.** The channel's events reach the participants'
    live streams; changes to entities are announced on the event bus, which
    the web UI follows.

A turn that fails ends the turn, not the agent: it is reported as
`TurnFailed` and the agent returns to idle. A turn cut off by a crash is
resumed when the agent is loaded again.

Details: [AGENT-TURN.md](AGENT-TURN.md), [CHANNELS.md](CHANNELS.md),
[EVENTS.md](EVENTS.md), [REST-SERVER.md](REST-SERVER.md#streams).

## Agents are made of components

What an agent can do is not programmed into the `AgentActor` but configured:
a personality lists **components**, and each component offers one or more
**facets** - traits through which it takes part in the turn. A component
contributes tools (`ToolProvider`), text for the model (`PromptProvider`), or
processes incoming messages (`PerceiveInProcessor`). Of some facets an agent
has exactly one: the `LLMClient`, the language model it talks to; the
`ContextProvider`, which assembles the model's context; and the
`ToolExecutor`, which runs tool calls. All three have built-in defaults that
a personality or the configuration can replace.

Components are wired by type, much like Spring beans: a component asks for
the facets it needs, and the container builds whatever provides them. User
commands such as `/help` are components as well, built once per session.

Details: [COMPONENTS.md](COMPONENTS.md),
[TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md).

## What an agent remembers

An agent's memory has several tiers, each with one meaning: the channel's
**history** (the record, never rewritten), the agent's **transcript** (what
its context is built from), **notes** (published in the channel),
**private notes** and **agent memory** (seen only by the agent). An agent
may also read the notes and history of other channels of its session, as
far as confidentiality allows. For every message an agent wrote, it can be
reconstructed exactly what its model saw, because the transcript is a chain
of digests.

Details: [MEMORY.md](MEMORY.md), [AGENT-TURN.md](AGENT-TURN.md).

## Reading what is there

What is in the system is read through one navigation, the **views**
(`caelumora_runtime::view`): from a session to its channels, from a
channel to its history, notes and members, from an agent among them to
what it saw - its transcript, its private notes, the context of each of
its messages, the turns it gave up. Each step asks its own access
question, in the name of whoever reads; a node is data that was allowed to
be reached and decides nothing, so an operation still checks its own
permission. The views read what is stored - through the running loop
where there is one - so running and stopped sessions are read alike and
reading starts nothing.

Two kinds of viewer walk the same steps: a user, who reads what they may
read and are cleared for, and an agent, which reads from its channel by
the confidentiality rules, through the `Turn` its components are called
with. The REST server reads through the views and answers with their
snapshots; the components read through them; neither asks the policy
itself.

Details: [SECURITY.md](SECURITY.md#views),
[MEMORY.md](MEMORY.md#reading-other-channels).

## State and persistence

Two stores, configured separately:

*   the **`EntityStore`** keeps long-lived documents - personalities,
    templates, agents, users and their profiles, sessions and channels;
*   the **`StateStore`** keeps what changes while a session runs - the
    channel's history, each agent's state in each channel, and each agent's
    own state.

Every document and every kind of state has exactly **one writer**, which
changes it under a lock, stores it and only then announces the change. For
entities, that writer is a **controller**; for state, the channel loop, the
agent's actor in its channel, or the agent's state handle. Agents and users
have one controller per process, however many sessions they take part in.

The server runs in one process today, but is cut so that it can run in
several (issue 122): a session runs in exactly one instance, while agents and
users may be active in several, kept in step through the global events.

Details: [STORAGE.md](STORAGE.md).

## Security and confidentiality

Two questions are kept apart:

*   **Who may do what** is decided by one `SecurityManager` per process, asked
    with an `ActionType` and the `SecurityAttributes` (owner and contributors)
    of the entity concerned. The default policy is ownership: owners do
    everything, contributors read and use. What a client may see, it reads
    through the runtime's **views** (see [Reading what is
    there](#reading-what-is-there)), which ask the policy at every step for
    that viewer; frontends never ask the policy themselves. Users come from
    a chain of user providers, and the REST server authenticates them with
    a short-lived signed token.
*   **What may flow where** is decided by classifications, not by who takes
    part. Every channel has a classification - a level from `public` to
    `strictly_confidential` and an audience of compartments, which the
    administrator configures - and one flow rule decides whether a channel
    may read another. Apart from that, the parts of a message are flagged
    if only the agent's owner may see them, and are never published then.

Details: [SECURITY.md](SECURITY.md), [CONFIDENTIALITY.md](CONFIDENTIALITY.md).

## Principles

The design repeats a few decisions; new code follows them.

*   **The runtime decides, frontends map.** What a user may see, which
    commands exist, whether a line is a message or a command - each is
    decided in one place in the runtime or the model. REST and the web UI
    translate it and repeat no rule.
*   **One writer per document.** No state is changed from two places; changes
    are stored first and announced afterwards, and events only announce -
    nothing changes because an event arrived.
*   **Stage, then commit.** A turn stages its changes and applies them when
    it completes, so a failed turn leaves nothing behind.
*   **Least authority.** Everything is handed only what it needs: a component
    gets a narrow context, not its actor's runtime; an actor gets its own
    channel, not the session.
*   **Refusals do not reveal.** Something the caller may not see is reported
    exactly like something that does not exist, so ids and names cannot be
    probed.
*   **Records are not recomputed.** Digests, resolved mentions and history
    limits are fixed when they arise and stored, so a later change of
    scheme or name cannot rewrite the past.

## Documents

*   [COMPONENTS.md](COMPONENTS.md) - components, facets, how they are wired,
    the built-in components.
*   [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md) - tools, their execution
    and approval, user commands, the built-in tools and commands.
*   [AGENT-TURN.md](AGENT-TURN.md) - the agent's cognitive cycle, the turn,
    what an agent saw, prompt parts.
*   [MEMORY.md](MEMORY.md) - notes, private notes, agent memory, reading other
    channels, the terms for what an agent remembers.
*   [CHANNELS.md](CHANNELS.md) - sessions, channels, actors, participants,
    moderators and mentions.
*   [EVENTS.md](EVENTS.md) - the event bus.
*   [REST-SERVER.md](REST-SERVER.md) - the REST API, authentication, errors
    and the server-sent event streams.
*   [WEB-UI.md](WEB-UI.md) - the browser frontend: how it talks to the server
    and what it shows.
*   [STORAGE.md](STORAGE.md) - entity and state store, controllers, several
    instances.
*   [SECURITY.md](SECURITY.md) - the security manager, ownership, views,
    users, authentication, secrets.
*   [CONFIDENTIALITY.md](CONFIDENTIALITY.md) - classifications, compartments
    and the flow rule, what only an owner sees, and what is still open.
*   [CONFIGURATION.md](CONFIGURATION.md) - the sections of the configuration
    file.
*   [CODING-GUIDELINES.md](CODING-GUIDELINES.md) - coding style, error
    handling, logging, tests.
