# 122 — The server cannot run in several instances

**Status:** open
**Area:** Observability and operations

## The problem

Everything the runtime keeps live - sessions, channels, actors, controllers, the agents' state, the
event bus - lives in one process. A second instance of the server against the same stores would run
the same session twice, keep its own copy of every agent that takes part in sessions of both, and
hear nothing of what the other one changed.

## Why it matters

One instance is a ceiling for load and a single point of failure. A database-backed store (issue
[26](26-database-user-backend.md) for users, and the same for entities) only helps once several
instances can share it.

## The model (agreed with the maintainer, see ARCHITECTURE.md, *Several instances*)

- **A session runs in exactly one instance**, with its channels, actors and their controllers.
  Requests for it are routed there.
- **Agents and users run in several instances at once** - an agent of user or global scope takes
  part in sessions of different instances. Each instance keeps one controller per agent and user
  (issue [121](121-agent-controller-per-process.md)) and one `AgentStateHandle` per agent (issue
  62); between instances they are kept in step through global events, including the agent's state.

## Considerations

- **An event bus between instances** (Redis, Kafka or the like): the `EventBus` publishes to it and
  republishes what arrives from it. Which events cross: at least the agent and user events and
  `AgentStateChanged`, whose receivers then reload the state - today `AgentStates` takes in nothing
  announced from elsewhere. Events carry no content where the content is private
  (`AgentStateChanged`); they must carry enough to decide whether they are new.
- **Versions.** Two instances can change the same agent at the same time; a lock in one process does
  not serialise that. Every document (and the agent's state) gets a version; a store refuses a write
  made on an outdated one; events carry the version they result in, and a controller skips one older
  than what it holds. Today an instance's own announcement coming back late can briefly set an older
  value again (see the test `a_change_made_here_is_not_undone_when_its_announcement_comes_back`);
  versions end that too.
- **Assigning sessions to instances**: who runs a session, how a request finds it, and what happens
  when an instance goes away (the crash recovery of issue [75](75-agent-resumes-half-turn.md)
  resumes a session in another instance).
- **Streams**: a user's server-sent event stream is served by one instance and must hear about
  everything the user may see, wherever it happens.

## Done when

Two instances run against shared stores and an event bus between them; a session runs in one of
them, an agent takes part in sessions of both, and a change to it in one instance reaches the other,
with concurrent writes detected rather than lost.

## History

- **2026-09-29** — Opened while working on issue 121, with the model agreed with the maintainer:
  sessions in exactly one instance, agents and users - with their state - in several, kept in step
  by global events.
- **2026-10-12** — Issue 323, step 5: `GlobalEvent` is read back as it is written. An event about an
  entity is `Entity { kind, id, change }` with an update as a JSON value, read by every instance
  the same way, whether it was published there or not; the names of kinds and custom changes are
  interned as they are read. The envelope that crosses the instances is `PublishedEvent` - the
  event with its attributes and classifications, which are serializable already -, so that the
  receiving bus decides who receives it. Versions, when they come, are a field of `Entity`.
