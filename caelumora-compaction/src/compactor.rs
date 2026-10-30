//! A compaction of an agent's context carried out: the transcript cut, the older part replaced by
//! what the agent's strategy puts there, and what was given in it given again (issues 284, 413).
//!
//! The newest part of the context - `keep`, a share of the model's context size - is kept as it
//! stands; where the cut lies is the [`Cut`]'s. What replaces the part before it is asked of the
//! agent's [`CompactionStrategy`]; what the agent had been given there is given again, as it is
//! now, by the component that gave it, its [`KnowledgeRestorer`] (issue 323), after the
//! replacement - what the strategy answers the agent still needs of it (issue 413). Nothing changes
//! until all of it is had: then the context moves to the compacted one at once (`Turn::compact`).

use crate::cut::{Cut, Given};
use crate::{Compaction, CompactionError, CompactionStrategy, GivenAgain, KnowledgeRestorer};
use caelumora_agents::turn::Turn;
use caelumora_model::actor::ActorId;
use caelumora_model::context::KnowledgeRef;
use caelumora_model::message::{Message, MessageId, MessageRole};
use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

/// Carries out the compactions of an agent's context, with its strategy and the restorers of its
/// components.
#[derive(Debug)]
pub(crate) struct Compactor {
    /// What replaces the part cut off.
    strategy: Arc<dyn CompactionStrategy>,
    /// The components of the agent that give again what they gave it, each for its kind.
    restorers: Vec<Arc<dyn KnowledgeRestorer>>,
    /// How much of the newest context is kept as it stands, as a share of the context size - of the
    /// context as it stands where no size is known.
    keep: f64,
}

impl Compactor {
    pub(crate) fn new(
        strategy: Arc<dyn CompactionStrategy>,
        restorers: Vec<Arc<dyn KnowledgeRestorer>>,
        keep: f64,
    ) -> Self {
        Self { strategy, restorers, keep }
    }

    /// Compacts the context of `turn`'s agent, moving the turn's context to the compacted one: what
    /// it did, or `None` if there is nothing to compact, and nothing was changed. Nothing is
    /// changed either if it fails.
    ///
    /// # Errors
    /// `TranscriptUnreadable` if where its entries stand or the part to replace cannot be read,
    /// `Prompt` if a text the context is worded with cannot be had, what the restorers or the
    /// strategy fail with, `NotReplaced` if the context could not be replaced.
    pub(crate) async fn compact(
        &self,
        turn: &mut Turn<'_>,
    ) -> Result<Option<Compaction>, CompactionError> {
        let Some(cut) = self.cut(turn)? else {
            return Ok(None);
        };
        let (restored, lost) = self.give_again(turn, &cut.wanted()).await?;
        let part = turn.context_until(cut.until()).await.map_err(|e| {
            tracing::warn!(
                "Agent {}: a compaction cannot read its transcript: {:#}",
                turn.situation().agent_id(),
                e
            );
            CompactionError::TranscriptUnreadable
        })?;
        let given: Vec<String> = restored.iter().map(|(name, _)| name.clone()).collect();
        let replacement = self.strategy.replace(turn, part, &given).await?;
        let (needed, not_needed): (Vec<_>, Vec<_>) =
            restored.into_iter().partition(|(name, _)| replacement.still_needed.contains(name));

        let (owner, classification) = cut.replaced_as(turn.situation().label());
        let agent = ActorId::Agent(turn.situation().agent_id());
        let instead = Message {
            id: MessageId::new(),
            sender_id: agent,
            sender_name: turn.agent_name().to_string(),
            role: MessageRole::User,
            content: Some(replacement.text),
            timestamp: Utc::now(),
            prompt_tokens: replacement.call.prompt_tokens,
            completion_tokens: replacement.call.completion_tokens,
            context_size: replacement.call.context_size,
            turn_id: owner,
            ..Message::new(classification)
        };
        let mut messages: Vec<(Message, Option<KnowledgeRef>)> = vec![(instead, None)];
        for (_, given) in &needed {
            let message = Message {
                id: MessageId::new(),
                sender_id: agent,
                sender_name: turn.agent_name().to_string(),
                role: MessageRole::User,
                content: Some(given.text.clone()),
                timestamp: Utc::now(),
                // Given in the turn, as `Turn::give` gives.
                turn_id: Some(turn.turn_id()),
                ..Message::new(
                    given
                        .knowledge
                        .classification
                        .clone()
                        .unwrap_or_else(|| turn.situation().label()),
                )
            };
            messages.push((message, Some(given.knowledge.clone())));
        }
        // Nothing can fail from here on: the transcript changes at once.
        turn.compact(messages, cut.keep_from())?.ok_or(CompactionError::NotReplaced)?;

        Ok(Some(Compaction {
            replaced: cut.messages_replaced(),
            kept: cut.messages_kept(),
            given_again: needed.into_iter().map(|(name, _)| name).collect(),
            not_needed: not_needed.into_iter().map(|(name, _)| name).collect(),
            lost,
        }))
    }

