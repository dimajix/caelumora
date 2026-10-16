//! The `summary_compaction` component: the older part of an agent's
//! transcript summarised by its own model and replaced by the summary. The
//! default compaction strategy of an agent.
//!
//! The newest part of the context - `keep`, a share of the model's context
//! size - is kept as it stands; the cut lies before a message, never between
//! a tool's call and its result (issue 284). What the agent was given in the
//! part summarised - a skill loaded - is given again, as it is now, by the
//! component that gave it (`KnowledgeRestorer`, issue 323).

use crate::turn::Turn;
use crate::compaction::{Compaction, CompactionError, CompactionStrategy, GivenAgain, KnowledgeRestorer};
use caelumora_runtime::component::{ComponentError, ComponentFacets, ComponentKind, ComponentRegistry, config_of_params};
use crate::prompt::titled;
use crate::llm::LLMClient;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::digest::Digest;
use caelumora_model::message::{Message, MessageEvent, MessageId, MessageRole};
use caelumora_model::store::EntityKind;
use caelumora_model::transcript::{as_they_stand, TranscriptEntry};
use caelumora_model::turn::GivenUpReason;
use caelumora_model::work::TurnId;
use futures::StreamExt;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;
use chrono::Utc;

pub const SUMMARY_COMPACTION_KIND: &str = "summary_compaction";

/// How much of the context a compaction keeps as it stands, as a share of
/// its model's context size, unless configured.
const DEFAULT_KEEP: f64 = 0.25;

const SUMMARY_TITLE: &str = "Summary of the conversation so far";

/// What the model is asked to summarise the older messages for: going on
/// from the summary alone.
const SUMMARY_INSTRUCTION: &str = "Summarise the conversation above for yourself, so that you can go on from the summary alone: \
    what the task is and who asked for it, what was decided and agreed, what was done - with which files, tools and results \
    that still matter -, and what is still open. Keep names, numbers and identifiers exact. Answer with the summary only.";

#[derive(Debug)]
pub struct SummaryCompaction {
    /// The agent's own model, which writes the summary.
    llm_client: Arc<dyn LLMClient>,
    /// How much of the newest context is kept as it stands, as a share of
    /// the context size - of the context as it stands where no size is
    /// known.
    keep: f64,
    /// The components of the agent that give again what they gave it, each
    /// for its kind.
    restorers: Vec<Arc<dyn KnowledgeRestorer>>,
}

/// What the component is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SummaryCompactionConfig {
    #[serde(default = "default_keep")]
    keep: f64,
}

fn default_keep() -> f64 {
    DEFAULT_KEEP
}

/// What the agent was given: an entity of a kind (`KnowledgeSource::Entity`).
type Given = (EntityKind, Uuid);

/// An entry of the active path of a transcript, as a compaction reads it:
/// its digest, what it gives if it is one given (`TranscriptEntry::Given`),
/// and its message as it stands, if it holds one.
type Entry = (Digest, Option<Given>, Option<Message>);

/// What `source` gave, if it is knowledge of an entity.
fn given(source: &KnowledgeSource) -> Option<Given> {
    match *source {
        KnowledgeSource::Entity { kind, id, .. } => Some((kind, id)),
        _ => None,
    }
}

