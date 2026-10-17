//! The `channel_history` component (issue 17): an agent reads the history of
//! another channel of its session - the latest messages, or those that
//! contain a text. Only channels the agent's channel may read by their
//! classifications can be read - as the turn's views answer
//! (`Situation::session`, issues 133, 163); every read is recorded as a
//! `KnowledgeRef` with the turn's messages.

use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::tool::{arguments, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::question::Answers;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::confidentiality::Classification;
use caelumora_model::history::HistoryEntry;
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

        let Some(session) = turn.situation().session().await? else {
            return Err(ToolError::ExecutionFailed("the turn is in no session to read from".to_string()));
        };
        let channel = session.channel_named(channel).await?;
        // Read from the end, only as far back as `last` such messages lie
        // (issue 379).
        let query = query.map(str::to_lowercase);
        let page = channel.history_page_of(None, last, &|item| wanted(item, query.as_deref())).await?;
        let messages: Vec<&Message> = page.items.iter().filter_map(HistoryEntry::said).collect();
        let more = page.earlier.is_some();
        // What was read is classified as the messages given (issue 168).
        let classification = Classification::join_all(messages.iter().map(|m| &m.classification));
        let source = KnowledgeSource::History { digest: page.digest, from: Some(channel.visible_from()).filter(|from| *from > 0) };
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

/// Whether `item` is a message the tool returns: one with text, containing
/// `query`, lowercase, if given - ignoring case.
fn wanted(item: &HistoryEntry, query: Option<&str>) -> bool {
    let Some(content) = item.said().and_then(|m| m.content.as_deref()).filter(|c| !c.is_empty()) else { return false };
    query.is_none_or(|q| content.to_lowercase().contains(q))
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
    fn a_message_with_text_containing_the_query_is_wanted_ignoring_case() {
        let said = |content: &str| HistoryEntry::Said { message: text(content) };
        assert!(wanted(&said("Tea time"), Some("tea")) && wanted(&said("More TEA"), Some("tea")));
        assert!(!wanted(&said("coffee"), Some("tea")));
        assert!(wanted(&said("coffee"), None), "any text without a query");
        assert!(!wanted(&said(""), None), "a message without text does not count");
    }
}
