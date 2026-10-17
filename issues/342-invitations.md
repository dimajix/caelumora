# 342 — Nobody can be invited to a session or channel: who is added is taken in without being asked

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** Nobody can be invited to a session or channel: who is added is taken in without being asked, told nothing beforehand and unable to decline.

## The problem

With issue [221](221-adding-session-participants.md), whoever may change a session takes someone
in directly, in one call: a user, or an agent through its owner, joins the session and a channel
without being asked. Nobody is told beforehand, nobody can decline, and a person finds themselves
in a conversation they never agreed to - with what they say there readable by its other members,
as far as the session's privacy (issue [134](134-session-privacy.md)) and `history` let them.

## Why it matters

- Joining a conversation is a decision of the one who joins. A private session or channel should
  be entered on invitation, not by being put into it.
- An invitation needs a state - sent, accepted, declined - and somebody must be told of it, though
  they cannot read the session yet.

## Considerations

221 decided the model this builds on (see its history of 2026-10-13):

- **Who may join is an access list.** A session's and a channel's `SecurityAttributes` hold `Join`
  entries: `{ Everyone, Join }` for one anybody may join, `{ Role(…), Join }` or
  `{ Group(…), Join }` for those who may join by themselves, `{ User(…), Join }` for one person.
  `Join` grants no reading.
- **An invitation is a personal `Join` entry.** Inviting adds `{ User(bob), Join }`; it is used up
  when bob joins, so that one removed later cannot take themselves back in. Entries for a role, a
  group or everybody are a rule, not an invitation, and stay.

What this issue adds on top:

- **Inviting** without taking in: the entry is added (`UpdateSession` / `UpdateChannel`), and
  nothing else - 221's single call adds the entry and takes the person in at once.
- **The invitation event**, delivered to the invited user although they may not read the session:
  published with attributes that name them for the delivery alone, as a channel announces the
  leave of a removed participant. It says what the invitee may see before deciding - the name and
  summary of the session or channel, who invites - and nothing of its history.
- **Accepting** is joining by oneself: `JOIN` asked in the invitee's own context, the clearance for
  the channel at the door, as 221 builds it.
- **Declining** removes the invitee's own entry - an operation of its own, allowed only for the
  entry that names the caller, since changing the access list is otherwise the owner's.
- **What an invitee sees**: a view of their open invitations - the sessions and channels whose
  access lists hold a `Join` entry naming them while they take no part - an access-list predicate
  as issue [332](332-policy-as-a-predicate.md) wants them. The web UI shows them and lets them
  accept or decline.
- **An agent** cannot be named by an access list (`SecurityAttributes::validate`). An invitation
  for an agent goes to its owner; whether an owner accepts for an agent, or brings their agent along
  once they joined, is to be decided.
- **Who may invite** besides the owner - the members, by a grant of their own - is a policy
  question, not decided here.
- **Withdrawing** an invitation is removing the entry by whoever added it.
- **Whether 221's direct way stays** once invitations exist - for the owner's own agents, say, or
  for a session where its users agreed in advance - is to be decided with this.

## Done when

- A user can be invited to a session or channel, is told of it, sees the invitation and can accept
  or decline it; an invitation can be withdrawn.
- Accepting uses up the personal `Join` entry; declining removes it.
- The security suite covers inviting, accepting, declining and withdrawing, and that an invitee
  reads nothing of the session before accepting.
- CHANNELS.md, SECURITY.md, REST-SERVER.md and WEB-UI.md describe it.

## History

- **2026-10-13** — Opened at the maintainer's request, from the discussion of
  [221](221-adding-session-participants.md): whom a session or channel may take in is held as
  `Join` entries of its access list, and an invitation is the personal one; 221 builds adding by the
  owner and joining by oneself, this issue the invitation as a step of its own.
