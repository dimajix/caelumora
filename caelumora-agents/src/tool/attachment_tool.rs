//! The `attachment_tool` component (issue 282): the tools with which an agent reaches the
//! attachments it heard, by the numbers its transcript gave them (`a3`) - shows its model an image
//! again, reads a text a page at a time, copies one into the workspace it may use to work on it
//! there, or attaches one to its next message. Each reads the message that carries the attachment
//! as it stands, through the view of the agent's channel: one a change removed is not found, and
//! its label is the current one.

use crate::prompt::{NAMESPACE, texts_of, writing_for_the_model};
use crate::situation::Situation;
use crate::tool::{arguments, described, page_of, tool_text, usable_workspace, ToolError, ToolOutput, ToolProvider, ToolResult};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_model::content::{ContentId, ContentRef};
use caelumora_model::message::MessageId;
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_model::question::Answers;
use caelumora_model::security::Grant;
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use caelumora_runtime::attachment::AttachmentManager;
use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use caelumora_runtime::terminal::{ByteStream, WorkspacePath};
use caelumora_runtime::view::ChannelView;
use caelumora_runtime::workspace::WorkspaceManager;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::error;

pub const ATTACHMENT_TOOL_KIND: &str = "attachment_tool";

pub const VIEW_ATTACHMENT: &str = "view_attachment";
pub const READ_ATTACHMENT: &str = "read_attachment";
pub const COPY_ATTACHMENT: &str = "copy_attachment";
pub const ATTACH: &str = "attach";

/// How many bytes of a text `read_attachment` gives at once, as `read_file` gives 64 KiB unless
/// configured.
const MAX_PAGE_BYTES: usize = 64 * 1024;

/// Where `copy_attachment` puts an attachment unless told: under this directory, by its name.
const COPIED_TO: &str = "attachments";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Named {
    #[serde(rename = "ref")]
    reference: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    #[serde(rename = "ref")]
    reference: String,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    line_count: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyTo {
    #[serde(rename = "ref")]
    reference: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug)]
pub struct AttachmentTool {
    /// What its tool set is for and its tools, as the prompt catalogue words them.
    description: String,
    tools: Vec<Tool>,
    /// The texts its results give.
    prompts: PromptTexts,
    /// Where an attachment is copied for the turn's next message.
    attachments: Arc<AttachmentManager>,
    /// Where the largest file a workspace takes is said.
    workspaces: Arc<WorkspaceManager>,
}

impl AttachmentTool {
    /// The provider, its texts from `prompts`, copying what it attaches through `attachments`,
    /// writing no larger file into a workspace than `workspaces` take.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub fn new(prompts: PromptTexts, attachments: Arc<AttachmentManager>, workspaces: Arc<WorkspaceManager>) -> Result<Self, PromptError> {
        let reference = || ("ref".to_string(), ToolParameters::string());
        let tool = |name: &str, parameters: ToolParameters, requires_approval: bool| described(&prompts, NAMESPACE, ATTACHMENT_TOOL_KIND, Tool {
            name: name.to_string(),
            description: String::new(),
            parameters,
            requires_approval,
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: ToolPresentation::default(),
        }, context! { copied_to => COPIED_TO });
        let tools = vec![
            tool(VIEW_ATTACHMENT, ToolParameters::object([reference()], ["ref"]), false)?,
            tool(READ_ATTACHMENT, ToolParameters::object([reference(), ("start_line".to_string(), ToolParameters::number()), ("line_count".to_string(), ToolParameters::number())], ["ref"]), false)?,
            // It writes into the workspace: always asked, as a write there is.
            tool(COPY_ATTACHMENT, ToolParameters::object([reference(), ("path".to_string(), ToolParameters::string())], ["ref"]), true)?,
            tool(ATTACH, ToolParameters::object([reference()], ["ref"]), false)?,
        ];
        let description = tool_text(&prompts, NAMESPACE, ATTACHMENT_TOOL_KIND, "description", ())?;
        Ok(Self { description, tools, prompts, attachments, workspaces })
    }

    /// The text `key` of its results, rendered with `values`; `None`, logged, if it cannot be had -
    /// the result is given without it.
    fn text(&self, key: &str, values: impl Serialize) -> Option<String> {
        tool_text(&self.prompts, NAMESPACE, ATTACHMENT_TOOL_KIND, key, values)
            .inspect_err(|e| error!("A result of the attachment tools is given without saying what it did: {e}"))
            .ok()
    }
}

