//! The `private_notes` component (issue 51): an agent notes what only it
//! should remember, and recalls it in all its channels of the session where
//! that tells no one anything new.
//!
//! Its tools `write_private_note`, `update_private_note` and
//! `revoke_private_note` change the agent's
//! private notes in the current channel - taken over when the turn
//! completes, like public notes (`notes`). Its prompt part gives the model
//! its private notes from this channel and from every other channel of the
//! session it takes or took part in, as far as they may be copied into
//! this one (`SessionView::own_private_notes`,
//! `ConfidentialityQuestions::may_copy_information`, issues 133, 264): a note
//! carries the classification of the channel it was written in, checked
//! when it is read; whoever may read that channel now does not matter, the
//! notes are the agent's own. It is a
//! `BeforeTurnInput` part, and every chain it shows is recorded as a
//! `KnowledgeRef`.
//!
//! "Private" means private from other agents and other users (issue 54):
//! its tools are `ToolVisibility::HIDDEN`, so their calls never reach the
//! channel, but the agent's owner sees them, and the prompt part, in the
//! context panel - the audit view of the one responsible for the agent.

use crate::agent::notes::{AgentNotes, NoteKind};
use crate::agent::turn::Turn;
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::context::{render_sections, Prompt, PromptPosition, PromptProvider, Section};
use crate::agent::work::AgentWork;
use crate::view::PrivateNotesElsewhere;
use crate::tool::{classification_argument, id_argument, reclassify_parameters, text_argument, ToolError, ToolOutput, ToolProvider, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::question::Answers;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy};
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::note::{Note, NoteId};
use caelumora_model::security::SecurityManager;
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use serde_json::Value;
use std::sync::Arc;

pub const PRIVATE_NOTES_KIND: &str = "private_notes";

pub const WRITE_PRIVATE_NOTE_TOOL_NAME: &str = "write_private_note";
pub const UPDATE_PRIVATE_NOTE_TOOL_NAME: &str = "update_private_note";
pub const REVOKE_PRIVATE_NOTE_TOOL_NAME: &str = "revoke_private_note";
pub const RECLASSIFY_PRIVATE_NOTE_TOOL_NAME: &str = "reclassify_private_note";

#[derive(Debug)]
pub struct PrivateNotesComponent {
    /// The policy that decides whether a note may be reclassified, asked in
    /// the turn's context - the agent's, with its owner's rights (issues
    /// 131, 133, 255).
    security_manager: Arc<dyn SecurityManager>,
    /// What may flow where (issue 304): what it recalls, how it reclassifies.
    confidentiality: Arc<dyn ConfidentialityPolicy>,
}

#[async_trait]
impl ToolProvider for PrivateNotesComponent {
    fn description(&self) -> String {
        "Your private notes: what only you should remember across your channels of this session.".to_string()
    }

    fn tools(&self) -> Vec<Tool> {
        vec![
            Tool {
                name: WRITE_PRIVATE_NOTE_TOOL_NAME.to_string(),
                description: "Note something privately - no other participant sees it; only the person \
                    responsible for you can inspect it. Do not quote or reveal your private notes in \
                    your answers. You are shown your private notes from now on, in this channel and in your other channels of this \
                    session where that reveals nothing to someone who may not know it. Keep a note short \
                    and self-contained, and update one rather than writing a second on the same thing."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("text", ToolParameters::string("What to note"))],
                    ["text"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
                presentation: ToolPresentation::Note,
            },
            Tool {
                name: UPDATE_PRIVATE_NOTE_TOOL_NAME.to_string(),
                description: "Change what one of your private notes of this channel says, by its number. \
                    Give its whole new text; it keeps its number."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("id", ToolParameters::number("The number of your private note")), ("text", ToolParameters::string("Its new text"))],
                    ["id", "text"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
                presentation: ToolPresentation::Note,
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
                presentation: ToolPresentation::Note,
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
                presentation: ToolPresentation::Note,
            },
        ]
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        let value = match name {
            WRITE_PRIVATE_NOTE_TOOL_NAME => self.execute_write_private_note(params, turn),
            UPDATE_PRIVATE_NOTE_TOOL_NAME => self.execute_update_private_note(params, turn),
            REVOKE_PRIVATE_NOTE_TOOL_NAME => self.execute_revoke_private_note(params, turn),
            RECLASSIFY_PRIVATE_NOTE_TOOL_NAME => self.execute_reclassify_private_note(params, turn).await,
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }?;
        // What these answer tells only what the model asked for: classified
        // as the turn (issue 168).
        Ok(ToolOutput::classified(value, turn.label()))
    }
}

