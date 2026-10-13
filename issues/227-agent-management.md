# 227 — Agents cannot be seen or managed on their own, only as members of channels

**Status:** open
**Area:** User interaction

## The problem

Agents exist only as a by-product of session templates and as members of channels:

- **Created** only when a session is built from a template (`template::agent`), which reuses an
  agent of the same name in its scope or makes a new one. There is no way to create one from a
  personality without a template.
- **Changed** by nobody: `AgentController::update_name` exists, but no route or UI calls it.
- **Seen** in the web UI only as a member of a channel - its history, contexts, notes and usage
  there, one channel at a time. `GET /agents` lists the agents a user may read, by id and name only
  ([164](164-soft-deletion-and-former-members.md)).
- **Deleted** from the member list; restored and purged only over REST. A user who deleted an agent
  cannot get it back in the web UI, since there is no trash of agents there.

What an agent has across channels is shown nowhere: its memory (`AgentState::memory`), its private
notes of all channels, its usage as a whole.

## Why it matters

- An agent of user scope lives on from session to session, with its memory: it is a thing of its
  own to its owner, not a member of one channel.
- Deleting an agent makes it leave every channel (164); without a place to see and restore it, a
  mistaken delete is final for a user of the web UI.
- The maintainer intends to manage personalities and session templates as well; agents belong in
  the same place.

## Considerations

- **The core** needs little that does not exist: a list of the user's agents - name, scope,
  personality, owner, the channels it takes part in - with their trash (`Views::agents`,
  `deleted_agents`, the web UI's `DeletedList`) and renaming (`update_name`, a route, `can_change`).
- **Creating an agent** from a personality, without a template, is only useful if it can then join
  a session: today a session's participants are fixed when it is created
  ([221](221-adding-session-participants.md)). To be decided with that issue.
- **Changing an agent's personality** changes its components, its tools and how far it may be
  cleared (`Personality::limiting_roles`); it may not belong to managing at all, rather to making a
  new agent.
- **What it has across channels** - its memory, its private notes everywhere, its usage - is a view
  of its own, close to the memory tiers ([19](19-memory-tiers.md)). Private notes and memory are its
  owner's to inspect (MEMORY.md); the views have the edges, a page does not.
- **Who may do what** is the owner's alone today. Sharing an agent - letting others use or change it
  - is [23](23-acls-in-security-attributes.md) and [142](142-roles.md); an administrator who sees
  everyone's agents and their trash is [125](125-invisible-admin.md).
- **Where in the web UI:** a page or panel of its own, beside the sessions, which personalities and
  templates can join once they are managed too.

## Done when

- A user sees the agents they may read in the web UI, with their personality and the channels they
  take part in, and can rename those they may change.
- A user sees the agents they deleted and can restore them there.
- It is decided, with 221, whether and how an agent is created without a template; and whether an
  agent's memory and private notes across channels are shown here or elsewhere.

## History

- **2026-10-05** — Opened at the maintainer's request after
  [164](164-soft-deletion-and-former-members.md), which made agents deletable but left their
  restoring to REST: the web UI has no place for agents outside of channels.
- **2026-10-11** — [314](314-session-manager-handles-agents.md) gave agents an owner, the
  `AgentManager` in the `RuntimeManager`, which deletes, restores and purges them; renaming and
  making one from a personality belong there too. A deleted agent now rests in its channels and
  comes back into them when it is restored, so a mistaken delete is no longer final - only the
  trash in the web UI is missing.
- **2026-10-13** — Decided with [221](221-adding-session-participants.md): creating an agent from a
  personality without a template is this issue's - `AgentManager::create`, to which the
  template's finding or making an agent (`template::agent`) moves, `POST /agents` and `GET
  /personalities`. The dialog that adds a participant to a channel gets a shortcut to it - a new
  agent from a personality, created and then taken in, in two calls - with whichever of the two
  issues is done last; 221 was done first.
