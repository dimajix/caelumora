# 24 — A `SecurityManager` backed by Open Policy Agent

**Status:** open
**Area:** Identity & access management

## The problem

`DummySecurityManager` approves everything, and it is the only implementation. Whatever replaces it
will encode a policy — who may start a session from which template, which agent may read which
secret, who may approve a tool call — and if that policy is Rust, then every change to it is a
recompile and a redeploy of the runtime. For the rules that differ per deployment, and that an
organisation's security people rather than its developers own, that is the wrong place.

Open Policy Agent is the usual answer: policy as Rego, evaluated against an input document,
versioned and tested separately from the application. The `SecurityManager` trait is already the
right seam — one method, one decision — so an OPA-backed implementation is a drop-in alternative
rather than a restructuring.

## Why it matters

Two things push in this direction. Policies that are interesting are conditional in ways an ACL
cannot express — time of day, the delegation chain ([12](12-user-and-agent-permissions.md)), the
number of agents a user already has running, deny rules — and writing an evaluator for that in-house
means building a policy language by accident. And an externalised policy can be reviewed and tested
as a unit, which for the component that decides every access is worth more than the convenience of
having it inline.

What it does *not* replace is [23](23-acls-in-security-attributes.md). A policy engine still needs
to know who owns an object and who was granted what; that data lives on the entities. OPA is a way
to evaluate policy over those attributes, not a way to avoid having them.

## Considerations

- **Embedded or sidecar.** OPA as a separate process reached over HTTP is the deployment everyone
  documents, and it costs a network round trip on a call that happens on every store operation and
  every tool call. Evaluating Rego in-process — a Rust Rego interpreter, or OPA compiled to Wasm —
  keeps the hot path local and still allows the policy bundle to be fetched and reloaded without a
  restart. For a single-process runtime ([21](21-scaling.md)) the embedded form is clearly the
  better default; the sidecar becomes interesting exactly when there are several runtime processes
  that must decide alike.
- **The check is on the hot path.** Every `EntityStore` call and every `CallTool` goes through it,
  and if listing filters elementwise ([23](23-acls-in-security-attributes.md)) then one list of N
  items is N evaluations. A decision cache — keyed on the context, the action shape and a version of
  the attributes — has to be part of the design rather than a later optimisation, and so does its
  invalidation when an ACL changes.
- **The input document is a contract.** `ActionType` currently carries borrowed references to whole
  entities (`&'a Personality`, `&'a Session`, `&'a ComponentDescriptor`). Serialising those
  wholesale into a policy input is both expensive and a leak: policies would start matching on
  internal fields, and the model could no longer be refactored without breaking deployments'
  policies. What is needed is an explicit, versioned projection — subject, action, resource kind,
  resource attributes — which is the same projection an ACL evaluator needs, and the reason 23
  should land first.
- **Fail-closed.** If the policy cannot be evaluated — bundle missing, sidecar unreachable, Rego
  error — the answer is deny. `ComponentFailPolicy` already has the vocabulary
  (`FailOpen`/`FailClosed`) but defaults to `FailOpen`, which is right for a context provider and
  wrong for a policy decision point. The startup case deserves its own thought: a runtime that
  starts with no policy and denies everything is at least honest, but it must say so loudly rather
  than look like a broken deployment.
- **Decision logging.** OPA produces a decision log, and a decision log is precisely the audit trail
  [05](05-postgres-event-recorder.md) is about. Having two independent audit streams that must be
  correlated afterwards is worse than one; the preference is for decisions to flow into the
  runtime's own `Recorder`, with the policy bundle version attached so that a past decision can be
  explained by the policy that made it.
- **Partial evaluation.** OPA can return a residual — "these are the conditions under which you may
  see a row" — instead of a yes/no per object. That is the efficient answer to list filtering and
  translates to a SQL predicate once there is a Postgres store ([03](03-postgres-entity-store.md)).
  It is also a significantly larger piece of work than a boolean check, and should not be a
  prerequisite for the first version.
- **Where the policy's data comes from.** Rego needs facts: a subject's effective roles, ownership,
  which agent belongs to whom. Pushing them into OPA as data documents means they can be stale;
  having the policy fetch them means latency inside the decision and a potential loop back into the
  runtime. Carrying the relevant attributes in the input document avoids most of this, which is
  another reason the input projection deserves care.
- **It must stay optional.** OPA is an operational dependency — a bundle to build, distribute and
  version — and the local, single-user deployment should not need it. The built-in ACL-evaluating
  manager from 23 stays the default; this is one more implementation of the trait, selected in
  `AppConfig` the way the secret provider chain already is.
- **Testing is a real benefit and should be used.** Rego policies are unit testable, and a policy
  test suite living in the repository is the only way the shipped example policy stays correct as
  `ActionType` grows.

## Done when

- The integration shape is decided — embedded evaluation or a sidecar — with the reason recorded
  here.
- The input document has an explicit, versioned schema that does not simply serialise internal model
  types, and `ActionType` maps onto it in one place.
- Evaluation failures deny, and denials are recorded with the policy version that produced them.
- A caching strategy exists for the per-call and per-list cases.
- An example policy plus its tests live in the repository, and the OPA backend is selectable in
  configuration without being required.

## History

- **2026-09-13** — Issue created alongside [23](23-acls-in-security-attributes.md), which supplies
  the attributes any policy — in-house or Rego — has to decide on.
