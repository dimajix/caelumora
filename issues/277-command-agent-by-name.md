# 277 — A command finds the agent it names only by its exact handle, not by its name

**Status:** done
**Area:** User interaction
**Summary:** A command such as `/wakeup` finds the agent it names only by its exact handle, so `/wakeup Sassy` or `@Sassy` names no agent.

## The problem

`/wakeup sassy` wakes the agent "Sassy", but `/wakeup Sassy`, `/wakeup @Sassy` and `/wakeup Dr. Who`
answer that there is no such agent. `WakeupCommandProvider::execute`
(`caelumora-runtime/src/command/wakeup_command.rs`) strips a leading `@` from the argument and
compares what is left as a string with the member's handle (`ChannelMember::handle`) - which is the
normalised form of the name (`mention::handle_of`: ASCII, lower case, other characters as `-`). What
the user typed is not normalised, so only the handle exactly as the system derives it is found.

The agent commands of issue [269](269-agent-commands.md) do the same: `UserActor::request_command`
(`caelumora-runtime/src/actor/user.rs`) compares the `@handle` that names the agent as a string
with `ChannelMember::handle`, so `/compact @Sassy` is refused as naming no agent that carries it
out.

Everywhere else a name is looked up regardless of case and of how it is written: `Name`
(`caelumora-model/src/value/name.rs`, issue 177) is compared by its folded form, and a mention in a
message is normalised with `handle_of` before it is looked up in the `MentionDirectory`.

## Why it matters

A user types the name as they see it - "Sassy", as the participant list shows it - and is told
there is no such agent, while the same name works in an `@`-mention in a message. Two lookups of the
same thing that behave differently are a source of confusion and of further copies of the mistake.

## Considerations

**Convert the argument to a `Name` and compare by it** (the maintainer's proposal). That is what
`Name` is for: "Sassy", "sassy" and "SASSY" are one name, and so are "Zoë" and "Zoe". It does not
cover the handle, though: the error message of `/wakeup` lists the agents as `@handle`, and
`@dr-who` as a `Name` is not "Dr. Who" (the fold keeps the `.` and the space). A command that takes
an agent should take it both as it is named and as it is mentioned.

**Normalise the argument with `handle_of` and compare handles.** One step, and it accepts every
spelling: `handle_of("Dr. Who")`, `handle_of("@Dr-Who")` and `handle_of("dr-who")` are all
`dr-who`. But a handle is coarser than a name - "Dr. Who" and "Dr-Who" are two names with one handle
- so it can be ambiguous, and `find` silently takes the first.

**Recommendation:** one lookup, owned by the channel, that both commands use - e.g.
`ChannelLoop::members_named(&str)` - which takes the text as a `Name` and, if no participant has
that name, as a handle (`handle_of`); it returns every match, so that a caller can refuse an
ambiguous one instead of picking one. `/wakeup` and `request_command` then lose their own string
comparison.

Next to it: `ChannelMember::name` is a `String`, as is `Actor::name()`. If it were a `Name`, the
comparison would need no conversion on the member's side; whether that is worth the change of
`Actor::name()` is to be decided with this issue.

## Done when

- `/wakeup Sassy`, `/wakeup sassy`, `/wakeup @Sassy` and `/wakeup Dr. Who` / `/wakeup @dr-who` find
  the agent, and an agent command takes `@Sassy` as well as `@sassy`.
- An argument that matches several agents is refused with their names, not resolved to one of them.
- Both commands look the agent up in one place; tests cover case, the handle form and ambiguity.

## History

- 2026-10-09: Opened at the maintainer's request: `/wakeup` compares the agent it is given as a
  string with the handle, so it depends on case; it should be taken as a `Name` and looked up by it.
- 2026-10-09: Done. `ChannelLoop::agents_named` finds the agents a text names - as a `Name` first,
  then as a handle (`handle_of`) - and both `/wakeup` and `UserActor::request_command` use it; an
  ambiguous text is refused with the names it matches. `Actor::name()` and `ChannelMember::name`
  are now a `Name`; the agent's and the user's actor read it from their entity, the approver's is
  `ActorId::approver_name()`, a name the code fixes (`Name::fixed`, checked by a test).
  `EventContext::new`/`next` and `Answering::own`/`to` take the sender's name as
  `impl Into<String>`, so that an actor hands its `Name` over as it is. `AgentController::name()`
  and `UserController::username()` still answer a `String`.
