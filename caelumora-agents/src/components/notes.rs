//! The `notes` component (issue 17): an agent notes what it wants to
//! remember in a channel, and is given what every agent of the channel noted.
//!
//! Its tools `write_note`, `update_note` and `revoke_note` change the agent's own notes - taken
//! over when the turn completes, see `crate::notes::AgentNotes`; `read_notes` reads the notes of
//! another channel of the session, or of all it may read. Every read is recorded as a
//! `KnowledgeRef` with the turn's messages. Its prompt part gives the model the notes of all agents
//! of the channel before the turn's input (`PromptPosition::BeforeTurnInput`) unless its
//! `position` says otherwise, not in the system prompt, so that changing notes do not invalidate
//! the prompt cache (issue 47).

use crate::notes::{AgentNotes, NoteKind};
use crate::prompt::NAMESPACE;
use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use crate::situation::Situation;
use crate::tool::{
    arguments, classification_argument, described, id_argument, reclassify_parameters,
    text_argument, tool_text, FixedTools, Tool, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use crate::turn::Turn;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::{KnowledgeRef, KnowledgeSource};
use caelumora_domain::note::{Note, NoteId};
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::session::SessionId;
use caelumora_domain::snapshot::PublishedNotes;
use caelumora_domain::tool::{ToolDescription, ToolParameters, ToolPresentation, ToolVisibility};
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

/// The text of its prompt part in the prompt catalogue.
const TEXT: &str = "agents.notes";

#[derive(Debug)]
pub struct NotesComponent {
    /// The texts of its prompt part (issue 57).
    prompts: PromptTexts,
    /// Where its prompt part goes: before the turn's input unless configured otherwise.
    position: PromptPosition,
}

impl NotesComponent {
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
        WRITE_NOTE_TOOL_NAME,
        UPDATE_NOTE_TOOL_NAME,
        REVOKE_NOTE_TOOL_NAME,
        RECLASSIFY_NOTE_TOOL_NAME,
    ];
    let mut tools = note_tools(prompts, NOTES_KIND, NoteKind::Public, names)?;
    let read_notes = ToolDescription {
        name: READ_NOTES_TOOL_NAME.to_string(),
        description: String::new(),
        parameters: ToolParameters::object(
            [("channel", ToolParameters::string())],
            Vec::<&str>::new(),
        ),
        requires_approval: false,
        visibility: ToolVisibility::SHOWN,
        classification: None,
        presentation: NOTE_LIST_PRESENTATION,
    };
    let description = described(prompts, NAMESPACE, NOTES_KIND, read_notes, ())?;
    tools.push(Arc::new(ReadNotesTool { description }));
    Ok(FixedTools::new(tool_text(prompts, NAMESPACE, NOTES_KIND, "description", ())?, tools))
}

/// The tools that write, update, revoke and reclassify the agent's notes of `kind`, named `write`,
/// `update`, `revoke` and `reclassify`, their texts those of the component kind `component` in
/// `prompts`: published notes' calls shown, private ones' hidden (issue 54). Reclassifying asks
/// for approval.
///
/// # Errors
/// `PromptError` if one of their texts cannot be had.
pub(crate) fn note_tools(
    prompts: &PromptTexts,
    component: &str,
    kind: NoteKind,
    [write, update, revoke, reclassify]: [&str; 4],
) -> Result<Vec<Arc<dyn Tool>>, PromptError> {
    let visibility = match kind {
        NoteKind::Public => ToolVisibility::SHOWN,
        NoteKind::Private => ToolVisibility::HIDDEN,
    };
    let described = |name: &str, parameters, requires_approval| {
        let tool = ToolDescription {
            name: name.to_string(),
            description: String::new(),
            parameters,
            requires_approval,
            visibility,
            classification: None,
            presentation: NOTE_PRESENTATION,
        };
        described(prompts, NAMESPACE, component, tool, ())
    };
    let text = ToolParameters::object([("text", ToolParameters::string())], ["text"]);
    let id_and_text = ToolParameters::object(
        [("id", ToolParameters::number()), ("text", ToolParameters::string())],
        ["id", "text"],
    );
    let id = ToolParameters::object([("id", ToolParameters::number())], ["id"]);
    Ok(vec![
        Arc::new(WriteNoteTool { description: described(write, text, false)?, kind }),
        Arc::new(UpdateNoteTool { description: described(update, id_and_text, false)?, kind }),
        Arc::new(RevokeNoteTool { description: described(revoke, id, false)?, kind }),
        Arc::new(ReclassifyNoteTool {
            description: described(reclassify, reclassify_parameters(prompts)?, true)?,
            kind,
        }),
    ])
}

