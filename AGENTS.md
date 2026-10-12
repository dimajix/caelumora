# Agent Guidelines

Welcome, coding agent! This document is your entry point for contributing to Caelumora. It holds the
rules that apply to every change; the details live in [docs/](docs/), and the table below says which
document to read before working on what.

## Documentation

Start with [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), which draws the overall picture. Before you
change something, read the document on its subject:

| When you work on ... | read |
|---|---|
| any code | [CODING-GUIDELINES.md](docs/CODING-GUIDELINES.md) |
| components, facets, `ComponentRegistry` | [COMPONENTS.md](docs/COMPONENTS.md) |
| tools, tool execution | [TOOLS.md](docs/TOOLS.md) |
| approvals, questions a tool asks a person | [APPROVALS.md](docs/APPROVALS.md) |
| user commands, commands of an agent | [COMMANDS.md](docs/COMMANDS.md) |
| the agent's cognitive cycle, `Turn`, transcript, prompt parts | [AGENT-TURN.md](docs/AGENT-TURN.md) |
| notes, private notes, agent memory, reading other channels | [MEMORY.md](docs/MEMORY.md) |
| skills: the catalogue, loading and writing them | [SKILLS.md](docs/SKILLS.md) |
| sessions, channels, actors, participants, moderators | [CHANNELS.md](docs/CHANNELS.md) |
| the event bus | [EVENTS.md](docs/EVENTS.md) |
| the REST API, login and tokens, server-sent event streams | [REST-SERVER.md](docs/REST-SERVER.md) |
| the web UI | [WEB-UI.md](docs/WEB-UI.md) |
| stores, controllers, persistence | [STORAGE.md](docs/STORAGE.md) |
| workspaces, their storages, commands run in them | [WORKSPACE.md](docs/WORKSPACE.md) |
| permissions, ownership, the views, users, authentication, secrets | [SECURITY.md](docs/SECURITY.md) |
| where a permission is checked, in whose name, by which concept | [PERMISSION-CHECKS.md](docs/PERMISSION-CHECKS.md) |
| confidentiality levels and labels | [CONFIDENTIALITY.md](docs/CONFIDENTIALITY.md) |
| `AppConfig` and its sections | [CONFIGURATION.md](docs/CONFIGURATION.md) |
| which settings suit which language model | [LLM-MODELS.md](docs/LLM-MODELS.md) |

## Issue Tracking

Open issues live in this repository, not in an external tracker. The overview is
[issues/INDEX.md](issues/INDEX.md); every issue additionally has its own file
`issues/<number>-<slug>.md` holding the reasoning, the options that were weighed, and the decision.

Read `issues/INDEX.md` before starting work. If what you are about to do corresponds to an issue,
work from that issue's file — it usually records why an obvious approach was already rejected. How
issues are kept up to date and created is described at the top of the index, under *How this is
maintained*. In short: an issue is a line in the index *and* a file, its status changes in both, the
description of a resolved issue is never rewritten (what happened goes under `## History`), and
numbers are never reused.

[issues/OPEN.md](issues/OPEN.md) lists only the issues that are open or in progress. It is generated
from the index by `issues/open.sh` and never edited by hand; the pre-commit hook in `.githooks`
keeps it up to date, once a clone has it enabled (`git config core.hooksPath .githooks`). The same
hook refuses a commit of Rust code while `cargo clippy --workspace --all-targets` warns.

## Rules for every change

The reasons and details are in [CODING-GUIDELINES.md](docs/CODING-GUIDELINES.md).

