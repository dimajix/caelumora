//! The `notes` component (issue 17): an agent notes what it wants to
//! remember in a channel, and is given what every agent of the channel noted.
//!
//! Its tools `note` and `revoke_note` change the agent's own notes - taken
//! over when the turn completes, see `crate::agent_notes::AgentNotes`;
//! `read_notes` reads
//! the notes of another channel of the session, or of all it may read. Every
//! read is recorded as a `KnowledgeRef` with the turn's messages. Its prompt part
//! gives the model the notes of all agents of the channel before the turn's
//! input (`PromptPosition::BeforeTurnInput`), not in the system prompt, so
//! that changing notes do not invalidate the prompt cache (issue 47).

use crate::agent_notes::{AgentNotes, NoteKind};
use crate::agent_turn::Turn;
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::context::{render_sections, Prompt, PromptPosition, PromptProvider, Section};
use crate::work::AgentWork;
use caelumora_model::snapshot::PublishedNotes;
use crate::tool::{arguments, classification_argument, id_argument, reclassify_parameters, text_argument, ToolError, ToolOutput, ToolProvider, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::note::{Note, NoteId};
use caelumora_model::security::{SecurityContext, SecurityManager};
use caelumora_model::tool::{Tool, ToolVisibility, ToolParameters};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tracing::warn;

pub const NOTES_KIND: &str = "notes";

pub const NOTE_TOOL_NAME: &str = "note";
pub const REVOKE_NOTE_TOOL_NAME: &str = "revoke_note";
pub const READ_NOTES_TOOL_NAME: &str = "read_notes";
pub const RECLASSIFY_NOTE_TOOL_NAME: &str = "reclassify_note";

#[derive(Debug)]
pub struct NotesComponent {
    /// The policy that decides whether a note may be reclassified, asked in
    /// `security_context` - the agent's (issues 131, 133).
    security_manager: Arc<dyn SecurityManager>,
    security_context: SecurityContext,
}

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
                visibility: ToolVisibility::SHOWN,
                classification: None,
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
                visibility: ToolVisibility::SHOWN,
                classification: None,
            },
            Tool {
                name: RECLASSIFY_NOTE_TOOL_NAME.to_string(),
                description: "Change how one of your notes in this channel is classified, by its number: \
                    its level and the compartments it is meant for, never above this channel. Releasing it - a lower level, or further compartments - makes it readable by a wider circle; a user has to approve it."
                    .to_string(),
                parameters: reclassify_parameters("your note"),
                requires_approval: true,
                visibility: ToolVisibility::SHOWN,
                classification: None,
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
                visibility: ToolVisibility::SHOWN,
                classification: None,
            },
        ]
    }

    async fn execute(&self, name: &str, params: &Value, turn: &mut Turn) -> ToolResult<ToolOutput> {
        let value = match name {
            NOTE_TOOL_NAME => self.execute_note(params, turn),
            REVOKE_NOTE_TOOL_NAME => self.execute_revoke_note(params, turn),
            READ_NOTES_TOOL_NAME => return self.execute_read_notes(params, turn).await,
            RECLASSIFY_NOTE_TOOL_NAME => self.execute_reclassify_note(params, turn).await,
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }?;
        // What these answer tells only what the model asked for: classified
        // as the turn (issue 168).
        Ok(ToolOutput::classified(value, turn.label()))
    }
}

