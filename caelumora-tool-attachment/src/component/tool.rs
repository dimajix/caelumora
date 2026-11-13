//! The facet `ToolProvider` of `attachments` (issue 282): the tools with which an agent reaches
//! the attachments it heard, by the numbers its transcript gave them (`a3`) - shows its model an
//! image again, reads a text a page at a time, copies one into the workspace it may use to work on
//! it there, or attaches one to its next message. Each reads the message that carries the
//! attachment as it stands, through the view of the agent's channel: one a change removed is not
//! found, and its label is the current one. What `attach` attaches, it has its turn copy
//! (`Turn::copy_attachment`); what `copy_attachment` copies, the workspace's terminal writes no
//! larger than a workspace takes (issue 516).

use super::{AttachmentsComponent, ATTACHMENTS_KIND};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{
    arguments, described, page_of, page_parameters, tool_text, workspace_reader, workspace_writer,
    TextPosition, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_api::error::ContentError;
use caelumora_api::reader::ChannelReader;
use caelumora_api::terminal::{ByteStream, WorkspacePath};
use caelumora_domain::content::{ContentId, ContentRef};
use caelumora_domain::message::MessageId;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::error;

const VIEW_ATTACHMENT: &str = "view_attachment";
const READ_ATTACHMENT: &str = "read_attachment";
const COPY_ATTACHMENT: &str = "copy_attachment";
const ATTACH: &str = "attach";

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
    start_offset: Option<usize>,
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

/// The tools, their texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of their texts cannot be had.
pub(super) fn tools(prompts: &PromptTexts) -> Result<Vec<Tool>, PromptError> {
    let reference = || ("ref", ToolParameters::string());
    let tool = |name: &str, parameters: ToolParameters, requires_approval: bool| {
        described(
            prompts,
            NAMESPACE,
            ATTACHMENTS_KIND,
            Tool {
                name: name.to_string(),
                description: String::new(),
                parameters,
                requires_approval,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: ToolPresentation::default(),
            },
            context! { copied_to => COPIED_TO },
        )
    };
    Ok(vec![
        tool(VIEW_ATTACHMENT, ToolParameters::object([reference()], ["ref"]), false)?,
        tool(
            READ_ATTACHMENT,
            ToolParameters::object([reference()].into_iter().chain(page_parameters()), ["ref"]),
            false,
        )?,
        // It writes into the workspace: always asked, as a write there is.
        tool(
            COPY_ATTACHMENT,
            ToolParameters::object([reference(), ("path", ToolParameters::string())], ["ref"]),
            true,
        )?,
        // What it attaches is shown by the answer that carries it; the call would only say so
        // again.
        Tool {
            presentation: ToolPresentation::QUIET,
            ..tool(ATTACH, ToolParameters::object([reference()], ["ref"]), false)?
        },
    ])
}

impl AttachmentsComponent {
    /// The text `key` of its results, rendered with `values`; `None`, logged, if it cannot be had -
    /// the result is given without it.
    fn text(&self, key: &str, values: impl Serialize) -> Option<String> {
        tool_text(&self.prompts, NAMESPACE, ATTACHMENTS_KIND, key, values)
            .inspect_err(|e| {
                error!("A result of the attachment tools is given without saying what it did: {e}")
            })
            .ok()
    }
}

/// The attachment the model of `turn` names `reference`: the message that carries it, and its id.
///
/// # Errors
/// `InvalidParameter` if its transcript numbered none so.
fn attachment_named(turn: &Turn<'_>, reference: &str) -> ToolResult<(MessageId, ContentId)> {
    turn.situation().agent_channel_state().attachments.named(reference).ok_or_else(|| {
        ToolError::InvalidParameter(format!(
            "there is no attachment '{reference}' in this conversation"
        ))
    })
}

/// Where the attachment the model of `turn` names `reference` lies (`attachment_named`) - and the
/// view of the agent's channel, through which it is read as that message stands.
///
/// # Errors
/// As `attachment_named`; `ExecutionFailed` if the turn is in no channel, and what reading the
/// channel reports.
async fn place_of(
    turn: &Turn<'_>,
    reference: &str,
) -> ToolResult<((MessageId, ContentId), Box<dyn ChannelReader>)> {
    let place = attachment_named(turn, reference)?;
    let channel = turn
        .situation()
        .channel()
        .await?
        .ok_or_else(|| ToolError::ExecutionFailed("the turn is in no channel".to_string()))?;
    Ok((place, channel))
}

/// The attachment the model of `turn` names `reference`, as its message stands, read through the
/// view of the agent's channel.
///
/// # Errors
/// As `place_of`; `InvalidParameter` if its message no longer carries it.
async fn attachment(turn: &Turn<'_>, reference: &str) -> ToolResult<ContentRef> {
    let ((message, id), channel) = place_of(turn, reference).await?;
    Ok(channel.attachment(message, id).await?)
}

/// The same with its content.
///
/// # Errors
/// As `attachment`; `ExecutionFailed` if its storage fails.
async fn attachment_content(
    turn: &Turn<'_>,
    reference: &str,
) -> ToolResult<(ContentRef, ByteStream)> {
    let ((message, id), channel) = place_of(turn, reference).await?;
    let (attachment, _, content) = channel.attachment_content(message, id).await?;
    Ok((attachment, content))
}

#[async_trait]
impl ToolProvider for AttachmentsComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    /// All of them, also before anything is numbered - a tool may give one in the turn under way,
    /// whose tools are fixed as it begins -, but `view_attachment` only to a model that sees
    /// images, `copy_attachment` only where a workspace may be written to, as the workspace's tools
    /// offer their writes.
    async fn offered(&self, situation: &Situation<'_>) -> Result<Vec<Tool>, String> {
        let sees = situation.profile().sees_images();
        let writable = match workspace_reader(situation).await {
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

    async fn execute(
        &self,
        name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
        match name {
            VIEW_ATTACHMENT => {
                let Named { reference } = arguments(params)?;
                let attachment = attachment(turn, &reference).await?;
                if !attachment.is_image() {
                    return Err(ToolError::InvalidParameter(format!("'{reference}' is no image but {}; read it or copy it into the workspace instead", attachment.media_type)));
                }
                let value =
                    json!({ "ref": reference, "name": attachment.name.as_str(), "shown": true });
                let classification = attachment.classification.clone();
                Ok(ToolOutput {
                    shown: vec![attachment],
                    ..ToolOutput::classified(value, classification)
                })
            }
            READ_ATTACHMENT => {
                let Read { reference, start_line, start_offset, line_count } = arguments(params)?;
                let (attachment, content) = attachment_content(turn, &reference).await?;
                let bytes: Vec<u8> = content
                    .map_ok(|chunk| chunk.to_vec())
                    .try_concat()
                    .await
                    .map_err(ContentError::unavailable)?;
                // Its bytes decide, not its label, which goes by its first bytes alone (issue 552).
                let text = String::from_utf8(bytes).map_err(|_| {
                    ToolError::InvalidParameter(format!("'{reference}' cannot be read as text: it is {} and not UTF-8; copy it into the workspace to work on it there", attachment.media_type))
                })?;
                let mut value = page_of(
                    &text,
                    &format!("'{reference}'"),
                    TextPosition::asked(start_line, start_offset),
                    line_count,
                    MAX_PAGE_BYTES,
                )?
                .value(&self.prompts);
                value["ref"] = json!(reference);
                value["name"] = json!(attachment.name.as_str());
                Ok(ToolOutput::classified(value, attachment.classification))
            }
            COPY_ATTACHMENT => {
                let CopyTo { reference, path } = arguments(params)?;
                // A workspace used in the channel takes what the channel holds: the attachment's
                // label flows into it as the channel's does.
                let writer = workspace_writer(turn).await?;
                let (workspace, terminal) = writer.terminal().await?;
                let (attachment, content) = attachment_content(turn, &reference).await?;
                let at = WorkspacePath::parse(
                    &path.unwrap_or_else(|| format!("{COPIED_TO}/{}", attachment.name.as_str())),
                )?;
                let written = terminal.write(&at, content).await?;
                Ok(ToolOutput::classified(
                    json!({ "ref": reference, "path": at.to_string(), "bytes": written }),
                    workspace.classification,
                ))
            }
            ATTACH => {
                let Named { reference } = arguments(params)?;
                let (message, id) = attachment_named(turn, &reference)?;
                let copy = turn.copy_attachment(message, id).await?;
                let (classification, name) = (copy.classification.clone(), copy.name.clone());
                turn.put_aside(copy);
                let value = json!({ "ref": reference, "name": name.as_str(), "attached": self.text("attached", ()).unwrap_or_default() });
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
                    Some(path) => {
                        format!("Copy attachment '{reference}' into the workspace as '{path}'")
                    }
                    None => format!(
                        "Copy attachment '{reference}' into the workspace under '{COPIED_TO}/'"
                    ),
                })
            }
            _ => self
                .tools
                .iter()
                .find(|tool| tool.name == name)
                .map(|tool| tool.description.clone())
                .ok_or_else(|| ToolError::UnknownTool(name.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::prompts;
    use bytes::Bytes;
    use caelumora_agents::testing::{self, agent_runtime};
    use caelumora_api::terminal::ByteStream;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::content::ContentRef;
    use caelumora_domain::context::ModelProfile;
    use caelumora_domain::history::HistoryEntry;
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::message::{Message, MessageRole};
    use caelumora_domain::scope::Scope;
    use caelumora_domain::security::Grant;
    use caelumora_domain::tool::ToolContent;
    use caelumora_domain::workspace::Workspace;
    use caelumora_kernel::session::SessionLoop;
    use caelumora_kernel::testing::{checks, participant_runtime_in, World};
    use caelumora_kernel::view::Viewer;
    use caelumora_kernel::workspace::WorkspaceSpec;
    use futures::stream;
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nthe rest";

    fn bytes(content: &'static [u8]) -> (u64, ByteStream) {
        (content.len() as u64, Box::pin(stream::iter([Ok(Bytes::from_static(content))])))
    }

    /// A session of alice's with her agent "helper" and her workspace "docs" in it.
    async fn session_with_helper(world: &World) -> (Arc<SessionLoop>, AgentId, Workspace) {
        let personality = world.personality("plain", None, &[]).await;
        let helper = world.agent("helper", world.alice, &personality).await;
        let alice = World::as_user(world.alice);
        let docs = world
            .kernel
            .workspace_manager()
            .create(
                &alice,
                WorkspaceSpec::new("docs", Scope::UserScope { user_id: world.alice }, "shell"),
            )
            .await
            .unwrap();
        let session = world.shared_session(&[helper], &[]).await;
        session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
        (session, helper, docs)
    }

    /// Alice says a message carrying `text` and an image in the first channel of `session`: the
    /// message as its history holds it.
    async fn said_with_attachments(
        world: &World,
        session: &SessionLoop,
        text: &'static [u8],
    ) -> Message {
        let main = &session.channels()[0];
        let channel = main.channel();
        let hers = World::as_user(world.alice);
        let attachments = world.kernel.attachment_manager();
        let notes =
            attachments.create(&hers, &channel, "notes.txt", None, bytes(text)).await.unwrap();
        let chart =
            attachments.create(&hers, &channel, "chart.png", None, bytes(PNG)).await.unwrap();
        main.user_actor(world.alice)
            .unwrap()
            .send_message(Some("look".to_string()), None, vec![notes.id, chart.id])
            .await
            .unwrap();
        let view =
            world.manager.views(Viewer::new(hers)).channel(session.id(), main.id()).await.unwrap();
        time::timeout(Duration::from_secs(5), async {
            loop {
                let page = view.history_page(None, 10).await.unwrap();
                if let Some(message) = page
                    .entries()
                    .filter_map(HistoryEntry::said)
                    .find(|said| !said.attachments.is_empty())
                {
                    break message.clone();
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("said")
    }

    fn tool() -> AttachmentsComponent {
        AttachmentsComponent::new(prompts()).unwrap()
    }

    /// A text of one line, longer than a page.
    static LONG_LINE: [u8; MAX_PAGE_BYTES + 1] = [b'x'; MAX_PAGE_BYTES + 1];

    #[tokio::test]
    async fn a_line_longer_than_a_page_is_read_in_parts_of_it() {
        let world = testing::world().await;
        let (session, helper, _) = session_with_helper(&world).await;
        let tool = tool();
        let mut state = AgentChannelState::default();
        let said = said_with_attachments(&world, &session, &LONG_LINE).await;
        state.perceive(vec![Message { role: MessageRole::User, ..said }]);
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);

        let args = json!({ "ref": "a1" });
        let first = tool.execute(READ_ATTACHMENT, &args, None, &mut turn).await.unwrap().value;
        assert_eq!(first["content"].as_str().map(str::len), Some(MAX_PAGE_BYTES));
        assert!(first["next"].as_str().unwrap().contains("start_offset 65536"), "{first}");
        let args = json!({ "ref": "a1", "start_offset": MAX_PAGE_BYTES });
        let last = tool.execute(READ_ATTACHMENT, &args, None, &mut turn).await.unwrap().value;
        assert_eq!(last["content"], "x");
        assert!(last.get("next").is_none(), "{last}");
    }

    /// Issue 552: whether an attachment can be read is told by all of its bytes. UTF-8 is read
    /// whatever control characters it holds, with none of them replaced; what is not UTF-8 is
    /// refused, also where only its end shows it and its label says text.
    #[tokio::test]
    async fn an_attachment_is_read_as_text_if_its_bytes_are_utf8() {
        let world = testing::world().await;
        let (session, helper, _) = session_with_helper(&world).await;
        let main = &session.channels()[0];
        let runtime = agent_runtime(participant_runtime_in(main, ActorId::Agent(helper)));
        let classification = main.classification();
        let mut late = vec![b'x'; 4096];
        late.push(0xff);
        let given = [
            ("colours.txt", Bytes::from_static(b"\x1b[1mbold\x1b[0m\x00\n")),
            ("late.txt", Bytes::from(late)),
        ]
        .map(|(name, bytes)| ToolContent { name: name.to_string(), bytes });
        let kept = runtime
            .create_content(ActorId::User(world.alice), classification.clone(), given.to_vec())
            .await
            .unwrap();
        assert_eq!(kept[1].media_type, "text/plain", "by its first bytes");
        let mut state = AgentChannelState::default();
        let unrecorded =
            Message { attachments: kept, classification, ..make_message(MessageId::new()) };
        state.perceive(vec![unrecorded]);
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);
        let tool = tool();

        let args = json!({ "ref": "a1" });
        let read = tool.execute(READ_ATTACHMENT, &args, None, &mut turn).await.unwrap();
        assert_eq!(read.value["content"], "\u{1b}[1mbold\u{1b}[0m\0\n", "nothing replaced");
        let args = json!({ "ref": "a2" });
        let refused = tool.execute(READ_ATTACHMENT, &args, None, &mut turn).await;
        assert!(
            matches!(&refused, Err(ToolError::InvalidParameter(e)) if e.contains("not UTF-8")),
            "{refused:?}"
        );
        drop(turn);
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }

    /// Issue 547: what a tool gave is read and copied by its number in the step after the call,
    /// before the channel has recorded the message of the result.
    #[tokio::test]
    async fn what_a_tool_kept_is_read_and_copied_before_its_channel_recorded_it() {
        let world = testing::world().await;
        let (session, helper, _) = session_with_helper(&world).await;
        let main = &session.channels()[0];
        let runtime = agent_runtime(participant_runtime_in(main, ActorId::Agent(helper)));
        let classification = main.classification();
        let page =
            ToolContent { name: "page.txt".to_string(), bytes: Bytes::from_static(b"text\n") };
        let kept = runtime
            .create_content(ActorId::User(world.alice), classification.clone(), vec![page])
            .await
            .unwrap();
        let mut state = AgentChannelState::default();
        let unrecorded =
            Message { attachments: kept, classification, ..make_message(MessageId::new()) };
        state.perceive(vec![unrecorded]);
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);
        let tool = tool();

        let args = json!({ "ref": "a1" });
        let read = tool.execute(READ_ATTACHMENT, &args, None, &mut turn).await.unwrap();
        assert_eq!(read.value["content"], "text\n");
        let copied = tool.execute(COPY_ATTACHMENT, &args, None, &mut turn).await.unwrap();
        assert_eq!(copied.value["path"], "attachments/page.txt");
        drop(turn);
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn an_attachment_heard_is_reached_by_its_number_as_its_message_stands() {
        let world = testing::world().await;
        let (session, helper, docs) = session_with_helper(&world).await;
        let tool = tool();
        let mut state = AgentChannelState::default();
        let before: Vec<String> = tool
            .offered(&testing::turn(&session, helper, world.alice, &mut state).situation())
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(
            before,
            [READ_ATTACHMENT, COPY_ATTACHMENT, ATTACH],
            "before anything is numbered too: a tool may give one in the turn under way"
        );
        state.turn = None;
        let said = said_with_attachments(&world, &session, b"line one\nline two\n").await;
        state.perceive(vec![Message { role: MessageRole::User, ..said.clone() }]);
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);
        let offered: Vec<String> =
            tool.offered(&turn.situation()).await.unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(
            offered,
            [READ_ATTACHMENT, COPY_ATTACHMENT, ATTACH],
            "no image to a model that does not see"
        );

        let (notes, chart) = (&said.attachments[0], &said.attachments[1]);
        let read = tool
            .execute(READ_ATTACHMENT, &json!({ "ref": "a1", "line_count": 1 }), None, &mut turn)
            .await
            .unwrap();
        assert_eq!(
            (read.value["content"].as_str(), read.value["total_lines"].as_u64()),
            (Some("line one\n"), Some(2)),
            "a page at a time"
        );
        assert_eq!(
            read.classification.as_ref(),
            Some(&notes.classification),
            "classified as it is"
        );
        assert!(
            matches!(tool.execute(READ_ATTACHMENT, &json!({ "ref": "a2" }), None, &mut turn).await, Err(ToolError::InvalidParameter(e)) if e.contains("image/png")),
            "an image is no text"
        );

        let viewed =
            tool.execute(VIEW_ATTACHMENT, &json!({ "ref": "a2" }), None, &mut turn).await.unwrap();
        assert_eq!(
            viewed.shown.iter().map(|shown| shown.id).collect::<Vec<_>>(),
            [chart.id],
            "shown, as it is attached"
        );
        assert!(
            matches!(tool.execute(VIEW_ATTACHMENT, &json!({ "ref": "a1" }), None, &mut turn).await, Err(ToolError::InvalidParameter(e)) if e.contains("text/plain")),
            "a text is no image"
        );
        assert!(
            matches!(
                tool.execute(VIEW_ATTACHMENT, &json!({ "ref": "a9" }), None, &mut turn).await,
                Err(ToolError::InvalidParameter(_))
            ),
            "none numbered so"
        );

        let copied =
            tool.execute(COPY_ATTACHMENT, &json!({ "ref": "a2" }), None, &mut turn).await.unwrap();
        assert_eq!(copied.value["path"], "attachments/chart.png");
        let place = world.files.path().join("shell").join(docs.id.to_string());
        assert_eq!(
            fs::read(place.join("attachments").join("chart.png")).unwrap(),
            PNG,
            "into the workspace"
        );
        assert_eq!(copied.classification.as_ref(), Some(&docs.classification));
        assert!(
            tool.tools().iter().find(|t| t.name == COPY_ATTACHMENT).unwrap().requires_approval,
            "a write, always asked"
        );

        tool.execute(ATTACH, &json!({ "ref": "a1" }), None, &mut turn).await.unwrap();
        let aside: Vec<ContentRef> =
            turn.situation().agent_channel_state().turn.as_ref().unwrap().attachments.clone();
        assert_eq!(aside.len(), 1);
        assert!(
            aside[0].id != notes.id && aside[0].key != notes.key && aside[0].digest == notes.digest,
            "a copy of its own: {aside:?}"
        );
        assert_eq!(
            tool.tools().iter().find(|t| t.name == ATTACH).unwrap().presentation,
            ToolPresentation::QUIET,
            "the answer shows what was attached, not the call"
        );
        drop(turn);

        // She takes the text away again: its number names nothing any more.
        let main = &session.channels()[0];
        main.user_actor(world.alice)
            .unwrap()
            .modify_message(said.id, Some("look".to_string()), None, vec![chart.id])
            .await
            .unwrap();
        let view = world
            .manager
            .views(Viewer::new(World::as_user(world.alice)))
            .channel(session.id(), main.id())
            .await
            .unwrap();
        time::timeout(Duration::from_secs(5), async {
            while view.message(said.id).await.unwrap().attachments.len() != 1 {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("modified");
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);
        assert!(
            matches!(
                tool.execute(READ_ATTACHMENT, &json!({ "ref": "a1" }), None, &mut turn).await,
                Err(ToolError::InvalidParameter(_))
            ),
            "as its message stands"
        );
        drop(turn);

        // A model that sees is offered to look at an image again.
        drop(testing::turn(&session, helper, world.alice, &mut state));
        if let Some(begun) = state.turn.as_mut() {
            begun.work.context.frame.profile =
                ModelProfile { max_images: 4, ..ModelProfile::default() };
        }
        let seeing = Turn::of(
            &mut state,
            agent_runtime(participant_runtime_in(&session.channels()[0], ActorId::Agent(helper))),
        )
        .unwrap();
        let offered: Vec<String> =
            tool.offered(&seeing.situation()).await.unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(offered, [VIEW_ATTACHMENT, READ_ATTACHMENT, COPY_ATTACHMENT, ATTACH]);
        drop(seeing);
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }

    /// M5 of the security suite: an agent reaches an attachment it heard (issue 282) through the
    /// view of its channel, as its owner, by the number its transcript gave it - asking what
    /// reaching the channel asks, nothing of its own; copying one into the workspace asks what
    /// writing there asks, and attaching one to its answer nothing more.
    #[tokio::test]
    async fn an_agent_reaches_what_it_heard_attached_as_its_owner_reads_the_channel() {
        let world = testing::world().await;
        let (session, helper, _) = session_with_helper(&world).await;
        let said = said_with_attachments(&world, &session, b"line one\nline two\n").await;
        let mut state = AgentChannelState::default();
        state.perceive(vec![Message { role: MessageRole::User, ..said }]);
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);
        let tool = tool();
        let (_, read) = world
            .asked(tool.execute(READ_ATTACHMENT, &json!({ "ref": "a1" }), None, &mut turn))
            .await;
        let (_, attached) =
            world.asked(tool.execute(ATTACH, &json!({ "ref": "a1" }), None, &mut turn)).await;
        let (_, copied) = world
            .asked(tool.execute(COPY_ATTACHMENT, &json!({ "ref": "a1" }), None, &mut turn))
            .await;
        let reaching = [
            "alice as helper: GetSession(alice's) yes",
            "alice as helper: GetChannel(alice's) yes",
        ];
        assert_eq!(
            read,
            checks(&reaching),
            "reaching the channel as its owner, nothing of its own"
        );
        assert_eq!(attached, checks(&reaching), "nor attaching it");
        // The workspace's writer is handed out as the session is reached (issue 516), then writing
        // into the workspace asks what it asks.
        let handed = ["alice as helper: GetSession(alice's) yes"];
        let writing = [
            "alice as helper: GetWorkspace(alice's) yes",
            "alice as helper: Publish yes",
            "alice as helper: Publish yes",
            "alice as helper: WriteWorkspace(alice's) yes",
        ];
        assert_eq!(
            copied,
            checks(&[&handed[..], &reaching[..], &writing[..], &reaching[..]].concat()),
            "copying: the writer handed out, what writing into the workspace asks, then reading it"
        );
        drop(turn);
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }
}
