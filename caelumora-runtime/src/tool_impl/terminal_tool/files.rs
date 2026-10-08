//! The file tools of `terminal_tool` (issue 251, step 9): the workspace's
//! files through its `Terminal`, whatever its storage - a shell is not
//! needed. Paths are relative to the workspace's top directory and made
//! `WorkspacePath`s before anything is asked.
//!
//! - `list_files`, `read_file` and `grep_files` only read; what they read
//!   is recorded as the turn's knowledge (`KnowledgeSource::Workspace`), a
//!   file's content by its digest.
//! - `write_file`, `replace_in_file` and `delete_file` change; the approver
//!   reads the change whole, and a rule derived from an approved one covers
//!   the file, whatever is written (`matcher`).
//!
//! A file is read whole, up to `READ_LIMIT`, and given as text: one that is
//! no UTF-8 is refused. A replacement reads, replaces and writes: what
//! changed the file in between is overwritten.

use super::{failed, tool, TerminalToolConfig};
use crate::agent_turn::Turn;
use crate::terminal::{ByteStream, FileEntry, Terminal, TerminalError, WorkspacePath};
use crate::tool::{arguments, ToolError, ToolResult};
use crate::view::WorkspaceView;
use caelumora_model::approval::ToolMatcher;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const LIST_FILES: &str = "list_files";
pub const READ_FILE: &str = "read_file";
pub const GREP_FILES: &str = "grep_files";
pub const WRITE_FILE: &str = "write_file";
pub const REPLACE_IN_FILE: &str = "replace_in_file";
pub const DELETE_FILE: &str = "delete_file";

/// The file tools, by name.
pub(super) const TOOLS: [&str; 6] = [LIST_FILES, READ_FILE, GREP_FILES, WRITE_FILE, REPLACE_IN_FILE, DELETE_FILE];

/// The largest file read or searched.
const READ_LIMIT: u64 = 4 * 1024 * 1024;
/// The most entries one listing gives.
const MAX_ENTRIES: usize = 1000;
/// The most files one search reads.
const MAX_FILES_SEARCHED: usize = 2000;
/// The most matches one search gives unless fewer are asked for.
const DEFAULT_MATCHES: usize = 100;
const MAX_MATCHES: usize = 1000;
/// The longest line of a match given, in characters.
const MATCH_CHARS: usize = 300;
/// The longest pattern, in characters, and how large it may compile.
const MAX_PATTERN_CHARS: usize = 500;
const MAX_COMPILED_PATTERN_BYTES: usize = 1 << 20;

