//! The `private_notes` component (issue 51): an agent notes what only it
//! should remember, and recalls it in all its channels of the session where
//! that tells no one anything new.
//!
//! Its tools `note_private` and `revoke_private_note` change the agent's
//! private notes in the current channel - taken over when the turn
//! completes, like public notes (`notes`). Its prompt part gives the model
//! its private notes from this channel and from every other channel of the
//! session that this channel may read by their classifications
//! (`AccessQuestions::may_channel_read`, issue 133): a note carries the
//! classification of the channel it was written in, checked when it is
//! read. It is a
//! `BeforeTurnInput` part, and every chain it shows is recorded as a
//! `KnowledgeRef`.
//!
//! "Private" means private from other agents and other users (issue 54):
//! both tools are `ToolConfidentiality::HIDDEN`, so their calls never reach the
//! channel, but the agent's owner sees them, and the prompt part, in the
//! context panel - the audit view of the one responsible for the agent.

use crate::agent_notes::{AgentNotes, NoteKind};
use crate::agent_turn::Turn;
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::context::{render_sections, Prompt, PromptPosition, PromptProvider, Section};
use crate::session_reader::{ChannelNotes, SessionReader};
use crate::tool::{classification_argument, id_argument, reclassify_parameters, text_argument, ToolError, ToolOutput, ToolProvider, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::confidentiality::ChannelRef;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::note::{Note, NoteId};
use caelumora_model::security::{SecurityContext, SecurityManager};
use caelumora_model::tool::{Tool, ToolVisibility, ToolParameters};
use serde_json::Value;
use std::sync::Arc;
use tracing::warn;

pub const PRIVATE_NOTES_KIND: &str = "private_notes";

pub const NOTE_PRIVATE_TOOL_NAME: &str = "note_private";
pub const REVOKE_PRIVATE_NOTE_TOOL_NAME: &str = "revoke_private_note";
pub const RECLASSIFY_PRIVATE_NOTE_TOOL_NAME: &str = "reclassify_private_note";

#[derive(Debug)]
pub struct PrivateNotesComponent {
    /// The agent's private notes in the session's other channels.
    reader: Arc<dyn SessionReader>,
    /// The policy that decides whether a note may be reclassified, asked in
    /// `security_context` - the agent's (issues 131, 133).
    security_manager: Arc<dyn SecurityManager>,
    security_context: SecurityContext,
}

#[async_trait]
impl ToolProvider for PrivateNotesComponent {
    fn tools(&self) -> Vec<Tool> {
        vec![
            Tool {
                name: NOTE_PRIVATE_TOOL_NAME.to_string(),
                description: "Note something privately - no other participant sees it; only the person \
                    responsible for you can inspect it. Do not quote or reveal your private notes in \
                    your answers. You are shown your private notes from now on, in this channel and in your other channels of this \
                    session where that reveals nothing to someone who may not know it. Keep a note short \
                    and self-contained; to change one, revoke it and note the new version."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("text", ToolParameters::string("What to note"))],
                    ["text"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
            },
            Tool {
                name: REVOKE_PRIVATE_NOTE_TOOL_NAME.to_string(),
                description: "Revoke one of your private notes of this channel, by its number."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("id", ToolParameters::number("The number of your private note"))],
                    ["id"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
            },
            Tool {
                name: RECLASSIFY_PRIVATE_NOTE_TOOL_NAME.to_string(),
                description: "Change how one of your private notes of this channel is classified, by its \
                    number: its level and the compartments it is meant for, never above this channel. It \
                    decides in which of your other channels you are shown it. Releasing it - a lower level, or further compartments - makes it readable by a wider circle; a user has to approve it."
                    .to_string(),
                parameters: reclassify_parameters("your private note"),
                requires_approval: true,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
            },
        ]
    }

    async fn execute(&self, name: &str, params: &Value, turn: &mut Turn) -> ToolResult<ToolOutput> {
        let value = match name {
            NOTE_PRIVATE_TOOL_NAME => self.execute_note_private(params, turn),
            REVOKE_PRIVATE_NOTE_TOOL_NAME => self.execute_revoke_private_note(params, turn),
            RECLASSIFY_PRIVATE_NOTE_TOOL_NAME => self.execute_reclassify_private_note(params, turn).await,
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }?;
        Ok(value.into())
    }
}

impl PrivateNotesComponent {
    /// `note_private(text)`: notes `text` privately, taken over when the
    /// turn completes.
    fn execute_note_private(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id = AgentNotes::note(turn, NoteKind::Private, text_argument(params)?);
        Ok(serde_json::json!({ "status": "noted privately", "id": id.0 }))
    }

    /// `reclassify_private_note(id, level, compartments?)`: reclassifies one
    /// of the agent's private notes of this channel, as the policy allows.
    async fn execute_reclassify_private_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: NoteId = id_argument(params)?;
        let classification = classification_argument(params)?;
        AgentNotes::reclassify(turn, NoteKind::Private, id, classification.clone(), self.security_manager.as_ref(), &self.security_context).await?;
        Ok(serde_json::json!({ "status": "reclassified", "id": id.0, "classification": classification.to_string() }))
    }

    /// `revoke_private_note(id)`: revokes one of the agent's private notes
    /// of this channel.
    fn execute_revoke_private_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: NoteId = id_argument(params)?;
        if AgentNotes::revoke(turn, NoteKind::Private, id) {
            Ok(serde_json::json!({ "status": "revoked", "id": id.0 }))
        } else {
            Err(ToolError::InvalidParameter(format!("you have no private note #{id} in this channel")))
        }
    }
}

