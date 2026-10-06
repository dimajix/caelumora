# Security

This document describes who may do what in Caelumora: the `SecurityManager` and the policies it
applies, ownership of stored entities, where permission checks belong in the code, and where users,
logins and secrets come from. How far *information* may travel between channels is a separate
question, answered by classifications in [CONFIDENTIALITY.md](CONFIDENTIALITY.md). The settings
mentioned here are listed in [CONFIGURATION.md](CONFIGURATION.md).

## The security model

Security is handled through a context that travels with every operation and explicit checks against
it.

*   **`SecurityContext`** carries the initiator and the sender of an event, and the session and
    channel it happens in. The initiator is the person an action was started for, also when an agent
    carries it out.
*   **`SecurityManager`** is the trait that authorizes actions: `check(context, action)`.
*   **`ActionType`** enumerates every securable action (`GetAgent`, `UpdateSession`,
    `ListPersonalities`, `CancelChannel`, ...). An action on a stored entity carries the entity's
    `SecurityAttributes`, not the entity itself.

### One manager per process

One `SecurityManager` is built per process and handed to everything that checks: the stores, the
`SecretManager` and the `RuntimeManager`, from which the agent's `GuardedToolExecutor` and every
`ChannelLoop` take it.

It is a constructor argument rather than something built where it is needed. That way a deployment's
policy cannot be silently replaced by a default in one corner of the runtime.

The configuration's `security` section chooses the policy:

*   `ownership` - the default, `OwnershipSecurityManager` (see below);
*   `allow_all` - `DummySecurityManager`, which allows everything to everybody; for tests and
    development only.

A policy defined outside the binary is [24](../issues/24-opa-security-manager.md).

## Ownership

Every stored entity - session, channel, agent, personality, session template, preconfigured
component - carries `SecurityAttributes`: an `owner_id` and a list of `contributors` (issue 11).
This is the one place ownership is recorded.

*   A session and its channels are owned by the user they were created for; their users and agents
    are contributors.
*   An agent is owned by whoever created it, and contributes to its own definition.
*   Initial data belongs to the system.
*   A `User` record belongs to the system, too (issue 94): only the user providers and the setup
    write it, so that nobody can grant themselves roles. What a user writes about themselves lives
    in a `UserProfile` of its own, owned by that user (issue 176) - how they introduce themselves
    and the language they prefer (issue 56). Its owner is derived from the user it belongs to rather
    than stored, so that nobody can write a profile of someone else that is theirs.
*   Introductions - a personality's, a user's profile's - are public: they are given to every agent
    in every channel their agents or their user take part in. A personality's introduction is
    written by whoever may change the personality, like its system prompt; a user's by the user.
*   Entities stored before owners were recorded have an empty owner and count as the system's.

### The ownership policy

The `OwnershipSecurityManager` decides by those attributes and the context's initiator:

*   The owner may do everything. Contributors may read and use - write into a session, cancel or
    wake a channel, ask an agent in it to answer again (`RetryMessage`, issue 234). What belongs to
    the system, everybody may read and use.
*   Creating is only allowed with oneself as owner; changing and deleting only for the owner.
*   What was deleted (issue 164) is the owner's to read (`ReadDeleted`) and to restore
    (`Restore`). Removing it for good (`Purge`) is the system's alone - the retention policy's
    ([07](../issues/07-session-deletion-audit.md)) - until there is an administrator
    ([125](../issues/125-invisible-admin.md)).
*   A message is its author's to modify and to take back (`ModifyMessage`, `RevokeMessage`, issue
    [234](../issues/234-modify-revoke-and-retry-messages.md)): whoever said it - the context's
    caller, an agent for its own message even when it acts for its owner - not the channel's owner.
*   Deleting an agent is its owner's (`DeleteAgent`). That it leaves channels owned by others is
    done in the runtime's name: it is a consequence of the deletion, not a change of those channels
    by the owner.
