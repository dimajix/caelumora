# Security

This document describes who may do what in Caelumora: the `SecurityManager`
and the policies it applies, ownership of stored entities, where permission
checks belong in the code, and where users, logins and secrets come from. How
far *information* may travel between channels is a separate question, answered
by confidentiality levels in [CONFIDENTIALITY.md](CONFIDENTIALITY.md). The
settings mentioned here are listed in [CONFIGURATION.md](CONFIGURATION.md).

## The security model

Security is handled through a context that travels with every operation and
explicit checks against it.

*   **`SecurityContext`** carries the initiator and the sender of an event,
    and the session and channel it happens in. The initiator is the person an
    action was started for, also when an agent carries it out.
*   **`SecurityManager`** is the trait that authorizes actions:
    `check(context, action)`.
*   **`ActionType`** enumerates every securable action (`GetAgent`,
    `UpdateSession`, `ListPersonalities`, `CancelChannel`, ...). An action on
    a stored entity carries the entity's `SecurityAttributes`, not the entity
    itself.

### One manager per process

One `SecurityManager` is built per process and handed to everything that
checks: the stores, the `SecretManager` and the `RuntimeManager`, from which
the agent's `GuardedToolExecutor` and every `ChannelLoop` take it.

It is a constructor argument rather than something built where it is needed.
That way a deployment's policy cannot be silently replaced by a default in one
corner of the runtime.

The configuration's `security` section chooses the policy:

*   `ownership` - the default, `OwnershipSecurityManager` (see below);
*   `allow_all` - `DummySecurityManager`, which allows everything to
    everybody; for tests and development only.

A policy defined outside the binary is
[24](../issues/24-opa-security-manager.md).

## Ownership

Every stored entity - session, channel, agent, personality, session template,
preconfigured component - carries `SecurityAttributes`: an `owner_id` and a
list of `contributors` (issue 11). This is the one place ownership is
recorded.

*   A session and its channels are owned by the user they were created for;
    their users and agents are contributors.
*   An agent is owned by whoever created it, and contributes to its own
    definition.
*   Initial data belongs to the system.
*   Entities stored before owners were recorded have an empty owner and count
    as the system's.

### The ownership policy

The `OwnershipSecurityManager` decides by those attributes and the context's
initiator:

*   The owner may do everything. Contributors may read and use - write into a
    session, cancel or wake a channel. What belongs to the system, everybody
    may read and use.
*   Creating is only allowed with oneself as owner; changing and deleting only
    for the owner.
*   The runtime acting on its own uses `SecurityContext::system()`
    (`ActorId::System`), which may do everything. This is deliberate and never
    a default: an empty context has no rights.

The confidentiality actions (`Publish`, `ReadChannel`, `JoinChannel`) are
decided by the channels' levels, not by ownership; see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md).

### How stores check

The stores ask the same way for every entity kind (`store_impl::access`); the
`EntityStore` trait itself is unchanged.

*   **Reads** are checked with the attributes as stored. An entity the caller
    may not read is reported as not found, so ids cannot be probed.
*   **Lists** are filtered by what the caller may read.
*   **Changes and deletions** are checked against the attributes *as stored*,
    never against those of the new value, which the caller controls.
*   **Creations** are checked against the new entity's attributes.

### How the SessionManager checks

What is not a store call asks first and acts afterwards.

*   The `SessionManager` finds a session for a caller in one place, a private
    `find`. It checks a running session like a stored one - `GetSession`,
    answering "not found" alike whether the session runs or not - for
    `open_session`, `stop_session`, `delete_session` and the views.
*   `stop_session` (`StopSession`) and `delete_session` check before they stop
    anything.
*   Renaming stores first - which is where it is checked - and changes the
    live entity only then.

### Not decided yet

*   Roles such as an administrator, and richer access lists
    ([23](../issues/23-acls-in-security-attributes.md)).
*   What happens to an owner's entities when the owner is removed. For now
    they stay.

## Where permission checks go

There are three ways code asks the `SecurityManager`, and each has its place.

### Access questions

A pure access question - "may they?", asked from outside an entity to decide
what to list or offer - is a method of
`caelumora_model::security::AccessQuestions`, not of the entity (issue 71).
They are asked:

*   by the runtime building views, which decide what a client lists and offers
    (`can_change`, `can_read_context`, issue 80);
*   by the `SessionReader` and the components, deciding what an agent may
    read of other channels or recall (see [MEMORY.md](MEMORY.md)); a
    component asks with the `SecurityManager` and `SecurityContext` of its
    `ComponentContext` (issue 131);
*   by an agent, deciding what it may publish or put up for approval.

The questions are `may_read_channel`, `may_change_channel`,
`may_channel_read`, `may_read_agent_context`, `may_publish` and
`may_call_tool`.

`AccessQuestions` is an extension trait on every `SecurityManager`, by blanket
implementation. Each question gets a name and one place, but only translates
into an `ActionType`. The decision stays with `check`, so no policy can answer
a question differently from the action.

### Operations that check themselves

An operation that checks its permission as part of what it does -
`ChannelLoop::cancel`, `add_participant`, a store's `update_channel` - keeps
that check itself. Only pure questions go into `AccessQuestions`, and none
onto the entities.

