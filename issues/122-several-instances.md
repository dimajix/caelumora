# 122 — The server cannot run in several instances

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Observability and operations
**Summary:** The server cannot run in several instances: everything live is in one process, and nothing keeps agents and users in step between processes.

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
- **2026-10-29** — The bus between instances, from a review of the event bus with the maintainer
  (issues [502](502-bus-subscribers-lose-events-silently.md),
  [503](503-bus-asks-every-stream.md), [504](504-bus-has-two-delivery-paths.md)). Proposed, not yet
  decided:
  - **What crosses: everything.** A user's stream is served by one instance and must hear of
    everything they may read, wherever it happens; agents, users, profiles, workspaces, projects
    and groups live in several instances. So every event reaches every instance - a fan-out, not a
    log partitioned by key.
  - **Not Kafka, not Redis.** Kafka brings partitions, consumer groups and a cluster to run for a
    fan-out it is not needed for. Redis pub/sub is simple but delivers at most once and is a
    service of its own besides the store.
  - **An outbox in Postgres, woken by `LISTEN/NOTIFY`.** Several instances need a shared store,
    which the JSON files are not, so Postgres is there. An event is written to an outbox table in
    the transaction that writes its entity - never announced without being written, nor the other
    way round -; `NOTIFY` only wakes the other instances, which read the outbox from the last
    sequence number they took. Ordered, without gaps, and read again after a lost connection; the
    payload limit of `NOTIFY` does not apply. The outbox is pruned after a while. That is what a
    log gives, on the store there is.
  - **What it needs first.** Every subscriber catches up after a gap, by reading anew from the
    store (502) - between processes loss is the rule. Events carry the revision they result in
    (issue [492](492-a-document-is-written-under-two-locks.md)), so that an instance skips one older
    than what it holds. The bus has one path in one order (504), where the events of other
    instances enter. Who receives an event is decided by the receiving bus, as before.
  - **To settle.** An event published outside a write - a session or channel started or stopped,
    an agent's state - has no transaction of its own: written to the outbox alone, or kept local
    where only its own instance needs it. And content in events (`channel::SAID` carries the
    message, `Updated` the change): through the outbox it lies in the store, where it lies anyway;
    thin events would have each receiving instance read it again.
  - **Not done:** events as the only source of the truth (event sourcing), and the store making the
    events of its writes itself (change data capture) - an update says what changed in the words of
    its kind (`NameChanged`), which a store does not know, and starting, stopping and joining are no
    writes.
