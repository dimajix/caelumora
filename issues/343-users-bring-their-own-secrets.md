# 343 — A user cannot bring a secret of their own

**Status:** open
**Area:** Identity and access management

## The problem

Every secret comes from the providers the configuration names - a file, the environment, later a
vault - and is put there by whoever runs the server. A user who wants an agent of their own to
reach a remote system with their own credential - their GitHub token for an `mcp_tool`, their
account at an external agent - has no place to put it: they would have to hand it to the operator,
who would then keep it among the server's secrets, readable by whatever the configuration grants.

Issue [310](310-secrets-have-no-owner.md) gives the configured secrets an access list and leaves
their owner the system, since nobody else puts their values there. A secret a user brings is the
other case: it is theirs, and only they decide who may use it.

## Why it matters

- **Agents of one's own** need credentials of one's own. An agent acts with its owner's rights
  (issue [255](255-whose-name-agents-act-in.md)); towards a remote system those rights are a
  credential, and today only the operator's can be used.
- **A shared configuration with personal credentials:** a preconfigured component - a GitHub tool
  an administrator shares - could reach the remote side as each agent's owner, with that owner's
  token, instead of with one token everybody shares.
- **Credentials should not travel through people.** Handing a token to the operator to put into a
  file is exactly what a place of one's own avoids.

## Considerations

### Where it fits

- **A personal secret lies in its user's scope** (`Scope::UserScope`), as a workspace or a skill
  does: owned by the user, with an access list like every entity. The question asked of it is the
  one 310 introduces for the configured secrets - `Use` of the kind `secret` -, only its attributes
  are the secret's own instead of a provider's.
- **It is found as a name is found** (`scope::find_by_name`, STORAGE.md): from the principal's place
  - for an agent its owner's -, first in the user's own scope, then in the global one, which is the
  configured chain as 310 filters it. A component that names `GITHUB_TOKEN` then reaches with the
  owner's own token where they brought one, and with the configured one otherwise.
- **Only the user's scope, not a session's.** A member of a session could otherwise put a secret
  into it that hides the one another member's agent means, so that the agent reaches with the
  attacker's token - pushing to the attacker's repository, writing into the attacker's account. In
  one's own scope one can only hide something from oneself.

### Where the values are kept

- The store keeps its documents as JSON on disk; a value cannot lie there in the clear.
- **Proposed:** the secret's metadata - its name, scope, attributes, origins - is an entity in the
  store; its value is kept beside it, encrypted (AES-GCM) with a key of the system that the server
  reads from the configured chain, as it reads the signing key of its tokens today.
- Later, a backend that writes the value into a vault instead, with the entity keeping only a
  reference to it.

### A value is written, never read back

- The REST API creates, replaces and deletes a secret and lists names and metadata; there is no
  way to read a value back, not even for its owner.
- No value is carried by an event, written to a log or shown to a model; it reaches the remote side
  as a header, as the configured ones do.

### Sharing, and where it may be sent

- Granting others `Use` lets their agents reach with the owner's credential - useful for a team's
  token. But a grantee can then write a component that sends it to a URL of their own.
- **Proposed rule of the model:** a secret granted to anybody but its owner must name the origins
  it may be sent to (310's binding); an owner using their own secret may leave them out.

### Turns caused by others

- An agent in a shared channel works on turns other members cause. Its tool calls then reach with
  its owner's credential, and the remote system attributes what is done to the owner. That follows
  from 255 - an agent acts with its owner's rights -, but a credential makes it visible outside.
- Either the approval of tool calls (`ApproveToolCall`, [APPROVALS.md](../docs/APPROVALS.md))
  carries this, and the documentation says so; or a personal secret is resolved only where the
  initiator is its owner, or the owner approved the call - the `SecurityContext` tells the
  initiator from the principal already.
- Whose credentials a call reaches with is delegation in the sense of issue
  [341](341-identity-ownership-delegation.md); when an agent may act for someone other than its
  owner, this is decided by delegation, not by ownership.

### Related

- [340](340-teams-as-owners.md): a team owning secrets is the same mechanism with a team's scope.
- [24](24-opa-security-manager.md): an external policy is given the secret's attributes, as for any
  entity.

## Done when

- A user creates, replaces and deletes secrets of their own through the REST API and the web UI,
  and can never read a value back.
- A component in an agent of theirs that names such a secret reaches with it; the same component in
  another user's agent does not.
- A secret granted to others names the origins it may be sent to, and is sent nowhere else.
- Values are kept encrypted and appear in no event, log or transcript.
- The security suite shows that a user can neither use another's secret without a grant nor hide
  one from another's agent.

## History

- **2026-10-13** — Opened at the maintainer's request, from the discussion of
  [310](310-secrets-have-no-owner.md): users should be able to build agents of their own that reach
  remote systems with their own credentials. Builds on 310, which decides the question asked of a
  secret and how the configured chain is filtered by it.
