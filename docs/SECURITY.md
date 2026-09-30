# Security

## Security Model

Security in Caelumora is handled through a combination of context and explicit checks.

*   **`SecurityContext`**: Carries information about the initiator and sender of an event, along with the session and channel context.
*   **`SecurityManager`**: A trait responsible for authorizing actions based on the `SecurityContext`.
*   **`ActionType`**: An enumeration of all securable actions (e.g., `GetAgent`, `UpdateSession`, `ListPersonalities`, `CancelChannel`) used by the `SecurityManager` for policy decisions. An action on a stored entity carries the entity's `SecurityAttributes`, not the entity.
*   **`AccessQuestions`**: The pure questions "may they?" asked from outside an entity - by the runtime building views that decide what a client lists and offers (`can_change`, `can_read_context`, issue 80), by the `SessionReader` deciding what an agent may read, by an agent deciding what it may publish or put up for approval: `may_read_channel`, `may_change_channel`, `may_channel_read`, `may_read_agent_context`, `may_publish`, `may_call_tool` (issue 71). An extension trait on every `SecurityManager` by blanket implementation: each question gets a name and one place, but only translates into an `ActionType` - the decision stays with `check`, and no policy can answer a question differently from the action. Operations that check their permission as part of what they do (`ChannelLoop::cancel`, `add_participant`, a store's `update_channel`) keep their check; only pure questions go here, not onto the entities.
*   **`Enforcement::require`**: how such an operation checks (issue 95) - `require(context, action, refusal)` answers a refusal with the operation's own error (`ChannelError::Forbidden`, `StoreError::NotFound` for what the caller may not see, `ToolError::PermissionDenied`, …) and logs every refusal alike, at `debug!`, by `ActionType::describe` - the action and what identifies its object, never content such as a tool call's arguments. Like `AccessQuestions`, an extension trait on every `SecurityManager`. `SecurityManager::check` is called directly only by these two and by filters: a store filtering a list, the event bus choosing whom an event reaches, the commands a user is shown.

One `SecurityManager` is built per process and handed to everything that checks: the stores, the `SecretManager` and the `RuntimeManager`, from which the agent's `GuardedToolExecutor` and every `ChannelLoop` take it. It is a constructor argument rather than something built where it is needed, so that a deployment's policy cannot be silently replaced by a default in one corner of the runtime. Which one is chosen by `AppConfig`'s `security` section: `ownership` (the default) or `allow_all` (`DummySecurityManager`, for tests and development). A policy outside the binary is [24](../issues/24-opa-security-manager.md).

### Ownership

Every stored entity — session, channel, agent, personality, session template, preconfigured component — carries `SecurityAttributes`: an `owner_id` and a list of `contributors` (issue 11). It is the one place ownership is recorded. A session and its channels are owned by the user they were created for, with their users and agents as contributors; an agent is owned by whoever created it and contributes to its own definition; initial data belongs to the system. Entities stored before owners were recorded have an empty owner and count as the system's.

The `OwnershipSecurityManager` decides by those attributes and the context's initiator — the person an action was started for, also when an agent carries it out:

*   the owner may do everything, contributors may read and use (writing into a session, cancelling or waking a channel), and what belongs to the system everybody may read and use;
*   creating is only allowed with oneself as owner, changing and deleting only for the owner;
*   the runtime acting on its own uses `SecurityContext::system()` (`ActorId::System`), which may do everything — deliberately, never as a default: an empty context has no rights.

The stores ask the same way for every entity kind (`store_impl::access`): reads with the attributes as stored, and an entity the caller may not read is reported as not found, so ids cannot be probed; lists are filtered by what the caller may read; changes and deletions are checked against the attributes *as stored*, never those of a new value, which the caller controls; creations against the new entity's. The `EntityStore` trait itself is unchanged. What is not a store call asks first and acts afterwards: the `SessionManager` finds a session for a caller in one place (a private `find`) that checks a running session like a stored one - `GetSession`, and "not found" alike whether it runs - for `open_session`, `stop_session`, `delete_session` and the views; `stop_session` (`StopSession`) and `delete_session` check before they stop anything; renaming stores first — which is where it is checked — and changes the live entity only then.

Not decided yet: roles such as an administrator, and richer access lists ([23](../issues/23-acls-in-security-attributes.md)); and what happens to an owner's entities when the owner is removed (for now they stay).


## Authentication in the REST server

Every route except `POST /auth/login` and `POST /auth/logout` requires a logged-in user. Login verifies username and password through the `UserManager` (see *User Providers*) and answers with a signed JWT (HS256) in an `HttpOnly`, `SameSite=Strict`, `Secure` cookie. There is no server-side session store; the token is the session. The `require_auth` middleware (`caelumora-rest-server/src/auth.rs`) verifies the cookie on each request and puts an `AuthenticatedUser` into it, from which handlers derive their `SecurityContext`. A cookie rather than an `Authorization` header, because `EventSource` cannot send headers and an `HttpOnly` cookie is out of reach of injected scripts.

Sessions are renewed by use: any request past half a token's lifetime gets fresh cookies. Server-sent events cannot do that — one long response cannot set headers again, and their reconnects deliberately do not renew either (see *Streams and their snapshot*) — so the web UI calls `POST /auth/refresh` on a timer *while its tab is visible*, which is what keeps a session alive for someone who is only watching a channel (issue 31). Next to the token cookie the server sets `caelumora_session_expires`, deliberately without `HttpOnly`: it holds only the expiry, and lets the UI (and any other tab) see when the next renewal is due.

A JWT cannot be revoked, so its validity is bounded instead: tokens are short-lived (`auth.token_lifetime_secs`, default 15 minutes) and renewed once past half their lifetime, and only on renewal is the `UserManager` asked whether the user still exists. The `auth_time` claim survives renewal, so a login ends after `auth.max_session_secs` (default 12 hours) regardless. The bound holds for the event streams too, because they end with the token they were opened with (issue 33). The signing key is the secret named by `auth.jwt_secret`, resolved through the secret chain; if it is missing, a random key is generated at startup and every restart logs everybody out.


## Secret Management

Component configuration (such as an `mcp_tool`'s `auth.secret`) may need to reference credentials without embedding them directly in persisted config. `SecretProvider` (`caelumora-model`) is a pluggable source of secrets; a `SecretManager` holds an ordered list of providers and tries them in turn, returning the first match. Built-in providers are `FileSecretProvider` (a flat JSON key/value file, re-read on every lookup so secrets can rotate without a restart) and `EnvSecretProvider` (process environment variables). `AppConfig`'s `secrets` section configures the provider chain; the default, if unspecified, is `[File("local/secrets.json"), Env]`. Every `SecretManager::resolve` call is gated through the `SecurityManager` (`ActionType::ReadSecret`) before any provider is consulted, consistent with how `EntityStore`/`StateStore` gate access on a `SecurityContext`.


## User Providers

Where users come from and how their credentials are verified is pluggable in the same way as secrets. A `UserProvider` (`caelumora-model`) is one source of users; a `UserManager` holds an ordered chain of them, configured in `AppConfig`'s `users` section. Looking a user up (`find_user`, `get_user`) and verifying a credential (`authenticate`) are separate operations, and verification never hands out the stored credential, so backends that verify by binding (LDAP) fit the same trait as backends that compare a hash. How a provider assigns `UserId`s is its own business; the contract is only that the same user keeps the same id, across restarts included.

The chain differs from the secret chain in how it fails: `Ok(None)` passes the question to the next provider, but a rejected credential or a failing provider ends it. A broken user file must deny, never fall through, and an unknown user is reported exactly like a wrong password. There is no default provider. The only built-in one so far, `accept_any`, is a development stand-in that accepts any username and password and keeps users in the `EntityStore`; it must be enabled explicitly and warns at startup (issues 10, 25).

## Where permission checks go

A pure access question - "may they?" asked from outside an entity, to
decide what to list or offer - is a method of
`caelumora_model::security::AccessQuestions`, not of the entity (issue 71).
What a client may see or be offered comes from a view the runtime builds
(`caelumora_model::view`, via `SessionManager::session_view` and
`channel_view`, issue 80), which asks those questions; a frontend never asks
them itself. A view is a snapshot for listing and offering - an operation
never relies on it and checks its own permission when it acts.
An operation that checks its permission as part of what it does keeps that
check itself, and makes it with `Enforcement::require` (issue 95), which
answers a refusal with the operation's own error and logs it.
