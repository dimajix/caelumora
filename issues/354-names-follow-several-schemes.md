# 354 — Names around sessions, channels and actors follow several schemes

**Status:** open
**Area:** Code structure

## The problem

The same kind of thing is named differently from type to type, and one word means three things:

- **Managers.** `SessionManager` says `open_session`, `stop_session`, `delete_session`,
  `restore_session`, `purge_session`, `create_session`; `AgentManager`, `GroupManager` and
  `WorkspaceManager` say `create`, `delete`, `restore`, `purge`. `SessionManager::find` and
  `running` both answer "the session, if …".
- **Finding by id and name.** `SessionLoop::channel_by_id`, `channel_by_name`, `channel_for`;
  `ChannelLoop::actor_by_id`, `user_actor`, `agents_named`, `participants_named`.
- **"Member", "participant", "actor".** The model's `Participant` is the reference in a document,
  an `Actor` is what runs; between them the runtime has `channel::Member` - what an actor is built
  from (`channel/member.rs:25`) - and `ChannelMember`, the snapshot of a running participant
  (`channel/mod.rs:62`), and the methods `members()`, `actor_ids()`, `participants_named()` on one
  type.
- **Single methods.** `ChannelLoop::put` (`channel/participants.rs:327`) puts questions to the
  users, which the name does not say; `SessionLoop::set_workspace` beside `update_name`,
  `update_privacy`, `update_description`; `ApproverActor::speaking` and
  `ExternalAgentActor::speaking_for` beside `Answering`; the steps of the `AgentActor` are
  `handle_waiting`, `handle_llm_turn`, `handle_busy`, `handle_executing_tools`,
  `handle_waiting_for_user`, but `Reflecting` and `Cancelling` are `end_turn`.
- **The kind of an actor is a string.** `Actor::kind() -> String` (`actor/mod.rs:33`) and
  `ChannelMember::kind: String` take one of four values - `user`, `agent`, `external_agent`,
  `approver` - and the tests compare strings.
- **Undoing.** `discard_channel`, `keep_channel` (`session/channels.rs:327`, `:339`), `take_back`
  (`session/manager.rs:244`) and the inline undo blocks are the same step - "stored, but failed:
  taken back in the runtime's name" - under three names.

## Why it matters

A reader who learnt `AgentManager::delete` looks for `SessionManager::delete`; one who met `Member`
in the channel meets `ChannelMember` over REST and `members()` returning the latter. Names are the
cheapest part of the API to keep consistent and the most visible.

## Considerations

- Managers: `create`, `open`, `stop`, `delete`, `restore`, `purge` on `SessionManager`; `find` and
  `running` kept apart by what they answer, or `running` folded into `find`.
- Finders: `channel(id)`, `channel_named(name)`, `channel_for(ctx, id)` on the session;
  `actor(id)`, `user_actor(id)`, `agents_named`, `participants_named` on the channel - "by id" is
  the plain name, "named" the lookup by name, "for" the one asked in someone's name.
- The internal `Member` says what it is: what an actor is built from, e.g. `ActorSource`; then
  `members()` and `member_ids()` are one pair on `ChannelMember`, and `actor_ids()` goes. To be
  decided with [356](356-member-questions-threaded-with-their-services.md), which touches the type.
- `put` → `address_questions`; `set_workspace` → `update_workspace`; `speaking`/`speaking_for` →
  an `Answering` constructor; the actor's steps named after their state, `handle_reflecting` and
  `handle_cancelling` or `end_turn` for both with the state as the argument it already has.
- `ActorKind` as an enum in the model with `Display` and `serde` giving the four strings as today,
  so that REST and the web UI see no change.
- One name for the undo step, e.g. `take_back_channel`, `take_back_session`.

## Done when

- The names above follow one scheme each, written down in CODING-GUIDELINES.md where it is a rule
  (the manager verbs, the finders).
- Done last in the epic's order, in a commit of its own, so that the renames touch nothing that
  another refactoring moves.

## History

- **2026-10-14** — From the review of agents, sessions and channels, epic
  [349](349-core-review-epic.md).
- **2026-10-14** — With [356](356-member-questions-threaded-with-their-services.md) the question
  who takes part in a channel got an owner of its own, named `Participation`
  (`channel/participation.rs`, formerly `channel/member.rs`), so that "membership" is left to the
  groups (`Memberships`, `StoreMemberships`). The rest of the internal "member" was agreed with the
  maintainer and left here, so that the scheme is decided once, with `members()`, `actor_ids()` and
  `ChannelMember`: `Member` → `ActorSource` (what an actor is built from, with the place it takes
  part in), `MemberKind` → `SourceKind`, `Participation::member_of` and `ChannelLoop::member_of` →
  `source_of`, `ChannelActor::member` → `source`. `ChannelMember`, `MemberView` and `members()`
  are what REST and the web UI see and are decided here as the consideration above says.
  `SourceKind` has the four kinds of the `ActorKind` proposed above, one variant each; the latter
  could be read off the former.
