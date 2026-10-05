# 164 — The store forgets what was deleted and who left

**Status:** done
**Area:** Persistence

## The problem

Deleting a session, a channel or an agent removes its document from the `EntityStore`, and removing
a participant from a channel removes it from the channel's document. Afterwards nothing says that
they were there:

- A memory entry's origin (`MemoryOrigin::channel`) and a `KnowledgeRef` can name a channel that is
  gone; the views answer `None` (`Views::channel_of`, [163](163-navigable-views.md)) - "gone" and
  "never there" are one answer, and what the agent read or remembered cannot be told apart from a
  dangling reference.
- An agent removed from a channel is a former member only by the messages it left in the history
  (`ChannelView::member`, `MemberView::former`, issue 163): one that was removed before it said
  anything, or whose messages the viewer may not see, is not found, though its state in the channel
  is still in the `StateStore`.
- Who deleted or removed what, and when, is nowhere ([07](07-session-deletion-audit.md),
  [140](140-audit-log.md)).

## Why it matters

- Auditability: a deletion is the one operation that destroys evidence (07). What an agent did in a
  channel it was removed from - its tool calls, what it saw - should stay readable to whom it is
  readable, and so should the fact that it was removed.
- The views (163) need to say that an edge leads to something that *was* there, not guess it from
  side effects.

## Considerations

- **Mark, do not remove.** A deleted entity stays in the store with a status (`deleted`, by whom,
  when) and is left out of every listing and refused to every read that is not explicitly about what
  was deleted. Likewise a participant that leaves a channel: `Status: Left` in the channel's
  document, with when and by whom, instead of being dropped from `participants`. The views then list
  current members and reach former ones from the document, and `channel_of` can answer "gone".
- **Tombstone versus content.** 07 separates the record that something existed and was deleted from
  its content, which a user may want removed for good. Marking covers the record; whether the
  content stays is 07's question.
- **Where the mark lives.** In the documents (`Session`, `Channel`, `Agent`, `Participant`), since
  the stores are documents; the `StateStore` keeps the agents' states as it does.
- Every place that deletes - `SessionManager::delete_session`, `SessionLoop::delete_channel`,
  `ChannelLoop::remove_participant`, the entity stores' `delete_*` - and every listing changes with
  it.

## Plan (2026-10-05)

Decided by the maintainer: what a user deletes is deleted *softly* - marked, hidden, kept - and a
*hard* delete, here called a purge, is an administrator's act or the result of a retention policy
he configures. Whether something deleted may be restored, and by whom, is the `SecurityManager`'s
decision like any other. This covers sessions, channels and agents with their state (histories,
transcripts, notes), and participants removed from a channel.

### The rules

- **Deleting marks.** The document stays in the `EntityStore` with `deleted: Some(Removal)` - who
  and when. It is the entity's state, like `created_at`, not a protocol: the act itself, and a
  purge, which leaves nothing to mark, belong to the audit log ([140](140-audit-log.md)).
- **What is marked is not there** for every read, listing, update and delete of the store - also
  for the system - so that no caller has to remember to filter. It is reached only by the reads
  that ask for what was deleted (`get_deleted_…`, `list_deleted_…`), with a permission of their
  own.
- **The runtime state stays** until the purge: the `StateStore` is not touched by a delete. It is
  reachable only through its channel or agent, so hiding the document hides it.
- **A channel is reached through its session.** Deleting a session marks the session only; its
  channels are hidden with it, because every path to a channel leads through its session
  (`session.channels`, `Views::channel_of`). So no cascade of marks, and restoring a session brings
  back exactly the channels that were not deleted themselves. Nothing in production may read a
  channel past its session; `EntityStore::list_channels` has no production caller and gets none.
- **Ids are never reused**: `create_…` of an id that is marked is a `Conflict`. Names are free
  again: a deleted channel or agent does not block its name, and restoring one whose name was taken
  meanwhile is refused as the conflict it is.
