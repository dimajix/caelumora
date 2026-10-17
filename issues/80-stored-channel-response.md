# 80 — A stored channel is described as not changeable, and the response builders are scattered

**Status:** done
**Area:** User interaction
**Epic:** [74](74-refactoring-epic.md)
**Summary:** A channel that is not running is reported as not changeable and its agents' context as not readable, because the REST server picks and asks the access questions itself, in response builders spread over three modules.

## The problem

`stored_channel_response` (`caelumora-rest-server/src/routes/events.rs:127-140`) answers
`can_change: false` and `can_read_context: false` for a channel that is not running - false instead
of unknown. Both are answerable from the `Channel` document: `can_read_context` is
`may_read_agent_context(viewer, &agent.security_attributes)`, `can_change` is
`may_change_channel(viewer, &channel.security_attributes)`. The running variant `channel_response`
(`routes/channels.rs:117`) asks the actors for what the participants already say (`kind` is the
literal `"agent"`, the `handle` is computed twice identically).

Around it, the response construction is spread over three modules: `channel_response` and
`knowledge_response` in `channels.rs`, `stored_channel_response`, `session_snapshot`,
`describe_participant` and `describe_session` in `events.rs` - which is why `routes::channels` is
the only `pub` route module and `sessions.rs:100` imports its response shape from the events module.
`models.rs` already has the pattern (`From<PublishedNotes>`, `From<SeenItem>`).

Underneath both: the REST server decides for itself which access question goes with which field and
asks the `SecurityManager` directly (`routes/mod.rs:58` `may_read_channel`, used by `channel_of`,
`list_channels` and `describe_session`; `channels.rs:127` `may_change_channel`), while for a stopped
session the same "may read" is decided by the `EntityStore` (`entity_store.get_channel(viewer, id)`,
`events.rs:88`). The same right is checked in two layers depending on whether the channel runs, and
every other client would have to repeat the REST server's choice of questions.

## Why it matters

The web UI hides the "change" and "context" controls for a channel that the viewer may in fact
change. And a builder per module means a new field is added in two places. As long as the frontend
picks the questions, a second frontend (or a runtime component that lists channels) builds its own
version of them, and the next field is forgotten in one of them the way `can_change` was here.

## Considerations

### Where the view of a channel is built

- **In REST, as first proposed.** One `ChannelResponse::describe(&Channel, &dyn SecurityManager,
  viewer)` in `models.rs`, used for stored and running channels alike (the running one through
  `ChannelLoop::channel()`); all `*_response`/`describe_*` move into `models.rs`. Least work, but
  REST keeps asking the `SecurityManager`, and the split between REST (running) and store (stopped)
  for "may read" remains.