/// Where the attachment the model of `turn` names `reference` lies: the message that carries it,
/// and its id - and the view of the agent's channel, through which it is read as that message
/// stands.
///
/// # Errors
/// `InvalidParameter` if its transcript numbered none so; `ExecutionFailed` if the turn is in no
/// channel, and what reading the channel reports.
async fn place_of(turn: &Turn<'_>, reference: &str) -> ToolResult<((MessageId, ContentId), ChannelView)> {
    let place = turn.situation().agent_channel_state().attachments.named(reference)
        .ok_or_else(|| ToolError::InvalidParameter(format!("there is no attachment '{reference}' in this conversation")))?;
    let channel = turn.situation().channel().await?.ok_or_else(|| ToolError::ExecutionFailed("the turn is in no channel".to_string()))?;
    Ok((place, channel))
}

/// The attachment the model of `turn` names `reference`, as its message stands, and the view of
/// the agent's channel it was read through.
///
/// # Errors
/// As `place_of`; `InvalidParameter` if its message no longer carries it.
async fn attachment(turn: &Turn<'_>, reference: &str) -> ToolResult<(ContentRef, ChannelView)> {
    let ((message, id), channel) = place_of(turn, reference).await?;
    Ok((channel.attachment(message, id).await?, channel))
}

/// The same with its content.
///
/// # Errors
/// As `attachment`; `ExecutionFailed` if its storage fails.
async fn attachment_content(turn: &Turn<'_>, reference: &str) -> ToolResult<(ContentRef, ByteStream)> {
    let ((message, id), channel) = place_of(turn, reference).await?;
    let (attachment, _, content) = channel.attachment_content(message, id).await?;
    Ok((attachment, content))
}