*   Import what you use with `use` at the top of the module and write it by its name: `impl
    Stored for Skill`, `Skill::KIND` - never `impl crate::removal::Stored for Skill` or
    `caelumora_model::skill::Skill::KIND`, also not for a single use or in a test. A path is
    written out only where an import cannot serve (see
    [CODING-GUIDELINES.md](docs/CODING-GUIDELINES.md#imports)).
*   No `unwrap()` or `expect()` in production code.
*   Errors in a turn end the turn, not the agent: don't let a turn-time error escape
    `AgentActor::run`.
*   Error messages name what failed and never contain secret values or file paths.
*   Log through `tracing`, never through `log` or `println!`; message content only at `trace!`,
    credentials never.
*   REST handlers return `Result<_, crate::error::ApiError>`; never map an error to a bare status
    code. Keep the REST API compatible with `caelumora-web-ui`.
*   A type takes the services it uses - the `SecurityManager`, the stores, the event bus - when it
    is made and keeps them as fields; it does not fetch them through the runtime at every use.
*   A pure access question is a method of `AccessQuestions`, an operation checks its own permission
    with `Enforcement::require` - before it asks anything of others - and a frontend never asks the
    `SecurityManager` itself (see [SECURITY.md](docs/SECURITY.md)). A plugin does the same through
    extension traits of its own - its questions on `AccessQuestions`, its views on `Views` - and
    adds nothing about its kind to the core (see [COMPONENTS.md](docs/COMPONENTS.md#plugins)).
*   A new action that acts in someone's name, or a change to what an action asks, comes with its
    test in the security suite (`caelumora-runtime/tests/security_suite.rs`, see
    [SECURITY.md](docs/SECURITY.md#the-security-suite)) - a plugin's in its own tests, in the
    suite's world (`caelumora_runtime::testing`, feature `test-support`).
*   Stored data need not stay readable: until further notice, old data may be deleted. A change to
    what is stored comes without compatibility code - no `serde` aliases, no readers of an older
    form. Instead, say in the summary which stored data has to be deleted.
*   Markdown documents (`docs/`, `issues/`, this file) wrap prose at 100 characters. Headings,
    table rows, code blocks and links that would not fit stay on one line; field lines such as
    `**Status:**` keep a line of their own.

## Contribution Workflow
1.  **Analyze**: Understand the requirement and how it fits into the architecture. Check
    [issues/INDEX.md](issues/INDEX.md) for an existing issue covering it.
2.  **Plan**: Identify the files that need modification and the tests required.
3.  **Implement**: Apply minimal, focused changes following the coding style.
4.  **Document**: Update the document in `docs/` on the subject, `docs/ARCHITECTURE.md` and/or
    `README.md` where your change changes a concept, a rule or how the parts fit together; what a
    type or method does in detail goes into its doc comments. What belongs where is described in
    [CODING-GUIDELINES.md](docs/CODING-GUIDELINES.md#what-goes-where).
5.  **Verify**: Run existing tests and add new ones to cover your changes.
6.  **Refactor**: Ensure the code is clean and well-documented.
7.  **Record**: If the change resolves or advances an issue, update its status in *both*
    `issues/INDEX.md` and the issue file, and append a dated entry under `## History`. If it
    uncovered something new, open an issue for it rather than leaving it in a comment.

### Architecture decisions
Architecture questions are discussed before they are implemented: where a method lives, which object
owns a responsibility, how context reaches the code that needs it, new types or layers. Lay out the
options with their trade-offs, give a recommendation, and wait for the decision. Proposals from the
maintainer are to be assessed honestly, not just accepted.

Prefer the clean structural solution over a pragmatic shortcut, even when it touches more code — and
say what it costs. Glue code that feels awkward (one object making another act on its behalf, a
parameter that only works around missing ownership) is a sign that something sits on the wrong
object; raise it rather than building around it.

Solve problems along the patterns the codebase already has before inventing new ones: a new
capability that resembles an existing one is built the same way (commands like tools: a `…Provider`
component facet, and something that receives the ready providers and checks permissions - for tools
the runtime's `ToolExecutor`, issues 127 and 280).
When a proposal adds a second implementation of something that exists, look for differences between
the two that have no reason — naming, construction, error handling — and propose aligning them, in
whichever direction is the better one. A deliberate difference is fine, but say why it exists.

### Keeping the big picture
A feature is not done when it works, but when the code around it still reads as if it had been
designed with the feature in mind. Before implementing, ask where the new thing belongs in the
overall structure; if it only fits by bending the existing code, propose the refactoring that makes
room for it first - as an architecture decision, with what it costs - instead of building the
feature in somehow. Proposing such refactorings is part of the work, not an extra.

*   **Few generic types, not many helpers.** A new type or trait needs a responsibility of its own.
    One that only bundles parameters, wraps a single call, or serves a single call site is a sign
    that the responsibility belongs to an existing type. Before adding a trait, check whether an
    existing one can be generalized to cover the case; before adding a family of near-identical
    types, check whether one generic type would do. Two values returned to a few callers close by
    are a tuple, not a new type
    ([CODING-GUIDELINES.md](docs/CODING-GUIDELINES.md#small-return-values)).
*   **One owner per concern.** Every crate, module and type can be described in one sentence; if
    the description of a change needs an "and also", something is on the wrong object. Logic that
    belongs together by subject lives together - its data, rules and checks in one place - and is
    not spread over crates because its callers happen to be there: `caelumora-model` holds data
    and rules, `caelumora-runtime` everything that runs, `caelumora-rest-server` only maps HTTP to
    the runtime (see [ARCHITECTURE.md](docs/ARCHITECTURE.md)). When a caller needs something an
    owner does not offer, extend the owner rather than computing it at the caller.
*   **A clear and consistent public API.** The same concept has the same name everywhere (the
    `…_id` of what it identifies, `load_…` for loading, `…Provider` for a facet). Similar things
    are constructed, configured and fail the same way. Make visible only what is used from outside
    (`pub(crate)`, `pub(super)` by default), and do not expose internals that make callers rebuild
    logic the owner should provide.
*   **No repeated logic.** When writing something that resembles existing code - a second `match`
    over the same kinds, a second copy of a check, a conversion done again - move it to one place
    (a method of the type it is about, a shared function, a generic) before adding the second use,
    not after the third. Repetition already present that your change touches is worth naming.
*   **Debt is raised, not built around.** Structural problems you notice next to your change go into
    your summary as a proposed refactoring or a new issue - never silently worked around.
*   **Restructure, then extend.** A refactoring that prepares a feature is a step of its own,
    without behaviour changes, so that both can be reviewed separately.

### Branches and commits
Every piece of work happens on a branch of its own, never directly on `main` - unless the
maintainer explicitly asks for a change on `main`, and except for opening an issue (below). Before
the first change, create a dedicated branch from the current `main` and name it after the work
(with the issue number if there is one, e.g. `issue-264-read-channel`). If you are already on such
a branch for the same work, keep using it.

A new issue is opened on `main` alone, so that issue numbers are counted consistently: a number is
taken where everybody takes them, and is published before anyone else can take it.

1.  Before opening it, update `main` from the remote repository (`git fetch`, then fast-forward
    `main` to `origin/main`), and take the next free number from that.
2.  The commit holds the issue alone - its file, its line in the index and the regenerated
    `issues/OPEN.md` - and no change of code or of anything else.
3.  As soon as that commit is made, push it, without waiting to be asked again: an issue that is
    committed but not pushed holds a number nobody else can see.
4.  Only then, for implementing the issue, create its branch from `main`.

Changing an existing issue - its status, its history, a decision recorded - belongs to the work on
it and happens on that work's branch.

Do not commit on your own: leave the changes in the working tree and say what they are, so that the
maintainer can review them first. Commit only when explicitly asked to - and then only what was
reviewed. The same holds for pushing, except for a new issue, which is pushed as soon as it is
committed (above).

When the work is done, ask whether the branch should be merged into `main`, and merge only after
the maintainer agreed. Before merging, update `main` from the remote repository (`git fetch`, then
fast-forward `main` to `origin/main`), then merge the branch into it. Resolve merge conflicts so
that the intent of both sides is kept; where that is not clear, ask instead of guessing. After a
merge that needed conflict resolution, run the tests and `cargo clippy --workspace --all-targets`
again before reporting the merge as done.