/// Where the entries `path` are cut, standing in the context at
/// `positions` (`AgentChannelState::token_positions`): the index of the
/// first entry kept. Before a message only, so that a tool's result stays
/// with its call: within the turn under way `under_way` before one of its
/// model's messages but the first - before that, its input would be
/// summarised and the turn kept -, before it before any message that is no
/// tool's result, up to the turn's input, and none in a turn of `whole`.
/// The oldest of them behind which at most `keep` tokens stand; the newest
/// if none is. `None` if there is none, or nothing before it to summarise.
fn cut_at(path: &[Entry], positions: &[u64], keep: u64, under_way: TurnId, whole: &HashSet<TurnId>) -> Option<usize> {
    let end = positions.last().copied().unwrap_or(0);
    let of_turn = |m: &Message| m.turn_id == Some(under_way);
    let turn_start = path.iter().position(|(_, _, m)| m.as_ref().is_some_and(of_turn));
    let first_call = path.iter().position(|(_, _, m)| m.as_ref().is_some_and(|m| of_turn(m) && m.role == MessageRole::Assistant));
    let candidates: Vec<usize> = path.iter()
        .enumerate()
        .filter(|(at, (_, _, message))| match message {
            None => false,
            Some(m) if of_turn(m) => m.role == MessageRole::Assistant && Some(*at) != first_call,
            Some(m) => m.role != MessageRole::Tool
                && turn_start.is_none_or(|start| *at < start)
                && m.turn_id.is_none_or(|turn| !whole.contains(&turn)),
        })
        .map(|(at, _)| at)
        .collect();
    let cut = candidates.iter().copied()
        .find(|&at| end.saturating_sub(positions.get(at).copied().unwrap_or(end)) <= keep)
        .or(candidates.last().copied())?;
    path[..cut].iter().any(|(_, _, message)| message.is_some()).then_some(cut)
}

/// The turns of `entries` a compaction must not cut apart: those given up
/// because the channel was lowered below them - its model is given only
/// the tools they called, which the summary of a part of one without its
/// end would not know (issues 141, 258).
fn kept_whole<'a>(entries: impl Iterator<Item = &'a TranscriptEntry>) -> HashSet<TurnId> {
    entries
        .filter_map(|entry| match entry {
            TranscriptEntry::TurnGivenUp { turn_id, reason: GivenUpReason::Lowered { .. }, .. } => Some(*turn_id),
            _ => None,
        })
        .collect()
}

/// What was given in `entries`, each once, in the order it was first
/// given (`TranscriptEntry::Given`): a skill a user gave and one the model
/// loaded alike (issue 389).
fn given_in(entries: &[Entry]) -> Vec<Given> {
    let mut given = Vec::new();
    for what in entries.iter().filter_map(|(_, gives, _)| gives.as_ref()) {
        if !given.contains(what) {
            given.push(*what);
        }
    }
    given
}

impl SummaryCompaction {
    /// How many tokens of a context of `size` are kept as they stand.
    fn keep_of(&self, size: u64) -> u64 {
        // A share of a token count: rounded down, what is lost is a token.
        (self.keep * size as f64) as u64
    }

    /// What the model is asked to summarise for: `SUMMARY_INSTRUCTION`, and
    /// to leave out what is given again.
    fn instruction(&self) -> String {
        let kinds: Vec<_> = self.restorers.iter().map(|r| r.kind().to_string()).collect();
        match kinds.is_empty() {
            true => SUMMARY_INSTRUCTION.to_string(),
            false => format!("{} Leave out the text of anything given to you as a {}: it is given to you again.", SUMMARY_INSTRUCTION, kinds.join(" or ")),
        }
    }

    /// What `wanted` - given in the part summarised and not in the part
    /// kept - is given again by the restorer of its kind, with that kind,
    /// and how much of it can no longer be given. What no restorer gives -
    /// a workspace's files - is not given again, and not lost either.
    async fn give_again(&self, turn: &mut Turn<'_>, wanted: &[Given]) -> Result<(Vec<(EntityKind, GivenAgain)>, usize), CompactionError> {
        let mut restored = Vec::new();
        let mut lost = 0;
        for restorer in &self.restorers {
            let kind = restorer.kind();
            let ids: Vec<Uuid> = wanted.iter().filter(|(of, _)| *of == kind).map(|(_, id)| *id).collect();
            if ids.is_empty() {
                continue;
            }
            let given = restorer.give_again(turn, &ids).await?;
            lost += ids.len().saturating_sub(given.len());
            restored.extend(given.into_iter().map(|given| (kind, given)));
        }
        Ok((restored, lost))
    }

