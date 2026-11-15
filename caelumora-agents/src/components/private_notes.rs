//! The `private_notes` component (issue 51): an agent notes what only it
//! should remember, and recalls it in all its channels of the session where
//! that tells no one anything new.
//!
//! Its tools `write_private_note`, `update_private_note` and `revoke_private_note` change the
//! agent's private notes in the current channel - taken over when the turn completes, like public
//! notes (`notes`). Its prompt part gives the model its private notes from this channel and from
//! every other channel of the session it takes or took part in, as far as they may be copied into
//! this one (`SessionReader::own_private_notes`, `ConfidentialityQuestions::may_copy_information`,
//! issues 133, 264): a note carries the classification of the channel it was written in, checked
//! when it is read; whoever may read that channel now does not matter, the notes are the agent's
//! own. It is a part before the turn's input (`PromptPosition::BeforeTurnInput`) unless its
//! `position` says otherwise, and every chain it shows is recorded as a `KnowledgeRef`.
//!
//! "Private" means private from other agents and other users (issue 54):
//! its tools are `ToolVisibility::HIDDEN`, so their calls never reach the
//! channel, but the agent's owner sees them, and the prompt part, in the
//! context panel - the audit view of the one responsible for the agent.
//!
//! Its command `/private-notes @agent` (issue 574), for the agent's owners alone (`owners_only`),
//! shows them what the prompt part shows the model: the private notes the agent recalls here.

use crate::command::AgentCommand;
use crate::components::notes::NOTE_PRESENTATION;
use crate::notes::{AgentNotes, NoteKind};
use crate::prompt::NAMESPACE;
use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use crate::situation::Situation;
use crate::tool::{
    classification_argument, described, id_argument, reclassify_parameters, text_argument,
    tool_text, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandError, CommandProvider, CommandResult};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_api::reader::{PrivateNotesElsewhere, ViewResult};
use caelumora_domain::command::{CommandDescription, CommandOutput};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::{KnowledgeRef, KnowledgeSource};
use caelumora_domain::note::{Note, NoteId};
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolVisibility};
use caelumora_domain::value::command_name::CommandName;
use serde_json::Value;
use std::sync::Arc;
use tracing::warn;

pub const PRIVATE_NOTES_KIND: &str = "private_notes";

pub const WRITE_PRIVATE_NOTE_TOOL_NAME: &str = "write_private_note";
pub const UPDATE_PRIVATE_NOTE_TOOL_NAME: &str = "update_private_note";
pub const REVOKE_PRIVATE_NOTE_TOOL_NAME: &str = "revoke_private_note";
pub const RECLASSIFY_PRIVATE_NOTE_TOOL_NAME: &str = "reclassify_private_note";

/// The command that shows the agent's owners its private notes.
const PRIVATE_NOTES_COMMAND_NAME: &str = "private-notes";

/// The text of its prompt part in the prompt catalogue.
const TEXT: &str = "agents.private_notes";

#[derive(Debug)]
pub struct PrivateNotesComponent {
    /// The texts of its prompt part (issue 57).
    prompts: PromptTexts,
    /// Where its prompt part goes: before the turn's input unless configured otherwise.
    position: PromptPosition,
    /// What its tool set is for and its tools, as the prompt catalogue
    /// words them.
    description: String,
    tools: Vec<ToolDescription>,
}

#[async_trait]
impl ToolProvider for PrivateNotesComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<ToolDescription> {
        self.tools.clone()
    }

    async fn execute(
        &self,
        name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
        let value = match name {
            WRITE_PRIVATE_NOTE_TOOL_NAME => self.execute_write_private_note(params, turn),
            UPDATE_PRIVATE_NOTE_TOOL_NAME => self.execute_update_private_note(params, turn),
            REVOKE_PRIVATE_NOTE_TOOL_NAME => self.execute_revoke_private_note(params, turn),
            RECLASSIFY_PRIVATE_NOTE_TOOL_NAME => {
                self.execute_reclassify_private_note(params, turn).await
            }
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }?;
        // What these answer tells only what the model asked for: classified
        // as the turn (issue 168).
        Ok(ToolOutput::classified(value, turn.situation().label()))
    }
}