    /// Where the transcript of `turn`'s agent is cut, keeping `keep` of its model's context size;
    /// `None` if there is nothing to replace.
    fn cut(&self, turn: &Turn<'_>) -> Result<Option<Cut>, CompactionError> {
        let situation = turn.situation();
        let (positions, _) =
            situation.token_positions()?.ok_or(CompactionError::TranscriptUnreadable)?;
        let state = situation.agent_channel_state();
        let size = state
            .used
            .and_then(|usage| usage.context_size)
            .or(positions.last().copied())
            .unwrap_or(0);
        Ok(Cut::of(&state.transcript, &positions, self.keep_of(size), turn.turn_id()))
    }

    /// How many tokens of a context of `size` are kept as they stand.
    fn keep_of(&self, size: u64) -> u64 {
        // A share of a token count: rounded down, what is lost is a token.
        (self.keep * size as f64) as u64
    }

    /// What `wanted` - given in the part replaced and not in the part kept - can be given again by
    /// the restorer of its kind, each by its kind and name - `skill release-notes` -, and how much
    /// of it can no longer be given. What no restorer gives - a workspace's files - is not given
    /// again, and not lost either.
    async fn give_again(
        &self,
        turn: &mut Turn<'_>,
        wanted: &[Given],
    ) -> Result<(Vec<(String, GivenAgain)>, usize), CompactionError> {
        let mut restored = Vec::new();
        let mut lost = 0;
        for restorer in &self.restorers {
            let kind = restorer.kind();
            let ids: Vec<Uuid> =
                wanted.iter().filter(|(of, _)| *of == kind).map(|(_, id)| *id).collect();
            if ids.is_empty() {
                continue;
            }
            let given = restorer.give_again(turn, &ids).await?;
            lost += ids.len().saturating_sub(given.len());
            restored
                .extend(given.into_iter().map(|given| (format!("{} {}", kind, given.name), given)));
        }
        Ok((restored, lost))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{two_calls, Scripted, SUMMARY};
    use async_trait::async_trait;
    use caelumora_agents::testing::workspace_turn;
    use caelumora_model::actor::EventContext;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::confidentiality::{Classification, Confidentiality};
    use caelumora_model::context::{KnowledgeSource, ModelProfile, SenderFormat};
    use caelumora_model::store::{EntityKind, Stored};
    use caelumora_model::transcript::TranscriptEntry;
    use caelumora_model::user::UserId;
    use caelumora_model::value::time_zone::TimeZone;
    use caelumora_model::work::Work;
    use caelumora_model::workspace::{Workspace, WorkspaceId};
    use caelumora_runtime::agent::state::AgentStateHandle;
    use caelumora_runtime::testing::actor_runtime_in;
    use caelumora_runtime::workspace::testing::{session_with_workspace, WithWorkspace};

    /// What the agent of a test is given, as a plugin gives it - a skill, say: a workspace, here,
    /// and its text.
    const GIVEN: &str = "[Skill: release-notes]
1. Collect.
";

    /// That `id` was given to the agent of `world`.
    fn knowledge(world: &WithWorkspace, id: WorkspaceId) -> KnowledgeRef {
        KnowledgeRef {
            channel: world.session.channels()[0].id(),
            session: world.session.id(),
            source: KnowledgeSource::entity::<Workspace>(id, None),
            classification: Some(Classification::general(Confidentiality::Internal)),
        }
    }

    /// Gives again what it knows, as the component that gave it would: `Some` the one id it can
    /// still give.
    #[derive(Debug)]
    struct Restorer(Option<WorkspaceId>);

    #[async_trait]
    impl KnowledgeRestorer for Restorer {
        fn kind(&self) -> EntityKind {
            Workspace::KIND
        }

        async fn give_again(
            &self,
            turn: &mut Turn<'_>,
            given: &[Uuid],
        ) -> Result<Vec<GivenAgain>, CompactionError> {
            Ok(given
                .iter()
                .filter(|id| self.0.is_some_and(|known| known == WorkspaceId::from(**id)))
                .map(|id| GivenAgain {
                    name: "release-notes".to_string(),
                    text: GIVEN.to_string(),
                    knowledge: KnowledgeRef {
                        source: KnowledgeSource::entity::<Workspace>(WorkspaceId::from(*id), None),
                        ..turn_knowledge(turn)
                    },
                })
                .collect())
        }
    }

    fn turn_knowledge(turn: &Turn<'_>) -> KnowledgeRef {
        KnowledgeRef {
            channel: turn.situation().channel_id(),
            session: turn.situation().session_id(),
            source: KnowledgeSource::entity::<Workspace>(WorkspaceId::new(), None),
            classification: Some(Classification::general(Confidentiality::Internal)),
        }
    }

    /// A compactor keeping a tenth of the context, with `strategy` and `restorers`.
    fn compactor(
        strategy: &Arc<Scripted>,
        restorers: Vec<Arc<dyn KnowledgeRestorer>>,
    ) -> Compactor {
        Compactor::new(Arc::<Scripted>::clone(strategy), restorers, 0.1)
    }

    /// The texts of the transcript's active branch, as they stand.
    fn texts(state: &AgentChannelState) -> Vec<String> {
        state.transcript.messages().into_iter().map(|m| m.content.unwrap_or_default()).collect()
    }

    /// The world's agent heard `u1`, was given `release-notes` - as a participant gives a skill
    /// with `/skill` -, answered `a1` in a turn of its own, then heard `u2`.
    async fn an_agent_with_a_skill_given(
        world: &WithWorkspace,
        state: &mut AgentChannelState,
    ) -> WorkspaceId {
        let skill = WorkspaceId::new();
        let heard = |text: &str| Message {
            content: Some(text.to_string()),
            ..Message::new(Classification::PUBLIC)
        };
        let u1 = heard("u1");
        state.perceive(vec![u1]);
        let mut turn = workspace_turn(world, state);
        turn.give(
            Message {
                content: Some(GIVEN.to_string()),
                ..Message::new(Classification::general(Confidentiality::Internal))
            },
            Some(knowledge(world, skill)),
        );
        drop(turn);
        let given = state.turn.take().unwrap().work.turn_id;
        state.finish_turn(given, Vec::new(), Vec::new());
        drop(workspace_turn(world, state));
        state.said(Message {
            sender_id: ActorId::Agent(world.agent),
            role: MessageRole::Assistant,
            ..heard("a1")
        });
        let answered = state.turn.take().unwrap().work.turn_id;
        state.finish_turn(answered, Vec::new(), Vec::new());
        let u2 = heard("u2");
        state.perceive(vec![u2]);
        skill
    }

    #[tokio::test]
    async fn the_older_messages_are_replaced_and_what_was_given_given_again() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let skill = an_agent_with_a_skill_given(&world, &mut state).await;
        let before = state.transcript.digest().unwrap();
        let strategy = Arc::new(Scripted::default());
        let compactor = compactor(&strategy, vec![Arc::new(Restorer(Some(skill)))]);

        let mut turn = workspace_turn(&world, &mut state);
        let compaction = compactor.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(
            compaction,
            Some(Compaction {
                replaced: 3,
                kept: 1,
                given_again: vec!["workspace release-notes".to_string()],
                not_needed: Vec::new(),
                lost: 0
            })
        );
        let (part, given) = strategy.asked.lock().unwrap().pop().expect("its strategy was asked");
        let asked: Vec<_> = part.iter().filter_map(|m| m.content.clone()).collect();
        assert!(
            asked.iter().any(|t| t == "u1")
                && asked.iter().any(|t| t == "a1")
                && !asked.iter().any(|t| t == "u2"),
            "the older messages, not those kept: {asked:?}"
        );
        assert_eq!(given, ["workspace release-notes"], "told what is given again");

        assert_eq!(texts(&state), [SUMMARY, GIVEN, "u2"]);
        assert!(
            matches!(state.transcript.entries().nth(1).map(|n| &n.entry), Some(TranscriptEntry::Given { source: Some(source), .. }) if source.about::<Workspace>().is_some_and(|(s, _)| s == skill)),
            "the skill as given"
        );
        assert!(state.transcript.prefix(&before).is_some(), "what it saw before stays");
        let replacement = state.transcript.messages();
        let under_way = state.turn.as_ref().map(|t| t.work.turn_id);
        assert_eq!(
            replacement[0].turn_id, None,
            "a summary of what precedes the turn alone is no turn's (issue 284)"
        );
        let summarised = Classification::join_all([
            &Classification::PUBLIC,
            &Classification::general(Confidentiality::Internal),
        ]);
        assert_eq!(
            Some(replacement[0].classification.clone()),
            summarised,
            "classified by what it replaces"
        );
        assert_eq!(replacement[1].turn_id, under_way, "what is given again is given in the turn");
        let knowledge = &state.turn.as_ref().unwrap().work.context.frame.knowledge;
        assert!(
            knowledge
                .iter()
                .any(|k| k.source.about::<Workspace>().is_some_and(|(s, _)| s == skill)),
            "the turn has read it"
        );
        let usage = state.used.expect("the replacement's call");
        assert_eq!(
            (usage.prompt_tokens, usage.completion_tokens),
            (40, 8),
            "the replacement's call counts"
        );
    }