/// What a note tool answers where the agent has no note `id` of `kind` to act on, which it would
/// have `done`: the model is told which notes it may act on.
fn missing(kind: NoteKind, id: NoteId, done: &str) -> ToolError {
    ToolError::InvalidParameter(match kind {
        NoteKind::Public => {
            format!("you have no note #{id}; only your own current notes can be {done}")
        }
        NoteKind::Private => format!("you have no private note #{id} in this channel"),
    })
}

/// `write_note(text)`, `write_private_note(text)`: notes `text` in the chain of its `kind`, taken
/// over when the turn completes.
#[derive(Debug)]
struct WriteNoteTool {
    description: ToolDescription,
    kind: NoteKind,
}

#[async_trait]
impl Tool for WriteNoteTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let id = AgentNotes::note(turn, self.kind, text_argument(params)?);
        let status = match self.kind {
            NoteKind::Public => "noted",
            NoteKind::Private => "noted privately",
        };
        Ok(ToolOutput::of_turn(serde_json::json!({ "status": status, "id": id.0 }), turn))
    }
}

/// `update_note(id, text)`, `update_private_note(id, text)`: changes the text of one of the
/// agent's own current notes of its `kind` in this channel.
#[derive(Debug)]
struct UpdateNoteTool {
    description: ToolDescription,
    kind: NoteKind,
}

#[async_trait]
impl Tool for UpdateNoteTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let id: NoteId = id_argument(params)?;
        if !AgentNotes::update(turn, self.kind, id, text_argument(params)?) {
            return Err(missing(self.kind, id, "updated"));
        }
        Ok(ToolOutput::of_turn(serde_json::json!({ "status": "updated", "id": id.0 }), turn))
    }
}

/// `revoke_note(id)`, `revoke_private_note(id)`: revokes one of the agent's own current notes of
/// its `kind` in this channel.
#[derive(Debug)]
struct RevokeNoteTool {
    description: ToolDescription,
    kind: NoteKind,
}

#[async_trait]
impl Tool for RevokeNoteTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let id: NoteId = id_argument(params)?;
        if !AgentNotes::revoke(turn, self.kind, id) {
            return Err(missing(self.kind, id, "revoked"));
        }
        Ok(ToolOutput::of_turn(serde_json::json!({ "status": "revoked", "id": id.0 }), turn))
    }
}

/// `reclassify_note(id, level, compartments?)`, `reclassify_private_note(…)`: reclassifies one of
/// the agent's own current notes of its `kind` in this channel, as the policy allows.
#[derive(Debug)]
struct ReclassifyNoteTool {
    description: ToolDescription,
    kind: NoteKind,
}

#[async_trait]
impl Tool for ReclassifyNoteTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let id: NoteId = id_argument(params)?;
        let classification = classification_argument(params)?;
        AgentNotes::reclassify(turn, self.kind, id, classification.clone()).await?;
        Ok(ToolOutput::of_turn(
            serde_json::json!({ "status": "reclassified", "id": id.0, "classification": classification.to_string() }),
            turn,
        ))
    }
}

/// `read_notes(channel?)`: the notes of the channel named, or of every
/// channel that may be read, each read recorded as knowledge - classified
/// as the notes read (issue 168).
#[derive(Debug)]
struct ReadNotesTool {
    description: ToolDescription,
}

#[async_trait]
impl Tool for ReadNotesTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        #[derive(Deserialize)]
        struct Asked {
            #[serde(default)]
            channel: Option<String>,
        }
        let Asked { channel } = arguments(params)?;
        let Some(session) = turn.situation().session().await? else {
            return Err(ToolError::ExecutionFailed(
                "the turn is in no session to read from".to_string(),
            ));
        };
        let channels = match channel.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(name) => vec![session.channel_named(name).await?],
            None => session.channels().await,
        };
        let here = turn.situation().channel_id();
        let mut listed = Vec::new();
        let mut read = Vec::new();
        let mut knowledge = Vec::new();
        for channel in channels {
            let mut notes = Vec::new();
            for agent_notes in channel.notes().await? {
                knowledge.extend(notes_ref(channel.id(), channel.session().id(), &agent_notes));
                for note in agent_notes.notes {
                    notes.push(listed_note(&agent_notes.name, &note, channel.id() == here));
                    read.push(note.classification);
                }
            }
            listed.push(serde_json::json!({ "channel": channel.name(), "notes": notes }));
        }
        let value = serde_json::json!({ "channels": listed });
        Ok(ToolOutput {
            classification: Classification::join_all(&read),
            knowledge,
            ..value.into()
        })
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
fn notes_ref(
    channel: ChannelId,
    session: SessionId,
    published: &PublishedNotes,
) -> Option<KnowledgeRef> {
    let ActorId::Agent(agent) = published.agent else { return None };
    let classification =
        Classification::join_all(published.notes.iter().map(|n| &n.classification));
    Some(KnowledgeRef {
        channel,
        session,
        source: KnowledgeSource::Notes { agent, digest: published.digest },
        classification,
        private: false,
    })
}

