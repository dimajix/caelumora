# 178 — Definitions cannot be looked up by name within their scope

**Status:** open
**Area:** Persistence

## The problem

`Scope` was meant as a name space: a personality, a session template or an agent lies in a scope - a
channel, a session, a user, or everywhere - and a lookup by name from some place sees what lies in
that place and in the places around it, the innermost first. A user who adds an agent to a channel
would be offered the personalities of that channel, of its session, their own and the global ones; a
personality of one session would exist nowhere else.

None of this is built:

- **No lookup.** Nothing looks a definition up by name with that hierarchy; `list_personalities`
  returns everything the caller may read.
- **The one check there is goes the wrong way.** A template finds an existing agent with
  `scope.contains(&agent.scope)` (`caelumora-runtime/src/template.rs`, `agent`): whether the
  *requested* scope contains the agent's. A request in `GlobalScope` therefore matches every agent
  of that name, the session agent of another user included, if the caller may read it. A lookup from
  a place needs the converse: whether the definition is visible from there.
- **The personality is ignored.** The same lookup finds an agent by name and scope only: a template
  that names "Jonathan" with another personality gets the old Jonathan with the old personality.
- **No uniqueness.** Two definitions of one name in one scope can be created.
- **No lifetime.** What lies in a session's or a channel's scope ought to go with it.

## Why it matters

Without it, every definition is global in effect, and scope is a field that only templates' agents
read - in a way that finds too much.

## Considerations

- **Scope is not ownership.** The owner (`SecurityAttributes`) decides who may change a definition,
  the scope where it can be found by name. A global personality belongs to the system; a personality
  of one session may belong to the session's owner and still be offered only there. Whether a caller
  may read what the lookup finds is asked as always.
- **Where the question lives.** "Visible from this place" is a question of the model (`Scope`),
  answered with the place's chain (channel, session, user, global); the store lists, the caller
  filters - or the `EntityStore` gets a lookup of its own, if a backend can do better (issue
  [03](03-postgres-entity-store.md)).
- **Shadowing.** An inner definition hides an outer one of the same name.
- **Reuse of agents.** A template's agent in `UserScope` is the one agent of that name and
  personality of the user, with its memory; in `SessionScope` a new one per session. That rule
  should be the lookup's, not a second one beside it.

## Done when

- A definition can be looked up by name from a place, seeing its own scope and the ones around it,
  innermost first; names are unique per scope.
- Templates find their agents through it, by personality as well as name.
- Definitions of a session's or a channel's scope go with it.

## History

- **2026-10-01** — Issue created at the maintainer's request, while dissolving `Metadata` (issue
  [177](177-metadata-on-some-entities.md)), which keeps `scope` on the definitions for this.
