//! The participants of a channel: adding, removing and leaving, their mentions, and finding them by
//! name.

use caelumora_model::value::name::Name;
use crate::actor::Actor;
use anyhow::Result;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::HistoryAccess;
use caelumora_model::mention::{handle_of, MentionDirectory};
use caelumora_model::question::{QuestionError, Questions};
use caelumora_model::security::{ActionType, SecurityContext};
use tracing::{info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use super::{ActorChange, Actors, ChannelActor, ChannelError, ChannelLoop, ChannelMember, LEAVE_GRACE};
use super::history::history_start;

impl ChannelLoop {
    /// Adds `participant` - a participant of the session - to the channel on
    /// behalf of `context` (issue 46), seeing as much of the history as
    /// `history` grants. Stored first (`UpdateChannel`, also asked as
    /// `ConfidentialityPolicy::cleared`), then its actor built and given the part of the
    /// history it may see (`Actor::catch_up`, issue 190), then started, the
    /// moderator and the mention directory told, and the join announced.
    /// A participant removed shortly before is added only once its old
    /// actor has ended (`left`, issue 171).
    ///
    /// # Errors
    /// `ChannelError::NotInSession`, `OutOfScope`, `AlreadyParticipant`,
    /// `StillLeaving` or `Forbidden`; a `StoreError` if `context` may not
    /// change the channel.
    pub async fn add_participant(&self, context: &SecurityContext, participant: ActorId, history: HistoryAccess) -> Result<()> {
        // Whether the actor may change the channel at all, before anything
        // is asked of the participant (issue 198, F2): who may not learns
        // nothing of its clearance or its personality.
        let attributes = self.security_attributes();
        self.require(context, ActionType::UpdateChannel(&attributes), "adding a participant to").await?;
        if self.actors.load().contains_key(&participant) {
            return Err(ChannelError::AlreadyParticipant { participant, channel: self.id }.into());
        }
        self.left(participant).await?;
        let found = self.session_loop()
            .and_then(|s| s.session().participants.iter().find(|p| p.actor_id() == participant).cloned())
            .ok_or(ChannelError::NotInSession { participant, channel: self.id })?;
        let member = self.member_of(&found).await?
            .ok_or(ChannelError::ParticipantGone { participant, channel: self.id })?;
        member.require_in_scope(self.session_id, self.id)?;
        member.require_cleared(self.confidentiality.as_ref(), context, &self.classification(), self.session_id, self.id)?;

        let messages = self.history().await;
        let start = history_start(history, &messages, chrono::Utc::now());
        self.channel.add_participant(context, found.clone(), start).await?;

        let started = async {
            let actor = Self::create_actor(Arc::clone(&self.channel), &member, &self.runtime).await?;
            actor.catch_up(self.visible_history(participant).await.messages()).await?;
            Ok::<_, anyhow::Error>(actor)
        }.await;
        let actor: Arc<dyn Actor> = match started {
            Ok(actor) => Arc::from(actor),
            Err(error) => {
                // Stored, but it cannot run: taken back out, in the runtime's
                // name - adding it was permitted.
                if let Err(undo) = self.channel.remove_participant(&SecurityContext::system(), participant).await {
                    warn!("{} was added to channel {} but could not be started nor removed again: {}", participant, self.id, undo);
                }
                return Err(error);
            }
        };
        let adopted = self.adopt(actor, member).await;
        let _ = self.changes_tx.send(ActorChange::Start(adopted));
        info!("{} joined channel {}", participant, self.id);
        Ok(())
    }

    /// Removes `participant` from the channel on behalf of `context` (issue
    /// 46): stored first (`UpdateChannel`), then its actor stopped - an agent
    /// at once, without finishing a turn; the approver discards its pending
    /// approvals. Until its task has ended, the participant cannot be added
    /// again (`left`, issue 171). The leave is announced by the
    /// `ChannelController` on the event bus (issue 73), and in the channel by
    /// `run` with a `ParticipantRemoved` in the name of `context`'s caller,
    /// once the actor's task has ended: what it still sends until then - the
    /// step it was in, the end of its turn - enters the channel as its own,
    /// and nothing of it follows the announcement (issues 162, 262).
    /// Its state is kept, for the record and for when it joins again.
    ///
    /// # Errors
    /// `ChannelError::ApproverStays` or `NotAParticipant`; a `StoreError` if
    /// `context` may not change the channel.
    pub async fn remove_participant(&self, context: &SecurityContext, participant: ActorId) -> Result<()> {
        if matches!(participant, ActorId::Approver(_)) {
            return Err(ChannelError::ApproverStays { channel: self.id }.into());
        }
        let Some(ChannelActor { actor, task, .. }) = self.actors.load().get(&participant).cloned() else {
            return Err(ChannelError::NotAParticipant { participant, channel: self.id }.into());
        };
        self.channel.remove_participant(context, participant).await?;
        self.leaving.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(participant, task);
        self.actors.rcu(|actors| {
            let mut actors = HashMap::clone(actors);
            actors.remove(&participant);
            actors
        });
        self.moderator.remove_actor(participant);
        self.rebuild_mentions();
        actor.stop();
        let _ = self.changes_tx.send(ActorChange::Stop(participant, context.clone()));
        info!("{} left channel {}, removed by {}", participant, self.id, context.caller_id);
        Ok(())
    }

    /// Waits until the actor `participant` had in the channel before has
    /// ended its task, if it was removed (issue 171): until then it may
    /// still store the participant's state - a turn given up - which a new
    /// actor loads as it is built, and what it sends would pass as the new
    /// actor's. At most `LEAVE_GRACE`: by then `run` has aborted it.
    ///
    /// # Errors
    /// `ChannelError::StillLeaving` if it has not ended even so - it blocks
    /// without ever reaching an `await`.
    async fn left(&self, participant: ActorId) -> Result<(), ChannelError> {
        let Some(task) = self.leaving.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&participant).cloned() else {
            return Ok(());
        };
        // Closed by `run` once it stops the actor; not if the channel does
        // not run, and then nothing is tracked to wait for.
        task.close();
        if tokio::time::timeout(LEAVE_GRACE, task.wait()).await.is_err() {
            warn!("{} is added to channel {} again, but its old actor has not ended", participant, self.id);
            return Err(ChannelError::StillLeaving { participant, channel: self.id });
        }
        self.leaving.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&participant);
        Ok(())
    }

    pub(super) fn rebuild_mentions(&self) {
        let mentions = Self::mention_directory(&self.actors.load());
        for handle in mentions.ambiguous() {
            warn!("Channel {}: handle @{} is shared by several participants; mentioning it addresses all of them", self.id, handle);
        }
        self.mentions.store(Arc::new(mentions));
    }

    pub fn actor_ids(&self) -> Vec<ActorId> {
        self.actors.load().keys().copied().collect()
    }

    /// All participants of the channel, in no particular order.
    pub fn members(&self) -> Vec<ChannelMember> {
        self.actors.load().values()
            .map(|ChannelActor { actor: a, .. }| {
                let name = a.name();
                let handle = Some(name.handle())
                    .filter(|h| Self::is_mentionable(a.id()) && !h.is_empty());
                ChannelMember { id: a.id(), name, handle, kind: a.kind() }
            })
            .collect()
    }

    /// The agents of the channel `text` names, as a user names one in a
    /// command (issue 277): those whose name it is - as a `Name`, regardless
    /// of case and spelling - or, if there are none, those whose handle it
    /// is, with or without its `@`, normalised as a mention is (`handle_of`).
    /// Several if it is ambiguous, none if it names no agent; sorted by name.
    pub fn agents_named(&self, text: &str) -> Vec<ChannelMember> {
        Self::named(self.members().into_iter().filter(|m| m.id.as_agent_id().is_some()).collect(), text)
    }

    /// The participants of the channel `text` names - agents, users and
    /// every other actor -, found as `agents_named` finds agents: for a
    /// command that takes anyone taking part, such as `/nudge` (issue 278).
    pub fn participants_named(&self, text: &str) -> Vec<ChannelMember> {
        Self::named(self.members(), text)
    }

    /// Those of `members` that `text` names: by name, or else by handle;
    /// sorted by name.
    fn named(mut members: Vec<ChannelMember>, text: &str) -> Vec<ChannelMember> {
        members.sort_by(|l, r| l.name.as_str().cmp(r.name.as_str()));
        if let Ok(name) = Name::new(text) {
            let named: Vec<_> = members.iter().filter(|m| m.name == name).cloned().collect();
            if !named.is_empty() {
                return named;
            }
        }
        let handle = handle_of(text);
        members.into_iter().filter(|m| !handle.is_empty() && m.handle.as_deref() == Some(handle.as_str())).collect()
    }

    /// The agents of the channel that carry out commands themselves (issue
    /// 269), each with the commands `issuer` may see of it, by handle; an
    /// agent with none is left out.
    pub(crate) async fn agent_commands(&self, issuer: &SecurityContext) -> Vec<(ChannelMember, Vec<caelumora_model::command::Command>)> {
        let mut found = Vec::new();
        for member in self.members().into_iter().filter(|m| m.id.as_agent_id().is_some()) {
            let Some(actor) = self.actor_by_id(member.id) else { continue };
            let commands = actor.commands(issuer).await;
            if !commands.is_empty() {
                found.push((member, commands));
            }
        }
        found.sort_by(|(l, _), (r, _)| l.handle.cmp(&r.handle));
        found
    }

    /// `questions` as put to the users of the channel (issue 165): every
    /// addressee found among them - a handle as its participants are
    /// mentioned, the agent's owner as `owner` (`Questions::resolve`).
    pub(crate) fn put(&self, questions: Questions, owner: ActorId) -> Result<Questions, QuestionError> {
        let users: Vec<ActorId> = self.actor_ids().into_iter().filter(|id| matches!(id, ActorId::User(_))).collect();
        questions.resolve(&self.mentions.load(), &users, owner)
    }

    fn is_mentionable(id: ActorId) -> bool {
        matches!(id, ActorId::User(_) | ActorId::Agent(_))
    }

    fn mention_directory(actors: &Actors) -> MentionDirectory {
        let names: Vec<(ActorId, Name)> = actors.values()
            .map(|a| &a.actor)
            .filter(|a| Self::is_mentionable(a.id()))
            .map(|a| (a.id(), a.name()))
            .collect();
        MentionDirectory::new(names.iter().map(|(id, name)| (*id, name.as_str())))
    }

    /// The participants `text` mentions by their `@`-handles, as the channel
    /// resolves them when a message enters it (issue 29) - for an actor
    /// that shows its own message before (issue 220).
    pub(crate) fn mentions_in(&self, text: &str) -> Vec<ActorId> {
        self.mentions.load().resolve(text)
    }
}

