# 244 — A participant's introduction cannot be seen in the web UI

**Status:** open
**Area:** User interaction

## The problem

Since issue [176](176-participants-in-system-prompt.md), agents and users have an introduction: an
agent's is its personality's (`Personality::introduction`), a user's is written by the user in
their `UserProfile`. The agents of a channel read them in their system prompt (the
`channel_members_prompt_provider`), but a user of the web UI does not: the menu of a participant in
the members panel offers history, private notes and tools, but nothing that says who the participant
is. A user who meets an agent in a channel learns its name and handle, not what it is there for.

## Why it matters

Agents read each other's introductions and address each other by them; the users in the same
channel should see at least as much. Who an agent is - what it is for, what it does - decides whom
one asks; for a user, who they say they are matters to the agents and the other users alike. The
maintainer's wish: a menu item on a participant that shows its personality, first of all its
introduction.

## Considerations

### The introduction, as the first step

- **It is already read.** `MemberView::introduction` gives an agent's from its personality and a
  user's from their profile, `None` for an approver and for a member that no longer exists. It is
  public like the name (issue 176), so it needs no permission of its own: whoever reads the channel
  reads it, as every agent in it does.
- **Where it travels.** Either `MemberSnapshot::introduction`, filled with a `Detail` of its own
  and given by `GET …/participants/{aid}` (which reads one member with `Detail::USAGE` today), or
  always with the member. Always costs a read of the personality or profile per member and snapshot
  (issue [182](182-entity-reads-not-cached.md): such reads are not cached); per participant asked
  for, one read when the dialog opens. The member list does not need it.
- **The web UI.** A menu item "Show profile" (or "About …") on every participant that is no
  approver, opening a panel like the tools panel: name, handle, the introduction - for a user marked
  as written by themselves, as in the prompt. A participant without an introduction says so.
- **Kept up.** A changed introduction reaches the panel when it is opened again; no event is needed
  for that.

### What else could be shown

Candidates, each with the question who may see it:

- **For agents:**
  - *Its personality*: name and catalogue line (`Personality::description`). Today only who may
    read the personality (`GetPersonality`) sees it, usually its owner; that two agents come from
    one personality is not public. Open whether the name of the personality is public like the
    introduction.
  - *Its kind*: run here or external (`PersonalityKind`). For confidentiality this matters: what an
    external agent is given leaves the system (issue [179](179-external-agents-via-a2a.md)). Little
    to hide, much to gain.
  - *Its owner*: who is responsible for it and answers for its tool calls (issue
    [14](14-approval-privilege.md)) - whom to turn to when it waits for an approval.
  - *What it may hear*: its limiting roles (`Personality::limiting_roles`) or what they clear it
    for. Explains why an agent cannot join a channel or misses messages; but roles are an internal
    of the security configuration, so perhaps only as the highest classification it is cleared for.
  - *Who sees its reasoning* (`Personality::reasoning_visibility`): explains why its reasoning is
    shown or not.
  - *Its language model* and context size: useful to judge an answer; but the model is configured
    in its components, which may name endpoints - at most the model's name, and perhaps only for
    the owner.
  - *What already has a menu item*: its tools (issue [194](194-tool-sets.md)), its token usage
    (issue [209](209-token-usage.md)), its state (issue [243](243-member-state-in-member-list.md)),
    its published notes. A profile panel could gather these instead of a menu item each.
- **For users:** the introduction only. Their roles, approval rules (issue
  [13](13-approval-modes.md)) and preferred language are the user's own.
- **For both:** since when and through whom they take part - only if a membership records it; issue
  [164](164-soft-deletion-and-former-members.md) records only who removed a member.

## Plan

The maintainer decided the first step: the introduction, and of an agent its kind, its owner and the
name of its personality. The rest of the candidates above stays open.

### What becomes public

- **The personality's name** becomes public like the introduction: read in the runtime's name, for
  whoever reads the channel. Only the name (`ScopedName`'s `Display`), not its scope, not its
  catalogue line, not the personality itself - `GetPersonality` stays as it is.
- **The kind** (`PersonalityKind`): public, it says where what the agent is given goes.
- **The owner** (`Agent::security_attributes.owner_id`) with their name. The owner need not take
  part in the session, and a user's name is known only to those who share a session with them
  (issue [120](120-participant-copies.md)). The agent acts with its owner's authority, though: who
  talks to it talks to them, and should know it. An agent of the system shows "system".
  Confirmed by the maintainer.

### Where it travels

- **`Detail::PROFILE`**, "of a member: who it is", like `USAGE` for its token use: the member list
  of a channel's stream goes without it, `GET …/participants/{aid}` asks for it besides `USAGE`.
  For a user it costs a read of the profile; for an agent the personality is read anyway
  (`can_retry`), the owner's name costs a read of the user.
- **`MemberSnapshot`** gains, filled only with `PROFILE`: `introduction`, `personality` (its name),
  `personality_kind`, `owner` and `owner_name` - flat, like the other details. All `None` for an
  approver and a member that no longer exists; the last four `None` for a user.
- **`MemberState::personality`** in place of the reads of it in `introduction` and `can_retry`:
  the agent's member state hands out its personality, read once as now, and `MemberView` takes
  name, kind and - with the agent's attributes - the owner from it, in one place for every kind.
- **`ChannelMemberResponse`** carries the same fields, left out where absent; the API stays
  compatible.

### The web UI

A menu item "Show profile" on every participant but the approver, opening a panel beside the chat
like the tools panel, filled by `getParticipant`: name and handle; for an agent its personality,
whether it runs here or elsewhere, and its owner; the introduction - a user's marked as written by
themselves - or that there is none.

## Done when

The menu of a participant in the members panel offers to show its profile, and the panel shows its
introduction - an agent's from its personality, a user's from their profile, marked as written by
themselves - for every participant but the approver. What else the panel shows is decided here
first.

## History

- **2026-10-06** — Opened on the maintainer's request: the introduction first, and a list of what
  else could be shown, to be decided.
- **2026-10-06** — Decided by the maintainer: the first step shows the introduction and, of an
  agent, its kind, its owner and the name of its personality. Planned as above; whether the owner's
  name may be shown to those who do not share a session with them is still to be confirmed.
- **2026-10-06** — The maintainer confirmed the plan, the owner's name included.
