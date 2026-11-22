//! The file tools of `workspace` (issue 251, step 9), each a type of its own (issue 584): the
//! workspace's files through its terminal, whatever its storage - a shell is not needed -, those
//! that only read through a `TerminalReader` (`reading`, issue 516), those that change through a
//! `Terminal` (`changing`). Paths are relative to the workspace's top directory and made
//! `WorkspacePath`s before anything is asked. Each tool lies in a file of its own, named after it;
//! this module holds what several of them share, and makes them all (`tools`).
//!
//! - `list_files`, `read_file` and `grep_files` only read; what they read they give with their
//!   output as knowledge (`ToolOutput::knowledge`, issue 538), by its path within the workspace
//!   (`KnowledgeSource::within`), a file's content with its digest, for the executor to record.
//!   `publish_file` reads too: it offers a file for download as a link to it as it is now, with its
//!   digest (issue 273), to those who may use the workspace in the channel - which gives nothing
//!   away, so it never needs approval (issue 275).
//! - `write_file`, `replace_in_file`, `move_file` and `delete_file` change; the approver reads the
//!   change whole, and a rule derived from an approved one covers the file, whatever is written -
//!   for a move, that move (`changing_alike`). `move_file` renames as well (issue 267): renaming
//!   is moving within a directory. `write_file` and `replace_in_file` give what they wrote as
//!   knowledge, by its path and the digest of what they wrote: what the agent knows of the file
//!   now.
//!
//! A file is read whole, up to `READ_LIMIT`, and given as text: one that is no UTF-8 is refused.
//! A file the agent last knew otherwise than it is now - another changed it since it read, wrote or
//! moved it - is neither written nor replaced in (issue 538): the model is told to read it again.
//! The check and the write are not one step: what changes the file between them is overwritten.

mod delete_file;
mod grep_files;
mod list_files;
mod move_file;
mod publish_file;
mod read_file;
mod replace_in_file;
mod write_file;

