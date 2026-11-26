//! The `private_notes` component (issue 51): an agent notes what only it
//! should remember, and recalls it in all its channels of the session where
//! that tells no one anything new.
//!
//! Its tools `write_private_note`, `update_private_note` and `revoke_private_note` change the
//! agent's private notes in the current channel - taken over when the turn completes, like public
//! notes (`notes`). Its prompt part gives the model its private notes from this channel and from
//! every other channel of the session it takes or took part in, as far as they may be copied into
//! this one (`NotesReader::own_private_elsewhere`,
//! `ConfidentialityQuestions::may_copy_information`, issues 133, 264): a note carries the
//! classification of the channel it was written in, checked when it is read; whoever may read that
//! channel now does not matter, the notes are the agent's own. It is a part before the turn's input
//! (`PromptPosition::BeforeTurnInput`) unless its `position` says otherwise, and every chain it
//! shows is recorded as a `KnowledgeRef`.
//!
//! "Private" means private from other agents and other users (issue 54):
//! its tools are `ToolVisibility::HIDDEN`, so their calls never reach the
//! channel, but the agent's owner sees them, and the prompt part, in the
//! context panel - the audit view of the one responsible for the agent.
//!
//! Its command `/private-notes @agent` (issue 574), for the agent's owners alone (`owners_only`),
//! shows them what the prompt part shows the model: the private notes the agent recalls here.

use crate::component::notes::note_tools;
use crate::extension::{NotesReader, NotesWriter};
use crate::note::{AgentNotes, Note, NoteKind, PrivateNotesElsewhere};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::command::AgentCommand;
use caelumora_agents::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{tool_text, FixedTools, ToolProvider};
use caelumora_agents::turn::Turn;
use caelumora_api::command::{Command, CommandError, CommandProvider, CommandResult};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::actor::ActorId;
use caelumora_domain::command::{CommandDescription, CommandOutput};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::{KnowledgeRef, KnowledgeSource};
use caelumora_domain::digest::Digest;
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::value::command_name::CommandName;
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
const TEXT: &str = "notes.private_notes";

#[derive(Debug)]
pub struct PrivateNotesComponent {
    /// The texts of its prompt part (issue 57).
    prompts: PromptTexts,
    /// Where its prompt part goes: before the turn's input unless configured otherwise.
    position: PromptPosition,
}

impl PrivateNotesComponent {
    /// The component, its prompt part at `position`, its texts from `prompts`.
    pub(crate) fn new(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { prompts, position }
    }
}

/// The tools of the component, their texts from `prompts`.
///
/// # Errors
/// `PromptError` if one of them cannot be had.
fn tools(prompts: &PromptTexts) -> Result<FixedTools, PromptError> {
    let names = [
        WRITE_PRIVATE_NOTE_TOOL_NAME,
        UPDATE_PRIVATE_NOTE_TOOL_NAME,
        REVOKE_PRIVATE_NOTE_TOOL_NAME,
        RECLASSIFY_PRIVATE_NOTE_TOOL_NAME,
    ];
    let tools = note_tools(prompts, PRIVATE_NOTES_KIND, NoteKind::Private, names)?;
    Ok(FixedTools::new(
        tool_text(prompts, NAMESPACE, PRIVATE_NOTES_KIND, "description", ())?,
        tools,
    ))
}

#[async_trait]
impl PromptProvider for PrivateNotesComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let (own, digest, elsewhere) = recalled(situation).await;

        let agent = ActorId::Agent(situation.agent_id());
        let place = NoteKind::Private.place();
        let private = |channel, digest, notes: &[Note]| KnowledgeRef {
            channel,
            // Its own session's channels only.
            session: situation.session_id(),
            source: KnowledgeSource::kept_by::<AgentNotes>(agent, place, digest),
            classification: Classification::join_all(notes.iter().map(|n| &n.classification)),
            private: true,
        };
        let mut knowledge: Vec<KnowledgeRef> =
            elsewhere.iter().map(|c| private(c.channel, c.digest, &c.notes)).collect();
        if !own.is_empty() {
            knowledge.push(private(situation.channel_id(), digest, &own));
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
        let Some(reader) = turn.situation().reader::<dyn NotesReader>().await else {
            warn!("/{} could not reach the private notes", PRIVATE_NOTES_COMMAND_NAME);
            return Err(CommandError::Failed {
                command: PRIVATE_NOTES_COMMAND_NAME.to_string(),
                reason: "the private notes could not be read".to_string(),
            });
        };
        let own = reader.own(NoteKind::Private).current();
        let elsewhere = reader.own_private_elsewhere().await;
        Ok(CommandOutput::private(listed(turn.agent_name(), &own, &elsewhere)))
    }
}

