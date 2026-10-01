# Agent Guidelines

Welcome, coding agent! This document is your entry point for contributing to
Caelumora. It holds the rules that apply to every change; the details live in
[docs/](docs/), and the table below says which document to read before working
on what.

## Documentation

Start with [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), which draws the
overall picture. Before you change something, read the document on its
subject:

| When you work on ... | read |
|---|---|
| any code | [CODING-GUIDELINES.md](docs/CODING-GUIDELINES.md) |
| components, facets, `ComponentRegistry` | [COMPONENTS.md](docs/COMPONENTS.md) |
| tools, tool execution, approvals, user commands | [TOOLS-AND-COMMANDS.md](docs/TOOLS-AND-COMMANDS.md) |
| the agent's cognitive cycle, `Turn`, transcript, prompt parts | [AGENT-TURN.md](docs/AGENT-TURN.md) |
| notes, private notes, agent memory, reading other channels | [MEMORY.md](docs/MEMORY.md) |
| sessions, channels, actors, participants, moderators | [CHANNELS.md](docs/CHANNELS.md) |
| the event bus | [EVENTS.md](docs/EVENTS.md) |
| the REST API, login and tokens, server-sent event streams | [REST-SERVER.md](docs/REST-SERVER.md) |
| the web UI | [WEB-UI.md](docs/WEB-UI.md) |
| stores, controllers, persistence | [STORAGE.md](docs/STORAGE.md) |
| permissions, ownership, the views, users, authentication, secrets | [SECURITY.md](docs/SECURITY.md) |
| confidentiality levels and labels | [CONFIDENTIALITY.md](docs/CONFIDENTIALITY.md) |
| `AppConfig` and its sections | [CONFIGURATION.md](docs/CONFIGURATION.md) |

## Issue Tracking

Open issues live in this repository, not in an external tracker. The overview
is [issues/INDEX.md](issues/INDEX.md); every issue additionally has its own
file `issues/<number>-<slug>.md` holding the reasoning, the options that were
weighed, and the decision.

Read `issues/INDEX.md` before starting work. If what you are about to do
corresponds to an issue, work from that issue's file — it usually records why
an obvious approach was already rejected. How issues are kept up to date and
created is described at the top of the index, under *How this is maintained*.
In short: an issue is a line in the index *and* a file, its status changes in
both, the description of a resolved issue is never rewritten (what happened
goes under `## History`), and numbers are never reused.

## Rules for every change

The reasons and details are in
[CODING-GUIDELINES.md](docs/CODING-GUIDELINES.md).

*   No `unwrap()` or `expect()` in production code.
*   Errors in a turn end the turn, not the agent: don't let a turn-time error
    escape `AgentActor::run`.
*   Error messages name what failed and never contain secret values or file
    paths.
*   Log through `tracing`, never through `log` or `println!`; message content
    only at `trace!`, credentials never.
*   REST handlers return `Result<_, crate::error::ApiError>`; never map an
    error to a bare status code. Keep the REST API compatible with
    `caelumora-web-ui`.
*   A pure access question is a method of `AccessQuestions`, an operation
    checks its own permission with `Enforcement::require`, and a frontend
    never asks the `SecurityManager` itself (see
    [SECURITY.md](docs/SECURITY.md)).

## Contribution Workflow
1.  **Analyze**: Understand the requirement and how it fits into the
    architecture. Check [issues/INDEX.md](issues/INDEX.md) for an existing
    issue covering it.
2.  **Plan**: Identify the files that need modification and the tests
    required.
3.  **Implement**: Apply minimal, focused changes following the coding style.
4.  **Document**: Update the document in `docs/` on the subject,
    `docs/ARCHITECTURE.md` and/or `README.md` if your changes introduce new
    architectural concepts, modules, or significant features.
5.  **Verify**: Run existing tests and add new ones to cover your changes.
6.  **Refactor**: Ensure the code is clean and well-documented.
7.  **Record**: If the change resolves or advances an issue, update its status
    in *both* `issues/INDEX.md` and the issue file, and append a dated entry
    under `## History`. If it uncovered something new, open an issue for it
    rather than leaving it in a comment.

### Architecture decisions
Architecture questions are discussed before they are implemented: where a
method lives, which object owns a responsibility, how context reaches the
code that needs it, new types or layers. Lay out the options with their
trade-offs, give a recommendation, and wait for the decision. Proposals from
the maintainer are to be assessed honestly, not just accepted.

Prefer the clean structural solution over a pragmatic shortcut, even when it
touches more code — and say what it costs. Glue code that feels awkward (one
object making another act on its behalf, a parameter that only works around
missing ownership) is a sign that something sits on the wrong object; raise
it rather than building around it.

Solve problems along the patterns the codebase already has before inventing
new ones: a new capability that resembles an existing one is built the same
way (commands like tools: a `…Provider` component facet, and something that
receives the ready providers and checks permissions - for tools the
runtime's `GuardedToolExecutor` in front of the agent's `ToolExecutor`
component, issue 127). When a proposal adds a
second implementation of something that exists, look for differences between
the two that have no reason — naming, construction, error handling — and
propose aligning them, in whichever direction is the better one. A deliberate
difference is fine, but say why it exists.

### Branches and commits
Until further notice, work directly on `main`. Do not create feature
branches unless asked to.

Do not commit on your own: leave the changes in the working tree and say
what they are, so that the maintainer can review them first. Commit only
when explicitly asked to - and then only what was reviewed. The same holds
for pushing.
