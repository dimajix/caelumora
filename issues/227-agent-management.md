# 227 — Agents cannot be seen or managed on their own, only as members of channels

**Status:** done
**Area:** User interaction
**Summary:** Agents cannot be seen or managed on their own, only as members of channels: no list, no renaming, no trash in the web UI, nothing of what an agent has across channels.

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
- **Who may do what** is the owner's alone today. Sharing an agent - letting others use or change
  it - is [23](23-acls-in-security-attributes.md) and [142](142-roles.md); an administrator who sees
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
- **2026-10-13** — Decided with the maintainer: the list shows name, scope, personality and owner;
  the channels an agent takes part in are shown on its page (`Views::agent`), only those the viewer
  may read. A new agent is made in the user's own scope from the web UI; in a session's scope only
  through the shortcut of `AddParticipantDialog`. Its memory and private notes across channels are
  not shown here but left to [19](19-memory-tiers.md); changing an agent's personality is not part
  of managing it. Renaming stays with the `AgentController`, not the `AgentManager` as the entry of
  2026-10-11 said: managers own the lifecycle of their entities, and an entity with a controller is
  changed through it, as sessions, channels and users are. The rule is written down in
  ARCHITECTURE.md (*Managers and controllers*); the `WorkspaceManager`, which owns more, is issue
  [344](344-workspace-manager-owns-their-use.md).
  First step done: `AgentManager::create` makes an agent - `CreateAgent` asked before the
  personality is read -, announces it and fails with `AgentError`, which the manager's other
  operations now answer too. The template only finds an agent (`template::agent`) and has the
  manager make one; `Agent::new` takes the personality's id.
- **2026-10-13** — Second step: the views reach an agent on its own (`Views::agent`, `AgentView`),
  its snapshot names its scope, owner and personality and whether the viewer may rename it
  (`may_change_agent`, `UpdateAgent`); `AgentView::channels` lists the channels it takes part in
  that the viewer reaches through their sessions. `Views::personalities` lists those the viewer may
  make an agent of. The REST server answers `AgentResponse` as before; its new fields and routes are
  the next step.
- **2026-10-13** — Third step: the REST server makes an agent of a personality (`POST /agents`), in
  the user's scope or the global one, as groups and workspaces are made; one of a session's scope
  is refused (`invalid_scope`) until it is decided how it is made. It reads one agent with the
  channels it takes part in that the user may read (`GET /agents/{id}`), renames it through its
  controller (`PATCH /agents/{id}`) and lists the personalities (`GET /personalities`);
  `AgentResponse` names its scope, owner and personality and whether the user may rename it.
  `AgentManager::create` takes the name as text and the scope, as the `GroupManager` and the
  `WorkspaceManager` do, and fails with `Invalid` for a name that does not fit.
- **2026-10-13** — Decided with the maintainer: a scope is where a name is found, not who owns
  what lies there, so an agent may be made in any scope its personality admits, `POST /agents`
  included. What lies in a session's or a channel's scope keeps the session from being deleted by
  whoever may not delete it (`ScopeNotEmpty`), however, so `AgentManager::create` asks whether the
  maker may read the session or the channel (`GetSession`, `GetChannel`) before it reads the
  personality - a channel of another session than the scope names is none. An agent made in a
  session's scope from the web UI is made there and then taken in; if taking it in fails, the
  client deletes it.
  So that the template is not an exception to this, it makes its agents once the session is, as a
  client would: `create_from_template` stores and starts the session with its users and approvers
  (`SessionTemplate::build` leaves the agents out), then finds or makes each agent and takes it
  into the session and the channels that name it (`template::take_in_agents`,
  `SessionLoop::add_participant`); if that fails, the session is deleted and purged again in the
  runtime's name, with what was made in its scope. An agent the template cannot make or take in
  now fails after the session was stored, and the session is taken back.
- **2026-10-13** — Fourth step, and done: the web UI lists the agents the user may read below the
  teams (`PageList`), with their trash, and loads the list again on every event about an agent. An
  agent's page (`AgentPage`) shows its personality, where it lies, whose it is and the channels it
  takes part in that the user may read, opened from there; whoever may renames and deletes it.
  "New agent" (`NewAgentDialog`) makes one in the user's own scope, of a personality that admits
  it. `AddParticipantDialog` makes a new agent and takes it in - the user's own or for the session
  alone, as they choose or its personality requires - and deletes it again if taking it in fails.
  Who owns an agent is shown as the user or "someone else": the REST API names the owner by id
  alone.
