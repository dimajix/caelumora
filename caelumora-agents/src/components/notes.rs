//! The `notes` component (issue 17): an agent notes what it wants to
//! remember in a channel, and is given what every agent of the channel noted.
//!
//! Its tools `write_note`, `update_note` and `revoke_note` change the
//! agent's own notes - taken over when the turn completes, see
//! `crate::notes::AgentNotes`; `read_notes` reads
//! the notes of another channel of the session, or of all it may read. Every
//! read is recorded as a `KnowledgeRef` with the turn's messages. Its prompt part
//! gives the model the notes of all agents of the channel before the turn's
//! input (`PromptPosition::BeforeTurnInput`), not in the system prompt, so
//! that changing notes do not invalidate the prompt cache (issue 47).

use crate::notes::{AgentNotes, NoteKind};
use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::prompt::{ContextError, render_sections, Prompt, PromptPosition, PromptProvider, Section};
use crate::work::AgentWork;
use caelumora_model::question::Answers;
use caelumora_model::snapshot::PublishedNotes;
use crate::tool::{arguments, classification_argument, id_argument, reclassify_parameters, text_argument, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::note::{Note, NoteId};
use caelumora_model::security::AccessPolicy;
use caelumora_model::tool::{Tool, ToolParameters, ToolPresentation, ToolVisibility};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tracing::warn;

/// One of an agent's notes, private notes or memory entries acted on
/// (issue 303): its `id` and what was done to it (`status`), its new
/// `text` where the arguments hold one - written or updated -, and for a
/// reclassification its new `classification`. What was revoked,
/// forgotten or reclassified is named by its number alone.
pub const NOTE_PRESENTATION: ToolPresentation = ToolPresentation::new("note");

/// The notes of channels read (issue 303): the `channels` of its result,
/// each with its `channel` name and its `notes` - each `by` whom, its
/// `text` and, in the reader's own channel, its `id`.
pub const NOTE_LIST_PRESENTATION: ToolPresentation = ToolPresentation::new("note_list");

pub const NOTES_KIND: &str = "notes";

pub const WRITE_NOTE_TOOL_NAME: &str = "write_note";
pub const UPDATE_NOTE_TOOL_NAME: &str = "update_note";
pub const REVOKE_NOTE_TOOL_NAME: &str = "revoke_note";
pub const READ_NOTES_TOOL_NAME: &str = "read_notes";
pub const RECLASSIFY_NOTE_TOOL_NAME: &str = "reclassify_note";

#[derive(Debug)]
pub struct NotesComponent {
    /// The policy that decides whether a note may be reclassified, asked in
    /// the turn's context - the agent's, with its owner's rights (issues
    /// 131, 133, 255).
    access_policy: Arc<dyn AccessPolicy>,
    /// What may flow where (issue 304): what it recalls, how it reclassifies.
    confidentiality: Arc<dyn ConfidentialityPolicy>,
}

#[async_trait]
impl ToolProvider for NotesComponent {
    fn description(&self) -> String {
        "Notes in this channel: what you want to remember here, and what the others noted.".to_string()
    }

    fn tools(&self) -> Vec<Tool> {
        vec![
            Tool {
                name: WRITE_NOTE_TOOL_NAME.to_string(),
                description: "Note something worth remembering in this channel - a fact, a decision, \
                    a preference of the user. Every participant of the channel is shown your notes \
                    from now on, you included. Keep a note short and self-contained, and update one \
                    rather than writing a second on the same thing."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("text", ToolParameters::string("What to note"))],
                    ["text"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: NOTE_PRESENTATION,
            },
            Tool {
                name: UPDATE_NOTE_TOOL_NAME.to_string(),
                description: "Change what one of your own notes in this channel says, by its number, when \
                    it is no longer quite right. Give its whole new text; it keeps its number."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("id", ToolParameters::number("The number of your note")), ("text", ToolParameters::string("Its new text"))],
                    ["id", "text"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: NOTE_PRESENTATION,
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
                presentation: NOTE_PRESENTATION,
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
                presentation: NOTE_PRESENTATION,
            },
            Tool {
                name: READ_NOTES_TOOL_NAME.to_string(),
                description: "Read what the agents of another channel of this session noted. Without \
                    a channel, lists every channel you may read with its notes - also how to find out \
                    which channels there are. The notes of your own channel are already shown to you. \
                    Notes of another channel come without numbers: they can only be updated, \
                    revoked or reclassified in that channel."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("channel", ToolParameters::string("The name of the channel; omit it for all channels"))],
                    Vec::<&str>::new(),
                ),
                requires_approval: false,
                visibility: ToolVisibility::SHOWN,
                classification: None,
                presentation: NOTE_LIST_PRESENTATION,
            },
        ]
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        let value = match name {
            WRITE_NOTE_TOOL_NAME => self.execute_write_note(params, turn),
            UPDATE_NOTE_TOOL_NAME => self.execute_update_note(params, turn),
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
    /// `write_note(text)`: notes `text`, published when the turn completes.
    fn execute_write_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id = AgentNotes::note(turn, NoteKind::Public, text_argument(params)?);
        Ok(serde_json::json!({ "status": "noted", "id": id.0 }))
    }

    /// `update_note(id, text)`: changes the text of one of the agent's own
    /// current notes.
    fn execute_update_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: NoteId = id_argument(params)?;
        if AgentNotes::update(turn, NoteKind::Public, id, text_argument(params)?) {
            Ok(serde_json::json!({ "status": "updated", "id": id.0 }))
        } else {
            Err(ToolError::InvalidParameter(format!("you have no note #{id}; only your own current notes can be updated")))
        }
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
        AgentNotes::reclassify(turn, NoteKind::Public, id, classification.clone(), self.access_policy.as_ref(), self.confidentiality.as_ref()).await?;
        Ok(serde_json::json!({ "status": "reclassified", "id": id.0, "classification": classification.to_string() }))
    }

    /// `read_notes(channel?)`: the notes of the channel named, or of every
    /// channel that may be read, each read recorded as knowledge - classified
    /// as the notes read (issue 168).
    async fn execute_read_notes(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<ToolOutput> {
        #[derive(Deserialize)]
        struct ReadNotes { #[serde(default)] channel: Option<String> }
        let ReadNotes { channel } = arguments(params)?;
        let Some(session) = turn.session().await else {
            return Err(ToolError::ExecutionFailed("the turn is in no session to read from".to_string()));
        };
        let channels = match channel.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(name) => vec![session.channel_named(name).await?],
            None => session.channels().await,
        };
        let here = turn.channel_id();
        let mut listed = Vec::new();
        let mut read = Vec::new();
        for channel in channels {
            let mut notes = Vec::new();
            for agent_notes in channel.notes().await? {
                if let Some(reference) = notes_ref(channel.id(), channel.session().id(), &agent_notes) {
                    turn.record_knowledge(reference);
                }
                for note in agent_notes.notes {
                    notes.push(listed_note(&agent_notes.name, &note, channel.id() == here));
                    read.push(note.classification);
                }
            }
            listed.push(serde_json::json!({ "channel": channel.name(), "notes": notes }));
        }
        let value = serde_json::json!({ "channels": listed });
        Ok(ToolOutput { value, classification: Classification::join_all(&read) })
    }
}