/// Whether the file tool `name` only reads.
fn read_only(name: &str) -> bool {
    matches!(name, LIST_FILES | READ_FILE | GREP_FILES)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct At {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    path: String,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    line_count: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grep {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max_matches: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Write {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replace {
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    path: String,
}

/// `text` as a path inside the workspace; the model's to correct if it is
/// none.
fn path(text: &str) -> ToolResult<WorkspacePath> {
    WorkspacePath::parse(text).map_err(failed)
}

pub(super) fn tools(config: &TerminalToolConfig) -> Vec<Tool> {
    let asks = |name: &str| config.requires_approval.required(read_only(name));
    let string = ToolParameters::string;
    vec![
        tool(
            LIST_FILES,
            "List a directory of the session's workspace: each entry's name, whether it is a directory, its size and when it was changed.".to_string(),
            ToolParameters::object([("path", string("The directory, relative to the workspace's top; the top unless given"))], Vec::<&str>::new()),
            asks(LIST_FILES),
            ToolPresentation::Default,
        ),
        tool(
            READ_FILE,
            "Read a text file of the session's workspace, whole or the lines asked for. A result too long to give at once says where to go on.".to_string(),
            ToolParameters::object(
                [
                    ("path", string("The file, relative to the workspace's top")),
                    ("start_line", ToolParameters::number("The first line to give, counted from 1; 1 unless given")),
                    ("line_count", ToolParameters::number("How many lines to give; all to the end unless given")),
                ],
                ["path"],
            ),
            asks(READ_FILE),
            ToolPresentation::Default,
        ),
        tool(
            GREP_FILES,
            format!("Search the text files of the session's workspace for a regular expression: each matching line with its file and line number, at most {MAX_MATCHES}."),
            ToolParameters::object(
                [
                    ("pattern", string("A regular expression (Rust syntax), matched against each line")),
                    ("path", string("The directory to search, with what lies below it, or a single file; the whole workspace unless given")),
                    ("max_matches", ToolParameters::number(format!("How many matches to give at most; {DEFAULT_MATCHES} unless given"))),
                ],
                ["pattern"],
            ),
            asks(GREP_FILES),
            ToolPresentation::Default,
        ),
        tool(
            WRITE_FILE,
            "Write a text file of the session's workspace, replacing what it held; the directories it lies in are made.".to_string(),
            ToolParameters::object([("path", string("The file, relative to the workspace's top")), ("content", string("What the file is to hold"))], ["path", "content"]),
            asks(WRITE_FILE),
            ToolPresentation::Default,
        ),
        tool(
            REPLACE_IN_FILE,
            "Replace a piece of text in a text file of the session's workspace. The text must occur exactly once - give enough of what surrounds it - unless every occurrence is to be replaced.".to_string(),
            ToolParameters::object(
                [
                    ("path", string("The file, relative to the workspace's top")),
                    ("old_text", string("The text to replace, exactly as it is in the file")),
                    ("new_text", string("The text to put in its place")),
                    ("replace_all", ToolParameters::boolean("Whether to replace every occurrence; false unless given")),
                ],
                ["path", "old_text", "new_text"],
            ),
            asks(REPLACE_IN_FILE),
            ToolPresentation::Default,
        ),
        tool(
            DELETE_FILE,
            "Delete a file, or an empty directory, of the session's workspace.".to_string(),
            ToolParameters::object([("path", string("The file or directory, relative to the workspace's top"))], ["path"]),
            asks(DELETE_FILE),
            ToolPresentation::Default,
        ),
    ]
}

/// What an approver reads: what is read, or the change whole.
pub(super) fn describe(name: &str, params: &Value) -> ToolResult<String> {
    Ok(match name {
        LIST_FILES => format!("List '{}' in the workspace", arguments::<At>(params)?.path.unwrap_or_default()),
        READ_FILE => format!("Read '{}' in the workspace", arguments::<Read>(params)?.path),
        GREP_FILES => {
            let Grep { pattern, path, .. } = arguments(params)?;
            format!("Search the workspace's '{}' for:\n{pattern}", path.unwrap_or_default())
        }
        WRITE_FILE => {
            let Write { path, content } = arguments(params)?;
            format!("Write '{path}' in the workspace ({} bytes):\n{content}", content.len())
        }
        REPLACE_IN_FILE => {
            let Replace { path, old_text, new_text, replace_all } = arguments(params)?;
            let how = if replace_all { "every occurrence" } else { "once" };
            format!("Replace in '{path}' in the workspace, {how}:\n--- old\n{old_text}\n+++ new\n{new_text}")
        }
        _ => format!("Delete '{}' from the workspace", arguments::<Delete>(params)?.path),
    })
}

/// For a change, the calls changing the same file; `None` for the others,
/// which are alike when exactly alike.
pub(super) fn matcher(name: &str, params: &Value) -> Option<ToolMatcher> {
    if !TOOLS.contains(&name) || read_only(name) {
        return None;
    }
    let path = params.get("path")?.as_str()?;
    Some(ToolMatcher { tool: name.to_string(), arguments: BTreeMap::from([("path".to_string(), regex::escape(path))]) })
}

/// The bytes of the file `at`, at most `READ_LIMIT`.
async fn content(terminal: &dyn Terminal, at: &WorkspacePath) -> ToolResult<Vec<u8>> {
    let (size, mut stream) = terminal.read(at).await.map_err(failed)?;
    if size > READ_LIMIT {
        return Err(ToolError::InvalidParameter(format!("'{at}' is {size} bytes, more than the {READ_LIMIT} a file may be read with; search it with {GREP_FILES}, or read part of it with a command")));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ToolError::ExecutionFailed(format!("reading '{at}' broke off: {}", e.kind())))?;
        if bytes.len() as u64 + chunk.len() as u64 > READ_LIMIT {
            return Err(ToolError::InvalidParameter(format!("'{at}' is larger than the {READ_LIMIT} bytes a file may be read with")));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// The file `at` as text.
async fn text(terminal: &dyn Terminal, at: &WorkspacePath) -> ToolResult<(String, caelumora_model::digest::Digest)> {
    let bytes = content(terminal, at).await?;
    let digest = caelumora_model::digest::content(&bytes);
    let text = String::from_utf8(bytes).map_err(|_| ToolError::InvalidParameter(format!("'{at}' is no text (not UTF-8)")))?;
    Ok((text, digest))
}

async fn write(terminal: &dyn Terminal, workspace: &WorkspaceView, at: &WorkspacePath, content: String) -> ToolResult<u64> {
    let stream: ByteStream = Box::pin(futures::stream::iter([Ok(bytes::Bytes::from(content))]));
    terminal.write(at, stream, workspace.max_file_bytes()).await.map_err(failed)
}

fn entry(entry: &FileEntry) -> Value {
    json!({ "name": entry.name, "is_dir": entry.is_dir, "size": entry.size, "modified": entry.modified })
}

/// `line` as a match shows it: at most `MATCH_CHARS` characters.
fn shown(line: &str) -> String {
    let line = line.trim_end_matches(['\r', '\n']);
    match line.char_indices().nth(MATCH_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

/// Searches `terminal` from `start` for `pattern`; what was searched and
/// the matches.
async fn search(terminal: &dyn Terminal, start: WorkspacePath, pattern: &regex::Regex, most: usize) -> ToolResult<Value> {
    let mut pending = vec![start];
    let (mut matches, mut searched, mut skipped, mut complete) = (Vec::new(), 0usize, 0usize, true);
    'walk: while let Some(at) = pending.pop() {
        let found = terminal.stat(&at).await.map_err(failed)?;
        if found.is_dir {
            let mut entries = terminal.list(&at).await.map_err(failed)?;
            entries.reverse();
            for listed in entries {
                let below = if at.is_root() { path(&listed.name)? } else { path(&format!("{at}/{}", listed.name))? };
                pending.push(below);
            }
            continue;
        }
        if searched == MAX_FILES_SEARCHED {
            complete = false;
            break;
        }
        searched += 1;
        if found.size > READ_LIMIT {
            skipped += 1;
            continue;
        }
        let Ok((text, _)) = text(terminal, &at).await else {
            skipped += 1;
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            if pattern.is_match(line) {
                if matches.len() == most {
                    complete = false;
                    break 'walk;
                }
                matches.push(json!({ "path": at.to_string(), "line": number + 1, "text": shown(line) }));
            }
        }
    }
    Ok(json!({ "matches": matches, "files_searched": searched, "files_skipped": skipped, "complete": complete }))
}

/// Runs the file tool `name` with `params` on `terminal`, the files of
/// `workspace`, recording in `turn` what was read.
pub(super) async fn execute(config: &TerminalToolConfig, name: &str, params: &Value, terminal: &dyn Terminal, workspace: &WorkspaceView, turn: &mut Turn<'_>) -> ToolResult<Value> {
    let read = |turn: &mut Turn<'_>, digest| turn.record_knowledge(KnowledgeRef {
        channel: turn.channel_id(),
        session: turn.session_id(),
        source: KnowledgeSource::Workspace { workspace: workspace.id(), digest },
        classification: Some(workspace.classification().clone()),
    });
    match name {
        LIST_FILES => {
            let at = path(&arguments::<At>(params)?.path.unwrap_or_default())?;
            let entries = terminal.list(&at).await.map_err(failed)?;
            read(turn, None);
            let listed: Vec<Value> = entries.iter().take(MAX_ENTRIES).map(entry).collect();
            Ok(json!({ "path": at.to_string(), "entries": listed, "complete": entries.len() <= MAX_ENTRIES }))
        }
        READ_FILE => {
            let Read { path: at, start_line, line_count } = arguments(params)?;
            let at = path(&at)?;
            let (text, digest) = text(terminal, &at).await?;
            read(turn, Some(digest));
            let lines: Vec<&str> = text.split_inclusive('\n').collect();
            let start = start_line.unwrap_or(1).max(1);
            if start > lines.len().max(1) {
                return Err(ToolError::InvalidParameter(format!("'{at}' has {} lines; start_line {start} is beyond them", lines.len())));
            }
            let wanted = lines.iter().skip(start - 1).take(line_count.unwrap_or(usize::MAX));
            let (mut given, mut end) = (String::new(), start - 1);
            for line in wanted {
                if given.len() + line.len() > config.max_output_bytes && end >= start {
                    break;
                }
                given.push_str(line);
                end += 1;
            }
            let mut result = json!({ "path": at.to_string(), "content": given, "start_line": start, "end_line": end, "total_lines": lines.len() });
            let asked_end = line_count.map_or(lines.len(), |count| (start - 1 + count).min(lines.len()));
            if end < asked_end {
                result["next"] = json!(format!("Not all of it fits into one result: read on with start_line {}.", end + 1));
            }
            Ok(result)
        }
        GREP_FILES => {
            let Grep { pattern, path: at, max_matches } = arguments(params)?;
            if pattern.chars().count() > MAX_PATTERN_CHARS {
                return Err(ToolError::InvalidParameter(format!("the pattern is longer than {MAX_PATTERN_CHARS} characters")));
            }
            let pattern = regex::RegexBuilder::new(&pattern).size_limit(MAX_COMPILED_PATTERN_BYTES).build()
                .map_err(|e| ToolError::InvalidParameter(format!("the pattern is no regular expression: {e}")))?;
            let at = path(&at.unwrap_or_default())?;
            let found = search(terminal, at, &pattern, max_matches.unwrap_or(DEFAULT_MATCHES).clamp(1, MAX_MATCHES)).await?;
            read(turn, None);
            Ok(found)
        }
        WRITE_FILE => {
            let Write { path: at, content } = arguments(params)?;
            let at = path(&at)?;
            let written = write(terminal, workspace, &at, content).await?;
            Ok(json!({ "path": at.to_string(), "bytes": written }))
        }
        REPLACE_IN_FILE => {
            let Replace { path: at, old_text, new_text, replace_all } = arguments(params)?;
            if old_text.is_empty() {
                return Err(ToolError::InvalidParameter("'old_text' must not be empty".to_string()));
            }
            let at = path(&at)?;
            let (text, digest) = text(terminal, &at).await?;
            read(turn, Some(digest));
            let found = text.matches(old_text.as_str()).count();
            let replaced = match (found, replace_all) {
                (0, _) => return Err(ToolError::InvalidParameter(format!("'old_text' does not occur in '{at}'"))),
                (1, _) | (_, true) => text.replace(old_text.as_str(), &new_text),
                (many, false) => return Err(ToolError::InvalidParameter(format!("'old_text' occurs {many} times in '{at}': give more of what surrounds it, or set replace_all"))),
            };
            write(terminal, workspace, &at, replaced).await?;
            Ok(json!({ "path": at.to_string(), "replacements": found }))
        }
        _ => {
            let at = path(&arguments::<Delete>(params)?.path)?;
            terminal.delete(&at).await.map_err(|e| match e {
                TerminalError::InvalidPath(_) if at.is_root() => ToolError::InvalidParameter("the workspace itself cannot be deleted".to_string()),
                e => failed(e),
            })?;
            Ok(json!({ "path": at.to_string(), "deleted": true }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolProvider;
    use crate::tool_impl::terminal_tool::tests::tool as terminal_tool;
    use crate::workspace::testing::session_with_workspace;
    use caelumora_model::agent::AgentChannelState;

    fn knowledge(state: &AgentChannelState) -> Vec<KnowledgeRef> {
        state.turn.as_ref().map(|turn| turn.work.context.knowledge.clone()).unwrap_or_default()
    }

    #[tokio::test]
    async fn files_are_written_listed_read_searched_replaced_and_deleted() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = world.turn(&mut state);
        let tool = terminal_tool(json!({}));
        let run = |name: &'static str, params: Value| (name, params);

        let calls = [
            run(WRITE_FILE, json!({ "path": "docs/plan.md", "content": "# Plan\nfirst step\nsecond step\n" })),
            run(WRITE_FILE, json!({ "path": "docs/notes.txt", "content": "a side step\n" })),
        ];
        for (name, params) in calls {
            let written = tool.execute(name, &params, None, &mut turn).await.unwrap();
            assert_eq!(written.classification.as_ref(), Some(&world.workspace.classification));
        }
        assert_eq!(std::fs::read_to_string(world.dir().join("docs/plan.md")).unwrap(), "# Plan\nfirst step\nsecond step\n");

        let listed = tool.execute(LIST_FILES, &json!({ "path": "docs" }), None, &mut turn).await.unwrap().value;
        let names: Vec<&str> = listed["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["notes.txt", "plan.md"]);

        let read = tool.execute(READ_FILE, &json!({ "path": "docs/plan.md", "start_line": 2, "line_count": 1 }), None, &mut turn).await.unwrap().value;
        assert_eq!((read["content"].as_str(), read["end_line"].as_u64(), read["total_lines"].as_u64()), (Some("first step\n"), Some(2), Some(3)));

        let found = tool.execute(GREP_FILES, &json!({ "pattern": "step$" }), None, &mut turn).await.unwrap().value;
        let hits: Vec<(String, u64)> = found["matches"].as_array().unwrap().iter().map(|m| (m["path"].as_str().unwrap().to_string(), m["line"].as_u64().unwrap())).collect();
        assert_eq!(hits, [("docs/notes.txt".to_string(), 1), ("docs/plan.md".to_string(), 2), ("docs/plan.md".to_string(), 3)]);
        assert_eq!(found["complete"], true);

        let ambiguous = tool.execute(REPLACE_IN_FILE, &json!({ "path": "docs/plan.md", "old_text": "step", "new_text": "move" }), None, &mut turn).await;
        assert!(matches!(&ambiguous, Err(ToolError::InvalidParameter(why)) if why.contains("occurs 2 times")), "{ambiguous:?}");
        let replaced = tool.execute(REPLACE_IN_FILE, &json!({ "path": "docs/plan.md", "old_text": "first step", "new_text": "first move" }), None, &mut turn).await.unwrap().value;
        assert_eq!(replaced["replacements"], 1);
        tool.execute(REPLACE_IN_FILE, &json!({ "path": "docs/plan.md", "old_text": "step", "new_text": "stride", "replace_all": true }), None, &mut turn).await.unwrap();
        assert_eq!(std::fs::read_to_string(world.dir().join("docs/plan.md")).unwrap(), "# Plan\nfirst move\nsecond stride\n");
        let absent = tool.execute(REPLACE_IN_FILE, &json!({ "path": "docs/plan.md", "old_text": "nowhere", "new_text": "x" }), None, &mut turn).await;
        assert!(matches!(absent, Err(ToolError::InvalidParameter(_))));

        let not_empty = tool.execute(DELETE_FILE, &json!({ "path": "docs" }), None, &mut turn).await;
        assert!(matches!(&not_empty, Err(ToolError::InvalidParameter(why)) if why.contains("not empty")), "{not_empty:?}");
        tool.execute(DELETE_FILE, &json!({ "path": "docs/notes.txt" }), None, &mut turn).await.unwrap();
        assert!(!world.dir().join("docs/notes.txt").exists());
        let outside = tool.execute(READ_FILE, &json!({ "path": "../secret" }), None, &mut turn).await;
        assert!(matches!(outside, Err(ToolError::InvalidParameter(_))), "the model's to correct");

        drop(turn);
        let read: Vec<Option<caelumora_model::digest::Digest>> = knowledge(&state).into_iter().map(|k| match k.source {
            KnowledgeSource::Workspace { workspace, digest } => {
                assert_eq!((workspace, k.classification.as_ref(), k.channel), (world.workspace.id, Some(&world.workspace.classification), world.session.channels()[0].id()));
                digest
            }
            other => panic!("{other:?}"),
        }).collect();
        // The listing and the search are one entry, each content read is
        // one: the file as written, after the first replacement, after the
        // second.
        assert_eq!((read.iter().filter(|d| d.is_none()).count(), read.iter().filter(|d| d.is_some()).count()), (1, 3), "{read:?}");
    }

    #[tokio::test]
    async fn a_file_longer_than_a_result_is_given_in_parts_and_one_that_is_no_text_not_at_all() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = world.turn(&mut state);
        let tool = terminal_tool(json!({ "max_output_bytes": 10 }));
        std::fs::write(world.dir().join("long.txt"), "line one\nline two\nline three\n").unwrap();
        std::fs::write(world.dir().join("image.bin"), [0xff, 0xfe, 0x00, 0x81]).unwrap();

        let first = tool.execute(READ_FILE, &json!({ "path": "long.txt" }), None, &mut turn).await.unwrap().value;
        assert_eq!((first["content"].as_str(), first["end_line"].as_u64()), (Some("line one\n"), Some(1)));
        assert!(first["next"].as_str().unwrap().contains("start_line 2"), "{first}");
        let last = tool.execute(READ_FILE, &json!({ "path": "long.txt", "start_line": 3 }), None, &mut turn).await.unwrap().value;
        assert_eq!(last["content"], "line three\n");
        assert!(last.get("next").is_none());
        let beyond = tool.execute(READ_FILE, &json!({ "path": "long.txt", "start_line": 9 }), None, &mut turn).await;
        assert!(matches!(beyond, Err(ToolError::InvalidParameter(_))));

        let binary = tool.execute(READ_FILE, &json!({ "path": "image.bin" }), None, &mut turn).await;
        assert!(matches!(&binary, Err(ToolError::InvalidParameter(why)) if why.contains("no text")), "{binary:?}");
        let searched = tool.execute(GREP_FILES, &json!({ "pattern": "line" }), None, &mut turn).await.unwrap().value;
        assert_eq!((searched["files_searched"].as_u64(), searched["files_skipped"].as_u64(), searched["matches"].as_array().unwrap().len()), (Some(2), Some(1), 3), "{searched}");
        let bad = tool.execute(GREP_FILES, &json!({ "pattern": "(" }), None, &mut turn).await;
        assert!(matches!(bad, Err(ToolError::InvalidParameter(_))));
    }

    #[tokio::test]
    async fn a_write_beyond_what_the_workspace_takes_is_refused() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = world.turn(&mut state);
        let tool = terminal_tool(json!({}));
        let too_large = "x".repeat(crate::workspace::testing::MAX_FILE_BYTES as usize + 1);
        let refused = tool.execute(WRITE_FILE, &json!({ "path": "big.txt", "content": too_large }), None, &mut turn).await;
        assert!(matches!(&refused, Err(ToolError::InvalidParameter(why)) if why.contains("larger than")), "{refused:?}");
        assert!(!world.dir().join("big.txt").exists());
    }

    #[test]
    fn the_approver_reads_the_change_whole_and_a_rule_covers_the_file() {
        let tool = terminal_tool(json!({}));
        let write = json!({ "path": "docs/a.md", "content": "hello\nworld" });
        assert_eq!(tool.describe(WRITE_FILE, &write).unwrap(), "Write 'docs/a.md' in the workspace (11 bytes):\nhello\nworld");
        let replace = json!({ "path": "a.md", "old_text": "x", "new_text": "y" });
        assert_eq!(tool.describe(REPLACE_IN_FILE, &replace).unwrap(), "Replace in 'a.md' in the workspace, once:\n--- old\nx\n+++ new\ny");
        let matcher = tool.matcher(WRITE_FILE, &write);
        assert_eq!((matcher.tool.as_str(), matcher.arguments.keys().map(String::as_str).collect::<Vec<_>>()), (WRITE_FILE, vec!["path"]), "not what is written");
        assert_eq!(matcher.arguments["path"], regex::escape("docs/a.md"));
        let reading = tool.matcher(READ_FILE, &json!({ "path": "a.md", "start_line": 2 }));
        assert_eq!(reading.arguments.len(), 2, "a read is alike when exactly alike");
    }
}