impl PrivateNotesComponent {
    /// `write_private_note(text)`: notes `text` privately, taken over when
    /// the turn completes.
    fn execute_write_private_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id = AgentNotes::note(turn, NoteKind::Private, text_argument(params)?);
        Ok(serde_json::json!({ "status": "noted privately", "id": id.0 }))
    }

    /// `update_private_note(id, text)`: changes the text of one of the
    /// agent's private notes of this channel.
    fn execute_update_private_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: NoteId = id_argument(params)?;
        if AgentNotes::update(turn, NoteKind::Private, id, text_argument(params)?) {
            Ok(serde_json::json!({ "status": "updated", "id": id.0 }))
        } else {
            Err(ToolError::InvalidParameter(format!("you have no private note #{id} in this channel")))
        }
    }

    /// `reclassify_private_note(id, level, compartments?)`: reclassifies one
    /// of the agent's private notes of this channel, as the policy allows.
    async fn execute_reclassify_private_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: NoteId = id_argument(params)?;
        let classification = classification_argument(params)?;
        AgentNotes::reclassify(turn, NoteKind::Private, id, classification.clone(), self.security_manager.as_ref(), self.confidentiality.as_ref()).await?;
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

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt> {
        let own = AgentNotes::current(work, NoteKind::Private);
        // This channel's come from the turn, with its own changes; the
        // others' through the agent itself.
        let elsewhere = match work.session().await {
            Some(session) => session.own_private_notes().await,
            None => Vec::new(),
        };

        let private = |channel, digest, notes: &[Note]| KnowledgeRef {
            channel,
            // Its own session's channels only.
            session: work.session_id(),
            source: KnowledgeSource::PrivateNotes { agent: work.agent_id(), digest },
            classification: Classification::join_all(notes.iter().map(|n| &n.classification)),
        };
        let mut knowledge: Vec<KnowledgeRef> = elsewhere.iter().map(|c| private(c.channel, c.digest, &c.notes)).collect();
        if !own.is_empty() {
            knowledge.push(private(work.channel_id(), AgentNotes::digest(work, NoteKind::Private), &own));
        }
        Ok(Prompt::of_knowledge(render(&own, &elsewhere), knowledge))
    }
}