use super::{of_workspace, Use, UsingTool};
use crate::component::WorkspaceParams;
use bytes::Bytes;
use caelumora_agents::tool::{Tool, ToolError, ToolOutput, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_api::terminal::{ByteStream, Terminal, TerminalReader, WorkspacePath};
use caelumora_domain::approval::{ArgumentMatcher, ToolMatcher};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::{KnowledgeRef, KnowledgeSource};
use caelumora_domain::digest::{self, ContentDigest, Digest};
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::tool::ToolDescription;
use caelumora_domain::workspace::{Workspace, WorkspaceId};
use delete_file::DeleteFileTool;
use futures::stream;
use futures::StreamExt;
use grep_files::GrepFilesTool;
use list_files::ListFilesTool;
use move_file::MoveFileTool;
use publish_file::PublishFileTool;
use read_file::ReadFileTool;
use replace_in_file::ReplaceInFileTool;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use write_file::WriteFileTool;

pub(super) const LIST_FILES: &str = "list_files";
pub(in crate::component) const READ_FILE: &str = "read_file";
pub(super) const GREP_FILES: &str = "grep_files";
pub(super) const WRITE_FILE: &str = "write_file";
pub(super) const REPLACE_IN_FILE: &str = "replace_in_file";
pub(super) const MOVE_FILE: &str = "move_file";
pub(super) const DELETE_FILE: &str = "delete_file";
pub(super) const PUBLISH_FILE: &str = "publish_file";

/// The largest file read or searched.
const READ_LIMIT: u64 = 4 * 1024 * 1024;

/// `text` as a path inside the workspace; the model's to correct if it is
/// none.
fn path(text: &str) -> ToolResult<WorkspacePath> {
    WorkspacePath::parse(text).map_err(ToolError::from)
}

/// The file tools, configured by `params`, their texts from `prompts`, each with how it uses the
/// workspace: reading its files, or changing them.
///
/// # Errors
/// `PromptError` if one of their texts cannot be had.
pub(super) fn tools(
    params: &Arc<WorkspaceParams>,
    prompts: &PromptTexts,
) -> Result<Vec<UsingTool>, PromptError> {
    let reads = params.requires_approval.required(true);
    let changes = params.requires_approval.required(false);
    let read = |tool: Arc<dyn Tool>| UsingTool { using: Use::Read, tool };
    let change = |tool: Arc<dyn Tool>| UsingTool { using: Use::Change, tool };
    Ok(vec![
        read(Arc::new(ListFilesTool::new(prompts, reads)?)),
        read(Arc::new(ReadFileTool::new(prompts, params, reads)?)),
        read(Arc::new(GrepFilesTool::new(prompts, reads)?)),
        change(Arc::new(WriteFileTool::new(prompts, changes)?)),
        change(Arc::new(ReplaceInFileTool::new(prompts, changes)?)),
        change(Arc::new(MoveFileTool::new(prompts, changes)?)),
        change(Arc::new(DeleteFileTool::new(prompts, changes)?)),
        read(Arc::new(PublishFileTool::new(prompts)?)),
    ])
}

/// For a change of `tool`, the calls changing the same places as one with `params` - the file
/// named by `keys`, for a move both of its places, whatever is written -; as its parameters tell
/// (`ToolMatcher::proposed`) where `params` name none.
fn changing_alike(tool: &ToolDescription, keys: &[&str], params: &Value) -> ToolMatcher {
    let arguments = keys
        .iter()
        .map(|key| {
            Some((
                key.to_string(),
                ArgumentMatcher::Pattern(regex::escape(params.get(key)?.as_str()?)),
            ))
        })
        .collect::<Option<BTreeMap<_, _>>>();
    match arguments {
        Some(arguments) => ToolMatcher { tool: tool.name.clone(), arguments },
        None => ToolMatcher::proposed(tool, params),
    }
}

/// The bytes of the file `at`, at most `READ_LIMIT`.
async fn content(terminal: &dyn TerminalReader, at: &WorkspacePath) -> ToolResult<Vec<u8>> {
    let (size, mut stream) = terminal.read(at).await?;
    if size > READ_LIMIT {
        return Err(ToolError::InvalidParameter(format!("'{at}' is {size} bytes, more than the {READ_LIMIT} a file may be read with; search it with {GREP_FILES}, or read part of it with a command")));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            ToolError::ExecutionFailed(format!("reading '{at}' broke off: {}", e.kind()))
        })?;
        if bytes.len() as u64 + chunk.len() as u64 > READ_LIMIT {
            return Err(ToolError::InvalidParameter(format!(
                "'{at}' is larger than the {READ_LIMIT} bytes a file may be read with"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// The file `at` as text, with the digest of its content.
pub(in crate::component) async fn text(
    terminal: &dyn TerminalReader,
    at: &WorkspacePath,
) -> ToolResult<(String, Digest)> {
    let bytes = content(terminal, at).await?;
    let digest = digest::content(&bytes);
    let text = String::from_utf8(bytes)
        .map_err(|_| ToolError::InvalidParameter(format!("'{at}' is no text (not UTF-8)")))?;
    Ok((text, digest))
}

/// Writes `content` to the file `at`: how many bytes were written, and the digest of what was.
async fn write(
    terminal: &dyn Terminal,
    at: &WorkspacePath,
    content: String,
) -> ToolResult<(u64, Digest)> {
    let digest = digest::content(content.as_bytes());
    let stream: ByteStream = Box::pin(stream::iter([Ok(Bytes::from(content))]));
    let written = terminal.write(at, stream).await.map_err(ToolError::from)?;
    Ok((written, digest))
}

/// The digest of the file `at`, read in parts, at most the largest file a workspace takes.
async fn digest(terminal: &dyn TerminalReader, at: &WorkspacePath) -> ToolResult<(u64, Digest)> {
    let limit = terminal.max_file_bytes();
    let (size, mut stream) = terminal.read(at).await?;
    if size > limit {
        return Err(ToolError::InvalidParameter(format!(
            "'{at}' is {size} bytes, more than the {limit} a file of the workspace may hold"
        )));
    }
    let (mut digest, mut read) = (ContentDigest::new(size), 0u64);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            ToolError::ExecutionFailed(format!("reading '{at}' broke off: {}", e.kind()))
        })?;
        read += chunk.len() as u64;
        if read > size {
            break;
        }
        digest.update(&chunk);
    }
    if read != size {
        return Err(ToolError::ExecutionFailed(format!(
            "'{at}' changed while it was read; try again once it is written"
        )));
    }
    Ok((size, digest.finish()))
}

/// Refuses to change the file `at`, which the agent last knew as `known` and is `now` (issue 538):
/// another - a person, a command, an agent of another channel - changed it since, whose change
/// writing would overwrite. The model is to read it again.
fn unchanged(at: &WorkspacePath, known: Digest, now: Digest) -> ToolResult<()> {
    if known == now {
        return Ok(());
    }
    Err(ToolError::InvalidParameter(format!(
        "'{at}' changed since you last read or wrote it: read it again before you change it"
    )))
}

/// `value`, which a tool gave of the file `at` of `workspace`, with what the work in `turn` knows
/// of it now, as of `digest` (`known`), classified as the workspace.
fn of_file(
    turn: &Turn<'_>,
    workspace: Workspace,
    at: &WorkspacePath,
    digest: Option<Digest>,
    value: Value,
) -> ToolOutput {
    let knew = known(turn, (workspace.id, &workspace.classification), at, digest);
    of_workspace(knowing(value, [knew]), workspace)
}

/// `value`, with what the tool read or wrote for it, `knowledge` (`ToolOutput::knowledge`).
fn knowing(value: Value, knowledge: impl IntoIterator<Item = KnowledgeRef>) -> ToolOutput {
    ToolOutput { knowledge: knowledge.into_iter().collect(), ..value.into() }
}

/// That the work in `turn` knows `at` in the files of `workspace` - its id and its
/// classification - (issue 538), as of `digest` where a file was read or written whole: what it
/// knows of it now.
pub(in crate::component) fn known(
    turn: &Turn<'_>,
    (id, classification): (WorkspaceId, &Classification),
    at: &WorkspacePath,
    digest: Option<Digest>,
) -> KnowledgeRef {
    KnowledgeRef {
        channel: turn.situation().channel_id(),
        session: turn.situation().session_id(),
        source: KnowledgeSource::within::<Workspace>(id, at.to_string(), digest),
        classification: Some(classification.clone()),
        private: false,
    }
}

/// The digest the agent of `turn` last knew the file `at` of `workspace` by, if it knew it (issue
/// 538).
fn last_known(turn: &Turn<'_>, workspace: &Workspace, at: &WorkspacePath) -> Option<Digest> {
    let situation = turn.situation();
    let state = situation.agent_channel_state();
    state.last_known::<Workspace>(situation.branch_thread_id(), workspace.id, &at.to_string())
}

/// What the tests of the file tools share.
#[cfg(test)]
mod testing {
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_domain::context::KnowledgeRef;

    /// What the turn under way in `state` - the one a test began, in whichever thread - was
    /// given to know.
    pub(super) fn knowledge(state: &AgentChannelState) -> Vec<KnowledgeRef> {
        state
            .turns()
            .next()
            .map(|turn| turn.work.context.frame.knowledge.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::grep_files::FILE_SEARCH_PRESENTATION;
    use super::list_files::FILE_LIST_PRESENTATION;
    use super::move_file::FILE_MOVE_PRESENTATION;
    use super::publish_file::FILE_PUBLISH_PRESENTATION;
    use super::read_file::FILE_READ_PRESENTATION;
    use super::replace_in_file::FILE_EDIT_PRESENTATION;
    use super::testing::knowledge;
    use super::write_file::FILE_WRITE_PRESENTATION;
    use super::*;
    use crate::component::testing::component;
    use caelumora_agents::testing::{tool_named, workspace_turn};
    use caelumora_agents::tool::ToolProvider;
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_domain::digest;
    use caelumora_domain::digest::Digest;
    use caelumora_domain::message::{Message, MessageRole};
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::ToolPresentation;
    use caelumora_domain::tool::{ToolCall, ToolCallResult, ToolResultStatus};
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use serde_json::json;
    use std::fs;

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn files_are_written_listed_read_searched_replaced_and_deleted() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let tool = component(json!({}));
        let run = |name: &'static str, params: Value| (name, params);
        // What each call that succeeded gives with its value, the executor records (issue 538).
        let mut known: Vec<KnowledgeRef> = Vec::new();

        let calls = [
            run(
                WRITE_FILE,
                json!({ "path": "docs/plan.md", "content": "# Plan\nfirst step\nsecond step\n" }),
            ),
            run(WRITE_FILE, json!({ "path": "docs/notes.txt", "content": "a side step\n" })),
        ];
        for (name, params) in calls {
            let written = tool_named(&tool, name).execute(&params, None, &mut turn).await.unwrap();
            assert_eq!(written.classification.as_ref(), Some(&world.workspace.classification));
            known.extend(written.knowledge);
        }
        assert_eq!(
            fs::read_to_string(world.dir().join("docs/plan.md")).unwrap(),
            "# Plan\nfirst step\nsecond step\n"
        );

        let listed = tool_named(&tool, LIST_FILES)
            .execute(&json!({ "path": "docs" }), None, &mut turn)
            .await
            .unwrap();
        known.extend(listed.knowledge);
        let listed = listed.value;
        let names: Vec<&str> = listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["notes.txt", "plan.md"]);

        let read = tool_named(&tool, READ_FILE)
            .execute(
                &json!({ "path": "docs/plan.md", "start_line": 2, "line_count": 1 }),
                None,
                &mut turn,
            )
            .await
            .unwrap();
        known.extend(read.knowledge);
        let read = read.value;
        assert_eq!(
            (read["content"].as_str(), read["end_line"].as_u64(), read["total_lines"].as_u64()),
            (Some("first step\n"), Some(2), Some(3))
        );

        let found = tool_named(&tool, GREP_FILES)
            .execute(&json!({ "pattern": "step$" }), None, &mut turn)
            .await
            .unwrap();
        known.extend(found.knowledge);
        let found = found.value;
        let hits: Vec<(String, u64)> = found["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| (m["path"].as_str().unwrap().to_string(), m["line"].as_u64().unwrap()))
            .collect();
        assert_eq!(
            hits,
            [
                ("docs/notes.txt".to_string(), 1),
                ("docs/plan.md".to_string(), 2),
                ("docs/plan.md".to_string(), 3)
            ]
        );
        assert_eq!(found["complete"], true);

        let ambiguous = tool_named(&tool, REPLACE_IN_FILE)
            .execute(
                &json!({ "path": "docs/plan.md", "old_text": "step", "new_text": "move" }),
                None,
                &mut turn,
            )
            .await;
        assert!(
            matches!(&ambiguous, Err(ToolError::InvalidParameter(why)) if why.contains("occurs 2 times")),
            "{ambiguous:?}"
        );
        let replaced = tool_named(&tool, REPLACE_IN_FILE).execute(&json!({ "path": "docs/plan.md", "old_text": "first step", "new_text": "first move" }), None, &mut turn).await.unwrap();
        assert_eq!(replaced.value["replacements"], 1);
        known.extend(replaced.knowledge);
        let replaced = tool_named(&tool, REPLACE_IN_FILE).execute(&json!({ "path": "docs/plan.md", "old_text": "step", "new_text": "stride", "replace_all": true }), None, &mut turn).await.unwrap();
        known.extend(replaced.knowledge);
        assert_eq!(
            fs::read_to_string(world.dir().join("docs/plan.md")).unwrap(),
            "# Plan\nfirst move\nsecond stride\n"
        );
        let absent = tool_named(&tool, REPLACE_IN_FILE)
            .execute(
                &json!({ "path": "docs/plan.md", "old_text": "nowhere", "new_text": "x" }),
                None,
                &mut turn,
            )
            .await;
        assert!(matches!(absent, Err(ToolError::InvalidParameter(_))));

        let not_empty = tool_named(&tool, DELETE_FILE)
            .execute(&json!({ "path": "docs" }), None, &mut turn)
            .await;
        assert!(
            matches!(&not_empty, Err(ToolError::InvalidParameter(why)) if why.contains("not empty")),
            "{not_empty:?}"
        );
        tool_named(&tool, DELETE_FILE)
            .execute(&json!({ "path": "docs/notes.txt" }), None, &mut turn)
            .await
            .unwrap();
        assert!(!world.dir().join("docs/notes.txt").exists());
        let outside = tool_named(&tool, READ_FILE)
            .execute(&json!({ "path": "../secret" }), None, &mut turn)
            .await;
        assert!(matches!(outside, Err(ToolError::InvalidParameter(_))), "the model's to correct");

        drop(turn);
        assert!(knowledge(&state).is_empty(), "recorded by the executor, not by the tool");
        let read: Vec<(Option<String>, Option<Digest>)> = known
            .into_iter()
            .map(|k| match k.source.about::<Workspace>() {
                Some((workspace, digest)) => {
                    assert_eq!(
                        (workspace, k.classification.as_ref(), k.channel),
                        (
                            world.workspace.id,
                            Some(&world.workspace.classification),
                            world.session.channels()[0].id()
                        )
                    );
                    (k.source.place().map(str::to_string), digest)
                }
                None => panic!("{:?}", k.source),
            })
            .collect();
        // Each by its place (issue 538): the two files as written, the listing of `docs`, the
        // file as read, the search of the whole workspace, the file after the first replacement
        // and after the second - as written, as read, as replaced.
        let plan = |text: &str| Some(digest::content(text.as_bytes()));
        let read: Vec<(Option<&str>, Option<Digest>)> =
            read.iter().map(|(place, digest)| (place.as_deref(), *digest)).collect();
        assert_eq!(
            read,
            [
                (Some("docs/plan.md"), plan("# Plan\nfirst step\nsecond step\n")),
                (Some("docs/notes.txt"), plan("a side step\n")),
                (Some("docs"), None),
                (Some("docs/plan.md"), plan("# Plan\nfirst step\nsecond step\n")),
                (Some(""), None),
                (Some("docs/plan.md"), plan("# Plan\nfirst move\nsecond step\n")),
                (Some("docs/plan.md"), plan("# Plan\nfirst move\nsecond stride\n"))
            ]
        );
    }

    /// What `output` knows, kept on the transcript of `turn` with a result of its call, as the
    /// executor keeps it (issue 538).
    fn kept(turn: &mut Turn<'_>, output: ToolOutput) {
        let call = ToolCall { id: Some("1".to_string()), ..make_tool_call("tool") };
        let result =
            ToolCallResult::of(&call, ToolResultStatus::Ok, String::new(), Classification::PUBLIC);
        turn.record_said(Message {
            role: MessageRole::Tool,
            tool_results: vec![ToolCallResult { knowledge: output.knowledge, ..result }],
            ..Message::new(Classification::PUBLIC)
        });
    }

    /// Issue 538: a file the agent last knew otherwise than it is now - changed by another since
    /// it read, wrote or moved it - is neither written nor replaced in, until the agent read it
    /// again; a search of it does not count as reading it. A file it never knew, and one gone
    /// since, are written as now.
    #[tokio::test]
    async fn a_file_changed_since_the_agent_knew_it_is_not_overwritten() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let tool = component(json!({}));
        let file = |name: &str| world.dir().join(name);
        let changed = |result: &ToolResult<ToolOutput>| {
            matches!(result, Err(ToolError::InvalidParameter(why))
                if why.contains("changed since you last read or wrote it"))
        };
        let write = |path: &str, content: &str| json!({ "path": path, "content": content });
        let replace =
            |old: &str, new: &str| json!({ "path": "a.txt", "old_text": old, "new_text": new });

        let written =
            tool_named(&tool, WRITE_FILE).execute(&write("a.txt", "one"), None, &mut turn).await;
        kept(&mut turn, written.unwrap());
        fs::write(file("a.txt"), "two").unwrap();
        let refused =
            tool_named(&tool, WRITE_FILE).execute(&write("a.txt", "three"), None, &mut turn).await;
        assert!(changed(&refused), "changed since written: {refused:?}");
        let refused = tool_named(&tool, REPLACE_IN_FILE)
            .execute(&replace("two", "four"), None, &mut turn)
            .await;
        assert!(changed(&refused), "nor replaced in: {refused:?}");
        let searched = json!({ "pattern": "two", "path": "a.txt" });
        let searched = tool_named(&tool, GREP_FILES).execute(&searched, None, &mut turn).await;
        kept(&mut turn, searched.unwrap());
        let refused =
            tool_named(&tool, WRITE_FILE).execute(&write("a.txt", "three"), None, &mut turn).await;
        assert!(changed(&refused), "a search is no reading: {refused:?}");
        assert_eq!(fs::read_to_string(file("a.txt")).unwrap(), "two", "as the other left it");

        let read = tool_named(&tool, READ_FILE)
            .execute(&json!({ "path": "a.txt" }), None, &mut turn)
            .await;
        kept(&mut turn, read.unwrap());
        let replaced = tool_named(&tool, REPLACE_IN_FILE)
            .execute(&replace("two", "four"), None, &mut turn)
            .await;
        kept(&mut turn, replaced.unwrap());
        let written =
            tool_named(&tool, WRITE_FILE).execute(&write("a.txt", "five"), None, &mut turn).await;
        assert!(written.is_ok(), "read again, and changed by itself since: {written:?}");
        kept(&mut turn, written.unwrap());
        assert_eq!(fs::read_to_string(file("a.txt")).unwrap(), "five");

        let deleted = tool_named(&tool, DELETE_FILE)
            .execute(&json!({ "path": "a.txt" }), None, &mut turn)
            .await;
        kept(&mut turn, deleted.unwrap());
        fs::write(file("a.txt"), "theirs").unwrap();
        let refused =
            tool_named(&tool, WRITE_FILE).execute(&write("a.txt", "six"), None, &mut turn).await;
        assert!(changed(&refused), "made anew by another: {refused:?}");
        fs::remove_file(file("a.txt")).unwrap();
        let written =
            tool_named(&tool, WRITE_FILE).execute(&write("a.txt", "seven"), None, &mut turn).await;
        assert!(written.is_ok(), "gone since: {written:?}");

        let moved = json!({ "from": "a.txt", "to": "b.txt" });
        kept(&mut turn, written.unwrap());
        let moved = tool_named(&tool, MOVE_FILE).execute(&moved, None, &mut turn).await;
        kept(&mut turn, moved.unwrap());
        fs::write(file("b.txt"), "theirs").unwrap();
        let refused =
            tool_named(&tool, WRITE_FILE).execute(&write("b.txt", "eight"), None, &mut turn).await;
        assert!(changed(&refused), "known where it was moved to: {refused:?}");

        fs::write(file("c.txt"), "theirs").unwrap();
        let never =
            tool_named(&tool, WRITE_FILE).execute(&write("c.txt", "mine"), None, &mut turn).await;
        assert!(never.is_ok(), "never known: written as now");
    }

    #[tokio::test]
    async fn a_file_longer_than_a_result_is_given_in_parts_and_one_that_is_no_text_not_at_all() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let tool = component(json!({ "max_output_bytes": 10 }));
        fs::write(world.dir().join("long.txt"), "line one\nline two\nline 3\n").unwrap();
        fs::write(world.dir().join("image.bin"), [0xff, 0xfe, 0x00, 0x81]).unwrap();

        let first = tool_named(&tool, READ_FILE)
            .execute(&json!({ "path": "long.txt" }), None, &mut turn)
            .await
            .unwrap()
            .value;
        assert_eq!(
            (first["content"].as_str(), first["end_line"].as_u64()),
            (Some("line one\n"), Some(1))
        );
        assert!(first["next"].as_str().unwrap().contains("start_line 2"), "{first}");
        let last = tool_named(&tool, READ_FILE)
            .execute(&json!({ "path": "long.txt", "start_line": 3 }), None, &mut turn)
            .await
            .unwrap()
            .value;
        assert_eq!(last["content"], "line 3\n");
        assert!(last.get("next").is_none());
        let beyond = tool_named(&tool, READ_FILE)
            .execute(&json!({ "path": "long.txt", "start_line": 9 }), None, &mut turn)
            .await;
        assert!(matches!(beyond, Err(ToolError::InvalidParameter(_))));

        let binary = tool_named(&tool, READ_FILE)
            .execute(&json!({ "path": "image.bin" }), None, &mut turn)
            .await;
        assert!(
            matches!(&binary, Err(ToolError::InvalidParameter(why)) if why.contains("no text")),
            "{binary:?}"
        );
        let searched = tool_named(&tool, GREP_FILES)
            .execute(&json!({ "pattern": "line" }), None, &mut turn)
            .await
            .unwrap()
            .value;
        assert_eq!(
            (
                searched["files_searched"].as_u64(),
                searched["files_skipped"].as_u64(),
                searched["matches"].as_array().unwrap().len()
            ),
            (Some(2), Some(1), 3),
            "{searched}"
        );
        let bad = tool_named(&tool, GREP_FILES)
            .execute(&json!({ "pattern": "(" }), None, &mut turn)
            .await;
        assert!(matches!(bad, Err(ToolError::InvalidParameter(_))));
    }

    #[test]
    fn the_approver_reads_the_change_whole_and_a_rule_covers_the_file_or_the_move() {
        let tool = component(json!({}));
        let write = json!({ "path": "docs/a.md", "content": "hello\nworld" });
        assert_eq!(
            tool_named(&tool, WRITE_FILE).describe(&write).unwrap().unwrap(),
            "Write 'docs/a.md' in the workspace (11 bytes):\nhello\nworld"
        );
        let replace = json!({ "path": "a.md", "old_text": "x", "new_text": "y" });
        assert_eq!(
            tool_named(&tool, REPLACE_IN_FILE).describe(&replace).unwrap().unwrap(),
            "Replace in 'a.md' in the workspace, once:\n--- old\nx\n+++ new\ny"
        );
        let matcher = tool_named(&tool, WRITE_FILE).matcher(&write);
        assert_eq!(
            (
                matcher.tool.as_str(),
                matcher.arguments.keys().map(String::as_str).collect::<Vec<_>>()
            ),
            (WRITE_FILE, vec!["path"]),
            "not what is written"
        );
        assert_eq!(matcher.arguments["path"], ArgumentMatcher::Pattern(regex::escape("docs/a.md")));
        let moving = json!({ "from": "docs/a.md", "to": "docs/b.md" });
        assert_eq!(
            tool_named(&tool, MOVE_FILE).describe(&moving).unwrap().unwrap(),
            "Rename 'docs/a.md' to 'docs/b.md' in the workspace"
        );
        assert_eq!(
            tool_named(&tool, MOVE_FILE)
                .describe(&json!({ "from": "docs/a.md", "to": "archive/a.md" }))
                .unwrap()
                .unwrap(),
            "Move 'docs/a.md' to 'archive/a.md' in the workspace"
        );
        let matcher = tool_named(&tool, MOVE_FILE).matcher(&moving);
        assert_eq!(
            matcher.arguments,
            BTreeMap::from([
                ("from".to_string(), ArgumentMatcher::Pattern(regex::escape("docs/a.md"))),
                ("to".to_string(), ArgumentMatcher::Pattern(regex::escape("docs/b.md")))
            ]),
            "that move, both of its places"
        );
        assert_eq!(
            tool_named(&tool, PUBLISH_FILE)
                .describe(&json!({ "path": "out/r.csv", "name": "Report.csv" }))
                .unwrap()
                .unwrap(),
            "Offer 'out/r.csv' in the workspace for download as 'Report.csv'"
        );
        let reading =
            tool_named(&tool, READ_FILE).matcher(&json!({ "path": "a.md", "start_line": 2 }));
        assert_eq!(reading.arguments.len(), 2, "a read is alike when exactly alike");
    }

    #[test]
    fn the_file_tools_are_presented_as_what_they_do_to_files() {
        let presented: BTreeMap<String, ToolPresentation> = component(json!({}))
            .tools()
            .iter()
            .map(|t| (t.description().name.clone(), t.description().presentation))
            .collect();
        assert_eq!(presented[LIST_FILES], FILE_LIST_PRESENTATION);
        assert_eq!(presented[READ_FILE], FILE_READ_PRESENTATION);
        assert_eq!(presented[GREP_FILES], FILE_SEARCH_PRESENTATION);
        assert_eq!(presented[WRITE_FILE], FILE_WRITE_PRESENTATION);
        assert_eq!(presented[REPLACE_IN_FILE], FILE_EDIT_PRESENTATION);
        assert_eq!(presented[MOVE_FILE], FILE_MOVE_PRESENTATION);
        assert_eq!(presented[PUBLISH_FILE], FILE_PUBLISH_PRESENTATION);
        assert_eq!(presented[DELETE_FILE], ToolPresentation::DEFAULT, "a deletion as every call");
    }
}