Such an operation checks with `Enforcement::require(context, action, refusal)`
(issue 95), another extension trait on every `SecurityManager`. It answers a
refusal with the operation's own error - `ChannelError::Forbidden`,
`StoreError::NotFound` for what the caller may not see,
`ToolError::PermissionDenied`, ... - and logs every refusal alike: at
`debug!`, by `ActionType::describe`, which names the action and what
identifies its object, never content such as a tool call's arguments.

### Filters

`SecurityManager::check` is called directly only by `AccessQuestions`, by
`Enforcement` and by filters: a store filtering a list, the event bus choosing
whom an event reaches, the commands a user is shown.

## Views

What a client lists and offers comes from the runtime as views of
`caelumora_model::view` (issue 80), each built for one viewer:

*   a `SessionView`: the session's participants and the channels the viewer
    may read;
*   a `ChannelView`: the channel's members with their handles and
    `can_read_context`, and `can_change`.

The `SessionManager` builds them with `session_views(ctx)`, `session_view(ctx,
id)` and `channel_view(ctx, session, channel)`, for running and stopped
sessions alike and without starting one. A channel is described from its
`Channel` document - the running channel's or the stored one - by one private
`describe_channel`, which asks the `AccessQuestions`. Names are read in the
runtime's name (issue 120).

The REST server only maps views to responses (`From<ChannelView> for
ChannelResponse` and the like, in `models.rs`) and asks the `SecurityManager`
nothing. A frontend never asks the access questions itself.

A view is a snapshot for listing and offering. An operation never relies on
it: it checks its own permission when it acts.

## Users

Where users come from and how their credentials are verified is pluggable, in
the same way as secrets.

*   A `UserProvider` (`caelumora-model`) is one source of users.
*   A `UserManager` holds an ordered chain of them, configured in the
    configuration's `users` section.

Looking a user up (`find_user`, `get_user`) and verifying a credential
(`authenticate`) are separate operations. Verification never hands out the
stored credential, so backends that verify by binding (LDAP) fit the same
trait as backends that compare a hash.

How a provider assigns `UserId`s is its own business. The only contract is
that the same user keeps the same id, across restarts included.

### How the chain fails

The user chain fails differently from the secret chain:

*   `Ok(None)` passes the question to the next provider.
*   A rejected credential or a failing provider ends it. A broken user file
    must deny, never fall through.
*   An unknown user is reported exactly like a wrong password.

There is no default provider. The only built-in one so far, `accept_any`, is a
development stand-in: it accepts any username and password and keeps users in
the `EntityStore`. It must be enabled explicitly and warns at startup (issues
10, 25).

## Authentication in the REST server

Every route except `POST /auth/login` and `POST /auth/logout` requires a
logged-in user.

*   **Login** verifies username and password through the `UserManager` and
    answers with a signed JWT (HS256) in an `HttpOnly`, `SameSite=Strict`,
    `Secure` cookie (`Secure` unless `auth.secure_cookie` is switched off, for
    plain HTTP in a test setup).
*   **No server-side session store:** the token is the session.
*   **`require_auth`** (`caelumora-rest-server/src/auth.rs`) verifies the
    cookie on each request and puts an `AuthenticatedUser` into it, from which
    handlers derive their `SecurityContext`.

A cookie rather than an `Authorization` header, because `EventSource` cannot
send headers, and an `HttpOnly` cookie is out of reach of injected scripts.

### Renewal

Sessions are renewed by use: any request past half a token's lifetime gets
fresh cookies.

Server-sent events cannot do that. One long response cannot set headers again,
and their reconnects deliberately do not renew either (see
[EVENTS-AND-STREAMS.md](EVENTS-AND-STREAMS.md)). So the web UI calls `POST
/auth/refresh` on a timer *while its tab is visible*; that is what keeps a
session alive for someone who is only watching a channel (issue 31).

Next to the token cookie the server sets `caelumora_session_expires`,
deliberately without `HttpOnly`. It holds only the expiry, and lets the UI -
and any other tab - see when the next renewal is due.

### Bounding a token

A JWT cannot be revoked, so its validity is bounded instead:

*   Tokens are short-lived (`auth.token_lifetime_secs`, default 15 minutes)
    and renewed once past half their lifetime.
*   Only on renewal is the `UserManager` asked whether the user still exists.
*   The `auth_time` claim survives renewal, so a login ends after
    `auth.max_session_secs` (default 12 hours) regardless.
*   The event streams end with the token they were opened with, so the bound
    holds for them too (issue 33).

The signing key is the secret named by `auth.jwt_secret`, resolved through the
secret chain. If it is missing, a random key is generated at startup, and
every restart logs everybody out.

## Secrets

Component configuration - such as an `mcp_tool`'s `auth.secret` - may need to
reference credentials without embedding them in persisted configuration.

*   A `SecretProvider` (`caelumora-model`) is a pluggable source of secrets.
*   A `SecretManager` holds an ordered list of providers and tries them in
    turn, returning the first match.

The built-in providers are:

*   `FileSecretProvider` - a flat JSON key/value file, re-read on every
    lookup, so secrets can rotate without a restart;
*   `EnvSecretProvider` - the process environment.

The configuration's `secrets` section sets the chain; without it, it is
`local/secrets.json`, then the environment.

Every `SecretManager::resolve` is checked with the `SecurityManager`
(`ActionType::ReadSecret`) before any provider is consulted, as the
`EntityStore` and `StateStore` check access on a `SecurityContext`.
