# Storage

Caelumora keeps two kinds of data apart: long-lived **entities** - definitions and identities - in
the entity stores, one `EntityStore<T>` per kind, and frequently changing runtime **state** in the
`StateStore`, with the histories the states hold read through the `HistoryStore`; preconfigured
components have a `ComponentStore` of their own. This document describes the stores, who writes
what, and the controllers that hold the entities of a running session live. How access to stored
entities is checked is in [SECURITY.md](SECURITY.md); which stores are used is configured in
[CONFIGURATION.md](CONFIGURATION.md).

## The entity store

The entity stores (`EntityStore<T>`, in `caelumora-model`) keep the long-lived domain entities:

*   **Personalities**, the definitions of agents, and **session templates**, which sessions are
    made from.
*   **Identities**: agents, sessions, channels and users. A channel is a document of its own, which
    its session lists by id.
*   **User profiles**: what a user says about themselves - introduction, preferred language,
    answers given in advance. One per user, apart from the `User` record (see
    [SECURITY.md](SECURITY.md#ownership) for why).
*   **Workspaces**: a tree of files a user owns and attaches to sessions (see
    [WORKSPACE.md](WORKSPACE.md)). The document names the storage its files lie in; the files are
    no part of it.
*   **Skills**: procedures written down for agents, a kind of the plugin `caelumora-skills` (see
    [SKILLS.md](SKILLS.md)).
*   **Artifacts**: results an agent published, each a user's, with its versions, a kind of the
    plugin `caelumora-artifacts`. Their content lies in a configured artifact storage, not in the
    document.
*   **Groups**: lists of actors an access list names as one subject - a user's teams, and the
    participants of every session and channel (see [SECURITY.md](SECURITY.md#groups)).
*   **The kinds of the plugins** (see [PLUGINS.md](PLUGINS.md)), kept like the
    core's.

Each kind is kept by a store of its own, `EntityStore<T>` (issue
[323](../issues/323-kinds-enumerated-in-the-core.md)), with the same operations for every kind.

**A backend keeps, the model checks** (issue
[446](../issues/446-stores-check-access-each-and-are-enumerated.md)). A backend - `MemoryStore`, for
tests, or `JsonFileStore`, of the plugin `caelumora-store-json` - is a `PersistenceBackend`: it
keeps the documents of every kind, each kind in a `DocumentStorage` of its own, and the states with
their histories and transcripts, and asks nothing. It offers each of them by a method of its own
(`documents`, `state_store`, `history_store`, `transcript_store`, issue
[448](../issues/448-persistence-inherits-the-state-stores.md)) and is none of them, so that a
backend that keeps only the one part - a database of the entities alone - refuses the other; the
memory and the JSON store are the three state stores themselves and hand out themselves. The
history and the transcript store read what the state store of the same backend writes, so all three
are taken from one backend. A storage keeps documents serialized, by id, and is not generic, so
that a backend opens the storage of any kind, a plugin's too, without knowing its type: the memory
store keeps them in a map, the JSON store one file per document in the kind's directory,
`entities/<kind>/<id>.json`. What every kind's store does - read the document as its type, ask the
policies, order the steps - is written once, in the model (`CheckedStore`, over `store::access`),
for every backend alike.

A backend is made ready once, at start (`PersistenceBackend::open`, issue
[447](../issues/447-store-seam-not-fit-for-a-database.md)): after every plugin registered, so after
the kinds of the core and of the plugins were opened, and before anything reads or writes. A
database connects there - by a secret, which the backend is given when it is made -, applies its
migrations and makes what its kinds need; the memory and the JSON files need nothing. A kind opened
later is the backend's to make ready when it is first used. `RuntimeBuilder::from_config` opens the
backends it makes; whoever makes a runtime of stores of their own (`RuntimeBuilder::new`) opens
them (`Persistence::open`), the backend of the states first, then that of the entities, if it is
another instance.

What the runtime holds is one `Persistence`, the stores: made of the backend the configuration names
under `persistence.entity_store`, the one it names under `persistence.state_store` - each a backend
of the `Backends` like every other, the core's `memory` and `json` or a plugin's (`dyn
PersistenceBackend`, see [CONFIGURATION.md](CONFIGURATION.md#backends)), the same type or two - and
the two policies (`Persistence::new`; `Persistence::over` for one backend in both roles). It takes
the documents from the first, as their kinds are opened, and the three state stores from the second
when it is made, together - so that a backend that keeps no states fails the start, as the state
store. It opens the store of a kind for whoever asks (`EntityStoreFactory::entity_store::<T>()`),
once - whoever asks for a kind again, the core or a plugin, is given the same store -, refuses a
kind whose name another kind has, and gives the component store and the state stores. The stores of
the core's kinds are `EntityStores`, which the runtime is given; a plugin asks for the store of its
kind while it registers. A manager is made with `&EntityStores` - or the store of its kind - and
keeps the stores of the kinds it uses as fields, as every service is held. Tests make their stores
the same way, `Persistence::in_memory()` for stores in memory that permit everything,
`Persistence::in_memory_with` for the policies of a test - both cannot fail, since the memory store
is the three state stores itself.

Every stored type says what it is to its store (`Stored`): its kind, `Stored::KIND`, an `EntityKind`
named by a string (`"skill"`) that is unique among the kinds; its id and the fields a query names;
the kind of entity it lies in, `Stored::Within` - a channel in a `Session`, every other kind of the
core in `NoEntity` -, by which its events name where it lies (issue 335, see
[EVENTS.md](EVENTS.md#the-form-of-an-event)); and, if its kind is `Scoped`, the name that is unique
in its scope (`Stored::unique_name`). The stores take all of it from the type they read and write:
what is asked of the `AccessPolicy` (`ActionType::get(T::KIND, …)`), the name in errors, logs and
the JSON store's directory, and which names to keep unique. A stored type can be copied and
serialized (`Stored` requires it), which is all a backend needs to keep it. A new kind is added
without a change to the stores or to `EntityKind`.

### Artifact content

The content of artifacts lies in the storages configured under `artifacts`, each an
`ArtifactStorage` that puts, reads and deletes a version without asking permissions - a backend of
the plugin `caelumora-artifacts`, made by the factory registered for its `type` while the plugin
registers (see [ARCHITECTURE.md](ARCHITECTURE.md#backends), issue 328). The `ArtifactManager` is the
one writer of artifact documents and the only code that holds the storages: a version's content is
kept before the document says it exists, and removed again if the document cannot be stored; an
artifact is purged with its content, the content first. It knows no workspace: where the content
comes from is its caller's. Artifacts are entities, not state: each has its own owner, permissions,
a name unique in its scope and a trash, and one that is kept outlives the session it was published
in.

### Preconfigured components

Preconfigured components are runtime configuration that personalities and templates refer to by
`kind: "ref"`. The `ComponentStore` keeps them as `StoredComponents` - one or several component
descriptors with the attributes of who may use and change them - with the same operations as
entities, checked the same way. A descriptor inside a personality or template carries no attributes;
it is covered by those of what holds it. The component store is opened where the entity store is.

### Names and scopes

Every entity has a name, and the fields that mean something for it:

*   **What is a scope** - a session, a channel, a user - has a name only.
*   **What lies in a scope** - a personality, a session template, an agent, a workspace, a skill, an
    artifact - has a name and a `Scope`: where it is found by name. The store finds one in exactly
    its scope; from a place, `scope::find_by_name` finds the innermost of a name - in the place's
    scope, in those around it or in the user's own.
*   **What is offered for choosing** - a personality, a session template - has a `description`, a
    catalogue line.

What lies in a session's or a channel's scope is used only within it and goes with it; a user's
scope or the global one only says where a name is found (see [CHANNELS.md](CHANNELS.md#sessions)).
A template asks for each agent in exactly the scope it names and reuses it only if it is of the
personality it names, so an agent's scope decides whether it is reused - one per user, with its
memory - or made anew for every session. A session template lies in no session's or channel's
scope. A workspace lies in a session's scope, its owner's or the global one - never a channel's,
since a session's channels share one - and belongs to a user wherever it lies; so does an artifact.

**Of one kind, a name is unique in its name space**, and the store keeps it so: nothing is created,
renamed, moved or restored under a name another of its kind has there, compared as names are
(`StoreError::NameTaken`). A channel's name space is its session's scope, a user's the global one;
a session's name is not unique.

*   The check reads every one of the kind that is not deleted, also those the caller may not read
    (see [SECURITY.md](SECURITY.md#how-stores-check)). What is deleted holds no name: restoring it
    is checked as creating it is.
*   It is made by the storage, in the same step as the write - under the write lock in memory,
    under the kind's lock in the JSON files, by a unique index in a database -, so that it holds
    against every writer of the storage. The store hands the storage the name with the document
    (`Document`); a storage that keeps bodies alone reads it back with what it was given when it
    was opened (`DocumentOf`). Where a storage needs a name as one value - a column of a unique
    index -, it takes its key: its scope and its folded name in one text (`ScopedName::key`, issue
    447), equal exactly where the names are. The store suite states the rule for every kind.

**Values with rules.** A text whose content has rules - a `Name`, a `Language`, the free texts
people write for others (`Text<R>`) - has a type made only through its constructor and checked again
when a document is read, so that a stored document that does not fit fails to load (`ValueError`,
reported as `StoreError::Invalid`). A value is checked, never changed: it is kept exactly as
written, and what identifies something is compared by a key made from it. A `Name` is compared by
its ASCII form, regardless of case and of how whitespace runs, so "Zoë" and "zoe" are one name; none
of these types derefs to `str`, which would bypass that. Above the model - loops, controllers,
events, REST requests - such a text is a string as given; the model makes the value where it is
assigned.

### Queries

Every list of the entity store takes a query (`caelumora_model::store::query`) and answers a page:

*   **A filter**, purely declarative, so that a database could push it into a `WHERE` clause. It
    names fields of the kind, each a path into the document as serialized. Values take the forms
    serde writes but are made without it, so that making a filter cannot fail; a tagged value meets
    an object that *has* its key with a matching value - containment, as `@>` in Postgres -, a
    variant alone (`FieldValue::variant`) an object that has its key or the string of a variant that
    holds nothing - as `?` in Postgres.
*   **A name prefix** (`Filter::NamePrefix`) compares a name as names are compared - regardless of
    case, accents and runs of whitespace -, for a search by what one has typed so far; **words in a
    text** (`Filter::Contains`) are compared the same way, for a search of what was written.
*   **Scope conditions** are made by the scope (`Scope::within`, `Scope::seen_from`), so that the
    rule lives with `Scope`.
*   **A limit and a cursor**, in the order of the entities' ids. A page cut short hands out the
    cursor to go on with; the page after it may be empty.
*   **Who may read what is asked afterwards**, entity by entity; a limit counts what the caller may
    read.

**A list reads in batches** (issue [447](../issues/447-store-seam-not-fit-for-a-database.md)). The
store asks the storage for the documents after the cursor, in the order of their ids - as `str`
orders them -, among what is deleted or not and meeting the filter, a batch at a time
(`DocumentStorage::scan`), the filter by the paths of its fields (`Filter::by_path`). A storage may
hand out more than it is asked for, never less: the store judges every document against the whole
filter again, reads it as its type and asks the policy, and reads the next batch until the page is
full. So a database translates what it can of a filter and leaves the rest to the store - a name
prefix and words in a text, which compare as names are folded -, and the JSON files judge the filter
on each file and leave the mark of a deletion to the store. What follows a page's limit is read only
as far as its batch. A page that stops at its limit hands out the cursor unless the storage showed
that nothing follows. A document a storage cannot read as it is kept - a file that is not JSON - or
one that cannot be read as its type is left out of a list and logged; any other failure of the
storage fails the list. Finding by name asks the storage for the one document of the name that is
not deleted (`DocumentStorage::named`), and then the policy, as a list asks.

Without an index, a list without a limit still goes through every entity of the kind. Every list
that narrows what it read says so in its query; what a query cannot say yet - a time comparison, a
lookup by normalized name, a viewer's clearance - stays code at the caller.

### Deleted entities

What a user deletes is kept, marked, and removed for good only by a purge - an administrator's act
or a retention policy's. The kinds that deleting marks implement `Deletable` and carry who deleted
them and when: sessions, channels, agents, users, user profiles, personalities, session templates
and workspaces. A preconfigured component is not marked: it is reached only through what refers to
it, and is deleted for good.

A marked document is not there for the ordinary operations, before anything is asked: not read,
listed, found by name, changed or deleted again - also not by the system, so that no caller has to
remember to leave it out. Its id stays taken, so seeding does not bring back deleted demo data.
Nothing created or changed may carry the mark: only deleting sets it. What was deleted is reached by
operations of its own, each with a permission of its own:

| Operation | asks | does |
|---|---|---|
| `get_deleted`, `list_deleted` | `ReadDeleted` | read the marked documents; what is not marked is not found |
| `restore` | `ReadDeleted`, then `Restore` | takes the mark away and returns the restored document |
| `purge` | `ReadDeleted`, then `Purge` | removes the document for good - only if it is marked, also when it is removed |

For what is classified - a channel, a workspace, a skill, an artifact -, every operation also asks
whether the caller is cleared for it, the deleted alike (issue 322, see
[SECURITY.md](SECURITY.md#how-stores-check)): the store is given a `ConfidentialityPolicy` beside
its `AccessPolicy`.

Purging only what is still marked keeps a purge from removing what was restored meanwhile. Restoring
and purging a session are serialised by the `SessionManager`, under the lock it also deletes and
starts a session with. What the runtime removes for good that is not marked - the channels of a
session being purged, a document whose creation failed - it marks first, in its own name. The state
of what was deleted stays in the `StateStore` until it is purged (see [Deleting
state](#deleting-state)).

### Demo data

Demo data is not part of any binary. The files the configuration's `seed` names hold preconfigured
components, personalities and session templates as the stores keep them, with fixed ids, and the
sections of the plugins - `skills` among them -, each seeded by the plugin that claims it
(`SeedSection`); a section no plugin claims is an error before anything is stored. At startup both
binaries put into the stores whatever they do not have yet, by id, as the system
(`RuntimeManager::seed`): restarting changes nothing, and a document changed in the store stays as
it is.

## The state store

The `StateStore` (in `caelumora-model`) stores the runtime state of channels and agents:

*   **`ChannelState`**: the channel's history (see [Histories](#histories)).
*   **`AgentChannelState`**: an agent in one channel, of either kind - processing state,
    transcript, notes, prompt texts and tool sets, and the turn under way (see
    [AGENT-TURN.md](AGENT-TURN.md)); its transcript, its prompt texts and its tool sets kept apart
    (see [Transcripts](#transcripts), [Prompt texts and tool sets](#prompt-texts-and-tool-sets)). It
    keeps no history of its own: what it was offered and said is its channel's history, as it saw
    it (issue 417) - and where it stands in it (`heard`, issue 223), written as it takes in and so
    stored with what it took in: what its inbox opens at when it runs again.
    An external agent keeps instead what it keeps of the remote side: the remote conversation and
    the tasks not yet ended.
*   **`AgentState`**: the agent itself, across its channels and sessions - so far its memory (see
    [MEMORY.md](MEMORY.md)).
*   **`UserChannelState`**: a user in one channel - up to where they read it (see
    [CHANNELS.md](CHANNELS.md#what-is-new-to-a-user)).

### What a state tells a store

Every state is `Journaled`: it tells what changed since it was last stored, as a sequence of
changes, and replays such changes onto what was stored before. Its changes are one enum per state,
named "state change" on purpose: a change of the state - what was said, noted, remembered - as
against an update of the entity.

*   A growing part records what was added: a chain tells the nodes added since it was last stored
    and its head, the same form the whole chain is stored in (see
    [AGENT-TURN.md](AGENT-TURN.md#the-transcript-as-a-chain)). Each node carries its position, and
    a node is merged only at the position it has, or at the one after the last if it is new. A
    position is a type of its own, `chain::Position` - its place in the order entries were added,
    across every branch, written as its number -, and so is everything that addresses entries by
    one: a page and its cursor, a delivery, a mark of what was read, a member's range, where a
    window begins.
*   A small part - the processing state, the turn under way, the tokens the agent's calls used, an
    external agent's open tasks and the classifications it was given, where an agent stands in its
    channel's history and the last message it took in - is recorded whole: it is changed in place
    and bounded by one turn, one exchange, or the labels in use. What would be summed over a whole
    chain is kept as such a part instead, so that a chain is never read whole for it. Every part a
    state stores is among its changes: one left out is stored only with the next snapshot, and a
    state loaded again stands where that left it (issue 440).

Replaying is idempotent, and nothing replayed is a change to store again. Every reference between
parts is a digest or an id, resolved when read and never while replaying, so the order across parts
loses nothing.

A put stores the state, whole or only what changed, and marks it stored on success; on failure the
changes stay pending and come with the next put. The `JsonFileStore` keeps a **snapshot** and a
**journal** per state: a put appends its changes to the journal, unless the journal has grown to the
size of the snapshot (and to `journal_floor`), when the state is written as a new snapshot and the
journal emptied. So the bytes written over a state's life are a small multiple of its final size
instead of growing with its square. What a crash leaves is read as it is: a torn last line is
dropped, and a journal that survived its snapshot replays onto it without effect.

### Histories

A channel's history (`History`, a chain of `HistoryEntry`) grows with the channel, without a bound,
and is read almost only in pages. It is the one record of what happened in the channel, for every
participant (issue 417), and is kept apart from the channel state's document (issue 379):

*   **The chain is the truth.** It is only appended to, and a change of a message is an entry after
    it. A store keeps the chain whole, under the channel's id, written by the put of the channel's
    state: first the entries added since it was last stored, then the document. Every entry is
    recorded with the actor whose envelope it was and whom it went to, resolved as it is recorded
    into what a store filters by (`Recorded { entry, sender, recipients }`, issue 417): every
    member at the time of the kinds it names - users, agents -, the channel's approvers, and some
    by name (`Recipients`), by the rule every channel delivers by. It is recorded with the head of
    the envelope it came in too - its context and when it was sent (issue 223) -, which no digest
    covers: what it is delivered again from (`Recorded::envelope`) to an actor whose inbox fell
    behind. What an actor depends on receiving is recorded for that alone - a command or a request
    to answer again asked of an agent, approvals, questions asked and closed -: entries that are
    neither items nor changes, in the chain alone, never in a page.
*   **The items are derived.** Beside the chain the store keeps the items as they stand - every
    message with every change since folded into it by the one rule of folding
    (`HistoryEntry::fold_into`), the marks of turns, steps, commands, activities, compactions and
    of who joined or left -
    each at the position of the entry that added it (`StandingItems`). So is who was a member
    when (`Memberships`): each member's ranges of positions, from the `from` of the `HistoryShared`
    before its `ParticipantJoined`, or else from its join, up to the `until` of its
    `ParticipantLeft` - where the channel stopped delivering to it. Both are derived entry by entry
    as the chain grows and rebuilt from it at any time (`HistoryStore::rebuild`); the store suite
    checks that the items equal the chain folded, and what each reader is given.
*   **The read side is the `HistoryStore`**, handed out by the `Persistence` beside the
    `StateStore`: a page of items before a position, of those that meet a filter, with the digest it
    was read from; the item that said a message, with what it was produced from; the chain from a
    position. Every read is a reader's (`Reader`): a participant, given what went to them and what
    they sent - the latter in full; an outsider - a session's owner who takes no part, a reader from
    another channel -, given what went to every user; the channel itself, for its own checks.
    Whether a reader is given an entry is one predicate (`Recorded::seen_by`) over what the store
    keeps of it - its sender, its recipients, whether anything of it is published, its position -
    and the reader's ranges: the channel everything; a participant what it sent, and what went to it
    by name or to every member of its kind while it was one; an outsider what went to every user. A
    database answers it as a `WHERE` over the items and a table of the ranges; a page counts what
    its reader sees. The filter is data too (issue 421): a `Filter` of the entity stores' queries
    over the fields of an item as it is recorded (`HistoryField`) - a message said, its sender, role
    and words, whom it mentions, its level and compartments -, made by the reader's needs
    (`Filter::said_by`, `heard`, `containing`, `readable` for what a reader from another channel may
    read) and judged on the item before its form is chosen, so that a database adds it to the same
    `WHERE`. In which form is `Recorded::as_seen_by`: as it was recorded for its sender, as
    everybody was given it (`Message::published`) for anyone else. Whatever reads a history without
    holding it reads there, whether the channel runs or not: the views - a page over REST, a
    message, what an agent saw -, the checks before a change, an actor's inbox that fell behind, the
    tool that reads another channel. The answer of a turn, what its end carried as it was delivered,
    is read from the messages of the turn said before its end (`HistoryStore::answer`). Where a
    participant's view begins is the start of their first range (`HistoryStore::memberships`). A
    page is addressed by the position (`seq`) of its oldest item; a client passes it back without
    computing with it.
*   **The holder keeps a window.** The channel loop holds the history from `history_from` on, a
    small part of the document: the last `WINDOW` (200) entries. After every put it lets go of what
    lies before (`keep_window`, `Chain::keep_from`), and a state is loaded from there. What a holder
    reads in memory reads from the end and stays within its window (see
    [AGENT-TURN.md](AGENT-TURN.md#what-is-held-in-memory)); a change of a message before the window
    is recorded, and the store applies it.
*   **The `JsonFileStore`** keeps a history in `<key>.history/` beside its state's snapshot: the
    chain in segments of 256 positions (`chain-<from>.json`) and `head.json` - the head and where
    the chain ends -, written after the segments; the items likewise (`items-<from>.json`), the
    memberships (`memberships.json`) and `items-head.json` - up to where both are derived -,
    written after them. Entries a crash
    left past the head's end are not there; items behind the chain are derived anew when the
    history is next loaded or appended to. A message is found by scanning the items backwards, and
    a fitting rewrites every items segment: slow where a database has indexes. In Postgres
    ([04](../issues/04-postgres-state-store.md)) a history is the tables `history_entries(key, seq,
    parent, digest, entry)`, `history_items(key, seq, message_id, sender, recipients, published,
    item)` and `history_memberships(key, member, from, until)` beside the documents, written in one
    transaction with them.

### Transcripts

An agent's transcript in a channel (`Transcript`, a chain of `TranscriptEntry`) grows with every
compaction and, for an external agent, with every call, each of which begins a branch of its own;
the model is given only the active one. It is kept apart from the agent's state document as the
history is (issue 380), with what differs:

*   **The same chain, nothing derived.** The put of the agent's state writes the entries added since
    it was last stored, after the history and before the document; the store keeps every branch.
    What stands of a branch is read from it as it is loaded (`as_they_stand`).
*   **The read side is the `TranscriptStore`**, handed out by the `Persistence`: the path to a
    digest, on whichever branch - what the context of an older message names -, a message as it
    stands where it was given, and which of some messages the transcript holds. The views read
    there, and an external agent's actor when it tells its remote agent of a change.
*   **The holder keeps the active branch**, from its root on (`transcript_from`), and lets go of
    what lies before after every put (`keep_window`, see
    [AGENT-TURN.md](AGENT-TURN.md#what-is-held-in-memory)).
*   **A digest may be there twice.** The holder dedupes an entry by its digest only within its
    window, so a root given again after the transcript began anew is a second entry where it would
    have been one. Both name the same content after the same predecessors; the store makes no
    digest unique, and a path is followed back from the newest entry with the digest.
*   **One way of keeping a chain.** Both stores keep a history's and a transcript's chain alike, by
    position: the `MemoryStore` as nodes by position (`KeptChain`), the `JsonFileStore` in segments
    and a head (`chains`), the transcript in `<agent>.transcript/` beside the agent's snapshot,
    without anything laid over it. A path is followed back segment by segment, a message looked for
    in every segment: slow where a database has indexes. In Postgres a transcript is the table
    `transcript_entries(agent, channel, seq, parent, digest, entry)`, with an index on the digest
    and on the message an entry holds, written in the transaction of the agent's state.

### Prompt texts and tool sets

The texts of the prompt parts an agent's model was given and the tool sets it was offered are kept
once by digest (`Kept`), and named by digest from every `ContextRef`. A system prompt changes with
every note and every participant, so a new text is kept nearly every turn. Beside them the frames of
the contexts its messages were produced from (`FrameRef`, issue 426): what a message was given
around its transcript - references to the prompt parts, the tools and the knowledge read, the
profile -, which stays the same over most of a turn, while the transcript moves on with every
message. A message in the history names its frame by digest, beside the digest of its transcript,
so that what its context repeats is written once and not with every message. They are kept apart
from the agent's state document as the transcript is (issue 383):

*   **Written first.** The put of the agent's state writes the texts and sets kept since it was last
    stored before the history, the transcript and the document, so that nothing stored names a text
    that is not there after a crash.
*   **Once by digest, idempotent.** A text kept again - the same system prompt in the next turn - is
    written as the same: a digest there already is not written again. A text nothing names any
    more, that of a turn cut off before it said anything, stays.
*   **Read by digest, through the `TranscriptStore`**: the texts of some digests, the sets of an
    offer, a frame, a message's context with its frame (`context`), and the texts and sets of a
    `ContextRef` (`named_by`) - what the context of an older message is resolved with.
*   **The holder keeps what its turn under way names**, and lets go of the rest after every put
    (`keep_window`); between turns it holds none, and a state loaded holds those its turn names, so
    that a turn cut off by a crash is resumed with them. A frame it lets go of at once: the turn
    under way holds its own whole, in its context.
*   **The `JsonFileStore`** keeps them in `<agent>.given/` beside the agent's snapshot, one file per
    digest (`prompt-<digest>.json`, `tools-<digest>.json`, `frame-<digest>.json`), and looks for
    the file before it writes one. In Postgres each is a table keyed by agent, channel and digest,
    written with `ON CONFLICT DO NOTHING`.

### The agent state has one owner per process

An agent may run in several channels and sessions at once, each with its own actor. Every one of
them is handed the same `AgentStateHandle`, so that there is one owner of the agent's state per
process. A turn does not hold the state: it reads a snapshot and stages changes. When it completes,
`Turn::commit` applies the staged changes to the state as it is at that moment, under the handle's
lock, stores it and announces `agent::STATE_CHANGED` without content. No actor thus overwrites
another's changes, and every next turn sees them. Numbers for new memory entries are handed out by
the handle at once, since turns in several channels may remember at the same time.

### One writer per state

Every state has exactly one writer, and its puts never overlap. A store may keep only what changed
since the state was last stored, and what changed is known of the one instance that was stored: a
second writer would have its changes replayed onto what the first stored. A lock in the store would
not help; the rule is the runtime's to keep:

*   `ChannelState`: its `ChannelLoop`;
*   `AgentChannelState`: the actor of that agent and channel;
*   `AgentState`: its handle;
*   `UserChannelState`: the user's controller, one per user in the instance;

each holding the state under a lock of its own while it changes and stores it.

This holds because a session starts only once, however many requests ask for it at the same time:
starting, stopping and deleting a session take one lock per session id, so a stop cannot miss a
start under way, and a deleted session cannot be started again meanwhile.

Entities have no such rule: two requests may change one at the same time, and the last write wins.
The JSON stores write every file through a temporary file of its own, so that neither write fails
for the other's.

### Work under way

What a restart is to start again is read from an index the state store keeps beside the agents'
states (issue 388), not by loading every state: a put of an `AgentChannelState` enters the agent
and channel in it while the state has work under way (`AgentChannelState::has_work_under_way`) and
takes them out with the put that has none, and deleting the state takes them out with it.
`StateStore::channels_with_work_under_way` lists the channels. Since a turn is stored after every
step, the index is as current after a crash as after an orderly shutdown.

The `JsonFileStore` keeps an empty file per entry, `runtime/under_way/<channel_id>.<agent_id>`,
written only when the entry comes or goes. It is entered before the state is written and taken out
after it: what a crash leaves between the two is a session started for nothing, never work that is
not started. The `MemoryStore` asks the states it keeps.

### Deleting state

Deleting a session or a channel keeps its state, hidden with it; purging removes the state too, with
its histories. The entity store and the state store may be different stores, so a purge deletes the
state explicitly - a channel's with that of every agent and user in it, an agent's own with its
state in every channel. It is never a side effect of the `EntityStore`.

## Controllers

The live state of the entities of a running session is held by controllers
(`caelumora_runtime::controller`): `SessionController`, `ChannelController`, `AgentController`,
`PersonalityController`, `UserController` and `WorkspaceController`, made by the
`ControllerManager`. The model only defines
the documents; keeping them live, writing them and announcing changes is the runtime's. What a
controller changes and what its entity's manager does is in
[ARCHITECTURE.md](ARCHITECTURE.md#managers-and-controllers).

*   **Live state.** A controller holds its entity for atomic, lock-free reads of the current value.
*   **One writer.** A controller is the one writer of its document. It holds a write lock while it
    changes the document, stores the change first - which is where the permission is checked - and
    only then takes it over live and announces it.
*   **Events.** Controllers publish their changes on the `EventBus` (see [EVENTS.md](EVENTS.md)) and
    take over changes announced elsewhere.
*   **One shape.** Every controller holds its document as a `LiveDocument` (issue 358), which does
    all of the above alike for every kind: a change told by the kind's update event
    (`Updatable::apply`, in the model) is applied, stored with when it was changed
    (`Updatable::touch`, `updated_at` where a kind keeps it), taken over, announced - to who reached
    the document as it was classified and as it is - and logged, once, by the controller. What a
    controller does besides - a group that follows the participants, an event of its own, the
    checks of a workspace, which is changed as stored rather than as live - it does between the
    steps of a `Writing`, the change under way. A running session's and a channel's loop only
    forward to their controller what does no more than that. The `UserController` holds no
    `LiveDocument`: the user's record belongs to the system and is only kept current, and their
    profile is created with its first change and announced as it stands, to its user and the
    runtime alone. The `WorkspaceController` also holds the storage its workspace lies in and the
    confidentiality policy, which a new classification is asked of. The `PersonalityController`
    is no writer at all (issue 406): personalities are changed in the store, and it only holds the
    one copy every running agent of a personality answers from (see below).

### Session-owned and per-instance controllers

A `SessionController` and its `ChannelController`s are made with the running session and live as
long as it does: a session runs once, so there is never a second. An agent or a user takes part in
any number of sessions, and a workspace may be attached to several and is used directly too, so the
`ControllerManager` keeps **one controller per agent, per personality, per user and per workspace
in the instance**: loaded when the first one needs it, shared by all that use it, and held only as
long as one of them holds it. An agent thus has one writer per instance. A personality's controller
is held beside its agent's, not by it - an agent's personality is fixed with the agent, and another
one could make it another kind of agent, which only a new actor can be. The manager also answers
what an actor is called - an entity's current name, or the fixed name of an actor that is none.

A missing record is `StoreError::NotFound`, which a lookup that expects it may be gone turns into
`None`; a channel then leaves that participant out (see [CHANNELS.md](CHANNELS.md)).

### Following changes

Neither the actors nor the sessions subscribe to changes of themselves - a running session follows
only whether its agents are deleted or restored, which decides which actors run in its channels (see
[CHANNELS.md](CHANNELS.md)). The `ControllerManager` takes every announced change to an agent, a
user or a workspace into its live controller, once per instance - the place where changes made in
other instances will arrive (see [Several instances](#several-instances)). An event carries the
resulting value, so taking over an instance's own change again changes nothing; a workspace is read
again as stored instead, since the announcement of a new access list does not carry it. A text in it
that does not fit the model's value is left out and logged.

No change of a personality is announced. Its controller takes in the personality as stored whenever
it is asked for - as an agent of it is resolved to take part, and by the `CommandReaper` - and from
then on every running agent of it answers by that: its limiting roles, who sees its reasoning, its
introduction. Only an agent's components stay as they were built, until its actor is built again.

## Several instances

The server is meant to run in several instances one day
([122](../issues/122-several-instances.md)). The runtime is cut so that this stays possible:

*   **A session runs in exactly one instance**, with its channels, their actors and their
    controllers. Requests for a session go to the instance that runs it.
*   **Agents and users run in several instances at once**, with an actor, a controller and an
    `AgentState` in each. Between instances they are kept in step through the global events, carried
    by an event bus the instances share.
*   **Writes across instances** cannot be serialised by a lock in one process. The shared stores
    will have to refuse a write made on an outdated version, and events will have to carry the
    version they result in, so that an instance skips one older than what it has.
