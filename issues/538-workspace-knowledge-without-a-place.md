# 538 — What an agent read of a workspace is known without the file it read

**Status:** done
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

## The plan

Decided with the maintainer, in steps of their own:

1. **A place within an entity.** `KnowledgeSource::Entity` takes an optional `place` - for a
   workspace the path of a file or directory - beside its digest: one shape for every entity, as
   issue 323 made it, rather than a variant of the kernel's for a plugin's files. The file tools
   record the place they read: `read_file` and `publish_file` with the file's digest, `list_files`
   and `grep_files` the directory without one.
2. **Knowledge at the tool result.** `ToolOutput` carries what the tool read, which the
   `ToolExecutor` records with the turn and keeps on the `ToolCallResult` - not given to the
   model -, so that the transcript says what the agent read, as a `Given` entry does; the frame
   keeps recording it too, as it does for what is given, for what each message was produced from.
   A file tool records what it wrote as well, by its place and the digest of what it wrote.
3. **A write checks what was read.** `write_file` and `replace_in_file` refuse to change a file
   whose digest differs from the one the agent last knew of its place on its branch, and tell the
   model to read it again; a file it never read is written as now. A refused call wrote nothing,
   and the next is approved as any call. `move_file` and `delete_file` stay as they are.
4. **A restorer for the workspace.** `workspace` gives again, after a compaction, the files read
   whole in the part replaced that are still needed, as they are now, each a `Given` entry with
   its place and digest; `Cut::wanted` tells what was given by its kind, id and place, and
   `KnowledgeRestorer::give_again` is handed the knowledge wanted of its kind.

Telling the model at a turn's start that a file it read has changed since is left to an issue of
its own.

## Done when

- What a file tool read is known by its workspace, its place and its digest.
- After a compaction, the files read on the branch replaced and still needed are given again as
  they are now.
- `replace_in_file` and `write_file` do not overwrite a file changed since the agent read it.

## History

- **2026-11-06** — Opened from the comparison of Claude Code's session logs with Caelumora's
  concepts that led to issue 537 (point B there).
- **2026-11-06** — Planned with the maintainer (*The plan*): the place in `KnowledgeSource::Entity`,
  the knowledge at the tool result and in the frame both, the check for writes and replacements
  alone.
- **2026-11-06** — Step 1 done: `KnowledgeSource::Entity` has a `place`, set by
  `KnowledgeSource::within` and read by `KnowledgeSource::place`, kept only where there is one; the
  source is no longer `Copy`. The file tools record the path they read - `list_files` and
  `grep_files` the directory, `read_file`, `publish_file` and `replace_in_file` the file with its
  digest -, so that a listing and a search of different directories are two pieces of knowledge
  now. What an agent saw shows the place (`KnowledgeRefResponse::place`, the web UI's context
  panel).
- **2026-11-06** — Step 2 done: a tool gives what it read with its output (`ToolOutput::knowledge`),
  which the `ToolExecutor` records with the turn and keeps on the result - where the result goes
  to the model, not where it is withheld, whose knowledge a tool used to record all the same. The
  result keeps the whole `KnowledgeRef`, not its source alone as planned: `read_history` and
  `read_notes` read other channels, which the source does not name. It is never given to the
  model nor published (`Message::published`, `names_only`): whoever may read what the agent saw
  reads it. The file tools, `read_history` and `read_notes` record nothing themselves any more,
  and `Turn::record_knowledge` is the crate's. `write_file` and `replace_in_file` give what they
  wrote, by its place and the digest of what they wrote; a replacement no longer gives the file as
  it read it.
- **2026-11-06** — Step 3 done: `AgentChannelState::last_known` tells the digest of a place as the
  agent last knew it - its last knowledge of the place that holds one, on the path as the model
  sees it whole (`as_seen_whole`, of which `given_as_they_stand` keeps what is given), then in the
  step under way and what is put aside for after it -, and `write_file` and `replace_in_file`
  refuse to change a file that differs from it, telling the model to read it again; their
  descriptions say so. A listing or a search holds no content and changes nothing of it; a file
  gone since, and one never known, are written as now. `move_file` gives the file where it lies
  now as the agent knew it where it was, so that it stays protected; `delete_file` gives nothing,
  as planned - a file made anew by another where the agent deleted one is not overwritten. The
  check and the write are not one step.
- **2026-11-06** — Step 4 done, and with it the issue: the compaction's cut reads what the agent
  knew whole in each entry as the model sees it - a gift, and what its tools read or wrote -, and
  tells one from another by `KnowledgeSource::of_the_same`, the same entity or the same place
  within it; a listing or a search is nothing to give again. `KnowledgeRestorer::give_again` is
  handed the knowledge of its kind wanted, as it was first known. `workspace` is a
  `KnowledgeRestorer` now: it gives each file again as it is now under its path
  (`tool_workspace.file`, "[File: ...]"), known so, where the channel may still use the workspace
  and the file is text no larger than a result holds; `caelumora-tool-workspace` links
  `caelumora-compaction` for it.
  After the review of this step: `publish_file` gives the place it offers without a digest - the
  model is given a link, not the file -, so that an offer is neither given again as a file nor
  counts as knowing it for a write. What the agent knows on the path is read by one function
  (`knowledge_as_seen_whole`) for the cut and `last_known` alike, and what of it was known whole
  by `KnowledgeSource::read_whole`. Where the workspace's storage cannot be reached, the restorer
  gives nothing rather than fail the compaction, as the prompt part leaves its instruction files
  out rather than fail the turn.