impl PrivateNotesComponent {
    /// The component, its texts from `prompts`, its prompt part at `position`; whether a note may
    /// be reclassified, the kernel decides, asked in the turn's context
    /// (`Turn::may_reclassify_note`, issue 516).
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(crate) fn new(position: PromptPosition, prompts: PromptTexts) -> Result<Self, PromptError> {
        let tools = vec![
            described(
                &prompts,
                NAMESPACE,
                PRIVATE_NOTES_KIND,
                ToolDescription {
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
                },
                (),
            )?,
            described(
                &prompts,
                NAMESPACE,
                PRIVATE_NOTES_KIND,
                ToolDescription {
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
                },
                (),
            )?,
            described(
                &prompts,
                NAMESPACE,
                PRIVATE_NOTES_KIND,
                ToolDescription {
                    name: REVOKE_PRIVATE_NOTE_TOOL_NAME.to_string(),
                    description: String::new(),
                    parameters: ToolParameters::object([("id", ToolParameters::number())], ["id"]),
                    requires_approval: false,
                    visibility: ToolVisibility::HIDDEN,
                    classification: None,
                    presentation: NOTE_PRESENTATION,
                },
                (),
            )?,
            described(
                &prompts,
                NAMESPACE,
                PRIVATE_NOTES_KIND,
                ToolDescription {
                    name: RECLASSIFY_PRIVATE_NOTE_TOOL_NAME.to_string(),
                    description: String::new(),
                    parameters: reclassify_parameters(&prompts)?,
                    requires_approval: true,
                    visibility: ToolVisibility::HIDDEN,
                    classification: None,
                    presentation: NOTE_PRESENTATION,
                },
                (),
            )?,
        ];
        let description = tool_text(&prompts, NAMESPACE, PRIVATE_NOTES_KIND, "description", ())?;
        Ok(Self { prompts, position, description, tools })
    }

    /// `write_private_note(text)`: notes `text` privately, taken over when
    /// the turn completes.
    fn execute_write_private_note(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id = AgentNotes::note(turn, NoteKind::Private, text_argument(params)?);
        Ok(serde_json::json!({ "status": "noted privately", "id": id.0 }))
    }

    /// `update_private_note(id, text)`: changes the text of one of the
    /// agent's private notes of this channel.
    fn execute_update_private_note(
        &self,
        params: &Value,
        turn: &mut Turn<'_>,
    ) -> ToolResult<Value> {
        let id: NoteId = id_argument(params)?;
        if AgentNotes::update(turn, NoteKind::Private, id, text_argument(params)?) {
            Ok(serde_json::json!({ "status": "updated", "id": id.0 }))
        } else {
            Err(ToolError::InvalidParameter(format!(
                "you have no private note #{id} in this channel"
            )))
        }
    }

    /// `reclassify_private_note(id, level, compartments?)`: reclassifies one
    /// of the agent's private notes of this channel, as the policy allows.
    async fn execute_reclassify_private_note(
        &self,
        params: &Value,
        turn: &mut Turn<'_>,
    ) -> ToolResult<Value> {
        let id: NoteId = id_argument(params)?;
        let classification = classification_argument(params)?;
        AgentNotes::reclassify(turn, NoteKind::Private, id, classification.clone()).await?;
        Ok(
            serde_json::json!({ "status": "reclassified", "id": id.0, "classification": classification.to_string() }),
        )
    }

    /// `revoke_private_note(id)`: revokes one of the agent's private notes
    /// of this channel.
    fn execute_revoke_private_note(
        &self,
        params: &Value,
        turn: &mut Turn<'_>,
    ) -> ToolResult<Value> {
        let id: NoteId = id_argument(params)?;
        if AgentNotes::revoke(turn, NoteKind::Private, id) {
            Ok(serde_json::json!({ "status": "revoked", "id": id.0 }))
        } else {
            Err(ToolError::InvalidParameter(format!(
                "you have no private note #{id} in this channel"
            )))
        }
    }
}

