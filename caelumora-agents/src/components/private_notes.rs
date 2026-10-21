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

use crate::prompt::NAMESPACE;
use crate::components::notes::NOTE_PRESENTATION;
use crate::notes::{AgentNotes, NoteKind};
use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::prompt::{ContextError, Prompt, PromptPosition, PromptProvider, texts_of, writing_for_the_model};
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use crate::situation::Situation;
use caelumora_runtime::view::PrivateNotesElsewhere;
use crate::tool::{described, classification_argument, id_argument, reclassify_parameters, tool_text, text_argument, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::question::Answers;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy};
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::note::{Note, NoteId};
use caelumora_model::security::AccessPolicy;
use caelumora_model::tool::{Tool, ToolParameters, ToolVisibility};
use serde_json::Value;
use std::sync::Arc;

pub const PRIVATE_NOTES_KIND: &str = "private_notes";

pub const WRITE_PRIVATE_NOTE_TOOL_NAME: &str = "write_private_note";
pub const UPDATE_PRIVATE_NOTE_TOOL_NAME: &str = "update_private_note";
pub const REVOKE_PRIVATE_NOTE_TOOL_NAME: &str = "revoke_private_note";
pub const RECLASSIFY_PRIVATE_NOTE_TOOL_NAME: &str = "reclassify_private_note";

/// The text of its prompt part in the prompt catalogue.
const TEXT: &str = "agents.private_notes";

#[derive(Debug)]
pub struct PrivateNotesComponent {
    /// The policy that decides whether a note may be reclassified, asked in
    /// the turn's context - the agent's, with its owner's rights (issues
    /// 131, 133, 255).
    access_policy: Arc<dyn AccessPolicy>,
    /// What may flow where (issue 304): what it recalls, how it reclassifies.
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    /// The texts of its prompt part (issue 57).
    prompts: PromptTexts,
    /// What its tool set is for and its tools, as the prompt catalogue
    /// words them.
    description: String,
    tools: Vec<Tool>,
}

#[async_trait]
impl ToolProvider for PrivateNotesComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
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
        Ok(ToolOutput::classified(value, turn.situation().label()))
    }
}

impl PrivateNotesComponent {
    /// The component, deciding by `access_policy` and `confidentiality`;
    /// its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub fn new(access_policy: Arc<dyn AccessPolicy>, confidentiality: Arc<dyn ConfidentialityPolicy>, prompts: PromptTexts) -> Result<Self, PromptError> {
        let tools = vec![
            described(&prompts, NAMESPACE, PRIVATE_NOTES_KIND, Tool {
                name: WRITE_PRIVATE_NOTE_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    [("text", ToolParameters::string())],
                    ["text"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
                presentation: NOTE_PRESENTATION,
            }, ())?,
            described(&prompts, NAMESPACE, PRIVATE_NOTES_KIND, Tool {
                name: UPDATE_PRIVATE_NOTE_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    [("id", ToolParameters::number()), ("text", ToolParameters::string())],
                    ["id", "text"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
                presentation: NOTE_PRESENTATION,
            }, ())?,
            described(&prompts, NAMESPACE, PRIVATE_NOTES_KIND, Tool {
                name: REVOKE_PRIVATE_NOTE_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    [("id", ToolParameters::number())],
                    ["id"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
                presentation: NOTE_PRESENTATION,
            }, ())?,
            described(&prompts, NAMESPACE, PRIVATE_NOTES_KIND, Tool {
                name: RECLASSIFY_PRIVATE_NOTE_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: reclassify_parameters(&prompts)?,
                requires_approval: true,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
                presentation: NOTE_PRESENTATION,
            }, ())?,
        ];
        let description = tool_text(&prompts, NAMESPACE, PRIVATE_NOTES_KIND, "description", ())?;
        Ok(Self { access_policy, confidentiality, prompts, description, tools })
    }

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
        AgentNotes::reclassify(turn, NoteKind::Private, id, classification.clone(), self.access_policy.as_ref(), self.confidentiality.as_ref()).await?;
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

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let own = AgentNotes::current(situation, NoteKind::Private);
        // This channel's come from the turn, with its own changes; the
        // others' through the agent itself.
        let elsewhere = match situation.session().await? {
            Some(session) => session.own_private_notes().await,
            None => Vec::new(),
        };

        let private = |channel, digest, notes: &[Note]| KnowledgeRef {
            channel,
            // Its own session's channels only.
            session: situation.session_id(),
            source: KnowledgeSource::PrivateNotes { agent: situation.agent_id(), digest },
            classification: Classification::join_all(notes.iter().map(|n| &n.classification)),
        };
        let mut knowledge: Vec<KnowledgeRef> = elsewhere.iter().map(|c| private(c.channel, c.digest, &c.notes)).collect();
        if !own.is_empty() {
            knowledge.push(private(situation.channel_id(), AgentNotes::digest(situation, NoteKind::Private), &own));
        }
        Ok(Prompt::of_knowledge(render(&self.prompts, &own, &elsewhere)?, knowledge))
    }
}

/// The prompt part of `prompts` for the private notes of this channel
/// (`own`) and of others, channels without notes left out; empty if there
/// are none. Notes of other channels cannot be changed here, so they go
/// unnumbered (issue 60).
fn render(prompts: &PromptTexts, own: &[Note], elsewhere: &[PrivateNotesElsewhere]) -> Result<String, PromptError> {
    let own: Vec<_> = own.iter().map(|n| context! { id => n.id.to_string(), text => n.text }).collect();
    let elsewhere: Vec<_> = elsewhere.iter()
        .filter(|c| !c.notes.is_empty())
        .map(|c| context! { name => c.name, notes => c.notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>() })
        .collect();
    if own.is_empty() && elsewhere.is_empty() {
        return Ok(String::new());
    }
    prompts.render(TEXT, context! { own, elsewhere, write => WRITE_PRIVATE_NOTE_TOOL_NAME, update => UPDATE_PRIVATE_NOTE_TOOL_NAME, revoke => REVOKE_PRIVATE_NOTE_TOOL_NAME })
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(PRIVATE_NOTES_KIND).provides::<dyn ToolProvider>().provides::<dyn PromptProvider>().uses::<dyn AccessPolicy>().uses::<dyn ConfidentialityPolicy>()),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let component = Arc::new(PrivateNotesComponent::new(
                Arc::clone(context.service::<dyn AccessPolicy>()?),
                Arc::clone(context.service::<dyn ConfidentialityPolicy>()?),
                texts,
            )?);
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
    use crate::testing;
    use caelumora_runtime::transport::Discard;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::message::MessageId;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::security::{AllowAllPolicy, OwnershipPolicy};
    use caelumora_model::session::ChannelId;
    use chrono::Utc;

