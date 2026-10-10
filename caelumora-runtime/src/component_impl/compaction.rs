//! The `compaction` component (issue 16): an agent's context replaced by a
//! shorter one when it grows too long for its model.
//!
//! - **`/compact @agent`**, a command the agent carries out itself (issue
//!   269), for its owners alone: the older part of its transcript is
//!   summarised by its own model and replaced by the summary; the newest
//!   messages - at least `keep_last`, from the start of the turn the oldest
//!   of them belongs to - are kept as they stand; the skills loaded in the
//!   part summarised are given again, as they are now.
//!
//! The context is replaced, not rewritten: the transcript begins anew on a
//! branch of its own, and what the agent saw before stays recorded and
//! resolvable (`AgentChannelState::compact`). Where the cut is made and what
//! replaces the older messages is this component's; the mechanism is the
//! state's. Compacting on the agent's own is issue 284.

use crate::agent::turn::Turn;
use crate::command::{AgentCommandProvider, CommandError, CommandResult, ProvidesCommands};
use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::context::titled;
use crate::llm::LLMClient;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::command::{Command, CommandOutput};
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::digest::Digest;
use caelumora_model::message::{Message, MessageEvent, MessageId, MessageRole};
use caelumora_model::message_history::{HistoryEntry, MessageHistory};
use caelumora_model::skill::SkillId;
use caelumora_model::transcript::{as_they_stand, TranscriptEntry};
use futures::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

pub const COMPACTION_KIND: &str = "compaction";
pub const COMPACT_COMMAND_NAME: &str = "compact";

/// How many messages a compaction keeps as they stand, unless configured.
const DEFAULT_KEEP_LAST: usize = 10;

const SUMMARY_TITLE: &str = "Summary of the conversation so far";

/// What the model is asked to summarise the older messages for: going on
/// from the summary alone.
const SUMMARY_INSTRUCTION: &str = "Summarise the conversation above for yourself, so that you can go on from the summary alone: \
    what the task is and who asked for it, what was decided and agreed, what was done - with which files, tools and results \
    that still matter -, and what is still open. Keep names, numbers and identifiers exact. Leave out the text of skills: \
    they are given to you again. Answer with the summary only.";

#[derive(Debug)]
pub struct CompactionComponent {
    /// The agent's own model, which writes the summary.
    llm_client: Arc<dyn LLMClient>,
    /// How many of the newest messages are kept as they stand.
    keep_last: usize,
}

/// What the component is configured with.
#[derive(Debug, Deserialize)]
struct CompactionConfig {
    #[serde(default = "default_keep_last")]
    keep_last: usize,
}

fn default_keep_last() -> usize {
    DEFAULT_KEEP_LAST
}

/// An entry of the active path of a transcript, as a compaction reads it:
/// its digest, the skill it gives if it is one given (`TranscriptEntry::Given`),
/// and its message as it stands, if it holds one.
type Entry = (Digest, Option<SkillId>, Option<Message>);

/// Where the entries `path` are cut so that at least the last `keep_last`
/// messages are kept: the index of the first entry kept - `path.len()` to
/// keep none -, moved back to the first message of the turn the oldest kept
/// message belongs to, so that no turn is cut apart - no tool call from its
/// result, no message from its turn's end. `None` if nothing would be left
/// to summarise.
fn cut_at(path: &[Entry], keep_last: usize) -> Option<usize> {
    let standing: Vec<usize> = path.iter().enumerate().filter(|(_, (_, _, message))| message.is_some()).map(|(at, _)| at).collect();
    let cut = match keep_last {
        0 => path.len(),
        keep => {
            let first_kept = *standing.get(standing.len().checked_sub(keep)?)?;
            match path[first_kept].2.as_ref().and_then(|m| m.turn_id) {
                Some(turn) => path.iter().position(|(_, _, m)| m.as_ref().is_some_and(|m| m.turn_id == Some(turn))).unwrap_or(first_kept),
                None => first_kept,
            }
        }
    };
    standing.iter().any(|&at| at < cut).then_some(cut)
}