#[async_trait]
impl PromptProvider for NotesComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let own = ActorId::Agent(situation.agent_id());
        let mut others = Vec::new();
        let published = match situation.channel().await {
            Ok(Some(channel)) => channel.notes().await,
            Ok(None) => Ok(Vec::new()),
            Err(e) => Err(e),
        };
        match published {
            Ok(published) => others = published,
            // The agent can still answer without the others' notes.
            Err(e) => {
                warn!("Reading the notes of channel {} failed: {}", situation.channel_id(), e)
            }
        }
        others.retain(|p| p.agent != own && !p.notes.is_empty());
        others.sort_by(|l, r| l.name.cmp(&r.name));
        let own_notes = AgentNotes::current(situation, NoteKind::Public);
        let text = render(
            &self.prompts,
            &own_notes,
            others.iter().map(|p| (p.name.as_str(), p.notes.as_slice())),
        )?;

        // What the part shows, as knowledge read (issue 17).
        let mut knowledge: Vec<KnowledgeRef> = others
            .iter()
            .filter_map(|p| notes_ref(situation.channel_id(), situation.session_id(), p))
            .collect();
        if !own_notes.is_empty() {
            let own_digest = AgentNotes::digest(situation, NoteKind::Public);
            let classification =
                Classification::join_all(own_notes.iter().map(|n| &n.classification));
            knowledge.push(KnowledgeRef {
                channel: situation.channel_id(),
                session: situation.session_id(),
                source: KnowledgeSource::Notes { agent: situation.agent_id(), digest: own_digest },
                classification,
                private: false,
            });
        }
        Ok(Prompt::of_knowledge(text, knowledge))
    }
}