#[cfg(test)]
mod tests {
    use caelumora_model::actor::ActorId;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_util::task::TaskTracker;
    
    use caelumora_model::security::DummySecurityManager;
    use crate::channel::tests::*;

    #[tokio::test]
    async fn an_agent_is_found_by_its_name_or_its_handle_however_it_is_written() {
        let (channel, _, _) = channel_with_sleepers(&["Sassy", "Dr. Who", "Dr-Who", "Zoë"]).await;
        let named = |text: &str| channel.agents_named(text).into_iter().map(|m| m.name.to_string()).collect::<Vec<_>>();

        for text in ["Sassy", "sassy", "SASSY", "@Sassy", "@sassy"] {
            assert_eq!(named(text), ["Sassy"], "{text}");
        }
        assert_eq!(named("Zoe"), ["Zoë"], "by its ASCII form, as a name");
        assert_eq!(named("@zoë"), ["Zoë"], "and as a mention");
        assert_eq!(named("dr. who"), ["Dr. Who"], "a name before a handle: no ambiguity");
        assert_eq!(named("dr-who"), ["Dr-Who"], "the other's name");
        assert_eq!(named("@dr-who"), ["Dr-Who", "Dr. Who"], "the handle of both: ambiguous");
        assert!(named("alice").is_empty(), "a user is no agent");
        assert!(named("nobody").is_empty() && named("@").is_empty() && named("").is_empty(), "nobody");
    }

    #[tokio::test]
    async fn a_participant_is_added_again_only_once_its_old_actor_has_ended() {
        let (channel, _, agent, _) = agent_channel(Arc::new(DummySecurityManager::new()), Arc::new(NeverAnswers), Default::default()).await;
        let participant = ActorId::Agent(agent);
        assert!(channel.left(participant).await.is_ok(), "nothing to wait for: it never left");

        // Its old actor takes a while to end, as one that stores its state.
        let old = TaskTracker::new();
        let ended = Arc::new(AtomicBool::new(false));
        let ending = Arc::clone(&ended);
        old.spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            ending.store(true, Ordering::SeqCst);
        });
        channel.leaving.lock().unwrap().insert(participant, old);

        channel.left(participant).await.expect("it ends within the grace");
        assert!(ended.load(Ordering::SeqCst), "not before its old actor ended");
        assert!(!channel.leaving.lock().unwrap().contains_key(&participant), "and is forgotten");
    }
}
