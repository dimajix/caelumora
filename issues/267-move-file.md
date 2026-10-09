# 267 — Agents cannot move or rename a file of the workspace

**Status:** done
**Area:** Tools and approvals

## The problem

The file tools of `terminal_tool` (issue [251](251-open-terminal.md), step 9) list, read, search,
write, replace in and delete files. Nothing moves or renames one. An agent that wants to rename a
file has to read it, write it under the new name and delete the old one - three calls, each
approved where changes need approval, the content passing through the model and its context, and
a directory cannot be moved that way at all. Where the storage has a shell, `run_command` with `mv`
works, but that needs a shell and is approved as an arbitrary command.

Both storages can move already: `Terminal::move_entry(from, to)` is implemented by the `local`
provider and by Open Terminal, with tests. It refuses a target that exists (`TerminalError::Exists`)
and the workspace's top, and makes the directories the target lies in. No tool and no REST route
uses it yet.

## Why it matters

Tidying a workspace - renaming a draft, moving results into a directory of their own - is ordinary
work for an agent that keeps files. Doing it through read, write and delete is slow, costly in
tokens, loses nothing only by luck (a file larger than `read_file` gives at once), and shows the
approver three changes where one was meant.

## Considerations

**One tool or two.** Moving and renaming are one operation in a file system: a rename is a move
within the same directory, which is how `mv` and `rename(2)` treat it. The options:

- **A. One tool `move_file` (`from`, `to`)** that both moves and renames, its description saying
  so. One way to do it; the model knows the semantics from `mv`; one tool more in every prompt.
  Like `delete_file`, which deletes an empty directory as well, its name says "file" and it moves a
  directory too.
- **B. Two tools**, `move_file` (`from`, `to`) and `rename_file` (`path`, `new_name`, within its
  directory). Renaming would not need the directory written out again, and the approver would read
  "rename" where that is what happens. But both would run the same `move_entry`; the model has to
  choose between two tools for the same act, with two ways to express one result; and every prompt
  carries one tool more. The difference between them is only in how the target is written.

Recommendation: A. A second tool adds no capability, only a second spelling of the same call, and
the "no repeated logic" rule argues against it. What B offers the approver - reading "rename" - A
can give as well: its description of the call says "Rename" where `from` and `to` lie in the same
directory, "Move" otherwise.

**What the tool does**, after the existing file tools:

- Parameters `from` and `to`, both relative to the workspace's top and made `WorkspacePath`s
  first.
- A target that exists is refused, the model's to correct (`InvalidParameter`, as `Exists` already
  is): overwriting is a deletion, which `delete_file` makes visible and approvable on its own. No
  `overwrite` flag.
- The directories the target lies in are made, as `write_file` does.
- The workspace's top is neither moved nor moved over; a directory moved into itself fails and is
  the model's to correct.
- It changes: approved where changes are (`"unless_read_only"` asks for it). It reads nothing, so
  it records no knowledge.
- The approver reads "Move 'a' to 'b' in the workspace" or "Rename 'a' to 'b' in the workspace".
- A rule derived from an approved move: write and replace match the path alone, whatever is
  written. A move changes two places; the rule should match both `from` and `to`, each exactly - a
  rule matching only `from` would let the same file be moved anywhere, a rule matching only `to`
  would let anything be moved there.
- Presented as `file_move` (`ToolPresentation::FileMove`, after issue
  [265](265-file-list-and-search-presentations.md)): "from → to", and whether it renamed or moved.

**Security suite:** a test as for `write_file` - the call asked as a call on the workspace, in the
name of whoever the agent acts for (`asked_for_a_call_on_the_workspace`).

**Not part of this:** moving and renaming in the web UI's file browser, which would need a REST
route over `move_entry`. Its own issue when wanted.

## Done when

Agents move and rename files and directories of the workspace with one call, approved as a change,
with a rule covering exactly that move; the web UI presents the call; the tests, the security
suite, `TOOLS-AND-COMMANDS.md`, `COMPONENTS.md` and `WEB-UI.md` say so.

## History

- **2026-10-09** — Opened at the maintainer's request, with the question whether moving and
  renaming should be one tool or two.
- **2026-10-09** — Decided with the maintainer: A, one tool `move_file` that moves and renames; a
  rule derived from an approved move matches both `from` and `to`. Done:
  - `move_file` in `terminal_tool::files`, over `Terminal::move_entry`: the approver reads "Rename"
    or "Move" (`renames`, both in one directory), the result says whether it `renamed`; a target
    that exists, a source that does not and the workspace's top are the model's to correct.
  - `ToolPresentation::FileMove`; the web UI's `FileMoveToolCallContent` shows "from → to" and
    whether it was renamed or moved.
  - Security suite: `an_agents_move_of_a_file_asks_what_a_change_asks`; its helper
    `asked_for_a_call_on_the_workspace` takes the files that lie there before the call.
