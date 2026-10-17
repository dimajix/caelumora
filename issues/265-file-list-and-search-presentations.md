# 265 — Listing and searching the workspace are shown as raw JSON

**Status:** done
**Area:** User interaction
**Summary:** The web UI shows a listing and a search of the workspace as raw JSON, while reading, writing and replacing in a file are shown as what they do.

## The problem

`read_file`, `write_file` and `replace_in_file` have presentations of their own (`file_read`,
`file_write`, `file_edit`, issue [251](251-open-terminal.md)): the web UI shows the file's path and
its text. `list_files` and `grep_files` are presented as every call (`ToolPresentation::Default`):
their arguments as a line of JSON, their result as the JSON object the tool returns - a listing as
`{ "path", "entries": [{ "name", "is_dir", "size", "modified" }], "complete" }`, a search as
`{ "matches": [{ "path", "line", "text" }], "files_searched", "files_skipped", "complete" }`.

## Why it matters

An agent that works in the workspace lists and searches before it reads or changes anything, so
these calls are among the most frequent in a chat. As raw JSON they are hard to read: which
directory, which entries, which pattern and where it matched is buried in field names and quotes.
And the file tools are shown unevenly - three of them as what they do to a file, two as data.

## Considerations

The pattern is set: a tool declares its presentation (`ToolPresentation`, issue
[165](165-ask-the-user-tool.md)), the web UI's `ToolCallView` has a handler for each
(`PRESENTATIONS`), and a client that does not know one falls back to the default. Two new variants
are needed; nothing else changes on the server, and an older client shows the calls as before.

**Names.** The existing ones say what is done to a file: `file_read`, `file_write`, `file_edit`.
In the same way: `file_list` for a directory listed, `file_search` for files searched. `grep` would
name the tool, not what it does.

**What the handlers show**, after the existing ones:

- `file_list`: the directory - the workspace's top where none is given - with how many entries it
  has, and the entries as the workspace's file browser shows them: a directory or a file, its
  name, a file's size, when it was changed. A listing cut off at its limit says so.
- `file_search`: the pattern as code and where it was searched, with how many matches in how many
  files; the matches grouped by file, each its line number and its line. A search cut off - at
  `max_matches` or at the files it may read - and files skipped as no text say so.

Both keep the frame's conventions: arguments not shown, a result whose content is the owner's
("not shown"), and a failed call's message.

**Size formatting.** The file browser has `formatSize`; the listing needs the same. It moves to
`messages.ts` beside `formatTimestamp`, which both already share, rather than being written twice.

`delete_file` stays with the default presentation: its arguments are a path, its result a flag;
there is nothing raw to make readable.

## Done when

`list_files` and `grep_files` are presented as `file_list` and `file_search`, the web UI shows them
as above, and `TOOLS-AND-COMMANDS.md` and `WEB-UI.md` say so.

## History

- **2026-10-09** — Opened at the maintainer's request, to be built after the presentations of
  reading, writing and replacing.
- **2026-10-09** — Done as considered: `ToolPresentation::FileList` and `FileSearch`, set on
  `list_files` and `grep_files`; the web UI's `FileListToolCallContent` and
  `FileSearchToolCallContent` show them, `formatSize` moved to `messages.ts`.