    /// The summary of the transcript up to and including `last`, written by
    /// the agent's model: its text, and the message its model answered with,
    /// for the tokens of the call.
    async fn summarise(&self, turn: &mut Turn<'_>, last: Digest) -> Result<(String, Message), CompactionError> {
        let mut input = turn.context_until(last).map_err(|e| {
            tracing::warn!("Agent {}: a compaction cannot read its transcript: {:#}", turn.agent_id(), e);
            CompactionError::TranscriptUnreadable
        })?;
        input.push(Message {
            role: MessageRole::User,
            content: Some(self.instruction()),
            sender_id: ActorId::Agent(turn.agent_id()),
            sender_name: turn.agent_name().to_string(),
            ..Message::new(turn.classification())
        });
        let mut stream = self.llm_client.chat_completion(&input, &[]).await;
        let mut answered: Vec<Message> = Vec::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(MessageEvent::Complete(message)) => answered.push(message),
                Ok(MessageEvent::Partial(_) | MessageEvent::EndOfTurn(_)) => {}
                Err(e) => {
                    tracing::warn!("Agent {}: a compaction could not ask its model: {}", turn.agent_id(), e);
                    return Err(CompactionError::ModelUnreachable);
                }
            }
        }
        let text = answered.iter().filter_map(|m| m.content.as_deref()).map(str::trim).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
        match answered.pop() {
            Some(last) if !text.is_empty() => Ok((text, last)),
            _ => Err(CompactionError::NoSummary),
        }
    }
}

