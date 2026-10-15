//! The `channel_history` component (issue 17): an agent reads the history of
//! another channel of its session - the latest messages, or those that
//! contain a text. Only channels the agent's channel may read by their
//! classifications can be read - as the turn's views answer
//! (`Turn::session`, issues 133, 163); every read is recorded as a
//! `KnowledgeRef` with the turn's messages.

use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::tool::{arguments, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::question::Answers;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::confidentiality::Classification;
use caelumora_model::message::Message;
use caelumora_model::tool::{Tool, ToolVisibility, ToolParameters};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub const CHANNEL_HISTORY_KIND: &str = "channel_history";

pub const READ_HISTORY_TOOL_NAME: &str = "read_history";

/// How many messages are returned unless the call asks for fewer.
const DEFAULT_LAST: usize = 20;
/// The most messages one call returns.
const MAX_LAST: usize = 100;

#[derive(Debug)]
pub struct ChannelHistoryComponent;

#[async_trait]
impl ToolProvider for ChannelHistoryComponent {
    fn description(&self) -> String {
        "Read the history of the other channels of this session.".to_string()
    }

    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: READ_HISTORY_TOOL_NAME.to_string(),
            description: format!(
                "Read the conversation of another channel of this session: its latest messages, or \
                those containing a text. Returns at most {MAX_LAST} messages, the latest {DEFAULT_LAST} \
                unless you ask for another number. Use `read_notes` without a channel to find out \
                which channels there are."
            ),
            parameters: ToolParameters::object(
                [
                    ("channel", ToolParameters::string("The name of the channel")),
                    ("last", ToolParameters::number("How many of the latest messages to return")),
                    ("query", ToolParameters::string("Only messages containing this text, ignoring case")),
                ],
                ["channel"],
            ),
            requires_approval: false,
            visibility: ToolVisibility::SHOWN,
            classification: None,
            presentation: Default::default(),
        }]
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        if name != READ_HISTORY_TOOL_NAME {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        #[derive(Deserialize)]
        struct ReadHistory {
            channel: String,
            #[serde(default)]
            last: Option<usize>,
            #[serde(default)]
            query: Option<String>,
        }
        let ReadHistory { channel, last, query } = arguments(params)?;
        let channel = channel.trim();
        if channel.is_empty() {
            return Err(ToolError::InvalidParameter("'channel' must name a channel".to_string()));
        }
        let last = last.map(|n| n.clamp(1, MAX_LAST)).unwrap_or(DEFAULT_LAST);
        let query = query.as_deref().map(str::trim).filter(|q| !q.is_empty());

        let Some(session) = turn.session().await else {
            return Err(ToolError::ExecutionFailed("the turn is in no session to read from".to_string()));
        };
        let channel = session.channel_named(channel).await?;
        let history = channel.history().await?;
        let said = history.messages();
        let (messages, more) = select(&said, last, query);
        // What was read is classified as the messages given (issue 168).
        let classification = Classification::join_all(messages.iter().map(|m| &m.classification));
        let source = KnowledgeSource::History { digest: history.digest, start: history.start };
        turn.record_knowledge(KnowledgeRef { channel: channel.id(), session: channel.session().id(), source, classification: classification.clone() });

        let value = serde_json::json!({
            "channel": channel.name(),
            "messages": messages.iter().map(|m| serde_json::json!({
                "from": m.sender_name,
                "at": m.timestamp.to_rfc3339(),
                "text": m.content.clone().unwrap_or_default(),
            })).collect::<Vec<_>>(),
            "more": more,
        });
        Ok(ToolOutput { value, classification })
    }
}

/// The latest `last` messages with text, of those containing `query` if
/// given, oldest first - and whether there are more such messages.
fn select<'a>(messages: &'a [Message], last: usize, query: Option<&str>) -> (Vec<&'a Message>, bool) {
    let query = query.map(str::to_lowercase);
    let matching: Vec<&Message> = messages.iter()
        .filter(|m| m.content.as_deref().is_some_and(|c| !c.is_empty()))
        .filter(|m| match &query {
            Some(q) => m.content.as_deref().is_some_and(|c| c.to_lowercase().contains(q)),
            None => true,
        })
        .collect();
    let start = matching.len().saturating_sub(last);
    (matching[start..].to_vec(), start > 0)
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(CHANNEL_HISTORY_KIND).provides::<dyn ToolProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(ChannelHistoryComponent));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::message::MessageId;

    fn text(content: &str) -> Message {
        Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }
    }

    #[test]
    fn the_latest_messages_are_selected_oldest_first() {
        let messages: Vec<Message> = ["one", "two", "three"].into_iter().map(text).collect();
        let (selected, more) = select(&messages, 2, None);
        let selected: Vec<&str> = selected.iter().map(|m| m.content.as_deref().unwrap()).collect();
        assert_eq!(selected, ["two", "three"]);
        assert!(more);
        assert!(!select(&messages, 3, None).1, "all of them");
    }

    #[test]
    fn a_query_selects_what_contains_it_ignoring_case() {
        let messages: Vec<Message> = ["Tea time", "coffee", "More TEA"].into_iter().map(text).collect();
        let (selected, more) = select(&messages, 10, Some("tea"));
        let selected: Vec<&str> = selected.iter().map(|m| m.content.as_deref().unwrap()).collect();
        assert_eq!(selected, ["Tea time", "More TEA"]);
        assert!(!more, "messages without text or the query do not count");
    }
}
