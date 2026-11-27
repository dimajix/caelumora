# 610 — A channel made by `/channel create` has no approver

**Status:** done
**Type:** bug
**Priority:** high
**Area:** Tools and approvals
**Summary:** `/channel create` makes its channel of `ChannelSpec::new(name)`, which names no internal actors, so the new channel has no approver, and an agent that calls a tool needing approval in it waits for a decision nobody can give, without a timeout and without anyone being asked.

## The problem

`CreateCommand::execute` (`caelumora-kernel/src/command/channel/create.rs`) creates the channel
of `ChannelSpec::new(name)` alone. `ChannelSpec::new` names no internal actors, and since issue
[420](420-channel-built-empty.md) `Channel::build` makes none of its own: what is not configured
is not there. The channel created so has neither an approver nor a moderator component; its
document says `"internal_actors": []` and `"components": []`.

An agent in it that calls a tool needing approval records `ApprovalRequested` to the approvers
(`recipients: approvers`). The channel has none, so nobody receives it: no user is asked, since
only the approver puts a request before the users, and no timeout runs, since the ten minutes
after which a request nobody decides times out are the approver's. The agent's turn stays in
`ExecutingTools`, and whatever is said to the agent afterwards waits behind that turn.

Seen in the terminal: in a channel `private` made by `/channel create private`, the agent `supi`,
asked to start a sub-agent, called `list_files` on `.`; the request was recorded, nothing
answered it, and the agent did not answer the question of what it was doing either. In the
session's `main` channel, which the TUI's default template gives an approver
(`caelumora-tui/defaults/default.yaml`), the approver would put the same call before the user.

The other ways to make a channel give it one: the session templates of the seed and of the TUI
name an `Approver` for each of their channels, and the web UI's dialog sends
`internal_actors: [{Approver: {strategy: 'Any'}}]` to the REST route
(`caelumora-web-ui/src/components/NewChannelDialog.tsx`). The command is the one way that makes a
channel in which no tool needing approval works, and it does not say so.

## Considerations

Issue 420 decided that internal actors are configured, with no fallback in `Channel::build`, and
that a channel without an approver approves nothing. Neither option below reopens that: the one
who makes the channel says what it has.

1. **The command names an approver, as the web UI's dialog does.** `/channel create` makes its
   channel of `ChannelSpec::new(name)` with one `InternalActorTemplate::Approver` deciding by
   `Any`. Small, the same choice the web UI's dialog makes, and `ChannelSpec::new` and the REST
   route keep saying "none unless named". The strategy is fixed in the command; a frontend that
   wants another sends its own spec.
2. **The command takes the session's choice.** The new channel gets the internal actors of the
   session's main channel, or of a template for new channels the session's template names.
   Follows what the session was made for, but needs a place for that choice, which nothing asks
   for yet beside this command.

Not part of this issue, though related: an agent that asks for approval in a channel without an
approver still waits for ever. Answering such a call at once, as issue
[118](118-malformed-call-put-up-for-approval.md) does for a call that cannot succeed, would end
the wait in any channel made without one - over REST, by a template - and is a question of its
own.

Recommended: option 1.

## Done when

- A channel made by `/channel create` has an approver, and a tool call needing approval in it is
  put before its users as in a channel made from a template.
- A test of the command holds it.
- `docs/COMMANDS.md` says of `/channel create` that the channel has an approver.

## History

- **2026-11-26** — Opened after an agent in a channel made by `/channel create` in the terminal
  waited for the approval of a `list_files` call that no approver received.
- **2026-11-27** — Done by option 1, as the maintainer decided. `/channel create` makes its
  channel of `ChannelSpec::new(name)` with one `InternalActorTemplate::Approver` deciding by `Any`,
  as the web UI's dialog asks for one; `ChannelSpec::new` and the REST route still name none
  unless asked to. A test of the command finds the approver stored with the channel and its actor
  running, which puts a call needing approval before the users as in a channel made from a
  template; `/help`, `docs/COMMANDS.md` and `docs/CHANNELS.md` say the channel has an approver.
  An agent asking for approval in a channel made without one - over REST, by a template - still
  waits, as the issue leaves it.