#[async_trait]
impl PromptProvider for PrivateNotesComponent {
    fn position(&self) -> PromptPosition {
        PromptPosition::BeforeTurnInput
    }

    async fn get_prompt(&self, turn: &Turn) -> Result<Prompt> {
        let own = AgentNotes::current(turn, NoteKind::Private);
        let mut elsewhere = self.reader.private_notes(turn.agent_id()).await.unwrap_or_else(|e| {
            // The agent can still answer without them.
            warn!("Reading the private notes of agent {} failed: {}", turn.agent_id(), e);
            Vec::new()
        });
        elsewhere.retain(|c| !c.notes.is_empty());
        elsewhere.sort_by(|l, r| l.channel.name.cmp(&r.channel.name));

        let private = |channel, digest| KnowledgeRef { channel, source: KnowledgeSource::PrivateNotes { agent: turn.agent_id(), digest } };
        let mut knowledge: Vec<KnowledgeRef> = elsewhere.iter().map(|c| private(ChannelRef { session_id: turn.channel().session_id, channel_id: c.channel.id }, c.digest)).collect();
        if !own.is_empty() {
            knowledge.push(private(turn.channel(), AgentNotes::digest(turn, NoteKind::Private)));
        }
        Ok(Prompt { text: render(&own, &elsewhere), knowledge })
    }
}

/// The prompt part for the private notes of this channel (`own`) and of
/// others; empty if there are none.
fn render(own: &[Note], elsewhere: &[ChannelNotes]) -> String {
    let mut sections = vec![Section::new(
        format!("In this channel (change them with `{NOTE_PRIVATE_TOOL_NAME}` and `{REVOKE_PRIVATE_NOTE_TOOL_NAME}`):"),
        own.iter().map(|n| format!("#{}: {}", n.id, n.text)).collect(),
    )];
    // Notes of other channels cannot be revoked here, so they go unnumbered.
    sections.extend(elsewhere.iter().map(|c| Section::new(
        format!("From channel {}:", c.channel.name),
        c.notes.iter().map(|n| n.text.clone()).collect(),
    )));
    render_sections(
        "[Your private notes - no other participant sees them; only the person responsible for you can inspect them. Do not quote or reveal them in your answers.]",
        &sections,
    )
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(PRIVATE_NOTES_KIND).provides::<dyn ToolProvider>().provides::<dyn PromptProvider>(),
        |_descriptor, context| {
            let component = Arc::new(PrivateNotesComponent {
                reader: context.session_reader.clone(),
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
    use crate::session_reader::ChannelInfo;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::message::MessageId;

    fn note(id: u32, text: &str) -> Note {
        Note { id: NoteId(id), text: text.to_string(), classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT, after: MessageId::new(), origin: Default::default(), written_at: chrono::Utc::now() }
    }

    #[test]
    fn nothing_noted_gives_no_prompt_part() {
        assert_eq!(render(&[], &[]), "");
    }

    #[test]
    fn the_prompt_part_shows_this_channel_first_then_the_others_by_name() {
        let elsewhere = [ChannelNotes { channel: ChannelInfo { id: Default::default(), name: "research".to_string() }, notes: vec![note(1, "Kaya prefers short answers.")], digest: None }];
        let text = render(&[note(2, "Remind Kaya of the deadline.")], &elsewhere);
        assert!(text.find("#2: Remind Kaya").unwrap() < text.find("From channel research:").unwrap(), "{text}");
        assert!(text.contains("- Kaya prefers short answers."));
    }

    #[tokio::test]
    async fn private_notes_are_numbered_apart_from_public_ones_and_taken_over_with_the_turn() {
        let mut state = AgentChannelState::default();
        let tools = PrivateNotesComponent {
            reader: crate::session_reader::detached(),
            security_manager: Arc::new(caelumora_model::security::DummySecurityManager::new()),
            security_context: Default::default(),
        };
        let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        turn.begin();
        AgentNotes::note(&mut turn, NoteKind::Public, "public".to_string());
        let noted = tools.execute(NOTE_PRIVATE_TOOL_NAME, &serde_json::json!({ "text": "secret" }), &mut turn).await.unwrap();
        assert_eq!(noted.value["id"], 1, "its own numbering");
        assert!(AgentNotes::current(&turn, NoteKind::Private).is_empty(), "not before the turn completes");
        assert!(turn.commit_turn().await.channel_state_changed, "the channel state changed");
        assert_eq!(AgentNotes::current(&turn, NoteKind::Private).iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["secret"]);
        assert_eq!(AgentNotes::current(&turn, NoteKind::Public).iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["public"], "the chains are apart");

        // The next turn.
        turn.begin();
        let unknown = tools.execute(REVOKE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 9 }), &mut turn).await;
        assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))));
        tools.execute(REVOKE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.unwrap();
        assert!(turn.commit_turn().await.channel_state_changed, "private notes are part of the channel state too");
        assert!(AgentNotes::current(&turn, NoteKind::Private).is_empty());
    }
}