#[async_trait]
impl PromptProvider for PrivateNotesComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let (own, elsewhere) = recalled(situation).await?;

        let private = |channel, digest, notes: &[Note]| KnowledgeRef {
            channel,
            // Its own session's channels only.
            session: situation.session_id(),
            source: KnowledgeSource::PrivateNotes { agent: situation.agent_id(), digest },
            classification: Classification::join_all(notes.iter().map(|n| &n.classification)),
            private: true,
        };
        let mut knowledge: Vec<KnowledgeRef> =
            elsewhere.iter().map(|c| private(c.channel, c.digest, &c.notes)).collect();
        if !own.is_empty() {
            knowledge.push(private(
                situation.channel_id(),
                AgentNotes::digest(situation, NoteKind::Private),
                &own,
            ));
        }
        Ok(Prompt::of_knowledge(render(&self.prompts, &own, &elsewhere)?, knowledge))
    }
}

/// `/private-notes @agent` (issue 574): shows the agent's owners the private notes it recalls in
/// the channel.
#[derive(Debug)]
struct PrivateNotesCommand(CommandDescription);

impl PrivateNotesCommand {
    /// The command, as `/help` tells it.
    fn new() -> Self {
        Self(CommandDescription {
            name: CommandName::fixed(PRIVATE_NOTES_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Shows what the agent noted for itself and recalls here: its private notes of this channel, and of the session's other channels as far as they may be told here.".to_string(),
            parameters: Vec::new(),
            // As what it saw (issue 54): its owners'.
            owners_only: true,
        })
    }
}

impl Command for PrivateNotesCommand {
    fn description(&self) -> &CommandDescription {
        &self.0
    }
}

#[async_trait]
impl AgentCommand for PrivateNotesCommand {
    /// Carries out `/private-notes`: the private notes the agent recalls in the channel, as its
    /// prompt part gives them (`recalled`), answered to the issuer alone.
    async fn execute(&self, _args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        let (own, elsewhere) = recalled(&turn.situation()).await.map_err(|e| {
            warn!("/{} could not read the private notes: {}", PRIVATE_NOTES_COMMAND_NAME, e);
            CommandError::Failed {
                command: PRIVATE_NOTES_COMMAND_NAME.to_string(),
                reason: "the private notes could not be read".to_string(),
            }
        })?;
        Ok(CommandOutput::private(listed(turn.agent_name(), &own, &elsewhere)))
    }
}

/// What the agent of the work in `situation` recalls of its private notes: those of this
/// channel, from the turn with its own changes, and those of the session's other channels it took
/// part in, through the agent itself, as far as they may be copied here
/// (`SessionReader::own_private_notes`).
///
/// # Errors
/// What reaching the session reports.
async fn recalled(
    situation: &Situation<'_>,
) -> ViewResult<(Vec<Note>, Vec<PrivateNotesElsewhere>)> {
    let own = AgentNotes::current(situation, NoteKind::Private);
    let elsewhere = match situation.session().await? {
        Some(session) => session.own_private_notes().await,
        None => Vec::new(),
    };
    Ok((own, elsewhere))
}

/// What `/private-notes` answers of the agent `agent`, which recalls its private notes `own` of
/// this channel - numbered, as it changes them here - and those of other channels (`elsewhere`),
/// which come without channels holding none (`SessionReader::own_private_notes`).
fn listed(agent: &str, own: &[Note], elsewhere: &[PrivateNotesElsewhere]) -> String {
    let mut text = String::new();
    if !own.is_empty() {
        text.push_str("\nIn this channel:\n");
        for note in own {
            text.push_str(&format!("- #{} {}\n", note.id, note.text));
        }
    }
    for channel in elsewhere {
        text.push_str(&format!("\nFrom channel {}:\n", channel.name));
        for note in &channel.notes {
            text.push_str(&format!("- {}\n", note.text));
        }
    }
    if text.is_empty() {
        return format!("{agent} recalls no private notes here.");
    }
    format!("The private notes {agent} recalls here:\n{}", text.trim_end())
}

/// The prompt part of `prompts` for the private notes of this channel (`own`) and of others, which
/// come without channels holding none (`SessionReader::own_private_notes`); empty if there are
/// none. Notes of other channels cannot be changed here, so they go unnumbered (issue 60).
fn render(
    prompts: &PromptTexts,
    own: &[Note],
    elsewhere: &[PrivateNotesElsewhere],
) -> Result<String, PromptError> {
    let own: Vec<_> =
        own.iter().map(|n| context! { id => n.id.to_string(), text => n.text }).collect();
    let elsewhere: Vec<_> = elsewhere.iter()
        .map(|c| context! { name => c.name, notes => c.notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>() })
        .collect();
    if own.is_empty() && elsewhere.is_empty() {
        return Ok(String::new());
    }
    prompts.render(TEXT, context! { own, elsewhere, write => WRITE_PRIVATE_NOTE_TOOL_NAME, update => UPDATE_PRIVATE_NOTE_TOOL_NAME, revoke => REVOKE_PRIVATE_NOTE_TOOL_NAME })
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(PRIVATE_NOTES_KIND)
                .params::<PositionParams>()
                .provides::<dyn ToolProvider>()
                .provides::<dyn PromptProvider>()
                .provides::<dyn CommandProvider<dyn AgentCommand>>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let position =
                PositionParams::position_of(&descriptor.params, PromptPosition::BeforeTurnInput)?;
            let component = Arc::new(PrivateNotesComponent::new(position, texts)?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::<PrivateNotesComponent>::clone(&component));
            facets.insert::<dyn PromptProvider>(component);
            let private_notes: Vec<Arc<dyn AgentCommand>> =
                vec![Arc::new(PrivateNotesCommand::new())];
            facets.insert::<dyn CommandProvider<dyn AgentCommand>>(Arc::new(private_notes));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::command::CommandVisibility;
    use caelumora_domain::ledger::Change;
    use caelumora_domain::message::MessageId;
    use caelumora_domain::note::NoteOrigin;
    use caelumora_kernel::testing::Discard;
    use chrono::Utc;

    fn note(id: u32, text: &str) -> Note {
        Note {
            id: NoteId(id),
            text: text.to_string(),
            classification: Classification::CHANNEL_DEFAULT,
            after: MessageId::new(),
            origin: Default::default(),
            written_at: Utc::now(),
        }
    }

    #[test]
    fn nothing_noted_gives_no_prompt_part() {
        assert_eq!(render(&testing::prompts(), &[], &[]).unwrap(), "");
    }

    #[test]
    fn the_prompt_part_shows_this_channel_first_then_the_others_by_name() {
        let elsewhere = [PrivateNotesElsewhere {
            channel: ChannelId::default(),
            name: "research".to_string(),
            notes: vec![note(1, "Kaya prefers short answers.")],
            digest: None,
        }];
        let text =
            render(&testing::prompts(), &[note(2, "Remind Kaya of the deadline.")], &elsewhere)
                .unwrap();
        assert!(
            text.find("#2: Remind Kaya").unwrap()
                < text.find("From channel research (change them only there):").unwrap(),
            "{text}"
        );
        assert!(
            text.contains("- Kaya prefers short answers."),
            "unnumbered: it cannot be revoked here"
        );
    }

    /// `/private-notes` lists what the prompt part shows: this channel's notes numbered, the
    /// others' by their channel, unnumbered; or that there are none.
    #[test]
    fn private_notes_are_listed_this_channels_numbered_then_the_others_by_channel() {
        let elsewhere = [PrivateNotesElsewhere {
            channel: ChannelId::default(),
            name: "research".to_string(),
            notes: vec![note(1, "Kaya prefers short answers.")],
            digest: None,
        }];
        assert_eq!(
            listed("helper", &[note(2, "Remind Kaya of the deadline.")], &elsewhere),
            "The private notes helper recalls here:\n\n\
             In this channel:\n\
             - #2 Remind Kaya of the deadline.\n\n\
             From channel research:\n\
             - Kaya prefers short answers.",
            "this channel's numbered, as they are changed here"
        );
        assert_eq!(
            listed("helper", &[], &[]),
            "helper recalls no private notes here.",
            "that there are none"
        );
    }

    /// The agent carries out `/private-notes` from its state as stored, and answers the issuer
    /// alone.
    #[tokio::test]
    async fn private_notes_are_shown_from_the_agents_state_to_the_issuer_alone() {
        let mut state = AgentChannelState::default();
        state.private_notes.push(
            Change::Add {
                id: NoteId(1),
                text: "secret".to_string(),
                classification: Classification::CHANNEL_DEFAULT,
            },
            NoteOrigin::default(),
            MessageId::default(),
        );
        let mut turn = Turn::begun(AgentId::new(), &mut state);

        let answer = PrivateNotesCommand::new().execute("", &mut turn).await.expect("carried out");
        assert_eq!(answer.visibility, CommandVisibility::Private, "to the issuer alone");
        assert!(answer.text.ends_with("In this channel:\n- #1 secret"), "{}", answer.text);
    }

    #[tokio::test]
    async fn private_notes_are_numbered_apart_from_public_ones_and_taken_over_with_the_turn() {
        let mut state = AgentChannelState::default();
        let tools = PrivateNotesComponent::new(PromptPosition::BeforeTurnInput, testing::prompts())
            .unwrap();
        let mut turn = Turn::begun(AgentId::new(), &mut state);
        AgentNotes::note(&mut turn, NoteKind::Public, "public".to_string());
        let noted = tools
            .execute(
                WRITE_PRIVATE_NOTE_TOOL_NAME,
                &serde_json::json!({ "text": "secret" }),
                None,
                &mut turn,
            )
            .await
            .unwrap();
        assert_eq!(noted.value["id"], 1, "its own numbering");
        assert!(
            AgentNotes::current(&turn.situation(), NoteKind::Private).is_empty(),
            "not before the turn completes"
        );
        assert!(
            turn.commit(false, &Discard).await.channel_state_changed,
            "the channel state changed"
        );
        assert_eq!(
            state.private_notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
            ["secret"]
        );
        assert_eq!(
            state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
            ["public"],
            "the chains are apart"
        );

        // The next turn.
        let mut turn = Turn::begun(AgentId::new(), &mut state);
        let unknown = tools
            .execute(
                REVOKE_PRIVATE_NOTE_TOOL_NAME,
                &serde_json::json!({ "id": 9 }),
                None,
                &mut turn,
            )
            .await;
        assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))));
        let unknown = tools
            .execute(
                UPDATE_PRIVATE_NOTE_TOOL_NAME,
                &serde_json::json!({ "id": 9, "text": "x" }),
                None,
                &mut turn,
            )
            .await;
        assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))));
        tools
            .execute(
                UPDATE_PRIVATE_NOTE_TOOL_NAME,
                &serde_json::json!({ "id": 1, "text": "a secret" }),
                None,
                &mut turn,
            )
            .await
            .unwrap();
        assert!(
            turn.commit(false, &Discard).await.channel_state_changed,
            "private notes are part of the channel state too"
        );
        assert_eq!(
            state
                .private_notes
                .current()
                .iter()
                .map(|n| (n.id.0, n.text.as_str()))
                .collect::<Vec<_>>(),
            [(1, "a secret")],
            "updated in its own chain"
        );
        assert_eq!(
            state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
            ["public"]
        );

        let mut turn = Turn::begun(AgentId::new(), &mut state);
        tools
            .execute(
                REVOKE_PRIVATE_NOTE_TOOL_NAME,
                &serde_json::json!({ "id": 1 }),
                None,
                &mut turn,
            )
            .await
            .unwrap();
        turn.commit(false, &Discard).await;
        assert!(state.private_notes.current().is_empty());
    }

    #[test]
    fn every_private_note_acted_on_is_shown_as_a_note_to_the_owner_alone() {
        let tools = PrivateNotesComponent::new(PromptPosition::BeforeTurnInput, testing::prompts())
            .unwrap()
            .tools();
        assert!(
            tools
                .iter()
                .all(|t| (t.presentation, t.visibility)
                    == (NOTE_PRESENTATION, ToolVisibility::HIDDEN))
        );
    }
}