- **Restoring** takes the mark away and nothing else: a session restored is started when it is
  opened, like any stored session; a channel restored into its session is listed in it again and
  started if the session runs.
- **Purging** removes the document and the state for good - what `delete` does today. Only what
  is marked is purged by a caller (delete first); the runtime undoes a failed creation with it as
  the system.

### Types and traits (`caelumora-model`)

- `Removal { by: ActorId, at: DateTime<Utc> }` (new, `removal.rs`): the mark - of a deleted entity
  and of a participant that left. `by` is `SecurityContext::caller_id`, as the log has it today.
- `trait Stored: Secured { fn deleted(&self) -> Option<&Removal> { None } }` - what the stores
  are generic over, so that every kind is read the same way - and `trait Deletable: Stored { fn
  set_deleted(&mut self, deleted: Option<Removal>); }` for the kinds deleting marks. Traits of
  their own: carrying attributes and being stored are two things, and `Secured` is implemented by
  what is not stored.
- `deleted: Option<Removal>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` on
  the deletable kinds - `Session`, `Channel`, `Agent`, `User`, `UserProfile`, and the definitions
  that are to be managed, `Personality` and `SessionTemplate`. Documents stored before load
  unchanged; no migration.
- **Not a preconfigured component** (`ComponentDescriptor`, decided by the maintainer): it is
  runtime configuration, reached only through the personalities and templates that refer to it by
  `kind: "ref"`, and hidden with them while they are marked - as a channel is with its session. It
  is kept in a `ComponentStore` of its own (see the history), implements `Stored` with no mark, is
  deleted for good as today, and has no restore or purge. A
  purge of a personality does not take its components along: another may refer to them. Deleting
  a component that is still referred to - by a marked personality too - leaves the reference
  dangling, as today; whether deleting it should be refused then is a question of referential
  integrity, not of this issue.
- `EntityKind { Personality, SessionTemplate, Agent, Session, Channel, User, UserProfile,
  Component }` (new) and three actions keyed by it, each carrying the stored attributes:
  `ActionType::ReadDeleted(EntityKind, &SecurityAttributes)`, `Restore(…)`, `Purge(…)`. Per-kind
  variants like `DeleteSession` would be 24 more; the kind as a value is the better shape, and the
  existing per-kind variants could follow it later - named here, not done. `Actions::entity`
  (`store_impl/access.rs`) and `StoreError::NotFound::entity` become the `EntityKind` instead of a
  `&'static str`.
- `Channel::former: Vec<FormerParticipant>` (`#[serde(default)]`) with `FormerParticipant {
  participant: Participant, left: Removal }`. A list of its own rather than a status in
  `participants`: six places read `participants` as "who is in the channel" (`ChannelLoop::new`,
  `participant_attributes`, `require_cleared`, `ChannelView::members`, `shows_approvals`, the
  controller), and `Participant` would change its stored shape. Its `HistoryLimit` is not kept: a
  participant added again is given a new one.
- `AccessQuestions`: `may_restore(context, kind, attributes)` and `may_purge(…)` for the
  capabilities in snapshots (`can_restore`, `can_purge`).
- `GlobalEvent::SessionRestored(SessionId)`, `ChannelRestored(SessionId, ChannelId)`,
  `AgentRestored(AgentId)`, published like their `…Created`. `…Deleted` stays what a delete
  announces. A purge announces nothing: nobody but the trash shows what it removes.

### `EntityStore` (trait, `MemoryStore`, `JsonFileStore`)

Per deletable kind, besides the five operations:

| Operation | asks | does |
|---|---|---|
| `delete_x(ctx, id) -> bool` | `DeleteX`, as today | marks; `false` if nothing there, or marked already |
| `restore_x(ctx, id) -> ()` | `Restore(kind, stored)` | removes the mark; `NotFound` if not marked or not readable |
| `purge_x(ctx, id) -> bool` | `Purge(kind, stored)` | removes the document, marked or not *(changed: only if marked, see the history)* |
| `get_deleted_x(ctx, id) -> X` | `ReadDeleted(kind, stored)` | the marked document; `NotFound` otherwise |
| `list_deleted_x(ctx) -> Vec<X>` | `ListX`, then `ReadDeleted` each | the marked documents |