*   The runtime acting on its own uses `SecurityContext::system()` (`ActorId::System`), which may do
    everything. This is deliberate and never a default: an empty context has no rights.

The confidentiality actions (`ReadChannel`, `Classify`, `Reclassify`, `Publish`, `JoinChannel`) are
decided by classifications, not by ownership: the ownership policy applies the
`ConfidentialityRules` of the configured compartments and roles to them, and adds only what is about
ownership - the actor putting a message into a channel is the one acting, a channel is reclassified
by its owner. Another policy may apply other rules (issue
[138](../issues/138-confidentiality-policy-in-ownership.md)); see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#who-applies-the-rules).

### How stores check

The stores ask the same way for every entity kind (`store_impl::access`); the `EntityStore` trait
itself is unchanged.

*   **Reads** are checked with the attributes as stored. An entity the caller may not read is
    reported as not found, so ids cannot be probed.
*   **Lists** are filtered by what the caller may read.
*   **Changes and deletions** are checked against the attributes *as stored*, never against those of
    the new value, which the caller controls.
*   **Creations** are checked against the new entity's attributes.
*   **What was deleted** is not there for any of these. Reading it as deleted asks `ReadDeleted`
    and answers "not found" alike; restoring asks that and `Restore`, purging that and `Purge`.
    What is not marked is neither restored nor purged.

### How the SessionManager checks

What is not a store call asks first and acts afterwards.

*   The `SessionManager` finds a session for a caller in one place, a private `find`. It checks a
    running session like a stored one - `GetSession`, answering "not found" alike whether the
    session runs or not - for `open_session`, `stop_session`, `delete_session` and the views.
*   `stop_session` (`StopSession`) and `delete_session` check before they stop anything.
*   `restore_session` and `purge_session` reach only what was deleted (`ReadDeleted`) and leave
    `Restore` and `Purge` to the store; `purge_session` then purges the session's channels in the
    runtime's name.
*   Renaming stores first - which is where it is checked - and changes the live entity only then.

### Not decided yet

*   Roles such as an administrator, and richer access lists
    ([23](../issues/23-acls-in-security-attributes.md)).
*   What happens to an owner's entities when the owner is removed. For now they stay.

## Where permission checks go

There are three ways code asks the `SecurityManager`, and each has its place.

### Access questions

A pure access question - "may they?", asked from outside an entity to decide what to list or offer -
is a method of `caelumora_model::security::AccessQuestions`, not of the entity (issue 71). They are
asked:

*   by the runtime building views, which decide what a client lists and offers (`can_change`,
    `can_read_context`, issue 80);
*   by the views an agent's turn navigates and by the components, deciding what an agent may read of
    other channels or recall (see [MEMORY.md](MEMORY.md)); a component asks with the
    `SecurityManager` and `SecurityContext` of its `ComponentContext` (issue 131);
*   by an agent, deciding what it may put up for approval;
*   by the approver, deciding whom it waits for (`may_approve`, issue 14); an answer it checks as
    the operation it counts for, with `Enforcement::require`.

The questions are `may_read_channel`, `may_change_channel`, `may_change_session`,
`may_channel_read`, `may_read_agent_context`, `may_read_agent_tools`, `may_approve`,
`may_delete_agent`, `may_call_tool`, `may_publish`, `may_modify_message`, `may_revoke_message`,
`may_reclassify`, `may_retry_message`, `may_see_channel`, `may_restore` and `may_purge`.

`AccessQuestions` is an extension trait on every `SecurityManager`, by blanket implementation. Each
question gets a name and one place, but only translates into an `ActionType`. The decision stays
with `check`, so no policy can answer a question differently from the action.

### Operations that check themselves

An operation that checks its permission as part of what it does - `ChannelLoop::cancel`,
`add_participant`, a store's `update_channel` - keeps that check itself. Only pure questions go into
`AccessQuestions`, and none onto the entities.

