//! The `channel_history` component (issue 17): an agent reads the history of another channel of its
//! session - the latest messages, or those that contain a text, in their words or in the
//! description of a call a person made -, each with what it says and the attachments it carries
//! (issue 282), and the calls a person made in it, in the words a model is given them in its own
//! channel (`calls_in_words`, issue 587). Only channels the agent's channel may read by their
//! classifications can be read - as the turn's views answer (`Situation::session`, issues 133,
//! 163); every read is recorded as a `KnowledgeRef` with the turn's messages. When a message was
//! said is written as the model is given the times of its own channel's (issue 424): in the turn's
//! time zone, in the same form.

use crate::components::notes::READ_NOTES_TOOL_NAME;
use crate::prompt::{texts_of, writing_for_the_model, NAMESPACE};
use crate::tool::{
    arguments, described, tool_text, FixedTools, Tool, ToolError, ToolOutput, ToolProvider,
    ToolResult,
};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::chain::Position;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::{calls_in_words, KnowledgeRef, KnowledgeSource};
use caelumora_domain::history::{HistoryEntry, HistoryField};
use caelumora_domain::message::Message;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::query::Filter;
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub const CHANNEL_HISTORY_KIND: &str = "channel_history";

pub const READ_HISTORY_TOOL_NAME: &str = "read_history";

/// How many messages are returned unless the call asks for fewer.
const DEFAULT_LAST: usize = 20;
/// The most messages one call returns.
const MAX_LAST: usize = 100;

/// The tool `read_history`.
#[derive(Debug)]
struct ReadHistoryTool {
    description: ToolDescription,
}

/// The tools of the component, their texts from `prompts`, told how many messages a call returns.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
fn tools(prompts: &PromptTexts) -> Result<FixedTools, PromptError> {
    // What its texts are told.
    let values = context! { max_last => MAX_LAST, default_last => DEFAULT_LAST, read_notes => READ_NOTES_TOOL_NAME };
    let read_history = ReadHistoryTool {
        description: described(
            prompts,
            NAMESPACE,
            CHANNEL_HISTORY_KIND,
            ToolDescription {
                name: READ_HISTORY_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    [
                        ("channel", ToolParameters::string()),
                        ("last", ToolParameters::number()),
                        ("query", ToolParameters::string()),
                    ],
                    ["channel"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: Default::default(),
            },
            &values,
        )?,
    };
    Ok(FixedTools::new(
        tool_text(prompts, NAMESPACE, CHANNEL_HISTORY_KIND, "description", &values)?,
        vec![Arc::new(read_history)],
    ))
}

#[async_trait]
impl Tool for ReadHistoryTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        #[derive(Deserialize)]
        struct Asked {
            channel: String,
            #[serde(default)]
            last: Option<usize>,
            #[serde(default)]
            query: Option<String>,
        }
        let Asked { channel, last, query } = arguments(params)?;
        let channel = channel.trim();
        if channel.is_empty() {
            return Err(ToolError::InvalidParameter("'channel' must name a channel".to_string()));
        }
        let last = last.map(|n| n.clamp(1, MAX_LAST)).unwrap_or(DEFAULT_LAST);
        let query = query.as_deref().map(str::trim).filter(|q| !q.is_empty());

        let Some(session) = turn.situation().session().await? else {
            return Err(ToolError::ExecutionFailed(
                "the turn is in no session to read from".to_string(),
            ));
        };
        let channel = session.channel_named(channel).await?;
        // Read from the end, only as far back as `last` such messages lie
        // (issue 379).
        let page = channel.history_page_of(None, last, Some(wanted(query))).await?;
        let messages: Vec<&Message> = page.entries().filter_map(HistoryEntry::said).collect();
        let more = page.earlier.is_some();
        // What was read is classified as the messages given (issue 168), with what the calls of a
        // person in them answered (issue 587).
        let classification = messages
            .iter()
            .map(|m| m.classification_with_results())
            .collect::<Option<Vec<_>>>()
            .and_then(|all| Classification::join_all(&all));
        let source = KnowledgeSource::History {
            digest: page.digest,
            from: Some(channel.visible_from().await?).filter(|from| *from > Position::START),
        };
        let read = KnowledgeRef {
            channel: channel.id(),
            session: channel.session().id(),
            source,
            classification: classification.clone(),
            private: false,
        };

        let time_zone = turn.situation().time_zone();
        let texts = turn.situation().texts();
        let messages: Vec<Message> = messages
            .into_iter()
            .map(|m| calls_in_words(&texts, m.clone()))
            .collect::<Result<_, _>>()
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let value = serde_json::json!({
            "channel": channel.name(),
            "messages": messages.iter().map(|m| {
                let mut said = serde_json::json!({
                    "from": m.sender_name,
                    "at": time_zone.format(m.timestamp),
                    "text": m.content.clone().unwrap_or_default(),
                });
                if !m.attachments.is_empty() {
                    said["attachments"] = m.attachments.iter().map(|a| serde_json::json!({ "name": a.name.as_str(), "type": a.media_type, "size": a.size_shown() })).collect();
                }
                said
            }).collect::<Vec<_>>(),
            "more": more,
        });
        Ok(ToolOutput { classification, knowledge: vec![read], ..value.into() })
    }
}