/// The skills loaded in `entries`, each once, in the order they were first
/// loaded: each given (`TranscriptEntry::Given`), and each a turn read -
/// recorded as knowledge with what it said afterwards (`load_skill`), as
/// `read` holds it by message.
fn skills_in(entries: &[Entry], read: &HashMap<MessageId, Vec<SkillId>>) -> Vec<SkillId> {
    let mut skills = Vec::new();
    for (_, given, message) in entries {
        let read = message.as_ref().and_then(|m| read.get(&m.id)).into_iter().flatten();
        for skill in given.iter().chain(read) {
            if !skills.contains(skill) {
                skills.push(*skill);
            }
        }
    }
    skills
}

/// The skills each message of `history` was produced after reading, by
/// message: what its context records as knowledge (`KnowledgeSource::Skill`).
fn skills_read(history: &MessageHistory) -> HashMap<MessageId, Vec<SkillId>> {
    history.all_entries()
        .filter_map(|node| match &node.entry {
            HistoryEntry::Said { message, context: Some(context) } => Some((message.id, context.knowledge.iter().filter_map(|k| match k.source {
                KnowledgeSource::Skill { skill, .. } => Some(skill),
                _ => None,
            }).collect::<Vec<_>>())),
            _ => None,
        })
        .filter(|(_, skills)| !skills.is_empty())
        .collect()
}

impl CompactionComponent {
    /// The summary of the transcript up to and including `last`, written by
    /// the agent's model: its text, and the message its model answered with,
    /// for the tokens of the call.
    async fn summarise(&self, turn: &mut Turn<'_>, last: Digest) -> CommandResult<(String, Message)> {
        let failed = |reason: &str| CommandError::Failed { command: COMPACT_COMMAND_NAME.to_string(), reason: reason.to_string() };
        let mut input = turn.context_until(last).map_err(|e| {
            tracing::warn!("Agent {}: /{} cannot read its transcript: {:#}", turn.agent_id(), COMPACT_COMMAND_NAME, e);
            failed("the transcript could not be read")
        })?;
        input.push(Message {
            role: MessageRole::User,
            content: Some(SUMMARY_INSTRUCTION.to_string()),
            sender_id: ActorId::Agent(turn.agent_id()),
            sender_name: turn.agent_name().to_string(),
            ..Message::new(turn.classification())
        });
        if !turn.count_model_call() {
            return Err(failed("the agent made all the calls of its model it may in one turn"));
        }
        let mut stream = self.llm_client.chat_completion(&input, &[]).await;
        let mut answered: Vec<Message> = Vec::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(MessageEvent::Complete(message)) => answered.push(message),
                Ok(MessageEvent::Partial(_) | MessageEvent::EndOfTurn(_)) => {}
                Err(e) => {
                    tracing::warn!("Agent {}: /{} could not ask its model: {}", turn.agent_id(), COMPACT_COMMAND_NAME, e);
                    return Err(failed("its model could not be asked"));
                }
            }
        }
        let text = answered.iter().filter_map(|m| m.content.as_deref()).map(str::trim).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
        match answered.pop() {
            Some(last) if !text.is_empty() => Ok((text, last)),
            _ => Err(failed("its model wrote no summary")),
        }
    }
}

impl ProvidesCommands for CompactionComponent {
    fn commands(&self) -> Vec<Command> {
        vec![Command {
            name: COMPACT_COMMAND_NAME.to_string(),
            description: "Compacts the agent's context: the older part of its conversation is replaced by a summary, the newest is kept. What it saw before stays recorded.".to_string(),
            usage: None,
            // Like approving its calls (issue 269): its owners'.
            owners_only: true,
        }]
    }
}