/// What the agent of the work in `situation` recalls of its private notes: those of this channel
/// as they stand, with the digest of their chain, and those of the session's other channels it
/// took part in, through the agent itself, as far as they may be copied here
/// (`NotesReader::own_private_elsewhere`); none where its notes cannot be reached.
async fn recalled(
    situation: &Situation<'_>,
) -> (Vec<Note>, Option<Digest>, Vec<PrivateNotesElsewhere>) {
    let Some(reader) = situation.reader::<dyn NotesReader>().await else {
        return (Vec::new(), None, Vec::new());
    };
    let own = reader.own(NoteKind::Private);
    (own.current(), own.digest(), reader.own_private_elsewhere().await)
}

/// What `/private-notes` answers of the agent `agent`, which recalls its private notes `own` of
/// this channel - numbered, as it changes them here - and those of other channels (`elsewhere`),
/// which come without channels holding none (`NotesReader::own_private_elsewhere`).
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
/// come without channels holding none (`NotesReader::own_private_elsewhere`); empty if there are
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

pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(PRIVATE_NOTES_KIND)
                .params::<PositionParams>()
                .reads::<dyn NotesReader>()
                .writes::<dyn NotesWriter>()
                .provides::<dyn ToolProvider>()
                .provides::<dyn PromptProvider>()
                .provides::<dyn CommandProvider<dyn AgentCommand>>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let position =
                PositionParams::position_of(&descriptor.params, PromptPosition::BeforeTurnInput)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(tools(&texts)?));
            facets.insert::<dyn PromptProvider>(Arc::new(PrivateNotesComponent::new(
                position, texts,
            )));
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
    use crate::note::NoteId;
    use crate::PROMPTS;
    use caelumora_agents::testing::prompts_with;
    use caelumora_agents::tool::NOTE_PRESENTATION;
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::message::MessageId;
    use caelumora_domain::tool::ToolVisibility;
    use chrono::Utc;

    fn note(id: u32, text: &str) -> Note {
        Note {
            id: NoteId(id),
            text: text.to_string(),
            classification: Classification::CHANNEL_DEFAULT,
            after: MessageId::new(),
            origin: (),
            written_at: Utc::now(),
        }
    }

    fn research() -> [PrivateNotesElsewhere; 1] {
        [PrivateNotesElsewhere {
            channel: ChannelId::default(),
            name: "research".to_string(),
            notes: vec![note(1, "Kaya prefers short answers.")],
            digest: None,
        }]
    }

    #[test]
    fn nothing_noted_gives_no_prompt_part() {
        assert_eq!(render(&prompts_with(NAMESPACE, PROMPTS), &[], &[]).unwrap(), "");
    }

    #[test]
    fn the_prompt_part_shows_this_channel_first_then_the_others_by_name() {
        let own = [note(2, "Remind Kaya of the deadline.")];
        let text = render(&prompts_with(NAMESPACE, PROMPTS), &own, &research()).unwrap();
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
        assert_eq!(
            listed("helper", &[note(2, "Remind Kaya of the deadline.")], &research()),
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

    #[test]
    fn every_private_note_acted_on_is_shown_as_a_note_to_the_owner_alone() {
        let tools = tools(&prompts_with(NAMESPACE, PROMPTS)).unwrap().tools();
        assert!(
            tools
                .iter()
                .map(|t| t.description())
                .all(|t| (t.presentation, t.visibility)
                    == (NOTE_PRESENTATION, ToolVisibility::HIDDEN))
        );
    }
}
