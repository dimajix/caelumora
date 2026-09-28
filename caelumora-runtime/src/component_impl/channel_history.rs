//! The `channel_history` component (issue 17): an agent reads the history of
//! another channel of its session - the latest messages, or those that
//! contain a text. Only channels the agent's channel may read by their
//! confidentiality levels can be read (`notes::SessionReader`, issue 61);
//! every read is recorded as a
//! `KnowledgeRef` with the turn's messages.

use crate::agent_turn::TurnContext;
use crate::component::{ComponentFacets, ComponentRegistry};
use crate::session_reader::{NotReadable, SessionReader};
use crate::tool::{ToolError, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::message::{KnowledgeRef, KnowledgeSource, Message};
use caelumora_model::tool::{Tool, ToolParameters, ToolConfidentiality};
use serde_json::Value;
use std::sync::Arc;

pub const CHANNEL_HISTORY_KIND: &str = "channel_history";

pub const READ_HISTORY_TOOL_NAME: &str = "read_history";

/// How many messages are returned unless the call asks for fewer.
const DEFAULT_LAST: usize = 20;
/// The most messages one call returns.
const MAX_LAST: usize = 100;

#[derive(Debug)]
pub struct ChannelHistoryReader {
    /// The session's channels.
    reader: Arc<dyn SessionReader>,
}

#[async_trait]
impl ToolProvider for ChannelHistoryReader {
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
            confidentiality: ToolConfidentiality::PUBLIC,
        }]
    }

    async fn execute(&self, name: &str, params: &Value, context: &mut TurnContext) -> ToolResult<Value> {
        if name != READ_HISTORY_TOOL_NAME {
            return Err(ToolError::UnknownTool(name.to_string()));
        }
        let channel = params.get("channel").and_then(Value::as_str).map(str::trim).unwrap_or_default();
        if channel.is_empty() {
            return Err(ToolError::InvalidParameter("'channel' must name a channel".to_string()));
        }
        let last = params.get("last").and_then(Value::as_u64).map(|n| (n as usize).clamp(1, MAX_LAST)).unwrap_or(DEFAULT_LAST);
        let query = params.get("query").and_then(Value::as_str).map(str::trim).filter(|q| !q.is_empty());

        let history = self.reader.history_of(channel).await.map_err(|e| match e.downcast_ref::<NotReadable>() {
            Some(not_readable) => ToolError::InvalidParameter(not_readable.to_string()),
            None => ToolError::ExecutionFailed(format!("reading failed: {e}")),
        })?;
        context.record_knowledge(KnowledgeRef { channel: history.channel.id, source: KnowledgeSource::History { digest: history.digest } });

        let (messages, more) = select(&history.messages, last, query);
        Ok(serde_json::json!({
            "channel": history.channel.name,
            "messages": messages.iter().map(|m| serde_json::json!({
                "from": m.sender_name,
                "at": m.timestamp.to_rfc3339(),
                "text": m.content.clone().unwrap_or_default(),
            })).collect::<Vec<_>>(),
            "more": more,
        }))
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
        CHANNEL_HISTORY_KIND,
        Box::new(|_descriptor, context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(ChannelHistoryReader { reader: context.session_reader() }));
            Ok(facets)
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::message::MessageId;

    fn text(content: &str) -> Message {
        Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::default() }
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
