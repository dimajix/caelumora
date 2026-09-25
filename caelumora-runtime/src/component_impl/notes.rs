//! The `notes` component (issue 17): an agent notes what it wants to
//! remember in a channel, and is given what every agent of the channel noted.
//!
//! Its tools `note` and `revoke_note` change the agent's own notes - taken
//! over when the turn completes, see `TurnContext::note`; `read_notes` reads
//! the notes of another channel of the session, or of all it may read. Every
//! read is recorded as a `KnowledgeRef` with the turn's messages. Its prompt part
//! gives the model the notes of all agents of the channel before the turn's
//! input (`PromptPosition::BeforeTurnInput`), not in the system prompt, so
//! that changing notes do not invalidate the prompt cache (issue 47).

use crate::actor::TurnContext;
use crate::component::{ComponentFacets, ComponentRegistry};
use crate::component_impl::context::{Prompt, PromptPosition, PromptProvider};
use crate::notes::{NotReadable, PublishedNotes};
use caelumora_model::channel::ChannelId;
use caelumora_model::message::{KnowledgeRef, KnowledgeSource};
use crate::tool::{ToolError, ToolProvider, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::note::{Note, NoteId};
use caelumora_model::tool::{Tool, ToolParameters};
use serde_json::Value;
use std::fmt::Write;
use std::sync::Arc;
use tracing::warn;

pub const NOTES_KIND: &str = "notes";

pub const NOTE_TOOL_NAME: &str = "note";
pub const REVOKE_NOTE_TOOL_NAME: &str = "revoke_note";
pub const READ_NOTES_TOOL_NAME: &str = "read_notes";

#[derive(Debug)]
pub struct NotesComponent;

#[async_trait]
impl ToolProvider for NotesComponent {
    fn tools(&self) -> Vec<Tool> {
        vec![
            Tool {
                name: NOTE_TOOL_NAME.to_string(),
                description: "Note something worth remembering in this channel - a fact, a decision, \
                    a preference of the user. Every participant of the channel is shown your notes \
                    from now on, you included. Keep a note short and self-contained. To change a \
                    note, revoke it and note the new version."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("text", ToolParameters::string("What to note"))],
                    ["text"],
                ),
                requires_approval: false,
            },
            Tool {
                name: REVOKE_NOTE_TOOL_NAME.to_string(),
                description: "Revoke one of your own notes in this channel, by its number, when it is \
                    no longer true or no longer needed."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("id", ToolParameters::number("The number of your note"))],
                    ["id"],
                ),
                requires_approval: false,
            },
            Tool {
                name: READ_NOTES_TOOL_NAME.to_string(),
                description: "Read what the agents of another channel of this session noted. Without \
                    a channel, lists every channel you may read with its notes - also how to find out \
                    which channels there are. The notes of your own channel are already shown to you."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("channel", ToolParameters::string("The name of the channel; omit it for all channels"))],
                    Vec::<&str>::new(),
                ),
                requires_approval: false,
            },
        ]
    }

    async fn execute(&self, name: &str, params: &Value, context: &mut TurnContext) -> ToolResult<Value> {
        match name {
            NOTE_TOOL_NAME => self.execute_note(params, context),
            REVOKE_NOTE_TOOL_NAME => self.execute_revoke_note(params, context),
            READ_NOTES_TOOL_NAME => self.execute_read_notes(params, context).await,
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }
}

impl NotesComponent {
    /// `note(text)`: notes `text`, published when the turn completes.
    fn execute_note(&self, params: &Value, context: &mut TurnContext<'_>) -> ToolResult<Value> {
        let text = params.get("text").and_then(Value::as_str).map(str::trim).unwrap_or_default();
        if text.is_empty() {
            return Err(ToolError::InvalidParameter("'text' must not be empty".to_string()));
        }
        let id = context.note(text.to_string());
        Ok(serde_json::json!({ "status": "noted", "id": id.0 }))
    }

    /// `revoke_note(id)`: revokes one of the agent's own current notes.
    fn execute_revoke_note(&self, params: &Value, context: &mut TurnContext<'_>) -> ToolResult<Value> {
        let id = params.get("id").and_then(Value::as_u64)
            .and_then(|id| u32::try_from(id).ok())
            .ok_or_else(|| ToolError::InvalidParameter("'id' must be the number of one of your notes".to_string()))?;
        if context.revoke_note(NoteId(id)) {
            Ok(serde_json::json!({ "status": "revoked", "id": id }))
        } else {
            Err(ToolError::InvalidParameter(format!("you have no note #{id}; only your own current notes can be revoked")))
        }
    }

    /// `read_notes(channel?)`: the notes of the channel named, or of every
    /// channel that may be read, each read recorded as knowledge.
    async fn execute_read_notes(&self, params: &Value, context: &mut TurnContext<'_>) -> ToolResult<Value> {
        let reader = context.session_reader()
            .ok_or_else(|| ToolError::ExecutionFailed("no other channels can be read here".to_string()))?;
        let channels = match params.get("channel").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()) {
            Some(name) => vec![reader.notes_of(name).await.map_err(read_error)?],
            None => {
                let mut all = Vec::new();
                for channel in reader.readable_channels().await.map_err(read_error)? {
                    all.push(reader.notes_of(&channel.name).await.map_err(read_error)?);
                }
                all
            }
        };
        let mut listed = Vec::new();
        for (channel, published) in channels {
            let mut notes = Vec::new();
            for agent_notes in published {
                if let Some(reference) = notes_ref(channel.id, &agent_notes) {
                    context.record_knowledge(reference);
                }
                for note in agent_notes.notes {
                    notes.push(serde_json::json!({ "by": agent_notes.name, "id": note.id.0, "text": note.text }));
                }
            }
            listed.push(serde_json::json!({ "channel": channel.name, "notes": notes }));
        }
        Ok(serde_json::json!({ "channels": listed }))
    }
}

