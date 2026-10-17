# 177 — Name, scope and description sit in `Metadata` on some entities only

**Status:** done
**Area:** Code structure
**Summary:** Name, scope and description sit in `Metadata` on some entities only, which brings fields to entities that mean nothing there.

## The problem

`Personality`, `SessionTemplate`, `Agent` and `Session` keep their name in `metadata: Metadata {
name, scope, description }`; a `Channel` has a `name` of its own, a `User` a `username`, a
`ComponentDescriptor` only its `id`. Where a name is read therefore depends on the kind of entity,
and the container brings fields to every entity that has it, whether they mean anything there or
not:

| Entity | `scope` read? | `description` read? |
|---|---|---|
| `Personality` | no | no (a catalogue line) |
| `SessionTemplate` | no | yes, `GET /templates` |
| `Agent` | yes: a template finds an agent by name and scope | no - always empty since issue [176](176-participants-in-system-prompt.md) |
| `Session` | no, only set from its template | no, only copied from its template |

## Why it matters

A field that means nothing on an entity is either left empty or filled with a copy (the session's
description from its template, until 176 the agent's from its personality) - the kind of copy issue
[120](120-participant-copies.md) removed for participants. And reading a name differently per kind
is a small trap at every call site.

## Considerations

- **Give `Channel` a `Metadata`, too.** Uniform on the surface, but the channel would get a scope
  that means nothing or contradicts its session, and an empty description: more dead fields, not
  fewer.
- **Dissolve `Metadata`; every entity keeps the fields it has a meaning for.** The rule that falls
  out of what `scope` was meant to be (a name space for looking definitions up by name, issue
  [178](178-scoped-lookup-by-name.md)):
  - **Entities that are a scope** - `Session`, `Channel`, `User` - have a name and no scope: a
    channel does not lie in a scope, it is one.
  - **Definitions that lie in a scope** - `Personality`, `SessionTemplate`, `Agent` - have a name
    and a scope, and a `description` where they are offered for choosing (personality, template).
- **Drop `scope` altogether.** Discussed and rejected: it is the agent's rule of reuse across
  sessions today (`UserScope`: one agent per user, with its memory; `SessionScope`: a new one per
  session), and the name space of issue 178 tomorrow.
- **A trait `Named`.** Only worth having once something works on names of several kinds generically;
  nothing does.
- **`User::username` stays.** It is the login identity, what the store finds a user by, in a record
  that belongs to the system (issue [94](94-users-unchecked-in-store.md)).

## Done when

- `Metadata` is gone. `Session` and `Channel` have a `name`; `Personality` and `SessionTemplate` a
  `name`, a `scope` and a `description`; `Agent` a `name` and a `scope`.
- The REST API is unchanged; `examples/seed.yaml` and the documents in `docs/` follow the new shape.

## History

- **2026-10-01** — Issue created at the maintainer's request, while working on issue 176. Decided:
  dissolve `Metadata` by the rule above, keep `scope` on the definitions, and start with a fresh
  store - documents stored in the old shape are not read any more, and no code is kept to read them.
  The lookup that `scope` is for is issue 178.
- **2026-10-01** — Done. `Metadata` is gone; its module `meta` holds only `Scope` and is now
  `scope`. `Session` has a `name`; `Personality` and `SessionTemplate` a `name`, a `scope` (global
  unless named) and a `description`; `Agent` a `name` and a `scope`. The template's own `scope:
  ScopeTemplate`, which only set the session's scope, is gone with it - `ScopeTemplate` remains for
  a template's agents. The REST API is unchanged; `examples/seed.yaml` and `docs/STORAGE.md` follow
  the new shape. The local store was started afresh.
- **2026-10-01** — Followed up at the maintainer's proposal: a name that something is looked up by
  has a type of its own. `Name` (`caelumora-model/src/name.rs`) is checked when made or read, keeps
  its text with whitespace tidied and compares regardless of case; `ScopedName` is a `Name` with its
  `Scope`, flattened into the document as `name` and `scope`, made from a text and a scope
  (`ScopedName::new`). `Personality`, `SessionTemplate` and `Agent` hold a `ScopedName` in their
  field `name`; a `Channel`, a `ChannelTemplate` and a template's agent a `Name`. A session's name
  stays a `String` - it is only shown - and so does a user's `username`, the login identity a user
  provider owns. Renaming a channel or an agent takes a `Name`; `POST …/channels` reads its `name`
  as one, so a text that is no name is a malformed request (`invalid_request_body`) and
  `SessionError::EmptyChannelName` is gone. Uniqueness per scope is still issue
  [178](178-scoped-lookup-by-name.md).
- **2026-10-01** — Names are compared by their ASCII form, at the maintainer's request: a `Name`
  keeps its text as written and, made once with it, the key it is compared and hashed by -
  transliterated by `any_ascii`, lowercased, whitespace tidied - so "Zoë" and "Zoe" are one name.
  Made eagerly rather than lazily (`OnceLock`): a name never changes and is made in two places only.
  Handles follow the same form - "Zoë" is `@zoe`, and a mention typed as `@Zoë` is normalised
  alike - for users, whose handle comes from their `username`, too. The web UI derives handles with
  the npm package `any-ascii`; crate and package are pinned to the same version (0.3.3), so that
  their tables agree.