impl NotesComponent {
    /// `note(text)`: notes `text`, published when the turn completes.
    fn execute_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id = AgentNotes::note(turn, NoteKind::Public, text_argument(params)?);
        Ok(serde_json::json!({ "status": "noted", "id": id.0 }))
    }

    /// `revoke_note(id)`: revokes one of the agent's own current notes.
    fn execute_revoke_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: NoteId = id_argument(params)?;
        if AgentNotes::revoke(turn, NoteKind::Public, id) {
            Ok(serde_json::json!({ "status": "revoked", "id": id.0 }))
        } else {
            Err(ToolError::InvalidParameter(format!("you have no note #{id}; only your own current notes can be revoked")))
        }
    }

    /// `reclassify_note(id, level, compartments?)`: reclassifies one of the
    /// agent's own current notes, as the policy allows.
    async fn execute_reclassify_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: NoteId = id_argument(params)?;
        let classification = classification_argument(params)?;
        AgentNotes::reclassify(turn, NoteKind::Public, id, classification.clone(), self.security_manager.as_ref(), &self.security_context).await?;
        Ok(serde_json::json!({ "status": "reclassified", "id": id.0, "classification": classification.to_string() }))
    }

    /// `read_notes(channel?)`: the notes of the channel named, or of every
    /// channel that may be read, each read recorded as knowledge - classified
    /// as the notes read (issue 168).
    async fn execute_read_notes(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<ToolOutput> {
        #[derive(Deserialize)]
        struct ReadNotes { #[serde(default)] channel: Option<String> }
        let ReadNotes { channel } = arguments(params)?;
        let Some(session) = turn.session() else {
            return Err(ToolError::ExecutionFailed("the turn is in no session to read from".to_string()));
        };
        let channels = match channel.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(name) => vec![session.channel_named(name).await?],
            None => session.channels().await,
        };
        let mut listed = Vec::new();
        let mut read = Vec::new();
        for channel in channels {
            let mut notes = Vec::new();
            for agent_notes in channel.notes().await? {
                if let Some(reference) = notes_ref(channel.id(), channel.session().id(), &agent_notes) {
                    turn.record_knowledge(reference);
                }
                for note in agent_notes.notes {
                    notes.push(serde_json::json!({ "by": agent_notes.name, "id": note.id.0, "text": note.text }));
                    read.push(note.classification);
                }
            }
            listed.push(serde_json::json!({ "channel": channel.name(), "notes": notes }));
        }
        let value = serde_json::json!({ "channels": listed });
        Ok(ToolOutput { value, classification: Classification::join_all(&read) })
    }
}

/// What reading `published` in `channel` of `session` records.
fn notes_ref(channel: ChannelId, session: SessionId, published: &PublishedNotes) -> Option<KnowledgeRef> {
    let ActorId::Agent(agent) = published.agent else { return None };
    let classification = Classification::join_all(published.notes.iter().map(|n| &n.classification));
    Some(KnowledgeRef { channel, session, source: KnowledgeSource::Notes { agent, digest: published.digest }, classification })
}

#[async_trait]
impl PromptProvider for NotesComponent {
    fn position(&self) -> PromptPosition {
        PromptPosition::BeforeTurnInput
    }

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt> {
        let own = ActorId::Agent(work.agent_id());
        let mut others = Vec::new();
        if let Some(session) = work.session() {
            let published = match session.channel(work.channel_id()).await {
                Ok(channel) => channel.notes().await,
                Err(e) => Err(e),
            };
            match published {
                Ok(published) => others = published,
                // The agent can still answer without the others' notes.
                Err(e) => warn!("Reading the notes of channel {} failed: {}", work.channel_id(), e),
            }
        }
        others.retain(|p| p.agent != own && !p.notes.is_empty());
        others.sort_by(|l, r| l.name.cmp(&r.name));
        let own_notes = AgentNotes::current(work, NoteKind::Public);
        let text = render(&own_notes, others.iter().map(|p| (p.name.as_str(), p.notes.as_slice())));

        // What the part shows, as knowledge read (issue 17).
        let mut knowledge: Vec<KnowledgeRef> = others.iter().filter_map(|p| notes_ref(work.channel_id(), work.session_id(), p)).collect();
        if !own_notes.is_empty() {
            let own_digest = AgentNotes::digest(work, NoteKind::Public);
            let classification = Classification::join_all(own_notes.iter().map(|n| &n.classification));
            knowledge.push(KnowledgeRef { channel: work.channel_id(), session: work.session_id(), source: KnowledgeSource::Notes { agent: work.agent_id(), digest: own_digest }, classification });
        }
        Ok(Prompt::of_knowledge(text, knowledge))
    }
}