/// The prompt part for the private notes of this channel (`own`) and of
/// others; empty if there are none.
fn render(own: &[Note], elsewhere: &[PrivateNotesElsewhere]) -> String {
    let mut sections = vec![Section::new(
        format!("In this channel (change them with `{WRITE_PRIVATE_NOTE_TOOL_NAME}`, `{UPDATE_PRIVATE_NOTE_TOOL_NAME}` and `{REVOKE_PRIVATE_NOTE_TOOL_NAME}`):"),
        own.iter().map(|n| format!("#{}: {}", n.id, n.text)).collect(),
    )];
    // Notes of other channels cannot be changed here, so they go unnumbered
    // (issue 60).
    sections.extend(elsewhere.iter().map(|c| Section::new(
        format!("From channel {} (change them only there):", c.name),
        c.notes.iter().map(|n| n.text.clone()).collect(),
    )));
    render_sections(
        "Your private notes",
        "No other participant sees them; only the person responsible for you can inspect them. Do not quote or reveal them in your answers.",
        &sections,
    )
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(PRIVATE_NOTES_KIND).provides::<dyn ToolProvider>().provides::<dyn PromptProvider>().uses::<dyn SecurityManager>().uses::<dyn ConfidentialityPolicy>(),
        |_descriptor, context| {
            let component = Arc::new(PrivateNotesComponent {
                security_manager: Arc::clone(context.service::<dyn SecurityManager>()?),
                confidentiality: Arc::clone(context.service::<dyn ConfidentialityPolicy>()?),
            });
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::<PrivateNotesComponent>::clone(&component));
            facets.insert::<dyn PromptProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Discard;
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
        let elsewhere = [PrivateNotesElsewhere { channel: caelumora_model::session::ChannelId::default(), name: "research".to_string(), notes: vec![note(1, "Kaya prefers short answers.")], digest: None }];
        let text = render(&[note(2, "Remind Kaya of the deadline.")], &elsewhere);
        assert!(text.find("#2: Remind Kaya").unwrap() < text.find("From channel research (change them only there):").unwrap(), "{text}");
        assert!(text.contains("- Kaya prefers short answers."), "unnumbered: it cannot be revoked here");
    }

    #[tokio::test]
    async fn private_notes_are_numbered_apart_from_public_ones_and_taken_over_with_the_turn() {
        let mut state = AgentChannelState::default();
        let tools = PrivateNotesComponent { security_manager: Arc::new(caelumora_model::security::DummySecurityManager::new()), confidentiality: Arc::new(caelumora_model::confidentiality::ConfidentialityRules::default()) };
        let mut turn = Turn::begun(AgentId::new(), "agent".to_string(), &mut state, crate::agent::state::AgentStateHandle::detached());
        AgentNotes::note(&mut turn, NoteKind::Public, "public".to_string());
        let noted = tools.execute(WRITE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "secret" }), None, &mut turn).await.unwrap();
        assert_eq!(noted.value["id"], 1, "its own numbering");
        assert!(AgentNotes::current(&turn.work(), NoteKind::Private).is_empty(), "not before the turn completes");
        assert!(turn.commit(false, &Discard).await.channel_state_changed, "the channel state changed");
        assert_eq!(state.private_notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["secret"]);
        assert_eq!(state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["public"], "the chains are apart");

        // The next turn.
        let mut turn = Turn::begun(AgentId::new(), "agent".to_string(), &mut state, crate::agent::state::AgentStateHandle::detached());
        let unknown = tools.execute(REVOKE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 9 }), None, &mut turn).await;
        assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))));
        let unknown = tools.execute(UPDATE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 9, "text": "x" }), None, &mut turn).await;
        assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))));
        tools.execute(UPDATE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1, "text": "a secret" }), None, &mut turn).await.unwrap();
        assert!(turn.commit(false, &Discard).await.channel_state_changed, "private notes are part of the channel state too");
        assert_eq!(state.private_notes.current().iter().map(|n| (n.id.0, n.text.as_str())).collect::<Vec<_>>(), [(1, "a secret")], "updated in its own chain");
        assert_eq!(state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["public"]);

        let mut turn = Turn::begun(AgentId::new(), "agent".to_string(), &mut state, crate::agent::state::AgentStateHandle::detached());
        tools.execute(REVOKE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn).await.unwrap();
        turn.commit(false, &Discard).await;
        assert!(state.private_notes.current().is_empty());
    }

    #[test]
    fn every_private_note_acted_on_is_shown_as_a_note_to_the_owner_alone() {
        let tools = PrivateNotesComponent { security_manager: Arc::new(caelumora_model::security::OwnershipSecurityManager::new()), confidentiality: Arc::new(caelumora_model::confidentiality::ConfidentialityRules::default()) }.tools();
        assert!(tools.iter().all(|t| (t.presentation, t.visibility) == (ToolPresentation::Note, ToolVisibility::HIDDEN)));
    }
}