#[async_trait]
impl AgentCommandProvider for CompactionComponent {
    async fn execute(&self, name: &str, args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        if name != COMPACT_COMMAND_NAME {
            return Err(CommandError::Unknown(name.to_string()));
        }
        let invalid = |reason: &str| CommandError::InvalidArguments { command: name.to_string(), reason: reason.to_string() };
        let failed = |reason: &str| CommandError::Failed { command: name.to_string(), reason: reason.to_string() };
        if !args.trim().is_empty() {
            return Err(invalid("it takes no arguments"));
        }
        let agent = ActorId::Agent(turn.agent_id());
        let path: Vec<Entry> = as_they_stand(turn.agent_channel_state().transcript.entries())
            .map(|(node, message)| {
                let given = match &node.entry {
                    TranscriptEntry::Given { source: KnowledgeSource::Skill { skill, .. }, .. } => Some(*skill),
                    _ => None,
                };
                (node.digest, given, message)
            })
            .collect();
        let cut = cut_at(&path, self.keep_last)
            .ok_or_else(|| invalid(&format!("there is nothing to compact: {} keeps its last {} messages as they are", turn.agent_name(), self.keep_last)))?;
        let (summarised, kept) = path.split_at(cut);
        let count = |entries: &[Entry]| entries.iter().filter(|(_, _, m)| m.is_some()).count();
        let keep_from = kept.first().map(|(digest, _, _)| *digest);
        let last_summarised = summarised.last().map(|(digest, _, _)| *digest).ok_or_else(|| failed("nothing was found to summarise"))?;

        // Skills still there as they were loaded are not given twice.
        let read = skills_read(&turn.agent_channel_state().history);
        let still_there = skills_in(kept, &read);
        let wanted: Vec<SkillId> = skills_in(summarised, &read).into_iter().filter(|s| !still_there.contains(s)).collect();
        let usable = match (wanted.is_empty(), turn.session()) {
            (true, _) => Vec::new(),
            (false, Some(session)) => session.views().usable_skills().await.map_err(|e| {
                tracing::warn!("Agent {}: /{} could not read the skills: {:#}", turn.agent_id(), name, e);
                failed("the skills could not be read")
            })?,
            (false, None) => return Err(failed("the agent is in no session")),
        };
        let skills: Vec<_> = wanted.iter().filter_map(|id| usable.iter().find(|s| s.id() == *id)).collect();

        let (text, answer) = self.summarise(turn, last_summarised).await?;
        let summary = Message {
            id: MessageId::new(),
            sender_id: agent,
            sender_name: turn.agent_name().to_string(),
            role: MessageRole::User,
            content: Some(titled(SUMMARY_TITLE, &text)),
            timestamp: chrono::Utc::now(),
            prompt_tokens: answer.prompt_tokens,
            completion_tokens: answer.completion_tokens,
            context_size: answer.context_size,
            // Written from all the turn had seen, which bounds it.
            ..Message::new(turn.label())
        };
        let mut replacement: Vec<(Message, Option<KnowledgeRef>)> = vec![(summary, None)];
        for skill in &skills {
            let given = Message {
                id: MessageId::new(),
                sender_id: agent,
                sender_name: turn.agent_name().to_string(),
                role: MessageRole::User,
                content: Some(skill.as_given()),
                timestamp: chrono::Utc::now(),
                ..Message::new(skill.classification().clone())
            };
            replacement.push((given, Some(skill.knowledge(turn.channel_id(), turn.session_id()))));
        }
        // Nothing can fail from here on: the transcript changes at once.
        turn.compact(replacement, keep_from).ok_or_else(|| failed("the context could not be replaced"))?;

        let mut answer = format!("{} compacted its context: {} messages summarised, {} kept as they were.", turn.agent_name(), count(summarised), count(kept));
        if !skills.is_empty() {
            answer.push_str(&format!(" Skills given again: {}.", skills.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")));
        }
        if skills.len() < wanted.len() {
            answer.push_str(&format!(" {} skill(s) it had loaded can no longer be loaded here and were not given again.", wanted.len() - skills.len()));
        }
        Ok(CommandOutput::channel(answer))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(COMPACTION_KIND).provides::<dyn AgentCommandProvider>(),
        |descriptor, context| async move {
            let CompactionConfig { keep_last } = crate::component::config_of_params(&descriptor.params)?;
            // The agent's own model writes the summary.
            let llm_client = context.one::<dyn LLMClient>().await?.facet;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn AgentCommandProvider>(Arc::new(CompactionComponent { llm_client, keep_last }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_impl::skills::{SkillsComponent, SKILL_COMMAND_NAME};
    use crate::context::PromptPosition;
    use crate::tool::ApprovalMode;
    use crate::workspace::testing::{session_with_workspace, WithWorkspace};
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::confidentiality::{Classification, Confidentiality};
    use caelumora_model::scope::Scope;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::tool::Tool;
    use std::sync::Mutex;

    /// A model that summarises everything as `"They want release notes."`,
    /// keeping what it was given.
    #[derive(Debug, Default)]
    struct Summariser(Mutex<Vec<Vec<Message>>>);

    #[async_trait]
    impl LLMClient for Summariser {
        async fn chat_completion(&self, context: &[Message], _: &[Tool]) -> futures::stream::BoxStream<'static, crate::llm::LLMResult<MessageEvent>> {
            self.0.lock().unwrap().push(context.to_vec());
            let answer = Message {
                role: MessageRole::Assistant,
                content: Some("They want release notes.".to_string()),
                prompt_tokens: Some(40),
                completion_tokens: Some(8),
                ..crate::llm::unlabelled_message()
            };
            futures::stream::iter(vec![Ok(MessageEvent::Complete(answer))]).boxed()
        }

        fn profile(&self) -> caelumora_model::context::ModelProfile {
            Default::default()
        }
    }

    fn entry(content: &str, turn: Option<caelumora_model::work::TurnId>) -> Entry {
        let message = Message { content: Some(content.to_string()), turn_id: turn, ..Message::new(Classification::PUBLIC) };
        (caelumora_model::digest::content(content.as_bytes()), None, Some(message))
    }

    #[test]
    fn the_cut_keeps_the_last_messages_and_never_parts_a_turn() {
        let turn = caelumora_model::work::TurnId::new();
        let path = vec![entry("u1", None), entry("call", Some(turn)), entry("result", Some(turn)), (caelumora_model::digest::content(b"end"), None, None), entry("u2", None)];
        assert_eq!(cut_at(&path, 1), Some(4), "the last message alone");
        assert_eq!(cut_at(&path, 2), Some(1), "the result is kept, so is the call of its turn");
        assert_eq!(cut_at(&path, 0), Some(5), "none kept");
        assert_eq!(cut_at(&path, 4), None, "everything kept: nothing to summarise");
        assert_eq!(cut_at(&path, 3), Some(1), "the call is the oldest kept: the first message alone is summarised");
    }

    /// The world's agent heard `u1`, was given the skill `release-notes`
    /// with `/skill`, answered `a1` in a turn of its own, then heard `u2`.
    async fn an_agent_with_a_skill_given(world: &WithWorkspace, state: &mut AgentChannelState) -> SkillId {
        let internal = Classification::general(Confidentiality::Internal);
        let skill = world.runtime.skill_manager()
            .create(&SecurityContext::system(), "release-notes", Scope::GlobalScope, "How we write a release note", "1. Collect.", internal).await.unwrap();
        let heard = |text: &str| Message { content: Some(text.to_string()), ..Message::new(Classification::PUBLIC) };
        let u1 = heard("u1");
        state.perceive(vec![u1.clone()], vec![u1]);
        let skills = SkillsComponent { position: PromptPosition::Start, requires_approval: ApprovalMode::Always };
        let mut turn = world.turn(state);
        AgentCommandProvider::execute(&skills, SKILL_COMMAND_NAME, "release-notes", &mut turn).await.expect("given");
        drop(turn);
        let given = state.turn.take().unwrap().work.turn_id;
        state.finish_turn(given, ActorId::Agent(world.agent), Vec::new(), Vec::new(), None);
        drop(world.turn(state));
        state.said(Message { sender_id: ActorId::Agent(world.agent), role: MessageRole::Assistant, ..heard("a1") });
        let answered = state.turn.take().unwrap().work.turn_id;
        state.finish_turn(answered, ActorId::Agent(world.agent), Vec::new(), Vec::new(), None);
        let u2 = heard("u2");
        state.perceive(vec![u2.clone()], vec![u2]);
        skill.id
    }

    #[tokio::test]
    async fn compact_replaces_the_older_messages_by_a_summary_and_gives_the_skills_again() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let skill = an_agent_with_a_skill_given(&world, &mut state).await;
        let before = state.transcript.digest().unwrap();
        let model = Arc::new(Summariser::default());
        let component = CompactionComponent { llm_client: Arc::<Summariser>::clone(&model), keep_last: 1 };

        let mut turn = world.turn(&mut state);
        let answer = AgentCommandProvider::execute(&component, COMPACT_COMMAND_NAME, "", &mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(answer.text, "Jonathan compacted its context: 3 messages summarised, 1 kept as they were. Skills given again: release-notes.");
        assert_eq!(answer.visibility, caelumora_model::command::CommandVisibility::Channel, "everyone should know");
        let given = model.0.lock().unwrap().pop().expect("its model was asked");
        let texts: Vec<_> = given.iter().filter_map(|m| m.content.clone()).collect();
        assert!(texts.iter().any(|t| t == "u1") && texts.iter().any(|t| t == "a1") && !texts.iter().any(|t| t == "u2"), "the older messages, not those kept: {texts:?}");
        assert_eq!(texts.last().map(String::as_str), Some(SUMMARY_INSTRUCTION), "asked for the summary last");

        let now: Vec<_> = state.transcript.messages().into_iter().map(|m| m.content.unwrap_or_default()).collect();
        assert_eq!(now, ["[Summary of the conversation so far]\nThey want release notes.\n", "[Skill: release-notes]\n1. Collect.\n", "u2"]);
        assert!(matches!(state.transcript.entries().nth(1).map(|n| &n.entry), Some(TranscriptEntry::Given { source: KnowledgeSource::Skill { skill: s, .. }, .. }) if *s == skill), "the skill as given");
        assert!(state.transcript.prefix(&before).is_some(), "what it saw before stays");
        let knowledge = &state.turn.as_ref().unwrap().work.context.knowledge;
        assert!(knowledge.iter().any(|k| matches!(k.source, KnowledgeSource::Skill { skill: s, .. } if s == skill)), "the turn has read it");
        let usage = state.usage(ActorId::Agent(world.agent)).expect("the summary's call");
        assert_eq!(usage.last_call_tokens, 48);
    }

    #[tokio::test]
    async fn compact_refuses_what_it_cannot_shorten_and_changes_nothing() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        an_agent_with_a_skill_given(&world, &mut state).await;
        let before = state.transcript.digest();
        let model = Arc::new(Summariser::default());
        let component = CompactionComponent { llm_client: Arc::<Summariser>::clone(&model), keep_last: 10 };

        let mut turn = world.turn(&mut state);
        let refused = AgentCommandProvider::execute(&component, COMPACT_COMMAND_NAME, "", &mut turn).await;
        assert!(matches!(refused, Err(CommandError::InvalidArguments { ref reason, .. }) if reason.contains("nothing to compact")), "{refused:?}");
        let refused = AgentCommandProvider::execute(&component, COMPACT_COMMAND_NAME, "hard", &mut turn).await;
        assert!(matches!(refused, Err(CommandError::InvalidArguments { .. })), "no arguments");
        drop(turn);
        assert!(model.0.lock().unwrap().is_empty(), "its model was not asked");
        assert_eq!(state.transcript.digest(), before);
    }
}