Such an operation checks with `Enforcement::require(context, action, refusal)` (issue 95), another
extension trait on every `SecurityManager`. It answers a refusal with the operation's own error -
`ChannelError::Forbidden`, `StoreError::NotFound` for what the caller may not see,
`ToolError::PermissionDenied`, ... - and logs every refusal alike: at `debug!`, by
`ActionType::describe`, which names the action and what identifies its object, never content such as
a tool call's arguments.

**The actor's own permission first.** An operation asks whether the actor may do it at all before
it asks anything of others - a participant's clearance, an agent's personality (issue 198). Who may
not do it is refused before the rest is looked at, and learns nothing of the others on the way:
`create_channel` asks `UpdateSession` first, as `delete_channel` does; `add_participant` asks
`UpdateChannel` first, as `remove_participant` does when the change is stored.

**Actors are controlled only through such operations.** `Actor::cancel`, `retry_turn`, `wakeup` and
`stop` check nothing: they are the consequence of an act that was checked - `ChannelLoop::cancel`,
`cancel_actor`, `wakeup`, `wakeup_actor`, `remove_participant`, `SessionLoop::delete_channel`, a
`RetryRequested` of a turn the channel admitted under `RetryMessage` (issue 235) - and know no
caller. So that this holds by structure rather than by discipline, the `Actor` trait, the
`Moderator` trait that is handed actors, and `ChannelLoop::actor_by_id` are crate-internal (issue
147): a frontend reaches its user's actor through `ChannelLoop::user_actor`, but none of the
control methods.

## The security suite

`caelumora-runtime/tests/security_suite.rs` (issue [198](../issues/198-security-scenarios.md)) runs
the whole runtime with the ownership policy, as a frontend does, with users of different
permissions. Its policy records every question it is asked; each test of an action asserts the
questions asked in a user's name, in order, for the action's owner, someone taking part and a
stranger. The suite is the record, in code, of which permissions each action needs, and issue 198
says what is expected and why.

**Every new action that acts in someone's name is added to the suite**, with what it asks and what
it answers to each of them; so is every change to what an existing action asks. A test of the suite
that changes is a change of the permissions an action needs, and is reviewed as one.

### Filters

`SecurityManager::check` is called directly only by `AccessQuestions`, by `Enforcement` and by
filters: a store filtering a list, the event bus choosing whom an event reaches, the commands a user
is shown.

## Views

What a frontend reads, it reads through the runtime's **views** (`caelumora_runtime::view`, issue
163): the sessions, their channels and their members as one viewer navigates them. A view is a node
the viewer stands on; its methods are the edges to the next ones, and each edge asks its own access
question, in the viewer's name:

