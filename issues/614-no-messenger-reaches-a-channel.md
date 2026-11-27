# 614 — No messenger reaches a channel

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Integrations
**Summary:** Nobody can talk with an agent through a messenger - Telegram, Matrix, Signal, Slack -: a user can reach a channel only through the web UI or the terminal, and an agent cannot take part in a chat of people who are no users of Caelumora.

## The problem

Everyone who takes part in a channel is a user of Caelumora or an agent; the frontends are the web
UI and `caelumora-tui`. A messenger is two things that today are both missing:

- **Another frontend for a user.** Alice wants to talk with her agents from her phone, in the
  messenger she uses anyway, and be told there of what concerns her (issue
  [594](594-a-user-is-told-of-what-concerns-them-in-no-one-way.md)).
- **A place where an agent meets people outside.** An agent answers customers in a Telegram group or
  takes part in a Matrix room of a project; those people are no users here, and know nothing of
  channels.

Two questions run through both: whether every chat on a messenger is a channel of its own, and
whose credentials reach the messenger.

## Why it matters

- A messenger is where people are; a company of agents
  ([617](617-epic-a-company-that-works-on-its-own.md)) that its people and its customers can only
  reach through a web UI is hard to reach.
- What arrives from a messenger is an event outside every channel that becomes an agent's work - the
  foundation [612](612-an-agent-cannot-keep-an-appointment.md) describes (*What the three
  share*) - and, unlike mail ([613](613-an-agent-cannot-read-or-send-email.md)), it is a
  conversation, which is what a channel already is.

## Considerations

### A chat is a channel

A messenger chat - a direct chat, a group, a room - has what a channel has: a line of messages, who
takes part, often threads. Mapping one chat to one channel keeps its history, its moderator, its
classification and who heard what where Caelumora keeps them for every conversation. The other way -
a messenger as a tool an agent sends with, and a notice in the agent's channel for what arrives -
loses the conversation: the chat's people would not be in the channel, the agent would speak to them
only through tool calls.

Recommended: one chat, one channel, bound to it by a **bridge** - an internal actor of the channel,
no participant, like the approver or the delegate (CHANNELS.md *Actors and envelopes*): it carries
what is said in the chat into the channel and what is said in the channel out into the chat. A chat
the bridge has not seen before becomes a channel of a session made from a template the bridge is
configured with, as the mail room of 613 is.

### Who speaks in the channel

- **A user of Caelumora** on the messenger: their messenger account is linked to their user, and
  what they write is written by their own actor - the rule that nobody writes into a channel but
  through their own actor holds (ARCHITECTURE.md *How a message travels*). This is a frontend like
  the others, built on what a user's actor already offers.
- **Someone outside**: no user, no actor, no rights. They need a kind of participant of their own -
  a *guest*, known by the messenger and their account there -, for whom the channel records what
  they said and who are shown nothing but what goes out into the chat. That is a new concept of the
  kernel, with every question of security it raises (SECURITY.md), and the same one a mail
  conversation as a channel would need (613, option D).

Recommended: the user's frontend first, since it needs no new concept; guests as a step - or an
issue - of their own, decided with the question of mail conversations.

### Whose credentials

A messenger account is reached with a token: a bot's token for Telegram, an access token of an
account for Matrix, the account of a linked device for Signal. Who acts on the messenger is that
account, so:

- **The bridge's account** for an agent among people outside: one bot per bridge - the bot is the
  agent's identity on the network, and its token a secret of the configuration
  (`vault/KEY`, SECURITY.md *Secrets*), with the messenger's API as its only origin. One agent per
  bot, so that people outside know whom they talk to.
- **The user's own account** for a user's frontend is not needed at all where the bridge's bot is
  what the user writes to: the user links their messenger account to their user once - by a code
  the web UI shows - and talks to the bot. A user's own token would only be needed to act as the
  user elsewhere on the network, which nothing here asks for. Where it is, it is a secret of the
  user's own (issue [343](343-users-bring-their-own-secrets.md)).

### What may flow

Everything that goes into a chat leaves the system: a channel bound to a chat with guests is public
at most, and the bridge carries out only what may be published, as at the door of an external agent
(CONFIDENTIALITY.md). A user's frontend carries what the user may read - their phone is theirs -,
but the messenger's operator reads along, unless it encrypts end to end; whether a channel above
some classification may be bridged at all is a setting of the bridge.

### Which messenger first

Telegram has the simplest bot API (HTTPS, long polling, no server of one's own); Matrix is open,
can be self-hosted and has rooms and threads; Signal has no official bot API; Slack and Teams are
for companies. Recommended: Telegram or Matrix first, behind a bridge that names no messenger, so
that a second one is a crate of its own - as the protocol of external agents is (`caelumora-a2a`,
issue 326).

## Done when

- The foundation of 612 is decided.
- A user talks in a channel through one messenger, linked to their user, and is told there of what
  concerns them.
- Which messenger came first, and whether guests come here or in an issue of their own, is decided
  and recorded.
- Covered by tests against a messenger stand-in; documented.

## History

- **2026-11-27** — Opened on the maintainer's proposal of a messenger integration, with first
  approaches.
