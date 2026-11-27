# 613 — An agent cannot read or send email

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Integrations
**Summary:** An agent has no mailbox: it cannot read mail from an IMAP account, learn that a new mail arrived, or send one, so mail - the way most people and companies outside reach each other - stays out of its reach, and with it any work that begins with a mail.

## The problem

An agent reaches the outside world through tools - MCP servers, OpenAPI servers, the web - and
through external agents. Mail is none of them. An MCP server for mail can be configured today, but
it gives an agent tools to call, not a way to learn that something arrived: the agent sees a new
mail only when someone asks it to look.

Three questions are open:

1. **How the agent learns of a new mail.** A mail can have nothing to do with what the agent is
   working on in its channels.
2. **Whose address it is.** Does every agent have an address of its own, does it belong to the
   personality, to the agent, to its owner?
3. **Whose credentials.** IMAP and SMTP need a password or a token; where it lies and who may use
   it.

## Why it matters

- Most work from outside arrives by mail: requests of customers, notices of systems, replies to
  what an agent sent. A company of agents ([617](617-epic-a-company-that-works-on-its-own.md))
  without mail has no door to the outside.
- It is the first case of the foundation [612](612-an-agent-cannot-keep-an-appointment.md)
  describes (*What the three share*): an event outside every channel that becomes an agent's work.
  The calendar is the case where the event is the kernel's own; mail is the first where it comes
  from outside and is untrusted.

## Considerations

### How the agent learns of a new mail

- **A. Only by tool.** The agent lists its mailbox when it is asked to, or on a recurring entry of
  the calendar (612) - "check the mailbox every 15 minutes". Simple, needs nothing but tools and
  the calendar; late by up to the interval, and every check costs a turn even when nothing came.
- **B. Given in a channel it works in** - a short notice, "a new mail from X: subject", given to
  read as a brief is. The agent learns at once, but a mail that has nothing to do with the channel
  interrupts its work there and enters a transcript and a history where it does not belong - and
  everybody in that channel may read what it is given there.
- **C. Given in a channel of its own, bound to the mailbox** - a *mail room*: a session the
  mailbox's owner sets up once from a template, in which the agent is told of each new mail, reads
  it with its tools, and decides: answers, hands it to a sub-agent, starts a session for it, or
  passes it on to a person. The mail stays out of every other conversation, and its owner sees what
  the agent did with it there.
- **D. Every mail thread a channel of its own.** A mail conversation becomes a channel, its mails
  messages of an outside sender; answers in the channel go out as mails. Faithful to the
  conversation, but a channel per thread multiplies channels quickly, and it needs participants who
  are no users - the same question as for messengers ([614](614-no-messenger-reaches-a-channel.md)).

Recommended: C, with A as what comes for free. B mixes unrelated work into a conversation; D
belongs to the question of outside participants, which 614 raises and which can come later.

Notification itself: IMAP `IDLE` where the server offers it, polling otherwise. Something has to
hold the connection while no turn runs, so the listener is a service on the kernel's side - a
plugin - and the tools a library beside it, as `caelumora-subagents` is cut.

### Whose address

- **The personality's.** Components are configured in a personality, so an `email` component there
  names the account. But a personality is a definition many agents may be made of: all of them
  would share one mailbox.
- **The agent's.** An address is an identity, and the agent is the identity; but an agent has no
  configuration of its own beside its personality - it is read through it.
- **A mailbox as a thing of its own** - an account with its servers, its address and the references
  to its secrets, owned by a user and granted to agents by its access list, named by the component
  of a personality. One mailbox could serve several agents (a shared `support@`), one agent several
  mailboxes; but it is a new kind with a manager, a store and views.

Most mailboxes of a company are roles - `support@`, `sales@` -, not persons, so the first option
fits more than it seems: one personality for each role, one agent of it. Recommended: the
personality's component for a start, with the secrets as below; a mailbox of its own once several
agents have to share one.

### Whose credentials

The component names its secrets by reference (`vault/KEY`, SECURITY.md *Secrets*), as
`mcp_server` does: what may use them is the vault's access list, where they may be sent its
`origins` - the IMAP and SMTP hosts. Today only the operator puts secrets there. A mailbox of a
user's own needs their own secret: issue
[343](343-users-bring-their-own-secrets.md), which has already proposed that a secret of one's own
lies only in the user's scope and must name its origins once it is granted to others.

### What may flow

- **In:** a mail is untrusted text from outside - an injection of instructions is the obvious
  attack. It is given as coming from outside, classified public, and its attachments go the way of
  attachments (issue 533), not into the prompt whole.
- **Out:** a mail leaves the system. Only what may be published may be sent - the door external
  agents pass (CONFIDENTIALITY.md) -, and sending asks for approval by default, as a component
  configured with `requires_approval`.

## Done when

- The foundation of 612 is decided.
- An agent with the component reads, searches and sends mail of its mailbox, sending approved as
  configured.
- A new mail is told to the agent where it is decided (option C as recommended), also after a
  restart, without a mail told twice.
- Its credentials are references to secrets, checked as every secret is; covered by tests against
  a local IMAP/SMTP server.

## History

- **2026-11-27** — Opened on the maintainer's proposal of a mail tool, with first approaches.