#[async_trait]
impl CompactionStrategy for SummaryCompaction {
    async fn compact(&self, turn: &mut Turn<'_>) -> Result<Option<Compaction>, CompactionError> {
        let agent = ActorId::Agent(turn.agent_id());
        let whole = kept_whole(turn.agent_channel_state().transcript.entries().map(|node| &node.entry));
        let (positions, _) = turn.work().token_positions().ok_or(CompactionError::TranscriptUnreadable)?;
        let size = turn.agent_channel_state().usage(agent).and_then(|usage| usage.context_size).or(positions.last().copied()).unwrap_or(0);
        let keep = self.keep_of(size);
        let path: Vec<Entry> = as_they_stand(turn.agent_channel_state().transcript.entries())
            .map(|(node, message)| {
                let gives = match &node.entry {
                    TranscriptEntry::Given { source: Some(source), .. } => given(source),
                    _ => None,
                };
                (node.digest, gives, message)
            })
            .collect();
        let Some(cut) = cut_at(&path, &positions, keep, turn.turn_id(), &whole) else {
            return Ok(None);
        };
        let (summarised, kept) = path.split_at(cut);
        let Some((last_summarised, _, _)) = summarised.last() else {
            return Ok(None);
        };
        let count = |entries: &[Entry]| entries.iter().filter(|(_, _, m)| m.is_some()).count();
        let keep_from = kept.first().map(|(digest, _, _)| *digest);

        // What is still there as it was given is not given twice.
        let still_there = given_in(kept);
        let wanted: Vec<Given> = given_in(summarised).into_iter().filter(|g| !still_there.contains(g)).collect();
        let (restored, lost) = self.give_again(turn, &wanted).await?;

        let (text, answer) = self.summarise(turn, *last_summarised).await?;
        // A summary of messages of the turn under way holds what the turn
        // read: it is the turn's, and goes with the turn if it is given up
        // as `Lowered`. One of what precedes the turn alone is no turn's,
        // classified by what it summarises - the call writing it was given
        // the transcript alone (issue 284).
        let of_turn = summarised.iter().any(|(_, _, message)| message.as_ref().is_some_and(|m| m.turn_id == Some(turn.turn_id())));
        let (owner, classification) = match of_turn {
            true => (Some(turn.turn_id()), turn.label()),
            false => {
                let summarised_as: Vec<&Classification> = summarised.iter().filter_map(|(_, _, message)| message.as_ref().map(|m| &m.classification)).collect();
                (None, Classification::join_all(summarised_as).unwrap_or_else(|| turn.label()))
            }
        };
        let summary = Message {
            id: MessageId::new(),
            sender_id: agent,
            sender_name: turn.agent_name().to_string(),
            role: MessageRole::User,
            content: Some(titled(SUMMARY_TITLE, &text)),
            timestamp: Utc::now(),
            prompt_tokens: answer.prompt_tokens,
            completion_tokens: answer.completion_tokens,
            context_size: answer.context_size,
            turn_id: owner,
            ..Message::new(classification)
        };
        let mut replacement: Vec<(Message, Option<KnowledgeRef>)> = vec![(summary, None)];
        for (_, given) in &restored {
            let message = Message {
                id: MessageId::new(),
                sender_id: agent,
                sender_name: turn.agent_name().to_string(),
                role: MessageRole::User,
                content: Some(given.text.clone()),
                timestamp: Utc::now(),
                // Given in the turn, as `Turn::give` gives.
                turn_id: Some(turn.turn_id()),
                ..Message::new(given.knowledge.classification.clone().unwrap_or_else(|| turn.label()))
            };
            replacement.push((message, Some(given.knowledge.clone())));
        }
        // Nothing can fail from here on: the transcript changes at once.
        turn.compact(replacement, keep_from).ok_or(CompactionError::NotReplaced)?;

        Ok(Some(Compaction {
            replaced: count(summarised),
            kept: count(kept),
            given_again: restored.iter().map(|(kind, given)| format!("{} {}", kind, given.name)).collect(),
            lost,
        }))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(SUMMARY_COMPACTION_KIND).provides::<dyn CompactionStrategy>().needs::<dyn LLMClient>().needs::<dyn KnowledgeRestorer>(),
        |descriptor, context| async move {
            let SummaryCompactionConfig { keep } = config_of_params(&descriptor.params)?;
            if !(0.0..1.0).contains(&keep) {
                return Err(ComponentError::InvalidConfig(format!("'keep' is a share of the context size, at least 0 and less than 1, not {keep}")));
            }
            // The agent's own model writes the summary.
            let llm_client = context.one::<dyn LLMClient>().await?.facet;
            // What the agent's components give again, each of its kind.
            let restorers = context.all::<dyn KnowledgeRestorer>().await?.into_iter().map(|restorer| restorer.facet).collect();
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn CompactionStrategy>(Arc::new(SummaryCompaction { llm_client, keep, restorers }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::workspace_turn;
    use caelumora_runtime::component::config_of_params;
    use caelumora_runtime::workspace::testing::{session_with_workspace, WithWorkspace};
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::confidentiality::{Classification, Confidentiality};
    use caelumora_model::context::{KnowledgeRef, KnowledgeSource, ModelProfile};
    use caelumora_model::store::{EntityKind, Stored};
    use caelumora_model::tool::Tool;
    use caelumora_model::workspace::{Workspace, WorkspaceId};
    use std::sync::Mutex;
    use caelumora_model::digest;
    use crate::llm::{LLMResult, unlabelled_message};
    use futures::stream::BoxStream;
    use serde_json::Value;

    /// What the agent of a test is given, as a plugin gives it - a skill,
    /// say: a workspace, here, and its text.
    const GIVEN: &str = "[Skill: release-notes]
1. Collect.
";

    /// That `id` was given to the agent of `world`.
    fn knowledge(world: &WithWorkspace, id: WorkspaceId) -> KnowledgeRef {
        KnowledgeRef { channel: world.session.channels()[0].id(), session: world.session.id(), source: KnowledgeSource::entity::<Workspace>(id, None), classification: Some(Classification::general(Confidentiality::Internal)) }
    }

    /// Gives again what it knows, as the component that gave it would:
    /// `Some` the one id it can still give.
    #[derive(Debug)]
    struct Restorer(Option<WorkspaceId>);

    #[async_trait]
    impl KnowledgeRestorer for Restorer {
        fn kind(&self) -> EntityKind {
            Workspace::KIND
        }

        async fn give_again(&self, turn: &mut Turn<'_>, given: &[Uuid]) -> Result<Vec<GivenAgain>, CompactionError> {
            Ok(given.iter().filter(|id| self.0.is_some_and(|known| known == WorkspaceId::from(**id))).map(|id| GivenAgain {
                name: "release-notes".to_string(),
                text: GIVEN.to_string(),
                knowledge: KnowledgeRef { source: KnowledgeSource::entity::<Workspace>(WorkspaceId::from(*id), None), ..turn_knowledge(turn) },
            }).collect())
        }
    }

    fn turn_knowledge(turn: &Turn<'_>) -> KnowledgeRef {
        KnowledgeRef { channel: turn.channel_id(), session: turn.session_id(), source: KnowledgeSource::entity::<Workspace>(WorkspaceId::new(), None), classification: Some(Classification::general(Confidentiality::Internal)) }
    }

    /// A model that summarises everything as `"They want release notes."`,
    /// keeping what it was given.
    #[derive(Debug, Default)]
    struct Summariser(Mutex<Vec<Vec<Message>>>);

    #[async_trait]
    impl LLMClient for Summariser {
        async fn chat_completion(&self, context: &[Message], _: &[Tool]) -> BoxStream<'static, LLMResult<MessageEvent>> {
            self.0.lock().unwrap().push(context.to_vec());
            let answer = Message {
                role: MessageRole::Assistant,
                content: Some("They want release notes.".to_string()),
                prompt_tokens: Some(40),
                completion_tokens: Some(8),
                ..unlabelled_message()
            };
            futures::stream::iter(vec![Ok(MessageEvent::Complete(answer))]).boxed()
        }

        fn profile(&self) -> ModelProfile {
            Default::default()
        }
    }

    fn entry(content: &str, role: MessageRole, turn: Option<TurnId>) -> Entry {
        let message = Message { content: Some(content.to_string()), role, turn_id: turn, ..Message::new(Classification::PUBLIC) };
        (digest::content(content.as_bytes()), None, Some(message))
    }

    fn end(turn: &str) -> Entry {
        (digest::content(turn.as_bytes()), None, None)
    }

    #[test]
    fn the_cut_lies_before_a_message_and_keeps_a_result_with_its_call() {
        let (earlier, under_way) = (TurnId::new(), TurnId::new());
        let path = vec![
            entry("u1", MessageRole::User, None),
            entry("call", MessageRole::Assistant, Some(earlier)),
            entry("result", MessageRole::Tool, Some(earlier)),
            end("end"),
            entry("u2", MessageRole::User, None),
        ];
        let positions = [0, 10, 20, 50, 50, 60];
        let none = HashSet::new();
        assert_eq!(cut_at(&path, &positions, 10, under_way, &none), Some(4), "the newest part that fits");
        assert_eq!(cut_at(&path, &positions, 45, under_way, &none), Some(4), "never before a result, which would part it from its call");
        assert_eq!(cut_at(&path, &positions, 50, under_way, &none), Some(1), "a finished turn may be cut apart");
        assert_eq!(cut_at(&path, &positions, 5, under_way, &none), Some(4), "the newest if nothing fits");
        assert_eq!(cut_at(&path, &positions, 60, under_way, &none), None, "nothing to summarise if everything fits");
        assert_eq!(cut_at(&path, &positions, 50, under_way, &HashSet::from([earlier])), Some(4), "not into a turn kept whole");
    }

    #[test]
    fn within_the_turn_under_way_the_cut_lies_before_a_call_of_its_model_but_the_first() {
        let under_way = TurnId::new();
        let path = vec![
            entry("u1", MessageRole::User, None),
            entry("a1", MessageRole::Assistant, Some(under_way)),
            entry("r1", MessageRole::Tool, Some(under_way)),
            entry("a2", MessageRole::Assistant, Some(under_way)),
            entry("r2", MessageRole::Tool, Some(under_way)),
            entry("a3", MessageRole::Assistant, Some(under_way)),
            entry("r3", MessageRole::Tool, Some(under_way)),
        ];
        let positions = [0, 10, 20, 40, 50, 70, 80, 100];
        let none = HashSet::new();
        assert_eq!(cut_at(&path, &positions, 60, under_way, &none), Some(3), "before its second call");
        assert_eq!(cut_at(&path, &positions, 85, under_way, &none), Some(3), "not before its first: its input would go and the turn stay");
        assert_eq!(cut_at(&path, &positions, 10, under_way, &none), Some(5), "its newest call and results stay");
        assert_eq!(cut_at(&path, &positions, 100, under_way, &none), None);
    }

    #[test]
    fn a_turn_given_up_for_a_lowered_channel_is_kept_whole() {
        let (lowered, failed) = (TurnId::new(), TurnId::new());
        let given_up = |turn_id, reason| TranscriptEntry::TurnGivenUp { turn_id, reason, at: Utc::now() };
        let entries = [
            given_up(lowered, GivenUpReason::Lowered { channel: Classification::PUBLIC }),
            given_up(failed, GivenUpReason::Left),
            TranscriptEntry::TurnFinished { turn_id: TurnId::new(), at: Utc::now() },
        ];
        assert_eq!(kept_whole(entries.iter()), HashSet::from([lowered]));
    }

    /// The world's agent heard `u1`, was given `release-notes` - as a
    /// member gives a skill with `/skill` -, answered `a1` in a turn of its
    /// own, then heard `u2`.
    async fn an_agent_with_a_skill_given(world: &WithWorkspace, state: &mut AgentChannelState) -> WorkspaceId {
        let skill = WorkspaceId::new();
        let heard = |text: &str| Message { content: Some(text.to_string()), ..Message::new(Classification::PUBLIC) };
        let u1 = heard("u1");
        state.perceive(vec![u1.clone()], vec![u1]);
        let mut turn = workspace_turn(world, state);
        turn.give(Message { content: Some(GIVEN.to_string()), ..Message::new(Classification::general(Confidentiality::Internal)) }, Some(knowledge(world, skill)));
        drop(turn);
        let given = state.turn.take().unwrap().work.turn_id;
        state.finish_turn(given, ActorId::Agent(world.agent), Vec::new(), Vec::new(), None);
        drop(workspace_turn(world, state));
        state.said(Message { sender_id: ActorId::Agent(world.agent), role: MessageRole::Assistant, ..heard("a1") });
        let answered = state.turn.take().unwrap().work.turn_id;
        state.finish_turn(answered, ActorId::Agent(world.agent), Vec::new(), Vec::new(), None);
        let u2 = heard("u2");
        state.perceive(vec![u2.clone()], vec![u2]);
        skill
    }

    #[tokio::test]
    async fn the_older_messages_are_replaced_by_a_summary_and_what_was_given_given_again() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let skill = an_agent_with_a_skill_given(&world, &mut state).await;
        let before = state.transcript.digest().unwrap();
        let model = Arc::new(Summariser::default());
        let strategy = SummaryCompaction { llm_client: Arc::<Summariser>::clone(&model), keep: 0.1, restorers: vec![Arc::new(Restorer(Some(skill)))] };

        let mut turn = workspace_turn(&world, &mut state);
        let compaction = strategy.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(compaction, Some(Compaction { replaced: 3, kept: 1, given_again: vec!["workspace release-notes".to_string()], lost: 0 }));
        let given = model.0.lock().unwrap().pop().expect("its model was asked");
        let texts: Vec<_> = given.iter().filter_map(|m| m.content.clone()).collect();
        assert!(texts.iter().any(|t| t == "u1") && texts.iter().any(|t| t == "a1") && !texts.iter().any(|t| t == "u2"), "the older messages, not those kept: {texts:?}");
        assert_eq!(texts.last(), Some(&strategy.instruction()), "asked for the summary last");
        assert!(strategy.instruction().ends_with("Leave out the text of anything given to you as a workspace: it is given to you again."), "and to leave out what is given again");

        let now: Vec<_> = state.transcript.messages().into_iter().map(|m| m.content.unwrap_or_default()).collect();
        assert_eq!(now, ["[Summary of the conversation so far]\nThey want release notes.\n", "[Skill: release-notes]\n1. Collect.\n", "u2"]);
        assert!(matches!(state.transcript.entries().nth(1).map(|n| &n.entry), Some(TranscriptEntry::Given { source: Some(source), .. }) if source.about::<Workspace>().is_some_and(|(s, _)| s == skill)), "the skill as given");
        assert!(state.transcript.prefix(&before).is_some(), "what it saw before stays");
        let replacement = state.transcript.messages();
        let under_way = state.turn.as_ref().map(|t| t.work.turn_id);
        assert_eq!(replacement[0].turn_id, None, "a summary of what precedes the turn alone is no turn's (issue 284)");
        let summarised = Classification::join_all([&Classification::PUBLIC, &Classification::general(Confidentiality::Internal)]);
        assert_eq!(Some(replacement[0].classification.clone()), summarised, "classified by what it summarises");
        assert_eq!(replacement[1].turn_id, under_way, "what is given again is given in the turn");
        let knowledge = &state.turn.as_ref().unwrap().work.context.knowledge;
        assert!(knowledge.iter().any(|k| k.source.about::<Workspace>().is_some_and(|(s, _)| s == skill)), "the turn has read it");
        let usage = state.usage(ActorId::Agent(world.agent)).expect("the summary's call");
        assert_eq!((usage.prompt_tokens, usage.completion_tokens), (40, 8), "the summary's call counts");
    }

    /// Issue 323: what can no longer be given where the agent is - a skill
    /// deleted meanwhile - is not given again, and the owners are told how
    /// much.
    #[tokio::test]
    async fn what_can_no_longer_be_given_is_counted_as_lost() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        an_agent_with_a_skill_given(&world, &mut state).await;
        // Deleted meanwhile: its restorer gives it no more.
        let strategy = SummaryCompaction { llm_client: Arc::new(Summariser::default()), keep: 0.1, restorers: vec![Arc::new(Restorer(None))] };

        let mut turn = workspace_turn(&world, &mut state);
        let compaction = strategy.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(compaction, Some(Compaction { replaced: 3, kept: 1, given_again: Vec::new(), lost: 1 }));
        let now: Vec<_> = state.transcript.messages().into_iter().map(|m| m.content.unwrap_or_default()).collect();
        assert_eq!(now, ["[Summary of the conversation so far]\nThey want release notes.\n", "u2"]);
    }

    #[tokio::test]
    async fn what_cannot_be_shortened_is_left_as_it_is() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let u1 = Message { content: Some("u1".to_string()), ..Message::new(Classification::PUBLIC) };
        state.perceive(vec![u1.clone()], vec![u1]);
        let before = state.transcript.digest();
        let model = Arc::new(Summariser::default());
        let strategy = SummaryCompaction { llm_client: Arc::<Summariser>::clone(&model), keep: DEFAULT_KEEP, restorers: Vec::new() };

        let mut turn = workspace_turn(&world, &mut state);
        let compaction = strategy.compact(&mut turn).await.expect("nothing to do is no failure");
        drop(turn);

        assert_eq!(compaction, None);
        assert!(model.0.lock().unwrap().is_empty(), "its model was not asked");
        assert_eq!(state.transcript.digest(), before);
    }