/// A failed read as the model is told it: which channel cannot be read, or
/// that reading failed.
fn read_error(error: anyhow::Error) -> ToolError {
    match error.downcast_ref::<NotReadable>() {
        Some(not_readable) => ToolError::InvalidParameter(not_readable.to_string()),
        None => ToolError::ExecutionFailed(format!("reading failed: {error}")),
    }
}

/// What reading `published` in `channel` records.
fn notes_ref(channel: ChannelId, published: &PublishedNotes) -> Option<KnowledgeRef> {
    let ActorId::Agent(agent) = published.agent else { return None };
    Some(KnowledgeRef { channel, source: KnowledgeSource::Notes { agent, digest: published.digest } })
}

#[async_trait]
impl PromptProvider for NotesComponent {
    fn position(&self) -> PromptPosition {
        PromptPosition::BeforeTurnInput
    }

    async fn get_prompt(&self, context: &TurnContext) -> Result<Prompt> {
        let own = ActorId::Agent(context.agent_id);
        let mut others = match context.session_reader() {
            Some(reader) => match reader.channel_notes().await {
                Ok(published) => published,
                Err(e) => {
                    // The agent can still answer without the others' notes.
                    warn!("Reading the notes of channel {} failed: {}", context.channel_id, e);
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        others.retain(|p| p.agent != own && !p.notes.is_empty());
        others.sort_by(|l, r| l.name.cmp(&r.name));
        let own_notes = context.notes();
        let text = render(&own_notes, others.iter().map(|p| (p.name.as_str(), p.notes.as_slice())));

        // What the part shows, as knowledge read (issue 17).
        let mut knowledge: Vec<KnowledgeRef> = others.iter().filter_map(|p| notes_ref(context.channel_id, p)).collect();
        if !own_notes.is_empty() {
            let own_digest = context.notes_digest();
            knowledge.push(KnowledgeRef { channel: context.channel_id, source: KnowledgeSource::Notes { agent: context.agent_id, digest: own_digest } });
        }
        Ok(Prompt { text, knowledge })
    }
}

/// The prompt part for `own` notes and those of `others` by name; empty if
/// there are none.
fn render<'a>(own: &[Note], others: impl Iterator<Item = (&'a str, &'a [Note])>) -> String {
    let mut text = String::new();
    let mut list = |heading: &str, notes: &[Note]| {
        let _ = writeln!(text, "{heading}");
        for note in notes {
            let _ = writeln!(text, "- #{}: {}", note.id, note.text);
        }
    };
    if !own.is_empty() {
        list("Your notes (change them with `note` and `revoke_note`):", own);
    }
    for (name, notes) in others {
        list(&format!("Notes of {name}:"), notes);
    }
    if text.is_empty() {
        return text;
    }
    format!("[Notes in this channel - what its participants noted to remember.]\n{text}")
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        NOTES_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(NotesComponent));
            facets.insert::<dyn PromptProvider>(Arc::new(NotesComponent));
            Ok(facets)
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
    use caelumora_model::message::MessageId;

    fn note(id: u32, text: &str) -> Note {
        Note { id: NoteId(id), text: text.to_string(), after: MessageId::new(), written_at: chrono::Utc::now() }
    }

    #[test]
    fn nothing_noted_gives_no_prompt_part() {
        assert_eq!(render(&[], std::iter::empty()), "");
    }

    #[test]
    fn the_prompt_part_lists_own_notes_first_then_the_others() {
        let others = [note(1, "The user is called Kaya.")];
        let text = render(&[note(2, "Answer in German.")], [("Sassy", &others[..])].into_iter());
        let own_at = text.find("#2: Answer in German.").unwrap();
        let others_at = text.find("Notes of Sassy:").unwrap();
        assert!(own_at < others_at, "{text}");
        assert!(text.contains("#1: The user is called Kaya."));
    }

    #[tokio::test]
    async fn notes_are_published_when_the_turn_completes_and_only_the_author_revokes() {
        let mut agent_state = AgentState::default();
        let mut state = AgentChannelState::default();
        let tools = NotesComponent;
        {
            let mut turn = TurnContext::new(AgentId::new(), "agent".to_string(), &mut agent_state, &mut state);
            let noted = tools.execute(NOTE_TOOL_NAME, &serde_json::json!({ "text": "tea" }), &mut turn).await.unwrap();
            assert_eq!(noted["id"], 1);
            assert!(turn.notes().is_empty(), "not before the turn completes");
            let unknown = tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 7 }), &mut turn).await;
            assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))), "not one of its notes");
            assert!(turn.record_turn(Vec::new()), "the turn changed notes");
            assert_eq!(turn.notes().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["tea"]);
        }
        {
            let mut turn = TurnContext::new(AgentId::new(), "agent".to_string(), &mut agent_state, &mut state);
            tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.unwrap();
            tools.execute(NOTE_TOOL_NAME, &serde_json::json!({ "text": "coffee" }), &mut turn).await.unwrap();
            turn.record_turn(Vec::new());
            let notes = turn.notes();
            assert_eq!(notes.iter().map(|n| (n.id.0, n.text.as_str())).collect::<Vec<_>>(), [(2, "coffee")], "a new number for the new version");
        }
        {
            // A turn that does not complete publishes nothing: the next one
            // starts without its changes.
            let mut turn = TurnContext::new(AgentId::new(), "agent".to_string(), &mut agent_state, &mut state);
            tools.execute(NOTE_TOOL_NAME, &serde_json::json!({ "text": "lost" }), &mut turn).await.unwrap();
            turn.next_turn(Default::default(), Default::default());
            assert!(!turn.record_turn(Vec::new()));
        }
    }
}