/// The prompt part for `own` notes and those of `others` by name; empty if
/// there are none.
fn render<'a>(own: &[Note], others: impl Iterator<Item = (&'a str, &'a [Note])>) -> String {
    let numbered = |notes: &[Note]| notes.iter().map(|n| format!("#{}: {}", n.id, n.text)).collect();
    let mut sections = vec![Section::new(format!("Your notes (change them with `{NOTE_TOOL_NAME}` and `{REVOKE_NOTE_TOOL_NAME}`):"), numbered(own))];
    sections.extend(others.map(|(name, notes)| Section::new(format!("Notes of {name}:"), numbered(notes))));
    render_sections("[Notes in this channel - what its participants noted to remember.]", &sections)
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(NOTES_KIND).provides::<dyn ToolProvider>().provides::<dyn PromptProvider>(),
        |_descriptor, context| {
            let component = Arc::new(NotesComponent {
                security_manager: context.security_manager.clone(),
                security_context: context.security_context.clone(),
            });
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(component.clone());
            facets.insert::<dyn PromptProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::message::MessageId;

    fn note(id: u32, text: &str) -> Note {
        Note { id: NoteId(id), text: text.to_string(), classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT, after: MessageId::new(), origin: Default::default(), written_at: chrono::Utc::now() }
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
    async fn a_note_is_reclassified_as_the_policy_allows_also_in_the_turn_it_was_noted() {
        use caelumora_model::confidentiality::{Classification, Confidentiality};
        let agent = AgentId::new();
        let mut state = AgentChannelState::default();
        let tools = NotesComponent {
            security_manager: Arc::new(caelumora_model::security::OwnershipSecurityManager::new()),
            security_context: SecurityContext::default().next(ActorId::Agent(agent)),
        };
        let mut turn = Turn::begun(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
        tools.execute(NOTE_TOOL_NAME, &serde_json::json!({ "text": "tea" }), &mut turn).await.unwrap();
        let above = serde_json::json!({ "id": 1, "level": "strictly_confidential" });
        assert!(matches!(tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &above, &mut turn).await, Err(ToolError::PermissionDenied(_))), "never above the channel");
        let unknown = serde_json::json!({ "id": 2, "level": "public" });
        assert!(matches!(tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &unknown, &mut turn).await, Err(ToolError::InvalidParameter(_))));
        let public_with_compartments = serde_json::json!({ "id": 1, "level": "public", "compartments": ["hr"] });
        assert!(matches!(tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &public_with_compartments, &mut turn).await, Err(ToolError::InvalidParameter(_))));
        tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1, "level": "public" }), &mut turn).await.expect("released");
        turn.commit(false).await;
        assert_eq!(state.notes.current()[0].classification, Classification::general(Confidentiality::Public));
    }

    #[tokio::test]
    async fn notes_are_published_when_the_turn_completes_and_only_the_author_revokes() {
        let mut state = AgentChannelState::default();
        let tools = NotesComponent {
            security_manager: Arc::new(caelumora_model::security::DummySecurityManager::new()),
            security_context: Default::default(),
        };
        {
            let mut turn = Turn::begun(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            let noted = tools.execute(NOTE_TOOL_NAME, &serde_json::json!({ "text": "tea" }), &mut turn).await.unwrap();
            assert_eq!(noted.value["id"], 1);
            assert!(AgentNotes::current(&turn.work(), NoteKind::Public).is_empty(), "not before the turn completes");
            let unknown = tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 7 }), &mut turn).await;
            assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))), "not one of its notes");
            assert!(turn.commit(false).await.channel_state_changed, "the turn changed notes");
            assert_eq!(state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["tea"]);
        }
        {
            let mut turn = Turn::begun(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.unwrap();
            tools.execute(NOTE_TOOL_NAME, &serde_json::json!({ "text": "coffee" }), &mut turn).await.unwrap();
            turn.commit(false).await;
            let notes = state.notes.current();
            assert_eq!(notes.iter().map(|n| (n.id.0, n.text.as_str())).collect::<Vec<_>>(), [(2, "coffee")], "a new number for the new version");
        }
        {
            // A turn that does not complete publishes nothing: the next one
            // starts without its changes.
            let mut turn = Turn::begun(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            tools.execute(NOTE_TOOL_NAME, &serde_json::json!({ "text": "lost" }), &mut turn).await.unwrap();
            let turn = Turn::begun(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            assert!(!turn.commit(false).await.channel_state_changed);
        }
    }

    #[tokio::test]
    async fn a_note_revoked_in_the_turn_that_noted_it_is_gone_and_its_number_used_up() {
        let mut state = AgentChannelState::default();
        let tools = NotesComponent {
            security_manager: Arc::new(caelumora_model::security::DummySecurityManager::new()),
            security_context: Default::default(),
        };
        {
            let mut turn = Turn::begun(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            tools.execute(NOTE_TOOL_NAME, &serde_json::json!({ "text": "draft" }), &mut turn).await.unwrap();
            tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.expect("noted in this turn");
            let again = tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await;
            assert!(matches!(again, Err(ToolError::InvalidParameter(_))), "revoked already");
            let revoked = tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1, "level": "public" }), &mut turn).await;
            assert!(matches!(revoked, Err(ToolError::InvalidParameter(_))), "a revoked note is not reclassified");
            turn.commit(false).await;
            assert!(state.notes.current().is_empty());
        }
        let mut turn = Turn::begun(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
        let noted = tools.execute(NOTE_TOOL_NAME, &serde_json::json!({ "text": "final" }), &mut turn).await.unwrap();
        assert_eq!(noted.value["id"], 2);
    }
}
