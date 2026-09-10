# 10 — User authentication

**Status:** open
**Area:** Identity & access management

## The problem

There is no authentication anywhere in the system. The REST handlers construct
a `SecurityContext` themselves rather than deriving one from a credential, and
the only `SecurityManager` implementation is `DummySecurityManager`
(`caelumora-model/src/security.rs:85`), which is not a policy.

Every caller is therefore whoever they claim to be, with unrestricted rights.

## Why it matters

Nothing else in the access-control area can be built on top of an unidentified
caller. [11](11-agent-ownership.md) needs an owner,
[12](12-user-and-agent-permissions.md) needs a subject to combine permissions
for, and [14](14-approval-privilege.md) is meaningless if anyone can claim to
be an approver. This issue gates all three.

It also gates deployment beyond a trusted single-user machine: the REST server
in its current form must not be exposed to a network.

## Considerations

- The structure to receive the result already exists. `SecurityContext`
  carries `initiator_id`, `caller_id` and `delegation_ids`; the work is to
  populate it from an authenticated credential at the REST boundary instead of
  fabricating it, and to give `SecurityManager` a real implementation.
- The `User` entity already lives in the `EntityStore`, so there is a place to
  hang credentials — though credential material should go through
  `SecretProvider`, not into the entity store as plaintext.
- Token-based (bearer/JWT) is the natural fit for the REST server and costs
  the least in the TUI and local-client paths, where authentication may be
  bypassed entirely by an explicit trusted-context construction. That bypass
  must be a single, obvious, greppable place.
- The `caelumora-local-client` case deserves a decision: it runs in-process
  and has no network boundary. Requiring authentication there is arguably
  pure cost — but so is having two code paths.

## Done when

- The REST server rejects unauthenticated requests.
- `SecurityContext` at every entry point reflects an authenticated identity.
- A real `SecurityManager` replaces `DummySecurityManager` outside of tests.

## History

- **2026-09-10** — Issue created from `TODO.md`.
