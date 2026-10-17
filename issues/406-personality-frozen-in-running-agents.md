# 406 — A running agent keeps its personality as it was when its actor was built

**Status:** open
**Type:** bug
**Priority:** medium
**Area:** Sessions and channels
**Summary:** A running agent's actor takes its personality once, when it is built, while other parts of the runtime read it anew from the store, so a change of a personality reaches some of its uses and not others - its limiting roles among them.

## The problem

An agent's personality reaches the runtime on two paths, which behave differently.

**Read anew for every use.** `AgentController::personality()` reads it from the store through
`personality_id` each time, "so that a change of the personality reaches it" (issue
[176](176-participants-in-system-prompt.md)). Its users:

*   `channel/participation.rs`: resolving a participant, and whether its owner may still read the
    personality (`require_runnable`, issue [191](191-channel-runtime-hands-out-runtime-manager.md));
*   `workspace/reaper.rs`: the agent's `limiting_roles`, for whether a workspace is still usable in
    a channel;
*   `view/member.rs`: the introduction and the kind, read when first asked for and kept with the
    `MemberView`.

**Taken once, when the actor is built.** `AgentActor::new` and `ExternalAgentActor::new` are given
the `Personality` as a value and keep parts of it:

*   `AgentBody::build` builds the `components` and puts `limiting_roles` into the agent's context
    ("limited by its personality", `actor/body.rs`); `security_context()` later takes the owner's
    roles anew, but the limit as it was built;
*   `AgentActor` copies `reasoning_visibility`.

So the same personality is current in one place and stale in another:

*   **Limiting roles.** The reaper judges a workspace by the personality's limit as it is now; the
    running agent acts with the limit as it was. A personality narrowed to fewer roles still lets
    its running agents act with the wider ones until their actors are built again - in a channel
    whose classification the narrowed limit would no longer clear.
*   **Who sees the reasoning.** A personality changed to `reasoning_visibility: owner` keeps
    showing its running agents' reasoning to the channel.
*   **A share withdrawn** (issue 191) stops an agent only where it is resolved again; a running one
    goes on.
*   **The introduction** is current in the views and in what other agents are told
    (`channel_members_prompt_provider`), but an actor has no introduction of its own to give - which
    the responder policy of issue [403](403-decision-model-policy.md) needs, through
    `Actor::describe()` returning an `ActorDescription` (decided there).

Personalities are changed today past any controller: there is no REST endpoint for it, the seed
does not overwrite one that exists, and a change is made in the store (with the JSON store, by
editing its file). The stale copies show as soon as that happens, and every future way of editing
personalities will run into them.

## Why it matters

*   The limiting roles are a security boundary ([CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md#personalities-limit-their-agents)):
    narrowing them has to take effect, and two parts of the runtime must not disagree on what an
    agent is cleared for.
*   Reading the personality from the store for every use is what issue
    [182](182-entity-reads-not-cached.md) wants to get rid of; copying it once is what makes it
    stale. Neither is the right place for it.
*   `describe()` (issue 403) should answer synchronously from what the actor holds, as `name()`
    does from the live agent document - which requires the personality to be held live as well.

## Considerations

### A personality held live

The pattern exists: the `AgentController` holds its agent as a `LiveDocument`, the `UserController`
its user, both kept in step when they change elsewhere. A personality is shared by many agents, so
it gets a controller of its own:

*   **A `PersonalityController`** per personality, held by the `ControllerManager` like the others
    and shared by the `AgentController`s of its agents. An agent cannot change its personality
    (`AgentUpdateEvent` has only `NameChanged`), so an `AgentController` keeps one for its life.
*   `AgentController::personality()` answers synchronously with the personality as it is now. The
    callers in `participation`, `reaper` and the views become simpler; the views keep reading the
    store where no session runs (as issue 182 discusses for agents and users).
*   The actor reads `limiting_roles`, `reasoning_visibility` and the introduction through it when
    it uses them, instead of keeping copies.

This is option (a) of issue 182, motivated here by correctness rather than by the number of reads;
182's caching follows from it for personalities.

### What cannot follow live

The `components` are built. A changed list of components takes effect when the actor is built
again - when the channel is started anew. That is the one deliberate exception, and it is said so
in [AGENT-TURN.md](../docs/AGENT-TURN.md) or [COMPONENTS.md](../docs/COMPONENTS.md). Whether a
running agent should be rebuilt on such a change is a question of its own, not of this issue.

### How a change reaches the controller

Personalities have no writer in the runtime today (182: "the decision whether personality changes
go through a controller at all"). Options:

*   **(a) The controller is their one writer**, as for agents and users: a change goes through it,
    is stored, taken over live and announced. Needs every path that changes a personality to go
    through it - none exists in the runtime yet, so nothing has to be redirected.
*   **(b) The store announces changes** and the controller follows. Covers changes made past the
    runtime (a file edited), but the JSON store cannot announce them, and it is a second mechanism
    beside `LiveDocument`.

Recommended: (a), since it is the existing pattern; an edit past the runtime is then seen when the
controller is made anew, as for agents and users today.

### Security

Taking `limiting_roles` live changes what a running agent may do: a narrowed limit holds at once,
a widened one too. Both belong in the security suite (`caelumora-agents/tests/security_suite.rs`):
an agent whose personality is narrowed while it runs is refused what the narrowed limit does not
clear; the reaper and the agent agree. A share withdrawn while the agent runs (191) - whether the
running actor stops, or only refuses from then on - is to be decided with it.

## Done when

*   A `PersonalityController` holds each personality in use live; `AgentController::personality()`
    answers from it without reading the store.
*   A running agent uses its personality's `limiting_roles`, `reasoning_visibility` and
    introduction as they are now; only its components stay as they were built, and the documents say
    so.
*   The reaper and the running agent judge by the same limit; the security suite covers a
    personality narrowed while its agent runs.
*   Issue 182 records which of its reads this removed.

## History

- **2026-10-17** — Opened while discussing issue 403: `Actor::describe()` needs the introduction
  live, and looking for where the personality is held showed that a running agent keeps parts of it
  as they were built while other parts of the runtime read it anew.
