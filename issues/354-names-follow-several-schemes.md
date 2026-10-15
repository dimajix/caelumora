# 354 — Names around sessions, channels and actors follow several schemes

**Status:** done
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
- **2026-10-15** — Done, in a commit of its own, with the maintainer's decisions on the code as it
  had become:
  - `SessionManager`: `create`, `open`, `stop`, `delete`, `restore`, `purge`, as the other
    managers. `find` and `running` stay apart: `find` asks whether the caller may read the session
    and answers it running or stored, `running` is the runtime's own look at what runs.
  - Finders: `SessionLoop::channel(id)`, `channel_named`, `channel_for`; `ChannelLoop::actor(id)`;
    `user_actor`, `agents_named` and `participants_named` stayed as they were.
  - `Member` → `ActorSource`, `MemberKind` → `SourceKind`, `Participation::member_of` and
    `ChannelLoop::member_of` → `source_of`, `ChannelActor::member` → `source`,
    `SessionLoop::member_to_take_in` → `source_to_take_in`, `start_member` → `start_source`.
    `ChannelMember`, `members()` and `MemberView` stay - "member" is what the views and REST call
    a participant of a channel as it is now -, and `actor_ids()` became `member_ids()`, their pair.
  - The kind of an actor as a string is gone instead of becoming an enum: nothing but two tests
    read `Actor::kind()` or `ChannelMember::kind`, and REST tells the kinds apart by the id
    (`ParticipantKind::of`). The tests ask the `ActorId`.
  - `ChannelLoop::put` → `address_questions`, `SessionLoop::set_workspace` → `update_workspace`.
  - `ExternalAgentActor::speaking_for` became `Answering::continuing(&Work)`, which
    `Turn::answering` uses as well. The external agent continued the work's contexts once more
    (`EventContext::next`), so that what came of a task later answered a template id no event
    carries (since 352), one level deeper in the span; it now answers the request, as a turn does.
  - The agent's steps: `handle_llm_turn` → `handle_planning`; `Reflecting` and `Cancelling` have
    a step each, `handle_reflecting` and `handle_cancelling`, over the shared `end_turn` - the
    former is where an agent would one day do something once a turn succeeded (keep what is worth
    remembering), which a cancelled one does not.
  - The undo step is a rollback, not an undo - what failed half-way is set back to how it was
    before, and "taken back" already means a message revoked: `SessionManager::roll_back_creation`
    (`take_back`), `SessionLoop::roll_back_channel_creation` (`discard_channel`) and
    `roll_back_channel_deletion` (`keep_channel`); the inline rollbacks say so in their comments.
  - CODING-GUIDELINES.md names the manager verbs, the finders and rolling back.
- **2026-10-15** — `ActorSource` renamed again, at the maintainer's request: "source" named one of
  its three uses - building the actor - and not the others, the questions whether it may take part
  and the participant kept with its running actor. It is the participant a document names
  (`Participant`), resolved in this instance and in its place: `ResolvedParticipant`, with
  `ResolvedKind`; `Participation::resolve` and `ChannelLoop::resolve` (`source_of`),
  `SessionLoop::resolve_to_take_in` (`source_to_take_in`), `start_participant` (`start_source`),
  `ChannelActor::participant` (`source`).
