# 221 — Nobody can join a session once it is created

**Status:** open
**Area:** Sessions and channels

## The problem

The participants of a session are fixed when it is created: a template names them
(`SessionTemplate::build`, `template::resolve_participants`), and its security attributes are
derived from them once - the owner, and every user and agent as a contributor
(`participant_attributes`). Afterwards nothing adds one:

- Adding a participant to a channel (issue [46](46-channel-participants.md),
  `ChannelLoop::add_participant`) takes only participants of the session (`NotInSession`
  otherwise), and the web UI's `AddParticipantDialog` offers only those.
- There is no way to find a user or agent to add: the REST API lists neither users nor agents.
- An agent can be created only by a template, when a session is created from it; there is no way to
  create one from a personality for a running session.

So a session that turns out to need another person, or another agent, has to be created anew.

## What is wanted

- Adding users and agents to a session, **from within a channel**: the dialog that adds a
  participant to the channel also offers who is not in the session yet, marked as such; choosing one
  adds them to the session and to the channel.
- Both an **existing user** and an **existing agent** can be chosen.
- Only who may in principle join the session is offered or accepted: as far as the
  `SecurityManager` allows it for whoever adds them, and by clearance and classification.
- Optionally, a **new agent** is created from an existing personality and added the same way.

## Considerations

### Who may add someone

Adding to a session changes it: `UpdateSession` on the session's attributes, i.e. by default its
owner. Whether contributors may add others is a policy question - with `ownership` they may not.
Adding to the channel goes on to need `UpdateChannel`, as today. Both are asked before anything is
asked of the one to be added (issue 198, F2: who may not add learns nothing about the candidate).

### Who may be added

- **A user**: any user the adder may name. Listing all users discloses who exists (`ListUsers`,
  `GetUser`); a search by name or handle, answering only exact or prefix matches, may be enough and
  discloses less. Whether a user must agree to be added is a question of its own (an invitation);
  not doing so is the simple first step.
- **An agent**: one the adder may use - their own, one of the system, one they contribute to
  (`GetAgent` under `ownership`). An external agent's personality may limit who may use it by roles
  (`Personality::limiting_roles`); the session's other users would act with it too.
- **Clearance**: for the channel it is added to, as today (`Member::require_cleared`,
  `JoinChannel` with the user's roles, or the agent owner's). For the session as such there is no
  classification to clear against; being a participant makes them a contributor, which lets them
  read every channel they are cleared for (`GetChannel`). Whether joining a session should require
  clearance for all its channels, or only for the one they are added to, is to be decided.
- **Privacy**: a session's privacy (issue [134](134-session-privacy.md)) says whom it lets read what
  was said in it; a new participant reads the history of the channels they join as far as
  `history` grants (issue 46). What their agents may read from other sessions joins with them
  (issue [143](143-joining-a-channel-with-foreign-knowledge.md)).

What the candidates are is the runtime's to answer, not the frontend's: an operation of the session
(or channel) view lists who the viewer may add, each with whether they are in the session already;
the web UI shows that list (see [SECURITY.md](../docs/SECURITY.md): a frontend never asks the
`SecurityManager`).

### Creating an agent from a personality

`CreateAgent` with the adder as owner, of a personality they may read (`GetPersonality`), named by
them - a name unique in the session, as a template's agents are - and scoped like a template's
agents (`SessionScope`: it exists for this session; `UserScope`: it is the adder's and may be used
again). A template does the same today (`ParticipantTemplate::instantiate`); the two should share
one way of creating an agent.

### How it is stored and run

- The session document gains the participant, and its attributes are derived again
  (`participant_attributes`), so that the new participant becomes a contributor. One operation of
  `SessionLoop`, checked by itself (`Enforcement::require`), as `ChannelLoop::add_participant` is.
- Adding to the session and to the channel: one request that does both, in that order - or the
  session first, then the existing channel operation. If the channel part fails (not cleared), the
  session keeps the participant or not: to be decided.
- The global stream announces the changed session, so that every client's view of its participants
  follows.
- Removing someone from a session, and what it means for their channels and agents, is not part of
  this; former members are issue [164](164-soft-deletion-and-former-members.md).

### REST and web UI

- `GET …/channels/{cid}/candidates` (or on the session) - whom the user may add, each marked whether
  in the session; `POST …/channels/{cid}/participants` accepting someone outside the session, or
  `POST /sessions/{id}/participants` first.
- `GET /personalities` (those the user may read) and a way to create an agent from one, with a name.
- `AddParticipantDialog`: the session's participants, then the others with the hint that they are
  not in the session yet and will be added to it, then "New agent from a personality".

### The security suite

Adding to a session is an action in someone's name: tests for who may add, who may be added (a user,
an agent not theirs, an external agent limited by roles), not cleared, and an agent created from a
personality the adder may not read.

## Done when

- From a channel, a user can add an existing user or agent who is not in the session yet; they join
  the session and the channel, and the dialog says that they were not in the session.
- Only who the adder may add, and who is cleared for the channel, is offered and accepted; the
  security suite covers it.
- Optionally, an agent is created from a personality and added the same way.
- CHANNELS.md, SECURITY.md, REST-SERVER.md and WEB-UI.md describe it.

## History

- **2026-10-04** — Issue created at the maintainer's request. Found: a session's participants are
  fixed by its template, the channel dialog offers only those, and the REST API lists neither users
  nor agents.