/// Which messages the tool returns: those that say something - words or attachments (issue 282) -
/// or carry the calls a person made (issue 587); if `query` is given, those whose words contain it,
/// or that carry a call of a person whose description does - as names are compared
/// (`Filter::Contains`).
fn wanted(query: Option<&str>) -> Filter<HistoryField> {
    match query {
        Some(query) => Filter::any_of([
            Filter::all_of([Filter::written(), Filter::containing(query)]),
            Filter::all_of([Filter::called_by_a_person(), Filter::with_a_call_containing(query)]),
        ]),
        None => Filter::any_of([Filter::saying(), Filter::called_by_a_person()]),
    }
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(CHANNEL_HISTORY_KIND).provides::<dyn ToolProvider>(),
        ),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(tools(&texts)?));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::AgentsLibrary;
    use crate::testing;
    use caelumora_domain::actor::testing::context_in;
    use caelumora_domain::actor::{ActorEvent, ActorId};
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::component::ComponentDescriptor;
    use caelumora_domain::config::AppConfig;
    use caelumora_domain::content::testing::make_content_ref;
    use caelumora_domain::history::{Audience, Recorded};
    use caelumora_domain::message::{MessageId, MessageRole};
    use caelumora_domain::scope::Scope;
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::ToolCall;
    use caelumora_kernel::command::SHELL_KIND;
    use caelumora_kernel::testing::World;
    use caelumora_kernel::workspace::WorkspaceSpec;

    /// `message`, said in a channel's line, recorded as the store keeps it.
    fn recorded(message: Message) -> Value {
        serde_json::to_value(Recorded::new(
            HistoryEntry::Said { message },
            ActorId::System,
            Audience::Everybody,
            &context_in(ChannelId::new()),
        ))
        .unwrap()
    }

    /// A message saying `content`, recorded as the store keeps it.
    fn said(content: &str) -> Value {
        recorded(Message {
            id: MessageId::new(),
            content: Some(content.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        })
    }

    #[test]
    fn a_message_with_text_containing_the_query_is_wanted_ignoring_case() {
        assert!(
            wanted(Some("tea")).matches(&said("Tea time"))
                && wanted(Some("tea")).matches(&said("More TEA"))
        );
        assert!(!wanted(Some("tea")).matches(&said("coffee")));
        assert!(wanted(None).matches(&said("coffee")), "any text without a query");
        assert!(!wanted(None).matches(&said("")), "a message without text does not count");
        let attached = Message {
            id: MessageId::new(),
            attachments: vec![make_content_ref("a.png", b"a")],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let attached = recorded(attached);
        assert!(wanted(None).matches(&attached), "unless it has attachments (issue 282)");
        assert!(!wanted(Some("tea")).matches(&attached), "which no query finds");
        let called = Message {
            id: MessageId::new(),
            role: MessageRole::User,
            tool_calls: vec![make_tool_call("run_command")],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        assert!(wanted(None).matches(&recorded(called.clone())), "a person's call (issue 587)");
        let of_an_agent = Message { role: MessageRole::Assistant, ..called.clone() };
        assert!(!wanted(None).matches(&recorded(of_an_agent)), "not an agent's, its own work");
        let described = ToolCall {
            description: Some("Run in the workspace:\nls -la".to_string()),
            ..make_tool_call("run_command")
        };
        let described = Message { tool_calls: vec![described], ..called.clone() };
        assert!(
            wanted(Some("LS -la")).matches(&recorded(described.clone()))
                && !wanted(Some("tea")).matches(&recorded(described.clone())),
            "a person's call by its description"
        );
        assert!(!wanted(Some("run_command")).matches(&recorded(called)), "none without one");
        let of_an_agent = Message { role: MessageRole::Assistant, ..described };
        assert!(!wanted(Some("ls")).matches(&recorded(of_an_agent)), "nor an agent's call");
    }

    /// Issue 587: a person's call - a command they ran - is read in the words a model is given it
    /// in its own channel, classified with what it answered, and found by its description.
    #[tokio::test]
    async fn a_person_s_call_is_read_in_words_and_classified_with_its_result() {
        let with_shell = |config: &mut AppConfig| {
            config.commands.component_nodes.push(ComponentDescriptor::new("shell", SHELL_KIND));
        };
        let world = World::configured(&[], &[&AgentsLibrary], |_| {}, with_shell).await;
        let alice = World::as_user(world.alice);
        let personality = world.personality("plain", None, &[]).await;
        let helper = world.agent("helper", world.alice, &personality).await;
        let session = world.shared_session(&[helper], &[]).await;
        let docs = WorkspaceSpec::new("docs", Scope::UserScope { user_id: world.alice }, "shell");
        let docs = world.kernel.workspace_manager().create(&alice, docs).await.unwrap();
        session.update_workspace(&alice, Some(docs.id), None).await.unwrap();
        let main = &session.channels()[0];
        let mut admitted = main.observe();
        let hers = main.user_actor(world.alice).unwrap();
        hers.execute_command("workspace run echo notes.md").await.expect("hers to run");
        while !matches!(
            admitted.recv().await.unwrap().event,
            ActorEvent::CompleteMessageGenerated(_)
        ) {}
        let tool =
            testing::tool_named(&tools(&testing::prompts()).unwrap(), READ_HISTORY_TOOL_NAME);
        let mut state = AgentChannelState::default();
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);

        let params = serde_json::json!({ "channel": main.name() });
        let read = tool.execute(&params, None, &mut turn).await.unwrap();
        let text = read.value["messages"][0]["text"].as_str().expect("her call");
        assert!(
            text.starts_with("I called `run_command` with ") && text.contains(r#"notes.md\n"#),
            "{text}"
        );
        assert_eq!(
            read.classification,
            Classification::join_all([&main.classification(), &docs.classification]),
            "as what it answered"
        );
        let sought = |query| serde_json::json!({ "channel": main.name(), "query": query });
        let found = tool.execute(&sought("ECHO notes"), None, &mut turn).await.unwrap();
        assert_eq!(found.value["messages"].as_array().map(Vec::len), Some(1), "by its description");
        let found = tool.execute(&sought("coffee"), None, &mut turn).await.unwrap();
        assert_eq!(found.value["messages"].as_array().map(Vec::len), Some(0), "or not at all");
        drop(turn);
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 538: what `read_history` read it gives with its output, for the executor to record:
    /// the history of the channel it read, by that channel and its session.
    #[tokio::test]
    async fn what_it_read_is_given_with_its_output_by_the_channel_read() {
        let world = testing::world().await;
        let personality = world.personality("plain", None, &[]).await;
        let helper = world.agent("helper", world.alice, &personality).await;
        let session = world.shared_session(&[helper], &[]).await;
        let main = &session.channels()[0];
        let tool =
            testing::tool_named(&tools(&testing::prompts()).unwrap(), READ_HISTORY_TOOL_NAME);
        let mut state = AgentChannelState::default();
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);

        let params = serde_json::json!({ "channel": main.name() });
        let read = tool.execute(&params, None, &mut turn).await.unwrap();
        assert!(
            matches!(read.knowledge.as_slice(), [k] if k.channel == main.id() && k.session == session.id() && matches!(k.source, KnowledgeSource::History { .. })),
            "{:?}",
            read.knowledge
        );
        drop(turn);
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }
}
