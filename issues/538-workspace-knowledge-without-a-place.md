# 538 — What an agent read of a workspace is known without the file it read

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** What an agent read of a workspace is recorded as the workspace and a digest, without the file, and only in the frame of the turn that read it, so a compaction cannot give a file read again, nothing tells that a file read has changed since, and a replacement overwrites what changed in between.

## The problem

The file tools of `workspace` record what they read as the turn's knowledge
(`caelumora-tool-workspace/src/component/tool/files.rs`, `known`):
`KnowledgeSource::entity::<Workspace>(id, digest)` - the workspace, and the digest of the file
where one was read whole. Three things follow:

1. **No place.** The knowledge names the workspace, not the file within it. Nothing can give a
   file read again, nor ask whether the file is still as it was read.
2. **Only in the turn's frame.** Knowledge read by a tool is recorded in the `FrameRef` of the turn
   that read it ([MEMORY.md](../docs/MEMORY.md), "Recording what was read"); only an entry
   `TranscriptEntry::Given` carries its knowledge in the transcript (`GivenSource::Knowledge`). A
   `KnowledgeRestorer` reads the transcript, so it sees the skills given, not the files read.
3. **What changed in between is overwritten.** `replace_in_file` reads, replaces and writes, and
   `write_file` writes, whatever the file holds now (`files.rs`, module docs: "what changed the file
   in between is overwritten"). A workspace is used by several channels and by people at once.

## Why it matters

- **After a compaction the agent has lost what it read.** Claude Code gives the files read again
  after a compaction (`compact_file_reference`), and tells the model when a file read has changed
  since (`edited_text_file`); the comparison with its session logs, under issue
  [537](537-prompt-parts-placed-anew-every-turn.md), found both missing here.
- **A stale write destroys another's work** without anybody being told: the agent writes from a
  version it read before another channel, a command or a person changed the file.

## Considerations

- **A place within an entity.** `KnowledgeSource::Entity` could take an optional place - for a
  workspace the `WorkspacePath` - beside its digest; or the workspace gets a source of its own. The
  first keeps one shape for every entity whose parts are read apart (an artifact's versions are
  not, a project's files may be).
- **Knowledge at the entry.** A tool result could carry the knowledge it read, as `Given` does,
  so that the transcript alone says what the agent knows. Then the frame's knowledge read by tools
  is the same twice; one of the two goes.
- **A restorer for the workspace.** With both, a `KnowledgeRestorer` of `workspace` gives again the
  files read on the branch replaced, by their place, as they are now - as the skills' does - and
  leaves out what the strategy judges no longer needed.
- **A write checks what was read.** `replace_in_file` and `write_file` could refuse to change a
  file whose digest differs from the one the agent last read on its branch, and tell the model to
  read it again ("modified since read"). A file never read is written as now. Whether a person's
  approval of the change should then be asked again is to be decided.
- **Told when a file changed.** Telling the model at a turn's start that a file it read has
  changed since is a prompt part of `workspace` at `Given` (issue 537); it needs the place too.

## Done when

- What a file tool read is known by its workspace, its place and its digest.
- After a compaction, the files read on the branch replaced and still needed are given again as
  they are now.
- `replace_in_file` and `write_file` do not overwrite a file changed since the agent read it.

## History

- **2026-11-06** — Opened from the comparison of Claude Code's session logs with Caelumora's
  concepts that led to issue 537 (point B there).
