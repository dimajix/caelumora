# 356 — The questions on a channel's member are asked with their place and their services threaded in each time

**Status:** done
**Area:** Code structure
**Epic:** [349](349-core-review-epic.md)
**Summary:** The questions on a channel's member take its session and channel and four services as parameters each time, so both loops hold the services and one method needs a clippy exception.

## The problem

`Member` (`channel/member.rs:25`) is what a participant's actor is built from, and the questions of
whether it may take part are its methods. Each takes again what `Member::of` was given:

- `Member::of(participant, session, channel, agents, controllers, security)` - six parameters
  (`channel/member.rs:45`);
- `context(session, channel)`, `clearances(session, channel)`, `require_in_scope(session, channel)`,
  `require_cleared(confidentiality, context, classification, session, channel)` - the same ids every
  time;
- `require_joinable(security, context, kind, attributes, session, channel, refusal)` - seven
  parameters, with an `#[expect(clippy::too_many_arguments)]` saying a bundling type "would serve
  this one method" (`:107`).

The four services the questions need - the agents' store, the `ControllerManager`, the
`SecurityManager`, the `ConfidentialityPolicy` - are assembled around them at five places:
`ChannelLoop::member_of` (`channel/mod.rs:355`), `SessionLoop::member_to_take_in`
(`session/participants.rs:130`), `require_admitted` taking the whole `RuntimeManager`
(`channel/member.rs:167`), and the four fields each of `ChannelLoop` and `SessionLoop` that exist
for them. `member_to_take_in` passes `ChannelId::default()` when no channel is named
(`session/participants.rs:100`, `:131`).

[229](229-services-fetched-through-the-runtime.md) named this: "`Member::of` and `require_cleared`
take what they need from one argument, not three threaded in next to it - or become methods of the
object that holds them."

## Why it matters

Who may take part in a channel is one question with one set of services, asked when a channel is
created, restored, joined, and when someone is added or an agent comes back. Today it is asked with
the services of whoever asks, so each caller holds them and the question has no place that could be
tested alone. The `clippy` exception is the symptom.

## Considerations

Two steps, the first cheap and independent:

1. **`Member` knows its place.** `Member::of` stores `session_id` and `channel_id` (the latter as
   `Option<ChannelId>`, so that the session's "no channel named" is not a default id); `context`,
   `clearances`, `require_in_scope`, `require_cleared` and `require_joinable` lose two parameters
   each, and the `expect` goes.
2. **A type owns the question.** `Membership { agents, controllers, security, confidentiality }` in
   `channel::member`, with `member_of(participant, session, channel) -> Result<Option<Member>>` and
   `require_admitted(ctx, &Channel)`; `Member::require_cleared` takes the policy from it, or
   `Membership` offers the `require_*` for a member. `ChannelLoop` and `SessionLoop` hold one
   `Membership` instead of four service fields; `require_admitted` no longer takes the
   `RuntimeManager`. The one sentence of the module - "who takes part in a channel: a participant
   with what its actor is built from, and whether it may take part" - is the type's; it bundles no
   parameters, it owns the services the question needs, as `ToolExecutor` does for tool calls.
   Alternatively the `ControllerManager` offers `member_of`, since it holds three of the four
   services already - but its sentence is "the controllers of one instance", and the agents' store
   and the policy would join it for this alone. An architecture decision, to be discussed first.
- The double check when someone is taken into the session and a channel at once -
  `member_to_take_in` asks scope and clearance, then `ChannelLoop::take_in` asks both again - stays:
  the first is "everything before anything is stored", the second the channel's own, as
  CHANNELS.md says. With one `Membership` both are one call.

## Done when

- No question on `Member` takes the session or channel id, and none takes a service it could be
  given once.
- `require_admitted` takes no `RuntimeManager`; `ChannelLoop` and `SessionLoop` hold the services of
  membership once.
- No behaviour changes; the admission tests (`channel/member.rs`, `channel/participants.rs`,
  `session/participants.rs`) pass unchanged.

## History

- **2026-10-14** — From the review of agents, sessions and channels, epic
  [349](349-core-review-epic.md).
- **2026-10-14** — Step 1 done, in the form decided with the maintainer: `Member` is a struct of
  what its actor is built from, the former enum now `MemberKind`, and its place - `session_id` and
  `channel_id: Option<ChannelId>`. `context`, `clearances`, `require_in_scope`, `require_cleared`
  and `require_joinable` take no ids any more, and the `clippy` exception is gone. The default id
  for "no channel" is made in one place, `Member::context`, since a `SecurityContext` names no
  channel as that. `require_in_scope` and `require_cleared` are questions of a channel; a member of
  the session alone answers them with nothing, as they are not asked of it today. Weighed and not
  taken: the place as a `Scope`, which also has kinds that are no place of a member and would have
  to be answered at every reading. The personality check of `Member::of` became
  `require_runnable`. Step 2 (`Membership`) is open.
- **2026-10-14** — Step 2 done, as Option 1 decided with the maintainer, under the name
  `Participation` rather than `Membership`: "membership" stays the groups' word (`Memberships`,
  `StoreMemberships`), and a channel's is "participant", as everywhere else; the module is
  `channel::participation`. `Participation` holds the agents' store, the `ControllerManager`, the
  `SecurityManager` and the `ConfidentialityPolicy`, and owns the questions that need them -
  `member_of` (the former `Member::of`), `agent_exists`, `require_joinable`, `require_cleared`,
  `require_admitted` and `require_admissible`, scope and clearance together as three callers asked
  them. `Member` keeps what needs no service: `kind`, `actor_id`, its context and clearances, and
  the scope check behind `require_admissible`. The `RuntimeManager` holds one `Participation`;
  `ChannelRuntime` offers it in place of the entity stores, which the channel read only for the
  agents' store. `ChannelLoop` holds it in place of its `agents` field, `SessionLoop` and
  `SessionManager` hold it in addition, and `require_admitted` no longer takes the `RuntimeManager`.
  Unlike "Done when" says, the loops keep their `security`, `confidentiality` and `controllers`:
  they use them for their own checks as well (messages, reclassifying, reading a channel,
  workspaces), so only `ChannelLoop::agents` goes. Weighed and not taken: the questions on the
  `ControllerManager`, which holds all four services already but whose sentence would gain an "and
  also" and which would depend on `channel`; and a `Member` carrying its services, which makes a
  value kept per actor heavy and still needs a second place to find members. Renaming `Member`
  itself is left to [354](354-names-follow-several-schemes.md).