    /// Issue 323: what can no longer be given where the agent is - a skill deleted meanwhile - is
    /// not given again, and the owners are told how much.
    #[tokio::test]
    async fn what_can_no_longer_be_given_is_counted_as_lost() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        an_agent_with_a_skill_given(&world, &mut state).await;
        // Deleted meanwhile: its restorer gives it no more.
        let strategy = Arc::new(Scripted::default());
        let compactor = compactor(&strategy, vec![Arc::new(Restorer(None))]);

        let mut turn = workspace_turn(&world, &mut state);
        let compaction = compactor.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(
            compaction,
            Some(Compaction {
                replaced: 3,
                kept: 1,
                given_again: Vec::new(),
                not_needed: Vec::new(),
                lost: 1
            })
        );
        let (_, given) = strategy.asked.lock().unwrap().pop().expect("its strategy was asked");
        assert!(given.is_empty(), "nothing is given again");
        assert_eq!(texts(&state), [SUMMARY, "u2"]);
    }

    /// Issue 413: what the agent no longer needs, as its strategy answers - a skill of a task
    /// that is over -, is not given again, and the owners are told so.
    #[tokio::test]
    async fn what_is_no_longer_needed_is_not_given_again() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let skill = an_agent_with_a_skill_given(&world, &mut state).await;
        let strategy = Arc::new(Scripted::needing_nothing());
        let compactor = compactor(&strategy, vec![Arc::new(Restorer(Some(skill)))]);

        let mut turn = workspace_turn(&world, &mut state);
        let compaction = compactor.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(
            compaction,
            Some(Compaction {
                replaced: 3,
                kept: 1,
                given_again: Vec::new(),
                not_needed: vec!["workspace release-notes".to_string()],
                lost: 0
            })
        );
        let (_, given) = strategy.asked.lock().unwrap().pop().expect("its strategy was asked");
        assert_eq!(given, ["workspace release-notes"], "asked about what can be given again");
        assert_eq!(texts(&state), [SUMMARY, "u2"], "the summary alone");
    }

    #[tokio::test]
    async fn what_cannot_be_shortened_is_left_as_it_is() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let u1 =
            Message { content: Some("u1".to_string()), ..Message::new(Classification::PUBLIC) };
        state.perceive(vec![u1]);
        let before = state.transcript.digest();
        let strategy = Arc::new(Scripted::default());
        let compactor = Compactor::new(Arc::<Scripted>::clone(&strategy), Vec::new(), 0.25);

        let mut turn = workspace_turn(&world, &mut state);
        let compaction = compactor.compact(&mut turn).await.expect("nothing to do is no failure");
        drop(turn);

        assert_eq!(compaction, None);
        assert_eq!(strategy.times_asked(), 0, "its strategy was not asked");
        assert_eq!(state.transcript.digest(), before);
    }

    #[tokio::test]
    async fn a_strategy_that_fails_leaves_the_context_as_it_is() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let skill = an_agent_with_a_skill_given(&world, &mut state).await;
        let before = state.transcript.digest();
        let strategy = Arc::new(Scripted::failing(|| CompactionError::NoSummary));
        let compactor = compactor(&strategy, vec![Arc::new(Restorer(Some(skill)))]);

        let mut turn = workspace_turn(&world, &mut state);
        let failed = compactor.compact(&mut turn).await;
        drop(turn);

        assert!(matches!(failed, Err(CompactionError::NoSummary)), "{failed:?}");
        assert_eq!(state.transcript.digest(), before, "nothing was changed");
    }

    #[tokio::test]
    async fn a_replacement_of_a_part_of_the_turn_under_way_is_the_turns() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let u1 =
            Message { content: Some("u1".to_string()), ..Message::new(Classification::PUBLIC) };
        state.perceive(vec![u1]);
        let compactor = compactor(&Arc::new(Scripted::default()), Vec::new());
        let mut turn = workspace_turn(&world, &mut state);
        two_calls(&mut turn, world.agent);
        let under_way = turn.turn_id();
        let label = turn.situation().label();
        let compaction = compactor.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(
            compaction.map(|c| (c.replaced, c.kept)),
            Some((3, 2)),
            "the turn's input and first call replaced, its newest call kept with its result"
        );
        let replacement = &state.transcript.messages()[0];
        assert_eq!(
            replacement.turn_id,
            Some(under_way),
            "it holds what the turn read: the turn's (issue 284)"
        );
        assert_eq!(replacement.classification, label, "as the turn writes");
    }

    /// Issue 430: the strategy is given the times of messages in the time zone of the turn, as the
    /// turn's own model is - not in UTC.
    #[tokio::test]
    async fn the_times_of_messages_are_given_in_the_time_zone_of_the_turn() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let u1 = Message {
            sender_id: ActorId::User(UserId::new()),
            sender_name: "alice".to_string(),
            role: MessageRole::User,
            content: Some("u1".to_string()),
            ..Message::new(Classification::PUBLIC)
        };
        let written = u1.timestamp;
        state.perceive(vec![u1]);
        let marking = ModelProfile {
            sender_format: SenderFormat::Prefix,
            message_times: true,
            ..ModelProfile::default()
        };
        let berlin = TimeZone::new("Europe/Berlin").unwrap();
        let channel = &world.session.channels()[0];
        state.begin_turn(
            Work::new(
                EventContext::new(world.session.id(), channel.id(), ActorId::None, String::new()),
                None,
                world.context.clone(),
                channel.classification(),
                berlin,
            )
            .given_as(marking),
        );
        let strategy = Arc::new(Scripted::default());
        let compactor = compactor(&strategy, Vec::new());
        let mut turn = Turn::of(
            world.agent,
            "Jonathan".to_string(),
            &mut state,
            AgentStateHandle::detached(),
            actor_runtime_in(channel, ActorId::Agent(world.agent)),
        )
        .expect("a turn was begun");
        two_calls(&mut turn, world.agent);
        compactor.compact(&mut turn).await.expect("compacted");
        drop(turn);

        let (part, _) = strategy.asked.lock().unwrap().pop().expect("its strategy was asked");
        assert_eq!(
            part[0].content,
            Some(format!("@alice ({}): u1", berlin.format(written))),
            "in Berlin's time, as the turn's model is given it"
        );
    }

    /// Issue 389: a skill the turn under way loaded in the part replaced is given again, though a
    /// later message of the turn is kept - it is found where its text stands, not in what the turn
    /// read since.
    #[tokio::test]
    async fn a_skill_loaded_in_the_part_replaced_is_given_again_though_its_turn_goes_on() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let u1 =
            Message { content: Some("u1".to_string()), ..Message::new(Classification::PUBLIC) };
        state.perceive(vec![u1]);
        let skill = WorkspaceId::new();
        let compactor =
            compactor(&Arc::new(Scripted::default()), vec![Arc::new(Restorer(Some(skill)))]);
        let mut turn = workspace_turn(&world, &mut state);
        let said = |content: &str, role| Message {
            sender_id: ActorId::Agent(world.agent),
            role,
            content: Some(content.repeat(40)),
            ..Message::new(Classification::PUBLIC)
        };
        turn.record_said(said("a", MessageRole::Assistant));
        turn.record_said(said("r", MessageRole::Tool));
        // Loaded by the call of `a`: given after its result.
        turn.give(
            Message {
                content: Some(GIVEN.to_string()),
                ..Message::new(Classification::general(Confidentiality::Internal))
            },
            Some(knowledge(&world, skill)),
        );
        turn.record_said(said("b", MessageRole::Assistant));
        turn.record_said(said("s", MessageRole::Tool));
        let compaction = compactor.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(
            compaction.map(|c| (c.replaced, c.kept, c.given_again)),
            Some((4, 2, vec!["workspace release-notes".to_string()])),
            "its text was replaced: given again"
        );
        assert_eq!(texts(&state)[1], GIVEN, "after the replacement");
    }

    /// What was given without being knowledge - an earlier summary, say - is replaced with the
    /// rest: nobody gives it again (issue 389).
    #[tokio::test]
    async fn what_was_given_without_being_knowledge_is_replaced() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let u1 =
            Message { content: Some("u1".to_string()), ..Message::new(Classification::PUBLIC) };
        state.perceive(vec![u1]);
        let strategy = Arc::new(Scripted::default());
        let compactor = compactor(&strategy, vec![Arc::new(Restorer(None))]);
        let mut turn = workspace_turn(&world, &mut state);
        turn.give(
            Message {
                content: Some("read this".repeat(40)),
                ..Message::new(Classification::PUBLIC)
            },
            None,
        );
        two_calls(&mut turn, world.agent);
        let compaction = compactor.compact(&mut turn).await.expect("compacted");
        drop(turn);

        assert_eq!(
            compaction.map(|c| (c.given_again, c.lost)),
            Some((Vec::new(), 0)),
            "nothing to give again, nothing lost"
        );
        let (part, _) = strategy.asked.lock().unwrap().pop().expect("its strategy was asked");
        assert!(
            part.iter().any(|m| m.content.as_deref().is_some_and(|c| c.starts_with("read this"))),
            "it was replaced"
        );
    }

    #[test]
    fn what_it_keeps_is_a_share_of_the_context_size() {
        let compactor = Compactor::new(Arc::new(Scripted::default()), Vec::new(), 0.25);
        assert_eq!(compactor.keep_of(1000), 250);
    }
}
