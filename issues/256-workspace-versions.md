# 256 — A workspace has no versions: an agent's bad change cannot be taken back

**Status:** open
**Area:** Context and knowledge

## The problem

A workspace (issue [253](253-workspace.md)) is a tree of files that every change overwrites. Its
files are changed by people in the panel, by agents with the file tools of `terminal_tool` and by
the commands they run (issue [251](251-open-terminal.md)) - in place, with nothing kept of what was
there. A conversation can be corrected - a message modified, taken back, answered again - but a
workspace does not branch with it: asking an agent to answer again does not take back what its
first answer wrote, and a command that deleted a directory leaves nothing to restore. Nor is it
recorded who changed a file, from which turn of which channel.

## Why it matters

- **Write tools are handed out without a way back.** Approval and the rules of a place are all
  that stand between a model and the files of a whole session; a mistake that slips past them -
  approved by a person who did not read closely, or allowed by a rule - is final. PLAN.md (W4) put
  it plainly: without versions, write tools are not safe to hand out. They are handed out now,
  behind approval by default.
- **Several channels share one workspace.** What a helper in one channel overwrites, the person
  in another loses; nobody can see which change came from where.
- **Changing classified content** (issue [169](169-changing-classified-content.md)) asks that
  every change of a container records where it was made. A workspace is such a container, and
  records nothing.

## Considerations

- **What a version is of.** A file, or the whole tree. File tools change single files, a command
  may change any number; a version per file covers the first alone, a snapshot of the tree both.
- **Where it is kept**, by what can see the changes:
  - **By Caelumora, around every change it makes** - an upload, a file tool's write: the old
    content kept with who wrote the new one, from which turn. Simple and independent of the
    storage, but blind to what a command changes, which passes Caelumora by.
  - **In the workspace, by git** - the storage commits after every change, a command's included,
    with the turn and the channel in the commit. Covers commands, gives diffs and restores for
    free, and works the same in a `local` directory and in an Open Terminal instance, through its
    shell. But a command can rewrite or delete the history it lives next to, and git needs to be
    there; the history counts against the storage's size.
  - **By the storage, as snapshots** - of a volume, a filesystem. Complete and out of the
    commands' reach, but specific to each provider, and coarse.
  - The provider is the one place that knows its storage, so whichever is chosen is a capability of
    `TerminalProvider`, as the shell is - not code in the tools.
- **When.** Around every write of a file tool and every command - a turn's changes as one, so that
  answering again can take back what the answer did - and before a person's upload overwrites a
  file.
- **Restoring.** By a person in the panel - a file, or the tree as it was before a turn - and
  perhaps by an agent with a tool; restoring is a change itself, approved and recorded as one.
- **What is kept, and how long.** A version holds what the workspace held, so it is classified as
  the workspace; lowering the workspace releases its versions too. Purging a workspace purges its
  versions. Retention follows the workspace's, or a limit of its own.
- **Related.** Issue [20](20-transactional-effects.md): a command is an effect nothing stages, and a
  version is what makes it undoable after the fact. Issue [254](254-commands-run-unbounded.md): a
  command still running when a version is taken changes the workspace afterwards.

## Done when

- Every change of a workspace - a person's, a file tool's, a command's - can be taken back, and
  who made it from which turn is recorded.
- A person can restore a file, or the workspace as it was before a turn, in the web UI; the
  security suite covers who may.
- STORAGE.md and CONFIDENTIALITY.md describe what is kept, for how long, and how it is classified.

## History

- **2026-10-08** — Opened with step 10 of 251's plan, as PLAN.md's W4: the workspace, its file
  tools and its shell are done, and none of them keeps what it overwrote.
