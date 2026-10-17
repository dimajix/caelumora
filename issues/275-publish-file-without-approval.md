# 275 — `publish_file` asks for approval, though it gives nothing away

**Status:** done
**Area:** Tools and approvals
**Summary:** `publish_file` asks for approval like the other file tools, though it changes nothing and gives the file to nobody who could not download it already.

## The problem

`publish_file` of `terminal_tool` (issue [273](273-publish-file.md)) asks for approval as the other
file tools do: always, unless `requires_approval` is `false` or `"unless_read_only"`. Under the
default, every file an agent offers for download waits for a user to approve it.

## Why it matters

Offering a result is the last step of most work in a workspace, and an approval there only delays
it without protecting anything. `publish_file` changes nothing, and the link it gives is downloaded
through the channel's workspace by those who may use it there - who could download the file
already. Its result tells the model the file's name, size and digest, not its content. So unlike
`read_file`, whose content reaches the model, there is nothing an approval would decide.

## Considerations

`command_output` and `kill_command` already never ask, whatever is configured, because they concern
a command approved before. `publish_file` joins them, for its own reason: it gives nothing away. It
stays a tool that only reads, so that the knowledge it records stays as it is.

## Done when

- `publish_file` never requires approval, whatever `requires_approval` says.
- The test of which calls ask says so, and `COMPONENTS.md` and `TOOLS-AND-COMMANDS.md` too.

## History

- 2026-10-09: Opened at the maintainer's request: `publish_file` should need no approval.
- 2026-10-09: Done. `publish_file` is offered with `requires_approval` false, whatever is
  configured, as `command_output` and `kill_command` are; it stays among the tools that only read.
  The test of which calls ask, the comments of `terminal_tool` and both documents say so.