*   `SessionManager::views(viewer)` is the root: `sessions()` are those the store lists for the
    viewer, `session(id)` asks `GetSession` (`may_read_session`). An agent's root is its own
    session, through its turn (`Turn::session`); it reads from its channel by the confidentiality
    rules, see [MEMORY.md](MEMORY.md#reading-other-channels);
*   `SessionView::channels()` lists the channels the viewer may read (`GetChannel`, and cleared for
    their classification, issue 133), `channel(id)` asks the same of one; `may_change()` asks
    `UpdateSession`;
*   `ChannelView::members()` and `member(actor)` - which also reaches an agent that took part and
    left, by the messages it left in the history (`MemberView::former`), so that what it saw stays
    readable; `may_change()` asks `UpdateChannel`;
*   `MemberView::may_read_context()` asks `ReadAgentContext`; `may_read_tools()` and `tool_sets()`
    ask `ReadAgentTools` with the agent's and the channel's attributes (issue 194), which the
    ownership policy grants whoever reads the agent and every member of the channel: what an agent
    can do there is no secret among those it works with; `may_approve()` asks `ApproveToolCall`,
    which the ownership policy grants the agent's owner and contributors, for an agent of the
    system the channel's owner (issue 14, see [TOOLS-AND-COMMANDS.md](TOOLS-AND-COMMANDS.md#who-may-approve));
*   `Views::channel_of(session, channel)` reaches what a memory entry's origin or a `KnowledgeRef`
    names, wherever it is, and answers `None` for what is gone or not the viewer's to read.

What does not exist and what the viewer may not read are one answer (`ViewError::SessionNotFound`,
`ChannelNotFound`), so that nothing can be probed. A node is reached through the running loop where
there is one and from the stores otherwise, so the same navigation serves running and stopped
sessions alike, and reaching a session does not start it. Names are read in the runtime's name
(issue 120).

A node answers with a **snapshot** (`caelumora_model::snapshot`, issue 80) for a client that lists
and offers: `SessionView::snapshot` with the participants and `can_change`; `ChannelView::snapshot`
with `can_change`; `MemberView::snapshot` with the handle and `can_read_context`. What else a
snapshot holds the caller chooses as `Detail` flags (issue 209) - a session's channels, a channel's
members, and of an agent member its token usage, its notes and the approvals its turn waits for -
all three read from its state at once (`MemberView::snapshot`) - so that what reads every agent's
state is paid only where it is shown. The flags are combined freely, none implies another: a detail
whose part is not asked for is not there (`USAGE` without `MEMBERS` gives no usage), a list not
asked for is empty. A detail that cannot be read is left out, and the rest is answered. The REST
server only maps snapshots to responses (`From<snapshot::ChannelSnapshot> for ChannelResponse` and
the like, in `models.rs`) and asks the `SecurityManager` nothing. A frontend never asks the access
questions itself.

A node is data the viewer was allowed to reach; it decides nothing. An operation never relies on it:
it checks its own permission when it acts.

## Users

Where users come from and how their credentials are verified is pluggable, in the same way as
secrets.

*   A `UserProvider` (`caelumora-model`) is one source of users.
*   A `UserManager` holds an ordered chain of them, configured in the configuration's `users`
    section.

Looking a user up (`find_user`, `get_user`) and verifying a credential (`authenticate`) are separate
operations. Verification never hands out the stored credential, so backends that verify by binding
(LDAP) fit the same trait as backends that compare a hash.

How a provider assigns `UserId`s is its own business. The only contract is that the same user keeps
the same id, across restarts included.

### How the chain fails

The user chain fails differently from the secret chain:

*   `Ok(None)` passes the question to the next provider.
*   A rejected credential or a failing provider ends it. A broken user file must deny, never fall
    through.
*   An unknown user is reported exactly like a wrong password.

There is no default provider. The only built-in one so far, `accept_any`, is a development stand-in:
it accepts any username and password and keeps users in the `EntityStore`. It must be enabled
explicitly and warns at startup (issues 10, 25). It gives users the roles its configuration names
for them, updated at every login; what roles grant - clearances - is described in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#clearances) (issue 142).

## Authentication

Users are authenticated by the frontend that serves them; the runtime only receives the
`SecurityContext` it builds. The REST server logs a user in through the `UserManager` and keeps the
login as a short-lived signed token in a cookie, renewed by use and bounded in its lifetime, since
it cannot be revoked. How it does that is described in
[REST-SERVER.md](REST-SERVER.md#authentication). The TUI logs nobody in: it acts as one fixed local
user (`UserId::default()`), recorded by the system on its first start.

## Secrets

Component configuration - such as an `mcp_tool`'s `auth.secret` - may need to reference credentials
without embedding them in persisted configuration.

*   A `SecretProvider` (`caelumora-model`) is a pluggable source of secrets.
*   A `SecretManager` holds an ordered list of providers and tries them in turn, returning the first
    match.

The built-in providers are:

*   `FileSecretProvider` - a flat JSON key/value file, re-read on every lookup, so secrets can
    rotate without a restart;
*   `EnvSecretProvider` - the process environment.

The configuration's `secrets` section sets the chain; without it, it is `local/secrets.json`, then
the environment.

Every `SecretManager::resolve` is checked with the `SecurityManager` (`ActionType::ReadSecret`)
before any provider is consulted, as the `EntityStore` and `StateStore` check access on a
`SecurityContext`.
