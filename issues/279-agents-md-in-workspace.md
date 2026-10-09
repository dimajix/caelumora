# 279 — An agent is not told of an AGENTS.md in its workspace, and has no skill to write one

**Status:** done
**Area:** Context and knowledge

## The problem

A workspace (issue [253](253-workspace.md)) often holds a project that comes with its own
instructions for agents: an `AGENTS.md` at its root - the convention Codex, Claude Code and others
read - that says how the project is built and tested, how it is laid out and what to keep to. An
agent of Caelumora working on that workspace does not know of it. The `workspace_prompt_provider`
(`caelumora-runtime/src/component_impl/workspace_prompt_provider.rs`) tells it the workspace's
name, its classification, its description and whether it has a shell - nothing of what is in it.
The agent finds the file only if it happens to list the root and decides to read it.

And where a workspace has no `AGENTS.md` yet, an agent cannot be asked to write one well: there is
no skill that says how to explore an existing project and what such a file should hold.

## Why it matters

The `AGENTS.md` is written for exactly this reader. Without it an agent works against the project's
conventions, runs the wrong commands, or spends a turn finding out what the file would have said.
It is also the one place such instructions can be shared between Caelumora's agents and the tools
people use on the same files outside of it - unlike a channel's `briefing` (issue 245), which says
what an agent should know about *the place it works in*, the `AGENTS.md` belongs to *the files* and
travels with them.

## Considerations

### Telling the agent

**Where.** The `workspace_prompt_provider` already says what the workspace is, is read afresh at
the start of every turn through the agent's views (`ChannelView::workspace`, which checks that the
channel may use it), and is classified as the workspace. The instruction belongs into its part; no
new component.

**What it says - three options:**

1. *Always the instruction*, "if there is an `AGENTS.md` at the root, read it before you work on the
   files". Costs nothing at turn start and keeps the part stable, but sends every agent looking
   for a file that is mostly not there, in every workspace.
2. *The instruction only if the file exists.* The provider looks at the root (`Terminal::stat`
   through `WorkspaceView::terminal()`) and only then says that there is an `AGENTS.md` and that it
   is to be read with `read_file` before working on the files. One cheap call per turn; the part
   changes only when the file appears or disappears, so the prompt cache holds. The agent reads it
   as any file, so the read is recorded as knowledge of the workspace
   (`KnowledgeSource::Workspace`) and stands in its transcript.
3. *The file's content in the prompt*, as Claude Code does. The agent needs no call, but the system
   prompt changes with every edit of the file - breaking the cache from there on -, its size has to
   be bounded, and it is a second way content of the workspace reaches the model besides the file
   tools, recorded differently. Skills were kept out of the system prompt for the same reasons
   ([SKILLS.md](../docs/SKILLS.md), issue 22).

**Recommendation:** option 2. If the storage cannot be reached at turn start, the part says what
it says today and leaves the file out - a workspace that is down must not cost the turn.

**Next to it, to decide with this issue:**

*   *Which names.* `AGENTS.md` alone, or a list as a parameter of the component (e.g. also
    `CLAUDE.md`), the first found being the one named.
*   *Nested files.* Some projects have an `AGENTS.md` in a subdirectory that applies there. The
    part could add "and the `AGENTS.md` of a directory you work in" - one more sentence, no further
    lookup; looking them all up at turn start is not worth it.

### The skill to write one

Skills are entities, not files of the code: the system's own come only from a seed (`skills:` in
`examples/seed.yaml`), and nothing is built in (see [SKILLS.md](../docs/SKILLS.md)).

1. *A skill in the example seed* (`write-agents-md`, global): how to explore an existing workspace
   with `list_files`, `grep_files`, `read_file` and - where there is a shell - `run_command` (build
   files, CI configuration, READMEs, the layout, the tests), what an `AGENTS.md` holds (how to build,
   test and run it; the layout; conventions; what not to do; where more is written), what it leaves
   out (what a reader finds in the code at once, anything secret), and that it is written with
   `write_file` and kept short. Fits the existing way at no cost; a deployment without that seed has
   no such skill.
2. *Skills shipped with the code*, present in every deployment. A new mechanism - who owns them,
   whether they can be changed, how a new version reaches an existing system - for one skill; that
   question belongs with the import of `SKILL.md` files, which SKILLS.md lists as not decided.

**Recommendation:** option 1 now; the second is a question of its own once more than one skill
should come with the system.

## Decision

Taken by the maintainer on 2026-10-09:

1. **Option 2 for telling the agent.** The `workspace_prompt_provider` looks at the root of the
   workspace at the start of every turn and names the files it finds there, asking the agent to
   read them with `read_file` before it works on the files. Their content does not go into the
   prompt.
2. **Several names.** Which files it looks for is a parameter of the component, a list of names,
   by default `AGENTS.md` and `CLAUDE.md`. Every one found at the root is named, not only the first:
   often one of them only points to the other, and each costs one `stat`.
3. **Subdirectories: one sentence, no search.** The part adds that a directory may have its own
   such file, which applies there, is to be read before working there, and takes precedence over
   one further up where they disagree - the rule of the `AGENTS.md` convention. The component does
   not look for them: a search of the whole workspace at every turn start is costly on a large or
   remote storage, changes the part whenever the layout changes, and finds mostly files of parts
   the turn does not touch. If agents turn out to miss them, the file tools could point to an
   unread one when they first touch its directory (as Claude Code does for nested `CLAUDE.md`) -
   a change of its own, since `terminal_tool` would have to know the convention and what the agent
   has already read (`KnowledgeSource::Workspace`).
4. **The skill in the example seed** (option 1): `write-agents-md`, global. Skills shipped with the
   code stay a question of their own.

## Done when

- An agent whose workspace holds one of the configured files (by default `AGENTS.md`,
  `CLAUDE.md`) at its root is told which in the `[Your workspace]` part and asked to read them
  before working on the files; one whose workspace holds none is not told to look for them.
- The part says that a subdirectory may have such a file of its own, which applies there and takes
  precedence over one further up.
- A storage that cannot be reached at turn start leaves the part as it is today, without failing
  the turn.
- The example seed has a skill that guides an agent through writing an `AGENTS.md` for an existing
  workspace.
- Tests cover the part with and without the file; the documents (AGENT-TURN.md for the part,
  TOOLS-AND-COMMANDS.md for the workspace, SKILLS.md where the seed's skills are mentioned) say it.

## History

- 2026-10-09: Opened at the maintainer's request: the workspace's prompt part should tell an agent
  to read an existing `AGENTS.md`, and a skill should help an agent write one for an existing
  workspace.
- 2026-10-09: Decided (see *Decision*): the part names the files found at the root - a list of
  names, by default `AGENTS.md` and `CLAUDE.md` -, says in one sentence that a subdirectory may have
  its own without searching for them, and the skill goes into the example seed.
- 2026-10-09: Done. `workspace_prompt_provider` takes `instruction_files` (by default `AGENTS.md`,
  `CLAUDE.md`), looks each up at the workspace's root with `Terminal::stat` and names those found;
  the sentence on a directory's own file stands in every part of a workspace, phrased as what to do
  where one is met, so that it sends no agent looking. A storage that cannot be reached leaves the
  files out with a warning. The example seed has the skill `write-agents-md`.