- **In the runtime, as a view (chosen, see History).** The model gets a `ChannelView`: the `Channel`
  document as a given viewer sees it, with `can_change` and, per member, `can_read_context`. The
  runtime builds it
  - on the `SessionManager`/`SessionLoop`, not on `ChannelLoop`, so that the entity does not grow
    the policy catalogue issue [71](71-access-helpers-on-entities.md) removed - for running and
    stored channels alike, and only for channels the viewer may read: e.g. `channels_for(viewer,
    session_id)` for lists and snapshots, `readable_channel(viewer, session_id, channel_id)` as the
    counterpart of REST's `channel_of`. REST only maps views to responses (`From<ChannelView> for
    ChannelResponse` in `models.rs`) and no longer knows the `SecurityManager`. A runtime component
    that lists or offers channels uses the same views.
  - Caveat: actor names follow the controller, participant copies do not - irrelevant until agents
    can be renamed.
  - A view is a snapshot. It serves listing and offering; an operation still checks its own
    permission when it acts (no time-of-check/time-of-use decisions on a view).

### What becomes of `AccessQuestions`

Views are answers, not questions: someone still asks. Today's callers of the trait:

| Caller | Question | With views |
|---|---|---|
| REST `routes/mod.rs:59` | `may_read_channel` | asked by the view builder |
| REST `channels.rs:127` | `may_change_channel` | asked by the view builder (`can_change`) |
| `AgentActor::may_read_context` (`agent.rs:1254`) | `may_read_agent_context` | asked by the view builder (`can_read_context`) |
| `ChannelSessionReader::may_read` (`session_reader.rs:147`) | `may_channel_read` | stays - not a view |

The first three are then asked in one place, the view builder, which calls `check(ctx,
ActionType::…)` itself; the trait no longer earns its keep for them. `may_channel_read` is not a
viewer looking at a channel but one channel reading another, in its owner's name, with the rule "a
channel reads itself" - a rule of information flow, asked at the moment the tool reads. It moves to
the runtime next to its only user (the session reader / session module). With that,
`AccessQuestions` is dissolved. The guarantee of its blanket implementation - no policy answers a
question differently from its `ActionType` - holds, because everything still goes through `check`.

Whether `Actor::may_read_context` stays on the trait (whether an actor has a context at all is the
actor's business, issue 71) or the view builder decides it from the participant kind is to be
settled when implementing.

## Done when

- A stored channel reports `can_change`/`can_read_context` from the policy.
- The runtime builds one `ChannelView` per channel and viewer, for running and stored channels
  alike, and decides which channels a viewer may read in both cases.
- The REST server no longer uses the `SecurityManager`; all response builders live in `models.rs`
  and map views; `routes::channels` is private.
- `AccessQuestions` is gone: the viewer questions are asked by the view builder, `may_channel_read`
  lives in the runtime.
- AGENTS.md (*Architecture decisions*) and ARCHITECTURE.md say that what a client may see or be
  offered comes from a view, and that an operation checks its own permission.

## History

- **2026-09-28** — Found in the structure review (`work/2026-09-28-code-review.md`, section 1, error
  6; 5.1.2 and 5.1.3).
- **2026-09-29** — Discussed with the maintainer: the access questions should not be asked by the
  REST server but in the runtime. Decided to build per-viewer views (`ChannelView`) in the runtime,
  used by REST and by runtime components alike, and to dissolve `AccessQuestions` in their favour;
  this revises the decision of issue 71 on where pure questions live. Kept in this issue rather than
  opening a new one. Problem, considerations and "done when" extended accordingly; nothing
  implemented yet.
- **2026-09-29** — Done, except that `AccessQuestions` stays, which the maintainer decided while it
  was being implemented. *Where views are built.* On the `SessionManager`, which alone knows whether
  a session runs, and which has the `RuntimeManager` already: `session_views(ctx)`,
  `session_view(ctx, id)` and `channel_view(ctx, session, channel)`, for running and stopped
  sessions alike and without starting one. The types are `caelumora_model::view` (`SessionView`,
  `ParticipantView`, `ChannelView`, `MemberView`). One private `describe_channel` builds a
  `ChannelView` from the `Channel` document - the running channel's (`ChannelLoop::channel()`) or
  the stored one - so the two cases cannot drift apart again: `can_change` and, per agent,
  `can_read_context` come from the policy for a stopped channel too. Which channels a viewer sees is
  decided there as well (`GetChannel`), for both cases. Names are read in the runtime's name, as
  before (issue 120); a member's name comes from its record, not the actor - the caveat above.
  `Actor::may_read_context` is gone: whether there is a context to read is decided from the
  participant kind, which works without an actor. *Rejected: dissolving `AccessQuestions`.* A first
  version removed the trait and had every caller use `check(ctx, ActionType::…)`. The maintainer
  preferred the named questions in one place to access checks scattered again over the callers; the
  trait stays with all six questions, and the view builder asks `may_read_channel`,
  `may_change_channel` and `may_read_agent_context`. Its doc and AGENTS.md now say that what a
  client is shown or offered comes from a view built with these questions. *The `SessionManager`
  made consistent,* as agreed with the maintainer: methods ending in `_session` act on a running
  session (its `SessionLoop`), those ending in `_view` describe one that may not run. A private
  `find(ctx, id)` answers `Found::Running(loop)` or `Found::Stored(session)` - no `Option`, and no
  unchecked branch: a running session is checked with `GetSession` like a stored one, and "not
  found" is the same answer either way. `open_session` takes the start lock and uses it
  (`load_session` went into it); `delete_session` uses it; `create_session` is private;
  `get_session`, which handed out a running session unchecked, and `running` are gone.
  `stop_session(ctx, id)` takes a context and checks the new `ActionType::StopSession` (the owner's
  alone in `OwnershipSecurityManager`, as a stop affects everybody in the session), and answers
  whether the session ran. Contexts are called `context` throughout.
  `SessionLoop::readable_channels` is gone, its job being the views'. *REST.* It maps views
  (`From<SessionView>`, `From<ChannelView>` and `KnowledgeRefResponse::describe` in `models.rs`);
  `channel_response`, `stored_channel_response`, `describe_session`, `session_snapshot` and
  `describe_participant` are gone, `routes::channels` is private, and no handler uses the
  `SecurityManager` or reads sessions from the store. `GET /sessions` lists `session_views` - more
  than it needs, cheap at today's sizes. Describing a channel no longer starts its session (`GET
  …/channels`, `GET …/channels/{cid}`). A stream's snapshot may be absent (`sse::stream`): a channel
  that can no longer be described to its viewer sends none. The JSON is unchanged; members are now
  listed in the channel's order instead of none in particular. Checked against a running server with
  JSON stores: a session described while running and, after a restart, stopped gives the same
  answer - `can_change` and `can_read_context` set for the owner - in `GET /sessions/{id}`, `GET
  …/channels/{cid}`, the global and the channel snapshot, without starting it; another user gets 404
  and an empty list.