    fn note(id: u32, text: &str) -> Note {
        Note { id: NoteId(id), text: text.to_string(), classification: Classification::CHANNEL_DEFAULT, after: MessageId::new(), origin: Default::default(), written_at: Utc::now() }
    }

    #[test]
    fn nothing_noted_gives_no_prompt_part() {
        assert_eq!(render(&testing::prompts(), &[], &[]).unwrap(), "");
    }

    #[test]
    fn the_prompt_part_shows_this_channel_first_then_the_others_by_name() {
        let elsewhere = [PrivateNotesElsewhere { channel: ChannelId::default(), name: "research".to_string(), notes: vec![note(1, "Kaya prefers short answers.")], digest: None }];
        let text = render(&testing::prompts(), &[note(2, "Remind Kaya of the deadline.")], &elsewhere).unwrap();
        assert!(text.find("#2: Remind Kaya").unwrap() < text.find("From channel research (change them only there):").unwrap(), "{text}");
        assert!(text.contains("- Kaya prefers short answers."), "unnumbered: it cannot be revoked here");
    }

    #[tokio::test]
    async fn private_notes_are_numbered_apart_from_public_ones_and_taken_over_with_the_turn() {
        let mut state = AgentChannelState::default();
        let tools = PrivateNotesComponent::new(Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()), testing::prompts()).unwrap();
        let mut turn = Turn::begun(AgentId::new(), &mut state);
        AgentNotes::note(&mut turn, NoteKind::Public, "public".to_string());
        let noted = tools.execute(WRITE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "secret" }), None, &mut turn).await.unwrap();
        assert_eq!(noted.value["id"], 1, "its own numbering");
        assert!(AgentNotes::current(&turn.situation(), NoteKind::Private).is_empty(), "not before the turn completes");
        assert!(turn.commit(false, &Discard).await.channel_state_changed, "the channel state changed");
        assert_eq!(state.private_notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["secret"]);
        assert_eq!(state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["public"], "the chains are apart");

        // The next turn.
        let mut turn = Turn::begun(AgentId::new(), &mut state);
        let unknown = tools.execute(REVOKE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 9 }), None, &mut turn).await;
        assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))));
        let unknown = tools.execute(UPDATE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 9, "text": "x" }), None, &mut turn).await;
        assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))));
        tools.execute(UPDATE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1, "text": "a secret" }), None, &mut turn).await.unwrap();
        assert!(turn.commit(false, &Discard).await.channel_state_changed, "private notes are part of the channel state too");
        assert_eq!(state.private_notes.current().iter().map(|n| (n.id.0, n.text.as_str())).collect::<Vec<_>>(), [(1, "a secret")], "updated in its own chain");
        assert_eq!(state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["public"]);

        let mut turn = Turn::begun(AgentId::new(), &mut state);
        tools.execute(REVOKE_PRIVATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn).await.unwrap();
        turn.commit(false, &Discard).await;
        assert!(state.private_notes.current().is_empty());
    }

    #[test]
    fn every_private_note_acted_on_is_shown_as_a_note_to_the_owner_alone() {
        let tools = PrivateNotesComponent::new(Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()), testing::prompts()).unwrap().tools();
        assert!(tools.iter().all(|t| (t.presentation, t.visibility) == (NOTE_PRESENTATION, ToolVisibility::HIDDEN)));
    }
}
