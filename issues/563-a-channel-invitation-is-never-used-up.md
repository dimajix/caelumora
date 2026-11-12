# 563 — An invitation to a channel is never used up

**Status:** in progress
**Type:** bug
**Priority:** medium
**Area:** Sessions and channels
**Summary:** A personal `Join` entry of a channel - an invitation - stays in its access list when the one it names joins, against the rule of issue 221 that SECURITY.md and CHANNELS.md state, so that someone removed from the channel takes themselves back in by `/channel join`.

## The problem

Issue [221](221-adding-session-participants.md) decided that a personal `Join` entry is an
invitation, used up when the one it names joins, so that one removed later cannot take themselves
back in; an entry for a role, a group or everyone is a rule and stays.
[SECURITY.md](../docs/SECURITY.md#access-lists) and
[CHANNELS.md](../docs/CHANNELS.md#taking-someone-into-the-session) state it for a session and a
channel alike.

Only the session keeps it: `Session::take_in` removes the entry that names whom it takes in.
`ChannelLoop::take_in` stores through `ChannelController::add_participant`, which adds the
participant and leaves the access list as it is - on every way in: joining by oneself
(`ChannelLoop::join`, also from `SessionLoop::join` with a channel), being added, founding. The
test of 221 (`whom_the_access_list_lets_join_takes_themselves_in_and_uses_up_an_invitation`)
checks the session's entry alone.

It was there before, through `POST /sessions/{id}/join` with a channel. Since issue
[560](560-commands-are-not-grouped-by-what-they-act-on.md) it is a line: bob, invited to "open"
and removed from it by its owner, runs `/channel join open` and is in it again.

## Considerations

- **Where the rule lives.** The session's is a method of the domain, `Session::take_in`. The
  channel's would be the same - `Channel::take_in`, also bringing a former participant back
  (issue 164) as `add_participant` does now -, called by `ChannelController::add_participant`
  in the change it stores: one rule, two documents, each its own.
- **Whose context changes the access list.** Changing the attributes is the owner's. One who
  joined is stored in the kernel's name, so the entry goes with it; one added is stored in the
  context of whoever added them, who may change the channel (`UpdateChannel`) - whether the store
  asks more for the access list changing in the same document is to be checked.
- **An agent.** An access list never names an agent (`SecurityAttributes::validate`): an owner's
  invitation lets them bring their own agent in, and `Session::take_in` removes only the entry
  naming whom it takes in, so it stays when the agent joins. The channel would do the same; whether
  bringing one's agent along should use up the owner's invitation is a question for issue
  [342](342-invitations.md), which builds invitations on this rule.
- **Every way in, or joining alone.** The session uses the entry up whoever takes the person in:
  an invitation taken over by being added is no longer needed. The channel follows it.

## Done when

- A personal `Join` entry of a channel is used up when the user it names comes into the channel,
  as the session's is; entries for a role, a group or everyone stay.
- The security suite covers it: invited to a channel, joined, removed, and refused on joining
  again - by `/channel join` and by `SessionLoop::join`.
- CHANNELS.md and SECURITY.md stay true; PERMISSION-CHECKS.md names what changing the entry asks,
  if anything.

## History

- **2026-11-12** — Opened from a finding of the review of issue
  [560](560-commands-are-not-grouped-by-what-they-act-on.md), step 4b: `/channel join` makes a
  channel's invitation that is never used up a way back in for whoever was removed.
- **2026-11-12** — Decided with the maintainer: the channel takes someone in as the session does,
  by a method of its document, `Channel::take_in`, on every way in; their invitation is used up by
  `SecurityAttributes::use_up_invitation`, which `Session::take_in` calls too, so that what tells an
  invitation from a rule stands once, at the access list that holds both. An owner who brings
  their agent along keeps their invitation, as in the session; whether that stays is issue
  [342](342-invitations.md)'s.

  Done: `ChannelController::add_participant` takes the participant in by `Channel::take_in`. The
  store asks nothing new - whoever may change the channel, or the kernel for one who joined, writes
  the access list with it.