/// The prompt part of `prompts` for `own` notes and those of `others` by
/// name, those without notes left out; empty if there are none.
fn render<'a>(
    prompts: &PromptTexts,
    own: &[Note],
    others: impl Iterator<Item = (&'a str, &'a [Note])>,
) -> Result<String, PromptError> {
    let numbered = |notes: &[Note]| {
        notes
            .iter()
            .map(|n| context! { id => n.id.to_string(), text => n.text })
            .collect::<Vec<_>>()
    };
    let others: Vec<_> = others
        .filter(|(_, notes)| !notes.is_empty())
        .map(|(name, notes)| context! { name, notes => numbered(notes) })
        .collect();
    if own.is_empty() && others.is_empty() {
        return Ok(String::new());
    }
    prompts.render(TEXT, context! { own => numbered(own), others, write => WRITE_NOTE_TOOL_NAME, update => UPDATE_NOTE_TOOL_NAME, revoke => REVOKE_NOTE_TOOL_NAME })
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(NOTES_KIND)
                .params::<PositionParams>()
                .provides::<dyn ToolProvider>()
                .provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let position =
                PositionParams::position_of(&descriptor.params, PromptPosition::BeforeTurnInput)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(tools(&texts)?));
            facets.insert::<dyn PromptProvider>(Arc::new(NotesComponent::new(position, texts)));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::AgentsLibrary;
    use crate::testing::{self, agent_runtime};
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::channel::testing::bare_channel;
    use caelumora_domain::channel::ThreadId;
    use caelumora_domain::message::MessageId;
    use caelumora_domain::session::testing::bare_session;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::work::Work;
    use caelumora_kernel::kernel::ParticipantRuntime;
    use caelumora_kernel::session::testing::detached_participant_in;
    use caelumora_kernel::testing::Discard;
    use caelumora_kernel::testing::TestKernel;
    use caelumora_kernel::testing::World;
    use caelumora_security::OwnershipPolicy;
    use chrono::Utc;
    use std::iter;

    /// The runtime of the actor of `agent`, in a channel of no running session, under the
    /// ownership policy: who may reclassify a note is decided by its authorship (issue 516).
    fn owned_runtime_of(agent: AgentId) -> ParticipantRuntime {
        let session = bare_session(SessionId::new());
        let channel = bare_channel(session.id);
        let kernel = TestKernel {
            access_policy: Arc::new(OwnershipPolicy::new()),
            libraries: vec![&AgentsLibrary],
            ..Default::default()
        }
        .build();
        detached_participant_in(kernel, session, channel, ActorId::Agent(agent))
    }

    /// A turn of `agent` under way in `state`, through the runtime of its actor under the
    /// ownership policy, which builds the context it acts in (issue 516).
    fn turn_of(agent: AgentId, state: &mut AgentChannelState) -> Turn<'_> {
        let runtime = agent_runtime(owned_runtime_of(agent));
        state.begin_turn(
            ThreadId::line(runtime.channel_id()),
            Work::new(Default::default(), None, Default::default(), Default::default()),
        );
        Turn::of(state, ThreadId::line(runtime.channel_id()), runtime).expect("a turn was begun")
    }

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
        assert_eq!(render(&testing::prompts(), &[], iter::empty()).unwrap(), "");
    }

    #[test]
    fn the_prompt_part_lists_own_notes_first_then_the_others() {
        let others = [note(1, "The user is called Kaya.")];
        let text = render(
            &testing::prompts(),
            &[note(2, "Answer in German.")],
            [("Sassy", &others[..])].into_iter(),
        )
        .unwrap();
        let own_at = text.find("#2: Answer in German.").unwrap();
        let others_at = text.find("Notes of Sassy:").unwrap();
        assert!(own_at < others_at, "{text}");
        assert!(text.contains("#1: The user is called Kaya."));
    }

    #[test]
    fn read_notes_numbers_only_the_notes_of_the_turns_channel() {
        let tea = note(3, "tea");
        assert_eq!(
            listed_note("Sassy", &tea, true),
            serde_json::json!({ "by": "Sassy", "id": 3, "text": "tea" })
        );
        assert_eq!(
            listed_note("Sassy", &tea, false),
            serde_json::json!({ "by": "Sassy", "text": "tea" })
        );
    }

    #[tokio::test]
    async fn a_note_is_reclassified_as_the_policy_allows_also_in_the_turn_it_was_noted() {
        use caelumora_domain::confidentiality::{Classification, Confidentiality};
        let agent = AgentId::new();
        let mut state = AgentChannelState::default();
        let tools = tools(&testing::prompts()).unwrap();
        // Asked in the context the kernel builds for the agent, the author of its own note.
        let mut turn = turn_of(agent, &mut state);
        testing::tool_named(&tools, WRITE_NOTE_TOOL_NAME)
            .execute(&serde_json::json!({ "text": "tea" }), None, &mut turn)
            .await
            .unwrap();
        let above = serde_json::json!({ "id": 1, "level": "strictly_confidential" });
        assert!(
            matches!(
                testing::tool_named(&tools, RECLASSIFY_NOTE_TOOL_NAME)
                    .execute(&above, None, &mut turn)
                    .await,
                Err(ToolError::PermissionDenied(_))
            ),
            "never above the channel"
        );
        let unknown = serde_json::json!({ "id": 2, "level": "public" });
        assert!(matches!(
            testing::tool_named(&tools, RECLASSIFY_NOTE_TOOL_NAME)
                .execute(&unknown, None, &mut turn)
                .await,
            Err(ToolError::InvalidParameter(_))
        ));
        let public_with_compartments =
            serde_json::json!({ "id": 1, "level": "public", "compartments": ["hr"] });
        assert!(matches!(
            testing::tool_named(&tools, RECLASSIFY_NOTE_TOOL_NAME)
                .execute(&public_with_compartments, None, &mut turn)
                .await,
            Err(ToolError::InvalidParameter(_))
        ));
        testing::tool_named(&tools, RECLASSIFY_NOTE_TOOL_NAME)
            .execute(&serde_json::json!({ "id": 1, "level": "public" }), None, &mut turn)
            .await
            .expect("released");
        turn.commit(false, &Discard).await;
        assert_eq!(
            state.notes.current()[0].classification,
            Classification::general(Confidentiality::Public)
        );
    }

    #[tokio::test]
    async fn notes_are_published_when_the_turn_completes_and_only_the_author_revokes() {
        let mut state = AgentChannelState::default();
        let tools = tools(&testing::prompts()).unwrap();
        {
            let mut turn = Turn::begun(AgentId::new(), &mut state);
            let noted = testing::tool_named(&tools, WRITE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "text": "tea" }), None, &mut turn)
                .await
                .unwrap();
            assert_eq!(noted.value["id"], 1);
            assert!(
                AgentNotes::current(&turn.situation(), NoteKind::Public).is_empty(),
                "not before the turn completes"
            );
            let unknown = testing::tool_named(&tools, REVOKE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "id": 7 }), None, &mut turn)
                .await;
            assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))), "not one of its notes");
            assert!(
                turn.commit(false, &Discard).await.channel_state_changed,
                "the turn changed notes"
            );
            assert_eq!(
                state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
                ["tea"]
            );
        }
        {
            let mut turn = Turn::begun(AgentId::new(), &mut state);
            testing::tool_named(&tools, REVOKE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "id": 1 }), None, &mut turn)
                .await
                .unwrap();
            testing::tool_named(&tools, WRITE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "text": "coffee" }), None, &mut turn)
                .await
                .unwrap();
            turn.commit(false, &Discard).await;
            let notes = state.notes.current();
            assert_eq!(
                notes.iter().map(|n| (n.id.0, n.text.as_str())).collect::<Vec<_>>(),
                [(2, "coffee")],
                "a new number for the new version"
            );
        }
        {
            // A turn that does not complete publishes nothing: the next one
            // starts without its changes.
            let mut turn = Turn::begun(AgentId::new(), &mut state);
            testing::tool_named(&tools, WRITE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "text": "lost" }), None, &mut turn)
                .await
                .unwrap();
            let turn = Turn::begun(AgentId::new(), &mut state);
            assert!(!turn.commit(false, &Discard).await.channel_state_changed);
        }
    }

    #[tokio::test]
    async fn an_updated_note_keeps_its_number_and_is_classified_as_the_turn_that_updated_it() {
        use caelumora_domain::confidentiality::{Classification, Confidentiality};
        let mut state = AgentChannelState::default();
        let tools = tools(&testing::prompts()).unwrap();
        let texts = |state: &AgentChannelState| {
            state.notes.current().into_iter().map(|n| (n.id.0, n.text)).collect::<Vec<_>>()
        };
        {
            let mut turn = Turn::begun(AgentId::new(), &mut state);
            testing::tool_named(&tools, WRITE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "text": "tea" }), None, &mut turn)
                .await
                .unwrap();
            testing::tool_named(&tools, WRITE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "text": "Berlin" }), None, &mut turn)
                .await
                .unwrap();
            let updated = testing::tool_named(&tools, UPDATE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "id": 2, "text": "Hamburg" }), None, &mut turn)
                .await
                .expect("noted in this turn");
            assert_eq!(updated.value, serde_json::json!({ "status": "updated", "id": 2 }));
            testing::tool_named(&tools, RECLASSIFY_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "id": 1, "level": "public" }), None, &mut turn)
                .await
                .expect("released");
            turn.commit(false, &Discard).await;
            assert_eq!(texts(&state), [(1, "tea".to_string()), (2, "Hamburg".to_string())]);
        }
        let mut turn = Turn::begun(AgentId::new(), &mut state);
        let internal = Classification::general(Confidentiality::Internal);
        turn.see(&internal);
        let unknown = testing::tool_named(&tools, UPDATE_NOTE_TOOL_NAME)
            .execute(&serde_json::json!({ "id": 7, "text": "x" }), None, &mut turn)
            .await;
        assert!(matches!(unknown, Err(ToolError::InvalidParameter(_))), "not one of its notes");
        let empty = testing::tool_named(&tools, UPDATE_NOTE_TOOL_NAME)
            .execute(&serde_json::json!({ "id": 1, "text": " " }), None, &mut turn)
            .await;
        assert!(matches!(empty, Err(ToolError::InvalidParameter(_))), "a note says something");
        testing::tool_named(&tools, UPDATE_NOTE_TOOL_NAME)
            .execute(&serde_json::json!({ "id": 1, "text": "coffee" }), None, &mut turn)
            .await
            .unwrap();
        testing::tool_named(&tools, REVOKE_NOTE_TOOL_NAME)
            .execute(&serde_json::json!({ "id": 2 }), None, &mut turn)
            .await
            .unwrap();
        let revoked = testing::tool_named(&tools, UPDATE_NOTE_TOOL_NAME)
            .execute(&serde_json::json!({ "id": 2, "text": "Munich" }), None, &mut turn)
            .await;
        assert!(
            matches!(revoked, Err(ToolError::InvalidParameter(_))),
            "a revoked note is not updated"
        );
        let noted = testing::tool_named(&tools, WRITE_NOTE_TOOL_NAME)
            .execute(&serde_json::json!({ "text": "Munich" }), None, &mut turn)
            .await
            .unwrap();
        assert_eq!(noted.value["id"], 3, "an update uses up no number");
        turn.commit(false, &Discard).await;
        let notes = state.notes.current();
        assert_eq!(texts(&state), [(1, "coffee".to_string()), (3, "Munich".to_string())]);
        assert_eq!(notes[0].classification, internal, "the release was of the old text");
    }

    #[tokio::test]
    async fn a_note_revoked_in_the_turn_that_noted_it_is_gone_and_its_number_used_up() {
        let mut state = AgentChannelState::default();
        let tools = tools(&testing::prompts()).unwrap();
        {
            let mut turn = Turn::begun(AgentId::new(), &mut state);
            testing::tool_named(&tools, WRITE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "text": "draft" }), None, &mut turn)
                .await
                .unwrap();
            testing::tool_named(&tools, REVOKE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "id": 1 }), None, &mut turn)
                .await
                .expect("noted in this turn");
            let again = testing::tool_named(&tools, REVOKE_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "id": 1 }), None, &mut turn)
                .await;
            assert!(matches!(again, Err(ToolError::InvalidParameter(_))), "revoked already");
            let revoked = testing::tool_named(&tools, RECLASSIFY_NOTE_TOOL_NAME)
                .execute(&serde_json::json!({ "id": 1, "level": "public" }), None, &mut turn)
                .await;
            assert!(
                matches!(revoked, Err(ToolError::InvalidParameter(_))),
                "a revoked note is not reclassified"
            );
            turn.commit(false, &Discard).await;
            assert!(state.notes.current().is_empty());
        }
        let mut turn = Turn::begun(AgentId::new(), &mut state);
        let noted = testing::tool_named(&tools, WRITE_NOTE_TOOL_NAME)
            .execute(&serde_json::json!({ "text": "final" }), None, &mut turn)
            .await
            .unwrap();
        assert_eq!(noted.value["id"], 2);
    }

    #[test]
    fn a_note_acted_on_is_shown_as_a_note_and_the_notes_read_as_a_list() {
        let tools: Vec<ToolDescription> = tools(&testing::prompts())
            .unwrap()
            .tools()
            .iter()
            .map(|t| t.description().clone())
            .collect();
        let notes: Vec<_> = tools
            .iter()
            .filter(|t| t.presentation == NOTE_PRESENTATION)
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(
            notes,
            [
                WRITE_NOTE_TOOL_NAME,
                UPDATE_NOTE_TOOL_NAME,
                REVOKE_NOTE_TOOL_NAME,
                RECLASSIFY_NOTE_TOOL_NAME
            ]
        );
        let read = tools.iter().find(|t| t.name == READ_NOTES_TOOL_NAME).unwrap();
        assert_eq!(read.presentation, NOTE_LIST_PRESENTATION);
    }

    /// Issue 538: what `read_notes` read it gives with its output, for the executor to record:
    /// the notes of each agent of each channel it read, by that channel and its session.
    #[tokio::test]
    async fn what_it_read_is_given_with_its_output_by_the_channel_read() {
        let world = testing::world().await;
        let personality = world.personality("plain", None, &[]).await;
        let helper = world.agent("helper", world.alice, &personality).await;
        let session = world.shared_session(&[helper], &[]).await;
        let main = &session.channels()[0];
        let tools = tools(&testing::prompts()).unwrap();
        let mut state = AgentChannelState::default();
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);

        let read = testing::tool_named(&tools, READ_NOTES_TOOL_NAME)
            .execute(&serde_json::json!({}), None, &mut turn)
            .await
            .unwrap();
        assert!(
            matches!(read.knowledge.as_slice(), [k] if k.channel == main.id() && k.session == session.id() && k.source == KnowledgeSource::Notes { agent: helper, digest: None }),
            "the helper's notes of main, none yet: {:?}",
            read.knowledge
        );
        drop(turn);
        world.kernel.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
    }
}