#[async_trait]
impl ToolProvider for AttachmentTool {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    /// Where the agent's transcript numbered attachments - elsewhere none, which is no tool
    /// unavailable but nothing to reach -; `view_attachment` only to a model that sees images,
    /// `copy_attachment` only where a workspace may be written to, as the workspace's tools offer
    /// their writes.
    async fn offered(&self, situation: &Situation<'_>) -> Result<Vec<Tool>, String> {
        if situation.agent_channel_state().attachments.is_empty() {
            return Ok(Vec::new());
        }
        let sees = situation.profile().images;
        let writable = match usable_workspace(situation).await {
            Ok(workspace) => workspace.may_write().await,
            Err(_) => false,
        };
        let offered = |tool: &&Tool| match tool.name.as_str() {
            VIEW_ATTACHMENT => sees,
            COPY_ATTACHMENT => writable,
            _ => true,
        };
        Ok(self.tools.iter().filter(offered).cloned().collect())
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        match name {
            VIEW_ATTACHMENT => {
                let Named { reference } = arguments(params)?;
                let (attachment, _) = attachment(turn, &reference).await?;
                if !attachment.is_image() {
                    return Err(ToolError::InvalidParameter(format!("'{reference}' is no image but {}; read it or copy it into the workspace instead", attachment.media_type)));
                }
                let value = json!({ "ref": reference, "name": attachment.name.as_str(), "shown": true });
                let classification = attachment.classification.clone();
                Ok(ToolOutput { value, classification: Some(classification), shown: vec![attachment] })
            }
            READ_ATTACHMENT => {
                let Read { reference, start_line, line_count } = arguments(params)?;
                let (attachment, content) = attachment_content(turn, &reference).await?;
                if !attachment.is_text() {
                    return Err(ToolError::InvalidParameter(format!("'{reference}' is no text but {}; copy it into the workspace to work on it there", attachment.media_type)));
                }
                let bytes: Vec<u8> = content.map_ok(|chunk| chunk.to_vec()).try_concat().await.map_err(|e| ToolError::ExecutionFailed(format!("the attachment cannot be read: {}", e.kind())))?;
                let mut value = page_of(&String::from_utf8_lossy(&bytes), &format!("'{reference}'"), start_line, line_count, MAX_PAGE_BYTES)?.value(&self.prompts);
                value["ref"] = json!(reference);
                value["name"] = json!(attachment.name.as_str());
                Ok(ToolOutput::classified(value, attachment.classification))
            }
            COPY_ATTACHMENT => {
                let CopyTo { reference, path } = arguments(params)?;
                // A workspace used in the channel takes what the channel holds: the attachment's
                // label flows into it as the channel's does.
                let (workspace, terminal) = turn.workspace_terminal(Grant::Write).await.map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
                let (attachment, content) = attachment_content(turn, &reference).await?;
                let at = WorkspacePath::parse(&path.unwrap_or_else(|| format!("{COPIED_TO}/{}", attachment.name.as_str())))?;
                let written = terminal.write(&at, content, self.workspaces.max_file_bytes()).await?;
                Ok(ToolOutput::classified(json!({ "ref": reference, "path": at.to_string(), "bytes": written }), workspace.classification))
            }
            ATTACH => {
                let Named { reference } = arguments(params)?;
                let (attachment, channel) = attachment(turn, &reference).await?;
                let copy = self.attachments.copy(&channel, &attachment).await?;
                let classification = copy.classification.clone();
                turn.put_aside(copy);
                let value = json!({ "ref": reference, "name": attachment.name.as_str(), "attached": self.text("attached", ()).unwrap_or_default() });
                Ok(ToolOutput::classified(value, classification))
            }
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }

    fn describe(&self, name: &str, params: &Value) -> ToolResult<String> {
        match name {
            COPY_ATTACHMENT => {
                let CopyTo { reference, path } = arguments(params)?;
                Ok(match path {
                    Some(path) => format!("Copy attachment '{reference}' into the workspace as '{path}'"),
                    None => format!("Copy attachment '{reference}' into the workspace under '{COPIED_TO}/'"),
                })
            }
            _ => self.tools.iter().find(|tool| tool.name == name).map(|tool| tool.description.clone()).ok_or_else(|| ToolError::UnknownTool(name.to_string())),
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(ATTACHMENT_TOOL_KIND).provides::<dyn ToolProvider>().uses::<AttachmentManager>().uses::<WorkspaceManager>()),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let attachments = Arc::clone(context.service::<AttachmentManager>()?);
            let workspaces = Arc::clone(context.service::<WorkspaceManager>()?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(AttachmentTool::new(texts, attachments, workspaces)?));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::context::ModelProfile;
    use caelumora_runtime::agent::state::AgentStateHandle;
    use caelumora_runtime::testing::actor_runtime_in;
    use caelumora_model::confidentiality::{Classification, Confidentiality};
    use caelumora_model::content::ContentRef;
    use caelumora_model::history::HistoryEntry;
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::scope::Scope;
    use caelumora_model::workspace::Workspace;
    use caelumora_runtime::session::SessionLoop;
    use caelumora_runtime::terminal::ByteStream;
    use caelumora_runtime::testing::World;
    use caelumora_runtime::view::Viewer;
    use futures::stream;
    use std::fs;
    use std::time::Duration;
    use tokio::time;
    use tokio_util::bytes::Bytes;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nthe rest";

    fn bytes(content: &'static [u8]) -> (u64, ByteStream) {
        (content.len() as u64, Box::pin(stream::iter([Ok(Bytes::from_static(content))])))
    }

    /// A session of alice's with her agent "helper" and her workspace "docs" in it.
    async fn session_with_helper(world: &World) -> (Arc<SessionLoop>, AgentId, Workspace) {
        let personality = world.personality("plain", None, &[]).await;
        let helper = world.agent("helper", world.alice, &personality).await;
        let alice = World::as_user(world.alice);
        let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "shell", None, Classification::general(Confidentiality::Internal)).await.unwrap();
        let session = world.shared_session(&[helper], &[]).await;
        session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
        (session, helper, docs)
    }

    /// Alice says a message carrying a text and an image in the first channel of `session`: the
    /// message as its history holds it.
    async fn said_with_attachments(world: &World, session: &SessionLoop) -> Message {
        let main = &session.channels()[0];
        let view = world.manager.views(Viewer::new(World::as_user(world.alice))).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
        let attachments = world.runtime.attachment_manager();
        let notes = attachments.create(&view, "notes.txt", None, bytes(b"line one\nline two\n")).await.unwrap();
        let chart = attachments.create(&view, "chart.png", None, bytes(PNG)).await.unwrap();
        main.user_actor(world.alice).unwrap().send_message(Some("look".to_string()), None, vec![notes.id, chart.id]).await.unwrap();
        time::timeout(Duration::from_secs(5), async {
            loop {
                let page = view.history_page(None, 10).await.unwrap();
                if let Some(message) = page.entries().filter_map(HistoryEntry::said).find(|said| !said.attachments.is_empty()) {
                    break message.clone();
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("said")
    }

    fn tool(world: &World) -> AttachmentTool {
        AttachmentTool::new(testing::prompts(), Arc::clone(world.runtime.attachment_manager()), Arc::clone(world.runtime.workspace_manager())).unwrap()
    }

    #[tokio::test]
    async fn an_attachment_heard_is_reached_by_its_number_as_its_message_stands() {
        let world = testing::world().await;
        let (session, helper, docs) = session_with_helper(&world).await;
        let tool = tool(&world);
        let mut state = AgentChannelState::default();
        assert_eq!(tool.offered(&testing::turn(&session, helper, world.alice, &mut state).situation()).await, Ok(Vec::new()), "nothing attached, nothing offered - and nothing unavailable");
        state.turn = None;
        let said = said_with_attachments(&world, &session).await;
        state.perceive(vec![Message { role: MessageRole::User, ..said.clone() }]);
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);
        let offered: Vec<String> = tool.offered(&turn.situation()).await.unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(offered, [READ_ATTACHMENT, COPY_ATTACHMENT, ATTACH], "no image to a model that does not see");

        let (notes, chart) = (&said.attachments[0], &said.attachments[1]);
        let read = tool.execute(READ_ATTACHMENT, &json!({ "ref": "a1", "line_count": 1 }), None, &mut turn).await.unwrap();
        assert_eq!((read.value["content"].as_str(), read.value["total_lines"].as_u64()), (Some("line one\n"), Some(2)), "a page at a time");
        assert_eq!(read.classification.as_ref(), Some(&notes.classification), "classified as it is");
        assert!(matches!(tool.execute(READ_ATTACHMENT, &json!({ "ref": "a2" }), None, &mut turn).await, Err(ToolError::InvalidParameter(e)) if e.contains("image/png")), "an image is no text");

        let viewed = tool.execute(VIEW_ATTACHMENT, &json!({ "ref": "a2" }), None, &mut turn).await.unwrap();
        assert_eq!(viewed.shown.iter().map(|shown| shown.id).collect::<Vec<_>>(), [chart.id], "shown, as it is attached");
        assert!(matches!(tool.execute(VIEW_ATTACHMENT, &json!({ "ref": "a1" }), None, &mut turn).await, Err(ToolError::InvalidParameter(e)) if e.contains("text/plain")), "a text is no image");
        assert!(matches!(tool.execute(VIEW_ATTACHMENT, &json!({ "ref": "a9" }), None, &mut turn).await, Err(ToolError::InvalidParameter(_))), "none numbered so");

        let copied = tool.execute(COPY_ATTACHMENT, &json!({ "ref": "a2" }), None, &mut turn).await.unwrap();
        assert_eq!(copied.value["path"], "attachments/chart.png");
        let place = world.files.path().join("shell").join(docs.id.to_string());
        assert_eq!(fs::read(place.join("attachments").join("chart.png")).unwrap(), PNG, "into the workspace");
        assert_eq!(copied.classification.as_ref(), Some(&docs.classification));
        assert!(tool.tools().iter().find(|t| t.name == COPY_ATTACHMENT).unwrap().requires_approval, "a write, always asked");

        tool.execute(ATTACH, &json!({ "ref": "a1" }), None, &mut turn).await.unwrap();
        let aside: Vec<ContentRef> = turn.situation().agent_channel_state().turn.as_ref().unwrap().attachments.clone();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].id != notes.id && aside[0].key != notes.key && aside[0].digest == notes.digest, "a copy of its own: {aside:?}");
        drop(turn);

        // She takes the text away again: its number names nothing any more.
        let main = &session.channels()[0];
        main.user_actor(world.alice).unwrap().modify_message(said.id, Some("look".to_string()), None, vec![chart.id]).await.unwrap();
        let view = world.manager.views(Viewer::new(World::as_user(world.alice))).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
        time::timeout(Duration::from_secs(5), async {
            while view.message(said.id).await.unwrap().attachments.len() != 1 {
                time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("modified");
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);
        assert!(matches!(tool.execute(READ_ATTACHMENT, &json!({ "ref": "a1" }), None, &mut turn).await, Err(ToolError::InvalidParameter(_))), "as its message stands");
        drop(turn);

        // A model that sees is offered to look at an image again.
        drop(testing::turn(&session, helper, world.alice, &mut state));
        if let Some(begun) = state.turn.as_mut() {
            begun.work.context.frame.profile = ModelProfile { images: true, ..ModelProfile::default() };
        }
        let seeing = Turn::of(helper, "helper".to_string(), &mut state, AgentStateHandle::detached(), actor_runtime_in(&session.channels()[0], ActorId::Agent(helper))).unwrap();
        let offered: Vec<String> = tool.offered(&seeing.situation()).await.unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(offered, [VIEW_ATTACHMENT, READ_ATTACHMENT, COPY_ATTACHMENT, ATTACH]);
        drop(seeing);
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }
}