- `Access` (`store_impl/access.rs`) is bound to `Stored`: `read`, `filter`, `change` and
  `delete` treat a marked entity as absent; new `read_deleted`, `restore`, `purge`. `Actions` gets
  the `kind`.
- `MemoryStore`: `delete_in` sets the mark; new `restore_in`, `purge_in` (today's `delete_in`),
  `get_deleted_in`, `list_deleted_in`.
- `JsonFileStore`: `delete_file` rewrites the document with the mark, at its path - one file per
  id stays true; `purge_file` is today's `delete_file`; `list_dir` and `read_file` filter.
- `find_user_by_name` skips the marked.
- The new operations exist for the seven deletable kinds, not for components: 35 signatures on the
  trait, each one line over the generic helpers.

`StateStore`: `delete_channel_state` stays and is called on a purge only. New
`delete_agent_state(agent_id)` - the agent's own state and its state in every channel - for the
purge of an agent.

### Policy (`OwnershipSecurityManager`)

- `Delete…`: the owner, as today.
- `ReadDeleted`, `Restore`: the owner - whoever deleted something of their own sees it in the
  trash and takes it back.
- `Purge`: the system only, until there is an administrator
  ([125](125-invisible-admin.md)); the retention policy ([07](07-session-deletion-audit.md)) acts
  as the system.
- A policy that wants a user's delete to be final configures the retention to purge at once (07).

### Runtime

- `SessionManager::delete_session(ctx, id)`: as today up to the store - `DeleteSession`, stop the
  loop, `ChannelDeleted` per channel, `SessionDeleted` - but `delete_session` marks, and neither
  the channels' documents nor their state are touched.
- `SessionManager::restore_session(ctx, id) -> Session`: `EntityStore::restore_session`, then
  `SessionRestored`. Not started.
- `SessionManager::purge_session(ctx, id)`: `get_deleted_session` (only what is marked),
  `purge_session`, then as the system `purge_channel` and `delete_channel_state` for every channel
  of `session.channels` and every deleted channel with its `session_id`.
- `SessionLoop::delete_channel(ctx, id)`: as today - out of `session.channels`, loop stopped,
  events - but the document is marked and the state stays.
- `SessionLoop::restore_channel(ctx, id) -> Arc<ChannelLoop>`: `get_deleted_channel`, must be of
  this session; the name must be free (`channel_by_name`); `restore_channel`;
  `SessionController::add_channel`; start the loop as `create_channel` does, from the stored state;
  `ChannelRestored`.
- `SessionLoop::purge_channel(ctx, id)`: `purge_channel`, `delete_channel_state`. *(Not made: a
  channel is purged with its session only; see the history.)*
- `discard_channel` and the rollback in `create_session` purge as the system.
- `ChannelController::remove_participant` moves the participant to `former` with the `Removal`;
  `add_participant` takes it out again. Attributes, history limits and events as today.
- Readers that change by the filter alone, each with its test: `Views::privacy_of` (a deleted
  session is gone: recall by the origin's privacy when written, as for a purged one),
  `template::agent` (a deleted agent's name is not reused but free), `SessionLoop::channel_by_name`.

### Views (`caelumora_runtime::view`)

- `ChannelView::former_members() -> Vec<MemberView>` from `Channel::former`;
  `ChannelView::member(actor)` looks there after `participants`. Finding a former agent by its
  messages stays for channels stored before; users become former members too.
  `MemberView::left() -> Option<&Removal>` replaces the `former` flag.
- `Reached<T> { There(T), Gone(Removal), Never }` (new) is what `Views::channel_of` and
  `Views::agent_of` answer, in place of `Option`. `Gone` only to a viewer who could read it were
  it there (the stored attributes, as the store asks); to everyone else it never was, so nothing
  can be probed. `Views::name_of` names a deleted agent by its stored name, so that old messages
  keep their sender.
- `Views::deleted_sessions()`, `SessionView::deleted_channels()`: the trash, through
  `list_deleted_…` as the viewer.
- Snapshots: `ChannelSnapshot::former_members: Vec<MemberSnapshot>` (with `Detail::MEMBERS`),
  `MemberSnapshot::left: Option<Removal>`; `SessionSnapshot` and `ChannelSnapshot` of the trash
  carry `deleted: Removal`, `can_restore`, `can_purge`.

### REST (`caelumora-rest-server`)

| Route | |
|---|---|
| `DELETE /sessions/{id}`, `DELETE …/channels/{cid}` | unchanged for clients: now soft |
| `GET /sessions?deleted=true`, `GET /sessions/{id}/channels?deleted=true` | the trash, each entry with `deleted`, `can_restore`, `can_purge` |
| `POST /sessions/{id}/restore`, `POST …/channels/{cid}/restore` | restore; `409` if the name is taken, `404` if not deleted or not visible |
| `POST /sessions/{id}/purge` | purge; `403` unless the policy allows. A channel is purged with its session only. |
| `GET …/channels/{cid}` | additionally `former_members`, each with `left` |
| `…/actors/{aid}/history`, `given-up-turns`, `messages/{mid}/context`, `GET …/participants/{aid}` | also for a former member: `member_of` asks `ChannelView::member` |
| `DELETE /agents/{id}`, `POST /agents/{id}/restore`, `POST /agents/{id}/purge`, `GET /agents?deleted=true` | step 6 |

`KnowledgeRefResponse` gets `channel_deleted: bool` from `Reached::Gone`. All additions are
additive for `caelumora-web-ui`. `ApiError::from_runtime` maps the new refusals; a restore blocked
by a name is the existing name conflict.

### Web UI

- The member list gets a folded group "Former members", each with when it left and - where
  `can_read_context` - the history icon, so that the turn an agent gave up when it was removed
  ([146](146-show-given-up-turns.md)) can be read.
- The sidebar gets "Deleted sessions", the channel list "Deleted channels": name, who and when,
  "Restore" where `can_restore`, "Delete for good" where `can_purge`, the latter confirmed.
- `SessionRestored` and `ChannelRestored` on the global stream reload the lists.

### Steps

1. **The mark and the filter** - no behaviour change. `Removal`, `Stored`, `Deletable`, `deleted`
   on the deletable kinds, `EntityKind` in `Actions`; both stores treat a marked document as
   absent. Nothing sets a mark yet; the store suite (`store_impl/suite.rs`) tests it with documents
   marked by hand. STORAGE.md.
2. **The store's operations.** `delete_x` marks; `restore_x`, `purge_x`, `get_deleted_x`,
   `list_deleted_x`; `ActionType::ReadDeleted`, `Restore`, `Purge`, the default policy and
   `AccessQuestions`; `StateStore::delete_agent_state`. In the same step every runtime caller of
   `delete_x` moves to `purge_x`, so that the runtime still deletes for good. Store suite, security
   suite (who may restore, purge and read what was deleted; nothing marked is probed). SECURITY.md,
   STORAGE.md.
3. **Former participants.** `Channel::former`, the controller, `ChannelView::former_members` and
   `member`, `MemberView::left`, the snapshots, REST `member_of` and `former_members`, the web UI's
   group. Independent of steps 1 and 2 except for `Removal`; closes the gap of 146. The test
   `session.rs` "a removed participant is gone" changes: it is former. CHANNELS.md, REST-SERVER.md,
   WEB-UI.md.
4. **Sessions and channels are deleted softly.** `delete_session`, `delete_channel`,
   `restore_…`, `purge_…` as above, the events, the tests that assert that state is gone
   (`session.rs` 782, 1073, 1651; `json.rs` 667) now assert that it stays and goes with the purge.
   Security suite: the acts in someone's name. CHANNELS.md, STORAGE.md.
5. **Reading what was deleted.** `Reached`, `Views::deleted_sessions`,
   `SessionView::deleted_channels`, the REST routes of the trash, restore and purge,
   `channel_deleted`, the web UI's trash. REST-SERVER.md, WEB-UI.md.
6. **Deleting an agent.** There is no way to delete an agent today - no route, no runtime method,
   `AgentDeleted` is never published. `RuntimeManager::delete_agent(ctx, id)`: `DeleteAgent`, mark,
   `AgentDeleted`, and its actor stopped in every running channel
   (`ChannelLoop::stop_participant`, without touching the document: it stays a participant that no
   longer exists, as `ChannelLoop::new` and `GoneMember` already treat one). `restore_agent` starts
   it again where the channel runs; `purge_agent` adds `delete_agent_state`. What it said stays in
   the channels' histories. The REST routes for agents are the first of their kind and can be
   decided when this step is due.

Steps 1 and 2 are the restructuring, 3 to 6 the features; each is a commit of its own. The
retention policy and the purge by it are [07](07-session-deletion-audit.md)'s, the administrator's
rights [125](125-invisible-admin.md)'s, the record of the acts [140](140-audit-log.md)'s.

### Not covered

- **Removing one entry of a chain** - a message from a history or a transcript. Purging a whole
  channel removes its chains; removing one entry would break every digest after it and needs
  redaction (the content goes, the entry's digest stays). Noted in 07.
- **Memory entries written in a purged channel** stay the agent's: [19](19-memory-tiers.md).
- **Participants of a session** (`Session::participants`) are not removed anywhere yet; a `former`
  list there follows the channel's when they are.

## Done when

- Deleting a session, a channel or an agent and removing a participant leave a mark with who and
  when, and nothing is removed from the store.
- Listings and reads leave out what is marked deleted, unless asked for what was.
- The views reach a former member from the channel's document and tell a gone channel from one that
  never was.
- What was deleted can be listed, restored and purged, each as the `SecurityManager` allows; a
  purge removes the document and its state.

## History

- **2026-10-01** — Opened by the maintainer in step 6 of [163](163-navigable-views.md): a `None` for
  a gone channel and a former member known by its messages are what the views can do until the store
  keeps what was deleted. Plays into [07](07-session-deletion-audit.md).
- **2026-10-05** — From [146](146-show-given-up-turns.md): the turn an agent gave up because it
  was removed is recorded, but the web UI offers an agent's history and given-up turns for members
  only, so it cannot be shown once the agent has left - exactly when it would be asked for. A former
  member known to the views would make it reachable.
- **2026-10-05** — Analysed and planned. Three things were found mixed in this issue: former
  participants, deleted entities, and who did it. A first proposal kept only the former participants
  here and left deleted entities to 07 and 140, because a mark keeps the content a user wants gone.
  The maintainer resolved that the other way: a user's delete is soft, a hard delete - a purge - is
  the administrator's or a retention policy's, and who may restore is the `SecurityManager`'s
  decision. With that, marking is right, and the section *Plan* says how: the mark and what the
  stores do with it, former participants as a list of their own, restore and purge, the views and
  the REST API, in six steps. Decided along the way: no cascade of marks from a session to its
  channels (a channel is only reached through its session); the mark carries who and when also for
  a participant that left, since it is the entity's state - the acts themselves stay 140's.
- **2026-10-05** — Step 1 done: the mark and the filter, without a change in behaviour.
  `caelumora_model::removal` with `Removal`, `Stored` and `Deletable` (both implemented by
  `impl_deletable!`); `deleted: Option<Removal>` on the seven deletable kinds, written only when
  set. `Access` is generic over `Stored` and treats a marked document as absent in `read`,
  `filter`, `change` and
  `delete`; both stores go through it, `find_user_by_name` through `list`. `Actions::entity` is an
  `EntityKind`. A suite test that both stores run (`marked_documents_are_absent`) marks documents
  by hand (`MarkByHand`), since nothing marks yet. STORAGE.md, section *Deleted entities*.

  Two departures from the plan. `StoreError::NotFound::entity` stays a `&'static str`: it also names
  what is no entity kind - a participant, a channel state - so only `Actions` carries the
  `EntityKind`, and errors take its `name()`. And creating or changing a document that carries the
  mark is refused (`Validation`): otherwise `update_x` with a mark set would delete without asking
  `DeleteX`, and with the mark taken away would restore without asking `Restore`.

  A first version also marked preconfigured components, for every kind to have the same
  operations. The maintainer decided against it: a component is runtime configuration reached only
  through what refers to it, so the trait was split into `Stored` (asked of every kind) and
  `Deletable` (the kinds deleting marks); see *Types and traits*.
- **2026-10-05** — A restructuring after step 1, at the maintainer's request and without a change in
  behaviour: preconfigured components moved out of the `EntityStore` into a `ComponentStore` of
  their own (`caelumora_model::store`), beside it as the `StateStore` is. With the entities all
  deletable but one, the component was the exception in the `EntityStore`; now each store's kinds
  have the same operations. The store keeps a `StoredComponent` - the `ComponentDescriptor`
  flattened, with `security_attributes` beside it - so that the descriptor is configuration only and
  lost the attributes it carried in every personality and template for nothing. The stored format
  is unchanged. `MemoryStore` and `JsonFileStore` implement the new trait too;
  `AppConfig::create_component_store` opens it where the entity store is (no configuration of its
  own), and `ComponentRegistry` takes it instead of the `EntityStore`. The store suite runs
  components in `component_store_behaves`. STORAGE.md, COMPONENTS.md, ARCHITECTURE.md,
  CONFIGURATION.md.
- **2026-10-05** — Steps 2 and 4 done together, at the maintainer's decision. Step 2 alone was to
  change no behaviour, so the runtime would have had to delete and then purge - a purge hidden in
  every delete. Together, deleting marks and purging is an operation of its own from the start.
  - **Store:** `delete_x` marks (`Removal` by the caller, now) and keeps the document; for the
    seven deletable kinds `restore_x`, `purge_x`, `get_deleted_x` and `list_deleted_x`, generic over
    `Deletable` in both stores (`delete_in`/`delete_file` mark, `restore_…`, `purge_…`,
    `get_deleted_…`, `list_deleted_…`; `remove_in`/`remove_file` delete a component for good).
    `Access` asks `ReadDeleted` for the trash and for restoring, `Purge` for purging - after
    whether the caller may see the entity, as it is or as deleted. `StateStore::delete_agent_state`
    for the purge of an agent (step 6).
  - **Policy:** `ActionType::ReadDeleted`, `Restore` and `Purge`, each with the `EntityKind` and the
    stored attributes; the owner reads and restores, purging is the system's alone.
    `AccessQuestions` has `may_restore` and `may_purge` for the snapshots of step 5.
  - **Runtime:** `SessionManager::delete_session` marks the session only; channels and state stay.
    `restore_session` takes the mark away and does not start it; `purge_session` purges only what
    was deleted, and with it - in the runtime's name - the channels it listed and those deleted
    from it before, with their state. `SessionLoop::delete_channel` marks; `restore_channel` asks
    `UpdateSession`, lists and starts it again (`add_channel_loop`, shared with `create_channel`),
    and refuses a name taken meanwhile. `GlobalEvent::SessionRestored` and `ChannelRestored`; a
    purge announces nothing. The rollbacks of a failed creation purge in the runtime's name.
  - **A channel is purged with its session only**, decided by the maintainer: purging touches the
    stores alone and needs no running loop, so it is not the `SessionLoop`'s, and `purge_session`
    already purges its channels - the document through the store, the state through the
    `StateStore`. A channel deleted from a session that stays stays in the trash until the session
    is purged. Should the retention policy ([07](07-session-deletion-audit.md)) or a route need to
    purge a single channel, it is a method of the `SessionManager`, not of the loop.
  - **Beyond the plan:** `restore_channel` checks its participants' clearance again
    (`require_cleared`), as `create_channel` does - their roles may have changed while it was
    deleted. Found while writing its test in the security suite.
  - **Tests:** the store suite deletes for real now (`deleting_marks`; `MarkByHand` is gone), a
    policy test in `store_impl::access`, the session tests that asserted that state is gone now
    assert that it stays until the purge, and the security suite has restoring and purging a
    session and a channel. SECURITY.md, STORAGE.md, CHANNELS.md, REST-SERVER.md.

  The readers the plan named to change by the filter alone - `Views::privacy_of`,
  `template::agent`, `SessionLoop::channel_by_name` - do not change in behaviour: a deleted session
  was gone for `privacy_of` before too, agents are deleted only from step 6 on, and
  `channel_by_name` looks at the running channels. They get no tests of their own here.

  A test helper in `agent_memory` that makes a session disappear now purges it instead of deleting
  it: it may create the session again under the same id, which a marked id refuses.

  **Purging only what is marked.** As planned, `purge_x` removed a document marked or not, for the
  cascade to the channels of a deleted session, which are hidden with it but not marked. The
  runtime checked "only what was deleted" before (`get_deleted_x`), so a restore in between would
  have been purged. Changed, as the maintainer agreed: `purge_x` purges only what is marked -
  `Access::purge` asks `ReadDeleted`, then `Purge` - and removes it only if it is still marked when
  it does (`MemoryStore` under its lock; `JsonFileStore` by reading the file once more, which
  narrows the window but cannot close it). The guarantee is the runtime's: `restore_session` takes
  the lock `delete_session` and `purge_session` hold. The cascade and the rollbacks of a failed
  creation mark first and purge then, in the runtime's name (`purge_channel_document`). A purge
  that removes nothing after all ends as `NotFound`, without touching the channels.
- **2026-10-05** — Step 3 done: former participants.
  - **Model:** `Channel::former: Vec<FormerParticipant>` (`participant`, `left: Removal`), written
    only when not empty. `ChannelController::remove_participant` moves the participant there, with
    the caller and the time; `add_participant` takes it out again. Nothing that reads
    `participants` changed.
  - **Views:** `ChannelView::former_members`, and `member` looks there after the participants; the
    fallback by messages stays for agents that left before. `MemberView` keeps `former()` and has
    `left()` - `None` for that fallback, whose removal is not known. The snapshot carries
    `former_members` (with `Detail::MEMBERS`, without the other details) and `MemberSnapshot::left`.
  - **REST:** `ChannelResponse::former_members`, `ChannelMemberResponse::left`; `member_of` finds
    former members, so a removed agent's history, given-up turns and contexts, and
    `GET …/participants/{aid}`, answer for it. This closes the gap noted from
    [146](146-show-given-up-turns.md): the turn an agent gave up when it was removed can be read.
  - **Web UI:** a folded group "Former members" below the member list, with when each left and the
    history icon where `can_read_context`.
  - **Tests:** the session test of removing a participant asserts the former entry and its removal
    on adding again; a view test reaches a former user and agent that never said anything; a REST
    test of `former_members`. CHANNELS.md, REST-SERVER.md, WEB-UI.md.

  Not done as planned: `MemberView::left()` does not replace the `former` flag - a former member
  found by its messages has left, but nobody recorded when.
- **2026-10-05** — Step 5 done: reading what was deleted.
  - **Views:** `Reached<T> { There, Gone(Removal), Never }` is what `Views::channel_of` answers:
    gone if the channel or its session was deleted and the viewer could read it were it there
    (`GetChannel`, `GetSession` on the stored attributes, looked up in the runtime's name), never
    there for anyone else. `Views::deleted_sessions` and `SessionView::deleted_channels` are the
    trash, read through `list_deleted_…` as the viewer; a channel may be restored by whoever may
    restore it and change its session.
  - **REST:** `GET /sessions?deleted=true` and `GET …/channels?deleted=true` (`DeletedResponse`:
    `id`, `name`, `deleted`, `can_restore`, `can_purge`), `POST /sessions/{id}/restore`,
    `POST /sessions/{id}/purge`, `POST …/channels/{cid}/restore`; `KnowledgeRefResponse` has
    `channel_deleted`.
  - **Web UI:** `DeletedList`, folded under the sessions and in every unfolded session, with
    "Restore" and "Delete for good" (confirmed by a second click) as the server allows, reloaded on
    the delete and restore events. The context panel now tells a deleted channel from one the user
    cannot see; before, it called every unnamed channel of the session deleted.
  - **Tests:** a view test of the trash, the view test of `channel_of` for gone and never, a REST
    test of `DeletedResponse`. REST-SERVER.md, WEB-UI.md, AGENT-TURN.md.

  Departures from the plan. The trash has an entry of its own, `DeletedSnapshot<I>` (sessions and
  channels alike), instead of `deleted`, `can_restore` and `can_purge` on `SessionSnapshot` and
  `ChannelSnapshot`: an entry of the trash needs none of the snapshots' details, and the fields
  would have been optional on two types. `Views::agent_of` and `name_of` stay as they are until
  step 6, since no agent can be deleted before; there a deleted agent is to be named by its stored
  name.
- **2026-10-05** — Step 6 done, and with it the issue: deleting an agent.
  - **Decided by the maintainer:** a deleted agent leaves every channel it takes part in, as a
    former participant (option b), rather than staying a participant without an actor; a restore
    brings it back into none. The REST routes come now, the web UI only as an action in the
    member list.
  - **Runtime:** `SessionManager::delete_agent`, `restore_agent`, `purge_agent` - on the
    `SessionManager`, not the `RuntimeManager` as planned, since only it knows the running
    sessions, and an event must not change a channel. Deleting asks `DeleteAgent` first, then
    removes the agent from the channels of every session it takes part in, in the runtime's name -
    a running channel through `ChannelLoop::remove_participant`, a stopped one through a channel
    controller of its own, under the session's start lock - then marks it and announces
    `AgentDeleted`. Restoring is refused if another agent took its name in its scope, and announces
    the new `AgentRestored`; purging removes its document and `delete_agent_state`.
  - **Views:** a deleted agent keeps its name (`Views::deleted_agent_name`, `GoneMember` with a
    name, no handle); `Views::agents` and `deleted_agents`; `MemberView::may_delete` and
    `MemberSnapshot::can_delete` (`AccessQuestions::may_delete_agent`).
  - **REST:** `GET /agents` (`?deleted=true`), `DELETE /agents/{id}`, `POST /agents/{id}/restore`
    and `/purge`; `Subject::Agent`, `agent_not_found`, and a `StoreError::Conflict` is now `409
    already_exists`. `ChannelMemberResponse::can_delete`.
  - **Web UI:** a delete button for agents in the member list, confirmed by a second click.
  - **Tests:** deleting, restoring, a name taken meanwhile and purging in `session.rs`, with a
    running and a stopped channel; the security suite for all three.
    CHANNELS.md, REST-SERVER.md, WEB-UI.md, SECURITY.md.

  Left open: the agent's controller is shared by its running actors (`ControllerManager`); until
  the last of them has ended after the deletion, adding the agent to a channel again could still
  find it. Narrow, and not worth a mechanism yet. A session's own participants
  (`Session::participants`) still list a deleted agent; nothing removes participants from sessions
  yet. A web UI for the trash of agents, and for agents at all, waits for agent management.
