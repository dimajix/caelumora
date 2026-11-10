# 556 — What an id lies in is found by searching, wherever only the id is passed on

**Status:** open
**Type:** question
**Priority:** medium
**Area:** Code structure
**Summary:** The stores, the state stores and the storages of content are told an entity by its id alone, so a backend whose layout follows what encloses an entity - `home` of issue 554 - has to search for it; the events answered the same question with `Address`, and whether an access path derived from an id should serve both is open.

## The problem

An entity is passed on by its id, and what it lies in is lost on the way, although the caller
mostly knows it:

- **The state stores** (`StateStore`, `HistoryStore`, `TranscriptStore`) take a channel by its
  `ChannelId` alone, in 21 methods. Every caller but one holds the session as well: the
  `ChannelLoop`, the `AgentRuntime`, `UserController::mark_read`, the purge of a session. The one
  that does not is the index of the work under way (`channels_with_work_under_way`), which knows
  channels only and reads each channel's document to find its session.
- **`DocumentStorage`** takes every document by its `Uuid` (`get`, `remove`), a channel too.
- **The storages of content** are given keys: `attachments/<channel>/<id>` names no session, and
  `artifacts/<project>/<artifact>/<content>` (issue 554, step 3) names the project, not where the
  project lies.
- **`CommandReaper`** (`caelumora-kernel/src/workspace/reaper.rs`) reads the channel's id from the
  scope string of a command and the session from the channel's document.

Where it is not known at all: a reference held by id - `ActorId::Agent`, `Agent::personality_id`,
a participant -, and what lies in a scope, whose scope is a value of its document, not of its type:
an agent may lie in the global scope, a user's, a session's or a channel's, and a project lifted
out of its session changes its scope.

The backend `home` lays a session out as one directory (issue 554), so it needs what lies above an
id. It searches: `sessions/*/channels/<id>`, `projects/<id>`, `sessions/*/projects/<id>`, and
remembers what it found, in memory, for each backend instance apart. That costs a look into every
session's directory the first time a channel or a project is asked for, a directory of its own for
the states of a channel no document names (`channels/<id>`, which only the store suite keeps), and
the rule that a channel's document comes before its states and they go before it, since otherwise
they would lie elsewhere.

## Why it matters

Every layout that follows what encloses an entity meets the same question, and answers it again:
the events did with `Address` (issue 335), `home` with a search. A path that can be derived from
an id would let a backend compute where something lies, a log or a person tell what an id belongs
to, and a storage of content place a key without asking anything.

## What is there

- **`Stored::Within`** names the kind of entity a kind lies in, fixed by the type: a `Channel` lies
  in a `Session`, every other kind in nothing (`NoEntity`).
- **`Address<T>`** is what an event names an entity by: `(SessionId, ChannelId)` for a channel, the
  id alone otherwise (`Enclosing::write`, `read`). It is used by the events, by `LiveDocument` to
  match them, and by the plugin `caelumora-artifacts` as a short name for a channel's two ids;
  never by a store.
- **`Scope`** is a name space - where something is found by name -, which issue 335 kept apart
  from what encloses an entity, although for a channel the two coincide.

## Considerations

- **A. Search, as `home` does now.** No interface changes; a search on the first touch, a cache per
  instance, and the directory of channels without a document.
- **B. Pass the address down.** The state stores, `DocumentStorage` and the keys of content take
  `Address<T>` where a kind lies in another. Weighed in issue 554 and not chosen: it changes 21
  methods of the state stores and `EntityStore` for channels, and leaves what lies in a scope to be
  searched all the same.
- **C. An address of every kind, from its document.** `Within` is fixed by the type; a scope is a
  value. An address read from the document (a method of `Stored`, not a type) would name what lies
  in a session's scope by its session - but a reference by id still names no address, and a lifted
  project would change its own.
- **D. Ids that carry their place.** A channel's id derived from its session's, or a composite id
  (issue 335 weighed and rejected a `ChannelAddress` as the channel's `Stored::Id`). Ids would stop
  being opaque, and what moves could not keep its id.
- **E. An index of the backend.** `home` keeps where each id lies - a file per id, a link - written
  with the document. A lookup is one read; the index is a second truth, to be kept right across a
  crash and across programs.

Whatever is chosen, it is to be decided whether `Within` and `Address` stay what they are, are
generalised, or give way to it.

## Done when

- It is decided whether an access path is derived from an id, and how; or that searching stays,
  with the reason.
- If something changes, `home` uses it in place of its search where it can, the decision is in
  `docs/STORAGE.md`, and `Within` and `Address` are generalised or their place beside it is said.

## History

- **2026-11-10** — Opened during step 4 of issue 554, which left `home` searching for what an id
  lies in. The maintainer holds that none of the options so far is satisfying, and that being able
  to derive the access path from a given id has value; to be thought through here, `Address`
  included.