/// A note as `read_notes` lists it. Its number only `here`, in the turn's
/// channel: the tools change notes of that channel only, and a number of
/// another channel's chain would name a different note here (issue 60).
fn listed_note(by: &str, note: &Note, here: bool) -> Value {
    if here {
        serde_json::json!({ "by": by, "id": note.id.0, "text": note.text })
    } else {
        serde_json::json!({ "by": by, "text": note.text })
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

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt, ContextError> {
        let own = ActorId::Agent(work.agent_id());
        let mut others = Vec::new();
        if let Some(session) = work.session().await {
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
    let mut sections = vec![Section::new(format!("Your notes (change them with `{WRITE_NOTE_TOOL_NAME}`, `{UPDATE_NOTE_TOOL_NAME}` and `{REVOKE_NOTE_TOOL_NAME}`):"), numbered(own))];
    sections.extend(others.map(|(name, notes)| Section::new(format!("Notes of {name}:"), numbered(notes))));
    render_sections("Notes in this channel", "What its participants noted to remember.", &sections)
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(NOTES_KIND).provides::<dyn ToolProvider>().provides::<dyn PromptProvider>().uses::<dyn AccessPolicy>().uses::<dyn ConfidentialityPolicy>(),
        |_descriptor, context| {
            let component = Arc::new(NotesComponent {
                access_policy: Arc::clone(context.service::<dyn AccessPolicy>()?),
                confidentiality: Arc::clone(context.service::<dyn ConfidentialityPolicy>()?),
            });
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::<NotesComponent>::clone(&component));
            facets.insert::<dyn PromptProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_runtime::transport::Discard;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::security::{SecurityContext, AllowAllPolicy, OwnershipPolicy};
    use caelumora_model::message::MessageId;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use chrono::Utc;

    fn note(id: u32, text: &str) -> Note {
        Note { id: NoteId(id), text: text.to_string(), classification: Classification::CHANNEL_DEFAULT, after: MessageId::new(), origin: Default::default(), written_at: Utc::now() }
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

    #[test]
    fn read_notes_numbers_only_the_notes_of_the_turns_channel() {
        let tea = note(3, "tea");
        assert_eq!(listed_note("Sassy", &tea, true), serde_json::json!({ "by": "Sassy", "id": 3, "text": "tea" }));
        assert_eq!(listed_note("Sassy", &tea, false), serde_json::json!({ "by": "Sassy", "text": "tea" }));
    }

    #[tokio::test]
    async fn a_note_is_reclassified_as_the_policy_allows_also_in_the_turn_it_was_noted() {
        use caelumora_model::confidentiality::{Classification, Confidentiality};
        let agent = AgentId::new();
        let mut state = AgentChannelState::default();
        let tools = NotesComponent { access_policy: Arc::new(OwnershipPolicy::new()), confidentiality: Arc::new(ConfidentialityRules::default()) };
        // Asked in the turn's context: the agent's, which reclassifies its own note.
        let context = SecurityContext::default().next(ActorId::Agent(agent), ActorId::None, ActorId::None);
        state.begin_turn(Default::default(), context, Default::default(), Vec::new(), Default::default());
        let mut turn = Turn::under_way(agent, &mut state);
        tools.execute(WRITE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "tea" }), None, &mut turn).await.unwrap();
        let above = serde_json::json!({ "id": 1, "level": "strictly_confidential" });
        assert!(matches!(tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &above, None, &mut turn).await, Err(ToolError::PermissionDenied(_))), "never above the channel");
        let unknown = serde_json::json!({ "id": 2, "level": "public" });
        assert!(matches!(tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &unknown, None, &mut turn).await, Err(ToolError::InvalidParameter(_))));
        let public_with_compartments = serde_json::json!({ "id": 1, "level": "public", "compartments": ["hr"] });
        assert!(matches!(tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &public_with_compartments, None, &mut turn).await, Err(ToolError::InvalidParameter(_))));
        tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1, "level": "public" }), None, &mut turn).await.expect("released");
        turn.commit(false, &Discard).await;
        assert_eq!(state.notes.current()[0].classification, Classification::general(Confidentiality::Public));
    }

    #[tokio::test]
    async fn notes_are_published_when_the_turn_completes_and_only_the_author_revokes() {
        let mut state = AgentChannelState::default();
        let tools = NotesComponent { access_policy: Arc::new(AllowAllPolicy::new()), confidentiality: Arc::new(ConfidentialityRules::default()) };
        {
            let mut turn = Turn::begun(AgentId::new(), &mut state);
            let noted = tools.execute(WRITE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "tea" }), None, &mut turn).await.unwrap();
            assert_eq!(noted.value["id"], 1);
            assert!(AgentNotes::current(&turn.work(), NoteKind::Public).is_empty(), "not before the turn completes");
            let unknown = tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 7 }), None, &mut turn).await;
            assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))), "not one of its notes");
            assert!(turn.commit(false, &Discard).await.channel_state_changed, "the turn changed notes");
            assert_eq!(state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["tea"]);
        }
        {
            let mut turn = Turn::begun(AgentId::new(), &mut state);
            tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn).await.unwrap();
            tools.execute(WRITE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "coffee" }), None, &mut turn).await.unwrap();
            turn.commit(false, &Discard).await;
            let notes = state.notes.current();
            assert_eq!(notes.iter().map(|n| (n.id.0, n.text.as_str())).collect::<Vec<_>>(), [(2, "coffee")], "a new number for the new version");
        }
        {
            // A turn that does not complete publishes nothing: the next one
            // starts without its changes.
            let mut turn = Turn::begun(AgentId::new(), &mut state);
            tools.execute(WRITE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "lost" }), None, &mut turn).await.unwrap();
            let turn = Turn::begun(AgentId::new(), &mut state);
            assert!(!turn.commit(false, &Discard).await.channel_state_changed);
        }
    }

    #[tokio::test]
    async fn an_updated_note_keeps_its_number_and_is_classified_as_the_turn_that_updated_it() {
        use caelumora_model::confidentiality::{Classification, Confidentiality};
        let mut state = AgentChannelState::default();
        let tools = NotesComponent { access_policy: Arc::new(AllowAllPolicy::new()), confidentiality: Arc::new(ConfidentialityRules::default()) };
        let texts = |state: &AgentChannelState| state.notes.current().into_iter().map(|n| (n.id.0, n.text)).collect::<Vec<_>>();
        {
            let mut turn = Turn::begun(AgentId::new(), &mut state);
            tools.execute(WRITE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "tea" }), None, &mut turn).await.unwrap();
            tools.execute(WRITE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "Berlin" }), None, &mut turn).await.unwrap();
            let updated = tools.execute(UPDATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 2, "text": "Hamburg" }), None, &mut turn).await.expect("noted in this turn");
            assert_eq!(updated.value, serde_json::json!({ "status": "updated", "id": 2 }));
            tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1, "level": "public" }), None, &mut turn).await.expect("released");
            turn.commit(false, &Discard).await;
            assert_eq!(texts(&state), [(1, "tea".to_string()), (2, "Hamburg".to_string())]);
        }
        let mut turn = Turn::begun(AgentId::new(), &mut state);
        let internal = Classification::general(Confidentiality::Internal);
        turn.see(&internal);
        let unknown = tools.execute(UPDATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 7, "text": "x" }), None, &mut turn).await;
        assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))), "not one of its notes");
        let empty = tools.execute(UPDATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1, "text": " " }), None, &mut turn).await;
        assert!(matches!(empty, Err(ToolError::InvalidParameter(_))), "a note says something");
        tools.execute(UPDATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1, "text": "coffee" }), None, &mut turn).await.unwrap();
        tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 2 }), None, &mut turn).await.unwrap();
        let revoked = tools.execute(UPDATE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 2, "text": "Munich" }), None, &mut turn).await;
        assert!(matches!(revoked, Err(ToolError::InvalidParameter(_))), "a revoked note is not updated");
        let noted = tools.execute(WRITE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "Munich" }), None, &mut turn).await.unwrap();
        assert_eq!(noted.value["id"], 3, "an update uses up no number");
        turn.commit(false, &Discard).await;
        let notes = state.notes.current();
        assert_eq!(texts(&state), [(1, "coffee".to_string()), (3, "Munich".to_string())]);
        assert_eq!(notes[0].classification, internal, "the release was of the old text");
    }

    #[tokio::test]
    async fn a_note_revoked_in_the_turn_that_noted_it_is_gone_and_its_number_used_up() {
        let mut state = AgentChannelState::default();
        let tools = NotesComponent { access_policy: Arc::new(AllowAllPolicy::new()), confidentiality: Arc::new(ConfidentialityRules::default()) };
        {
            let mut turn = Turn::begun(AgentId::new(), &mut state);
            tools.execute(WRITE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "draft" }), None, &mut turn).await.unwrap();
            tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn).await.expect("noted in this turn");
            let again = tools.execute(REVOKE_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn).await;
            assert!(matches!(again, Err(ToolError::InvalidParameter(_))), "revoked already");
            let revoked = tools.execute(RECLASSIFY_NOTE_TOOL_NAME, &serde_json::json!({ "id": 1, "level": "public" }), None, &mut turn).await;
            assert!(matches!(revoked, Err(ToolError::InvalidParameter(_))), "a revoked note is not reclassified");
            turn.commit(false, &Discard).await;
            assert!(state.notes.current().is_empty());
        }
        let mut turn = Turn::begun(AgentId::new(), &mut state);
        let noted = tools.execute(WRITE_NOTE_TOOL_NAME, &serde_json::json!({ "text": "final" }), None, &mut turn).await.unwrap();
        assert_eq!(noted.value["id"], 2);
    }

    #[test]
    fn a_note_acted_on_is_shown_as_a_note_and_the_notes_read_as_a_list() {
        let tools = NotesComponent { access_policy: Arc::new(OwnershipPolicy::new()), confidentiality: Arc::new(ConfidentialityRules::default()) }.tools();
        let notes: Vec<_> = tools.iter().filter(|t| t.presentation == NOTE_PRESENTATION).map(|t| t.name.as_str()).collect();
        assert_eq!(notes, [WRITE_NOTE_TOOL_NAME, UPDATE_NOTE_TOOL_NAME, REVOKE_NOTE_TOOL_NAME, RECLASSIFY_NOTE_TOOL_NAME]);
        let read = tools.iter().find(|t| t.name == READ_NOTES_TOOL_NAME).unwrap();
        assert_eq!(read.presentation, NOTE_LIST_PRESENTATION);
    }
}