    #[tokio::test]
    async fn a_summary_of_a_part_of_the_turn_under_way_is_the_turns() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let u1 = Message { content: Some("u1".to_string()), ..Message::new(Classification::PUBLIC) };
        state.perceive(vec![u1.clone()], vec![u1]);
        let strategy = SummaryCompaction { llm_client: Arc::new(Summariser::default()), keep: 0.1, restorers: Vec::new() };
        let mut turn = workspace_turn(&world, &mut state);
        let said = |content: &str, role| Message { sender_id: ActorId::Agent(world.agent), role, content: Some(content.repeat(40)), ..Message::new(Classification::PUBLIC) };
        for (content, role) in [("a", MessageRole::Assistant), ("r", MessageRole::Tool), ("b", MessageRole::Assistant), ("s", MessageRole::Tool)] {
            turn.push_message(said(content, role));
        }
        let under_way = turn.turn_id();
        let label = turn.label();
        let compaction = strategy.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(compaction.map(|c| (c.replaced, c.kept)), Some((3, 2)), "the turn's input and first call summarised, its newest call kept with its result");
        let summary = &state.transcript.messages()[0];
        assert_eq!(summary.turn_id, Some(under_way), "it holds what the turn read: the turn's (issue 284)");
        assert_eq!(summary.classification, label, "as the turn writes");
    }

    /// Issue 389: a skill the turn under way loaded in the part summarised
    /// is given again, though a later message of the turn is kept - it is
    /// found where its text stands, not in what the turn read since.
    #[tokio::test]
    async fn a_skill_loaded_in_the_part_summarised_is_given_again_though_its_turn_goes_on() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let u1 = Message { content: Some("u1".to_string()), ..Message::new(Classification::PUBLIC) };
        state.perceive(vec![u1.clone()], vec![u1]);
        let skill = WorkspaceId::new();
        let strategy = SummaryCompaction { llm_client: Arc::new(Summariser::default()), keep: 0.1, restorers: vec![Arc::new(Restorer(Some(skill)))] };
        let mut turn = workspace_turn(&world, &mut state);
        let said = |content: &str, role| Message { sender_id: ActorId::Agent(world.agent), role, content: Some(content.repeat(40)), ..Message::new(Classification::PUBLIC) };
        turn.push_message(said("a", MessageRole::Assistant));
        turn.push_message(said("r", MessageRole::Tool));
        // Loaded by the call of `a`: given after its result.
        turn.give(Message { content: Some(GIVEN.to_string()), ..Message::new(Classification::general(Confidentiality::Internal)) }, Some(knowledge(&world, skill)));
        turn.push_message(said("b", MessageRole::Assistant));
        turn.push_message(said("s", MessageRole::Tool));
        let compaction = strategy.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(compaction.map(|c| (c.replaced, c.kept, c.given_again)), Some((4, 2, vec!["workspace release-notes".to_string()])), "its text was summarised: given again");
        let now: Vec<_> = state.transcript.messages().into_iter().map(|m| m.content.unwrap_or_default()).collect();
        assert_eq!(now[1], GIVEN, "after the summary");
    }

    /// What was given without being knowledge - an earlier summary, say -
    /// is summarised with the rest: nobody gives it again (issue 389).
    #[tokio::test]
    async fn what_was_given_without_being_knowledge_is_summarised() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let u1 = Message { content: Some("u1".to_string()), ..Message::new(Classification::PUBLIC) };
        state.perceive(vec![u1.clone()], vec![u1]);
        let model = Arc::new(Summariser::default());
        let strategy = SummaryCompaction { llm_client: Arc::<Summariser>::clone(&model), keep: 0.1, restorers: vec![Arc::new(Restorer(None))] };
        let mut turn = workspace_turn(&world, &mut state);
        turn.give(Message { content: Some("read this".repeat(40)), ..Message::new(Classification::PUBLIC) }, None);
        let said = |content: &str, role| Message { sender_id: ActorId::Agent(world.agent), role, content: Some(content.repeat(40)), ..Message::new(Classification::PUBLIC) };
        turn.push_message(said("a", MessageRole::Assistant));
        turn.push_message(said("r", MessageRole::Tool));
        turn.push_message(said("b", MessageRole::Assistant));
        turn.push_message(said("s", MessageRole::Tool));
        let compaction = strategy.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(compaction.map(|c| (c.given_again, c.lost)), Some((Vec::new(), 0)), "nothing to give again, nothing lost");
        let asked = model.0.lock().unwrap().pop().expect("its model was asked");
        assert!(asked.iter().any(|m| m.content.as_deref().is_some_and(|c| c.starts_with("read this"))), "it was summarised");
    }

    #[test]
    fn what_it_keeps_is_a_share_of_the_context_size() {
        let strategy = SummaryCompaction { llm_client: Arc::new(Summariser::default()), keep: 0.25, restorers: Vec::new() };
        assert_eq!(strategy.keep_of(1000), 250);

        let config = |params| config_of_params::<SummaryCompactionConfig>(&params).map(|c| c.keep);
        assert_eq!(config(Value::Null).ok(), Some(DEFAULT_KEEP));
        assert!(config(serde_json::json!({ "keep_last": 10 })).is_err(), "what it no longer takes is refused");
        assert!(config(serde_json::json!({ "threshold": 0.8 })).is_err(), "when is its policy's");
    }
}
