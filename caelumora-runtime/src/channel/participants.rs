//! The participants of a channel: adding, removing and leaving, their mentions, and finding them by
//! name.

use super::{
    ActorChange, Actors, ChannelActor, ChannelError, ChannelLoop, ChannelParticipant,
    ResolvedParticipant, LEAVE_GRACE,
};
use anyhow::Result;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::chain::Position;
use caelumora_model::channel::Channel;
use caelumora_model::channel::HistoryAccess;
use caelumora_model::channel::Participant;
use caelumora_model::command::Command;
use caelumora_model::mention::{handle_of, MentionDirectory};
use caelumora_model::question::{QuestionError, Questions};
use caelumora_model::security::{ActionType, SecurityContext};
use caelumora_model::store::Stored;
use caelumora_model::value::name::Name;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time;
use tracing::{info, warn};

/// How someone comes into a channel (`ChannelLoop::take_in`): the one way
/// in, asked for in three ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// Added by whoever may change the channel (`UpdateChannel`).
    Added,
    /// By their own act, where the channel lets them join (`JOIN`).
    Joining,
    /// As a founder of a channel just created (issue 420), named by whoever
    /// created it: creating it (`CreateChannel`) was what was asked.
    Founding,
}

impl ChannelLoop {
    /// Adds `participant` - a participant of the session - to the channel on
    /// behalf of `context` (issue 46), seeing as much of the history as
    /// `history` grants: whoever may change the channel (`UpdateChannel`),
    /// asked before anything of the participant (issue 198, F2) - who may not
    /// learns nothing of its clearance or its personality. Then taken in as
    /// [`Self::take_in`] says.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if `context` may not change the channel; as
    /// [`Self::take_in`].
    pub async fn add_participant(
        &self,
        context: &SecurityContext,
        participant: ActorId,
        history: HistoryAccess,
    ) -> Result<()> {
        self.require(
            context,
            |attributes| ActionType::update(Channel::KIND, attributes),
            "adding a participant to",
        )
        .await?;
        self.take_in(context, participant, history, Entry::Added).await
    }

    /// Takes `participant` - a participant of the session, and whom `context`
    /// acts for: themselves, or their own agent - into the channel by their
    /// own act (issue 221), where an entry of the channel's access list lets
    /// them join (`JOIN`). They see the whole history, as whoever may change
    /// the channel let them by letting them join. Stored in the runtime's
    /// name, once joining was permitted; then as [`Self::take_in`] says.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if they may not join; as [`Self::take_in`].
    pub async fn join(&self, context: &SecurityContext, participant: ActorId) -> Result<()> {
        self.take_in(context, participant, HistoryAccess::All, Entry::Joining).await
    }

    /// Takes `participant` - a participant of the session - into the
    /// channel as one of its founders (issue 420), on behalf of `context`,
    /// who just created it: given what is said from its join on, as
    /// everybody who is there from the start - nothing is shared with it
    /// (issue 417). Asked nothing beyond what anyone taken in
    /// is asked - creating the channel was permitted, with whom it names -
    /// and stored in the runtime's name; then as [`Self::take_in`] says. For
    /// the creation of a channel alone (`SessionLoop::create_channel`,
    /// `SessionManager::create_from_template`): anything later is adding.
    ///
    /// # Errors
    /// As [`Self::take_in`].
    pub(crate) async fn found(
        &self,
        context: &SecurityContext,
        participant: ActorId,
    ) -> Result<()> {
        self.take_in(context, participant, HistoryAccess::None, Entry::Founding).await
    }

    /// Takes `participant` into the channel, on behalf of `context`, seeing
    /// as much of the history as `history` grants, by the way `entry` says:
    /// joining is asked once the participant is resolved. Stored first
    /// (also asked as `ConfidentialityPolicy::cleared`), then its actor
    /// started (`start_participant`) - taken back out of the document if it
    /// cannot be - and the join announced, on the event bus and in the
    /// channel, in the name of `context`'s caller - who added them, or who
    /// joined (issue 408). A participant removed shortly
    /// before is added only once its old actor has ended (`left`, issue
    /// 171).
    ///
    /// # Errors
    /// `ChannelError::NotInSession`, `OutOfScope`, `AlreadyParticipant`,
    /// `StillLeaving`, `NotCleared` or `Forbidden`; a `StoreError` if
    /// `context` may not change the channel.
    async fn take_in(
        &self,
        context: &SecurityContext,
        participant: ActorId,
        history: HistoryAccess,
        entry: Entry,
    ) -> Result<()> {
        if self.actors.load().contains_key(&participant) {
            return Err(ChannelError::AlreadyParticipant { participant, channel: self.id }.into());
        }
        self.left(participant).await?;
        let found = self
            .session_loop()
            .and_then(|s| {
                s.session().participants.iter().find(|p| p.actor_id() == participant).cloned()
            })
            .ok_or(ChannelError::NotInSession { participant, channel: self.id })?;
        let resolved = self
            .resolve(&found)
            .await?
            .ok_or(ChannelError::ParticipantGone { participant, channel: self.id })?;
        // Joining or founding was permitted; storing is the runtime's.
        let storing = match entry {
            Entry::Added => context.clone(),
            Entry::Joining => {
                let channel = self.id;
                self.participation
                    .require_joinable(
                        &resolved,
                        context,
                        Channel::KIND,
                        &self.security_attributes(),
                        |refused| ChannelError::Forbidden { action: "joining", channel, refused },
                    )
                    .await?;
                SecurityContext::system()
            }
            Entry::Founding => SecurityContext::system(),
        };
        self.participation.require_admissible(&resolved, context, &self.classification())?;

        let from = match entry {
            Entry::Founding => None,
            Entry::Added | Entry::Joining => Some(self.history_start(history).await?),
        };
        self.channel.add_participant(&storing, found.clone()).await?;

        if let Err(error) = self.start_participant(resolved, from, Some(context)).await {
            // Stored, but it cannot run: taken back out, in the runtime's
            // name - adding it was permitted.
            if let Err(undo) =
                self.channel.remove_participant(&SecurityContext::system(), participant).await
            {
                warn!(
                    "{} was added to channel {} but could not be started nor removed again: {}",
                    participant, self.id, undo
                );
            }
            return Err(error);
        }
        info!("{} joined channel {}", participant, self.id);
        Ok(())
    }

    /// Removes `participant` from the channel on behalf of `context` (issue
    /// 46): stored first (`UpdateChannel`), then its actor stopped - an agent
    /// at once, without finishing a turn; the approver discards its pending
    /// approvals. Until its task has ended, the participant cannot be added
    /// again (`left`, issue 171). The leave is announced by the
    /// `ChannelController` on the event bus (issue 73), and in the channel by
    /// `run` with a `ParticipantLeft` in the name of `context`'s caller,
    /// which its history records (issue 408), once the actor's task has
    /// ended: what it still sends until then - the
    /// step it was in, the end of its turn - enters the channel as its own,
    /// and nothing of it follows the announcement (issues 162, 262).
    /// Its state is kept, for the record and for when it joins again.
    ///
    /// A participant without an actor - a deleted agent resting in the
    /// channel, one that cannot run - is removed from the document alone
    /// (issue 314): restored, it does not come back here. An internal actor
    /// of the channel - its approver - is none (issue 420).
    ///
    /// # Errors
    /// `ChannelError::NotAParticipant`; a `StoreError` if `context` may not
    /// change the channel.
    pub async fn remove_participant(
        &self,
        context: &SecurityContext,
        participant: ActorId,
    ) -> Result<()> {
        if !self.channel().participants.iter().any(|p| p.actor_id() == participant) {
            return Err(ChannelError::NotAParticipant { participant, channel: self.id }.into());
        }
        let running = self.actors.load().get(&participant).cloned();
        self.channel.remove_participant(context, participant).await?;
        if let Some(running) = running {
            self.leave_actor(participant, running, context);
        }
        info!("{} left channel {}, removed by {}", participant, self.id, context.caller_id);
        Ok(())
    }

    /// Has the actors of the channel's agents follow whether the agents
    /// exist (issue 314) - of `only`, or of every agent taking part if
    /// `None`: a deleted agent rests in the channel, its actor stopped in the
    /// runtime's name and the document unchanged; a restored one has its
    /// actor built and started again, given the part of the history it may
    /// see, as when it joined. Either is announced as a leave or a join, in
    /// the runtime's name. One
    /// that cannot be read or run is logged and left as it is. Of a running
    /// one only whether it exists is asked, not again whether it may run.
    pub(crate) async fn follow_agents(&self, only: Option<AgentId>) {
        let agents: Vec<Participant> = self
            .channel()
            .participants
            .iter()
            .filter(|p| matches!(p, Participant::Agent(id) if only.is_none_or(|only| only == *id)))
            .cloned()
            .collect();
        for participant in agents {
            let Participant::Agent(id) = participant else { continue };
            let actor = participant.actor_id();
            match self.actors.load().get(&actor).cloned() {
                // Running: only whether it still exists matters.
                Some(running) => match self.participation.agent_exists(id).await {
                    Ok(true) => {}
                    Ok(false) => {
                        self.leave_actor(actor, running, &SecurityContext::system());
                        self.channel.announce_resting(id, true);
                        info!("{} was deleted and rests in channel {}", actor, self.id);
                    }
                    Err(e) => warn!(
                        "Whether {} of channel {} exists cannot be read: {}",
                        actor, self.id, e
                    ),
                },
                None => {
                    let rejoined = match self.resolve(&participant).await {
                        Ok(Some(resolved)) => match self.rejoining_from(actor).await {
                            Ok(from) => self
                                .start_participant(
                                    resolved,
                                    Some(from),
                                    Some(&SecurityContext::system()),
                                )
                                .await
                                .map(|()| true),
                            Err(e) => Err(e.into()),
                        },
                        Ok(None) => Ok(false),
                        Err(e) => Err(e),
                    };
                    match rejoined {
                        Ok(true) => {
                            self.channel.announce_resting(id, false);
                            info!(
                                "{} was restored and takes part in channel {} again",
                                actor, self.id
                            );
                        }
                        Ok(false) => {}
                        Err(e) => warn!(
                            "{} of channel {} cannot be run and is left out: {:#}",
                            actor, self.id, e
                        ),
                    }
                }
            }
        }
    }

    /// Starts the actor of `resolved` - the one way every actor of the
    /// channel is started: as the channel is built (`ChannelLoop::new`), as
    /// a participant is added or joins (`take_in`), as an agent is restored
    /// (`follow_agents`). Once the actor it had before has ended (`left`),
    /// built, taken in (`adopt`) and handed to `run` to start, which gives
    /// it the part of the history it may see - from the position `from` on,
    /// nothing if `None`: what is said from its start on -, records what it
    /// was given and announces its join in the name of `joined`'s caller
    /// (issues 408, 417, 423) - nothing if `None`, for an actor that starts
    /// again with its channel. `take_in` waits for `left` before it stores
    /// anything already; here it then finds nothing left to wait for.
    ///
    /// # Errors
    /// `ChannelError::StillLeaving`, or what building the actor reports; it
    /// is not taken in then.
    pub(super) async fn start_participant(
        &self,
        resolved: ResolvedParticipant,
        from: Option<Position>,
        joined: Option<&SecurityContext>,
    ) -> Result<()> {
        self.left(resolved.actor_id()).await?;
        let actor = self.create_actor(&resolved).await?;
        self.adopt(Arc::from(actor), resolved, from, joined.cloned()).await;
        Ok(())
    }

    /// Makes `running`, the actor of `participant`, leave (`Actor::leave`),
    /// and takes it out of the channel's actors, the moderator and the
    /// mentions; `run` announces it in the name of `remover` once its task
    /// has ended. The document is the caller's.
    fn leave_actor(&self, participant: ActorId, running: ChannelActor, remover: &SecurityContext) {
        let ChannelActor { actor, task, .. } = running;
        let name = actor.name();
        self.leaving
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(participant, task);
        self.actors.rcu(|actors| {
            let mut actors = HashMap::clone(actors);
            actors.remove(&participant);
            actors
        });
        self.moderator.remove_actor(participant);
        self.rebuild_mentions();
        actor.leave();
        let _ = self.changes_tx.send(ActorChange::Leave(participant, name, remover.clone()));
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
        let Some(task) = self
            .leaving
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&participant)
            .cloned()
        else {
            return Ok(());
        };
        // Closed by `run` once it stops the actor; not if the channel does
        // not run, and then nothing is tracked to wait for.
        task.close();
        if time::timeout(LEAVE_GRACE, task.wait()).await.is_err() {
            warn!(
                "{} is added to channel {} again, but its old actor has not ended",
                participant, self.id
            );
            return Err(ChannelError::StillLeaving { participant, channel: self.id });
        }
        self.leaving.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&participant);
        Ok(())
    }

    pub(super) fn rebuild_mentions(&self) {
        let mentions = Self::mention_directory(&self.actors.load(), &self.channel());
        for handle in mentions.ambiguous() {
            warn!("Channel {}: handle @{} is not a single participant's; mentioning it addresses more than one", self.id, handle);
        }
        self.mentions.store(Arc::new(mentions));
    }

    /// The ids of all participants of the channel, as `participants` lists them.
    pub fn participant_ids(&self) -> Vec<ActorId> {
        self.participants().into_iter().map(|participant| participant.id).collect()
    }

    /// All participants of the channel that run, in no particular order:
    /// not its internal actors (issue 420), which take no part.
    pub fn participants(&self) -> Vec<ChannelParticipant> {
        let channel = self.channel();
        self.actors
            .load()
            .values()
            .filter(|running| !channel.is_internal(running.actor.id()))
            .map(|ChannelActor { actor: a, .. }| {
                let name = a.name();
                let handle = Some(name.handle()).filter(|h| !h.is_empty());
                ChannelParticipant { id: a.id(), name, handle }
            })
            .collect()
    }

    /// The agents of the channel `text` names, as a user names one in a
    /// command (issue 277): those whose name it is - as a `Name`, regardless
    /// of case and spelling - or, if there are none, those whose handle it
    /// is, with or without its `@`, normalised as a mention is (`handle_of`).
    /// Several if it is ambiguous, none if it names no agent; sorted by name.
    pub fn agents_named(&self, text: &str) -> Vec<ChannelParticipant> {
        Self::named(
            self.participants().into_iter().filter(|m| m.id.as_agent_id().is_some()).collect(),
            text,
        )
    }

    /// The participants of the channel `text` names - agents and users -,
    /// found as `agents_named` finds agents: for a
    /// command that takes anyone taking part, such as `/nudge` (issue 278).
    pub fn participants_named(&self, text: &str) -> Vec<ChannelParticipant> {
        Self::named(self.participants(), text)
    }

    /// Those of `participants` that `text` names: by name, or else by handle;
    /// sorted by name.
    fn named(mut participants: Vec<ChannelParticipant>, text: &str) -> Vec<ChannelParticipant> {
        participants.sort_by(|l, r| l.name.as_str().cmp(r.name.as_str()));
        if let Ok(name) = Name::new(text) {
            let named: Vec<_> = participants.iter().filter(|m| m.name == name).cloned().collect();
            if !named.is_empty() {
                return named;
            }
        }
        let handle = handle_of(text);
        participants
            .into_iter()
            .filter(|m| !handle.is_empty() && m.handle.as_deref() == Some(handle.as_str()))
            .collect()
    }

    /// The agents of the channel that carry out commands themselves (issue
    /// 269), each with the commands `issuer` may see of it, by handle; an
    /// agent with none is left out.
    pub(crate) async fn agent_commands(
        &self,
        issuer: &SecurityContext,
    ) -> Vec<(ChannelParticipant, Vec<Command>)> {
        let mut found = Vec::new();
        for participant in self.participants().into_iter().filter(|m| m.id.as_agent_id().is_some())
        {
            let Some(actor) = self.actor(participant.id) else { continue };
            let commands = actor.commands(issuer).await;
            if !commands.is_empty() {
                found.push((participant, commands));
            }
        }
        found.sort_by(|(l, _), (r, _)| l.handle.cmp(&r.handle));
        found
    }

    /// `questions` of a call made in `context`, addressed to the users of the
    /// channel (issue 165): every addressee found among them - a handle as
    /// its participants are mentioned, the agent's owner and whoever caused
    /// the call as `context` names them (`Questions::resolve`).
    pub(crate) fn address_questions(
        &self,
        questions: Questions,
        context: &SecurityContext,
    ) -> Result<Questions, QuestionError> {
        let users: Vec<ActorId> = self
            .participant_ids()
            .into_iter()
            .filter(|id| matches!(id, ActorId::User(_)))
            .collect();
        questions.resolve(&self.mentions.load(), &users, context)
    }

    /// Whom of `actors`, those of `channel`, a mention can name: its
    /// participants, not its internal actors (issue 420).
    fn mention_directory(actors: &Actors, channel: &Channel) -> MentionDirectory {
        let names: Vec<(ActorId, Name)> = actors
            .values()
            .filter(|running| !channel.is_internal(running.actor.id()))
            .map(|running| (running.actor.id(), running.actor.name()))
            .collect();
        MentionDirectory::new(names.iter().map(|(id, name)| (*id, name.as_str())))
    }

    /// The participants `text` mentions by their `@`-handles, as the channel
    /// resolves them when a message enters it (issue 29) - everyone for
    /// `@all` or `@channel` (issue 401) - for an actor that shows its own
    /// message before (issue 220).
    pub(crate) fn mentions_in(&self, text: &str) -> Vec<ActorId> {
        self.mentions.load().resolve(text)
    }
}

#[cfg(test)]
mod tests {
    use caelumora_model::actor::ActorId;
    use caelumora_model::chain::Position;
    use caelumora_model::channel::{Channel, HistoryAccess, JOINED, LEFT};
    use caelumora_model::history::HistoryEntry;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_util::task::TaskTracker;

    use crate::channel::tests::*;
    use crate::testing::Script;

    use caelumora_model::value::name::Name;

    use crate::actor::user::ChannelWatch;
    use crate::actor::{Actor, Shown};
    use crate::channel::ChannelLoop;
    use crate::view::Viewer;
    use caelumora_model::actor::Envelope;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::security::{Grant, SecurityContext, Subject};
    use caelumora_model::session::ChannelId;

    use crate::error::is_forbidden;
    use crate::session::testing::*;
    use crate::session::{ChannelSpec, SessionError, SessionManager};
    use anyhow::Error;
    use caelumora_model::user::UserId;
    use tokio::time;
    #[tokio::test]
    async fn an_agent_is_found_by_its_name_or_its_handle_however_it_is_written() {
        let (channel, _, _) = channel_with_sleepers(&["Sassy", "Dr. Who", "Dr-Who", "Zoë"]).await;
        let named = |text: &str| {
            channel.agents_named(text).into_iter().map(|m| m.name.to_string()).collect::<Vec<_>>()
        };

        for text in ["Sassy", "sassy", "SASSY", "@Sassy", "@sassy"] {
            assert_eq!(named(text), ["Sassy"], "{text}");
        }
        assert_eq!(named("Zoe"), ["Zoë"], "by its ASCII form, as a name");
        assert_eq!(named("@zoë"), ["Zoë"], "and as a mention");
        assert_eq!(named("dr. who"), ["Dr. Who"], "a name before a handle: no ambiguity");
        assert_eq!(named("dr-who"), ["Dr-Who"], "the other's name");
        assert_eq!(named("@dr-who"), ["Dr-Who", "Dr. Who"], "the handle of both: ambiguous");
        assert!(named("alice").is_empty(), "a user is no agent");
        assert!(
            named("nobody").is_empty() && named("@").is_empty() && named("").is_empty(),
            "nobody"
        );
    }

    #[tokio::test]
    async fn all_and_channel_mention_every_participant_of_the_moment() {
        let (channel, alice, sleepers) = channel_with_sleepers(&["Sassy", "All"]).await;
        let everyone: HashSet<ActorId> =
            [ActorId::User(alice), sleepers[0].id(), sleepers[1].id()].into();

        for text in ["@all, what do you think?", "@channel?"] {
            assert_eq!(
                channel.mentions_in(text).into_iter().collect::<HashSet<_>>(),
                everyone,
                "{text}"
            );
        }
        assert_eq!(channel.mentions_in("@sassy"), [sleepers[0].id()], "a handle still names one");
    }

    #[tokio::test]
    async fn a_participant_is_added_again_only_once_its_old_actor_has_ended() {
        let (channel, _, agent, _) = scripted_channel(Script::Hangs).await;
        let participant = ActorId::Agent(agent);
        assert!(channel.left(participant).await.is_ok(), "nothing to wait for: it never left");

        // Its old actor takes a while to end, as one that stores its state.
        let old = TaskTracker::new();
        let ended = Arc::new(AtomicBool::new(false));
        let ending = Arc::clone(&ended);
        old.spawn(async move {
            time::sleep(Duration::from_millis(200)).await;
            ending.store(true, Ordering::SeqCst);
        });
        channel.leaving.lock().unwrap().insert(participant, old);

        channel.left(participant).await.expect("it ends within the grace");
        assert!(ended.load(Ordering::SeqCst), "not before its old actor ended");
        assert!(!channel.leaving.lock().unwrap().contains_key(&participant), "and is forgotten");
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn only_who_is_cleared_takes_part_in_or_reads_a_channel() {
        use crate::channel::ChannelError;
        use caelumora_model::confidentiality::{
            Clearance, Compartment, Compartments, Confidentiality, ConfidentialityRules,
        };
        use caelumora_model::role::{RoleId, Roles};
        use caelumora_model::user::User;

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let roles = Roles::new([
            (
                RoleId::new(),
                "staff".to_string(),
                vec![Clearance { level: Confidentiality::Internal, compartment: None }],
            ),
            (
                RoleId::new(),
                "hr".to_string(),
                vec![Clearance {
                    level: Confidentiality::Confidential,
                    compartment: Some(Compartment::new("hr")),
                }],
            ),
        ])
        .unwrap();
        let ids = roles.clone();
        let (_, manager) = owned_runtime(ConfidentialityRules::new(compartments).with_roles(roles));
        let user = |name: &str, roles: &[&str]| User {
            id: UserId::new(),
            username: Name::new(name).unwrap(),
            email: String::new(),
            roles: ids.ids_of(roles).unwrap(),
            deleted: None,
            revision: Default::default(),
        };
        let (alice, bob) = (user("alice", &["staff", "hr"]), user("bob", &["staff"]));
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        let mut session = make_session("hers", alice.id);
        session.security_attributes =
            session.security_attributes.clone().granting(Subject::Actor(bob_id), Grant::Read);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let hr_confidential =
            Classification::new(Confidentiality::Confidential, [Compartment::new("hr")]).unwrap();
        let with = |name: &str, classification: &Classification, participants| ChannelSpec {
            classification: Some(classification.clone()),
            ..spec(name, participants)
        };
        let refused = session
            .create_channel(&alice_ctx, with("hr", &hr_confidential, vec![bob_id]))
            .await
            .expect_err("bob is not cleared for HR");
        assert!(
            matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { participant, .. }) if *participant == bob_id),
            "{refused}"
        );
        let hr = session
            .create_channel(&alice_ctx, with("hr", &hr_confidential, vec![]))
            .await
            .expect("alice is");
        let joining = hr
            .add_participant(&alice_ctx, bob_id, HistoryAccess::All)
            .await
            .expect_err("nor may he join");
        assert!(
            matches!(joining.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { .. })),
            "{joining}"
        );

        let team = session
            .create_channel(
                &alice_ctx,
                with("team", &Classification::general(Confidentiality::Internal), vec![bob_id]),
            )
            .await
            .expect("both are cleared for internal");
        let raising = team
            .reclassify(&alice_ctx, Classification::general(Confidentiality::Confidential))
            .await
            .expect_err("above bob");
        assert!(
            matches!(raising.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { .. })),
            "{raising}"
        );
        assert_eq!(
            team.classification(),
            Classification::general(Confidentiality::Internal),
            "unchanged"
        );

        // Alice, no longer in HR - as her context says from now on -, no
        // longer sees its channel, though she owns it.
        let alice_ctx = alice_ctx.with_roles(ids.ids_of(&["staff"]).unwrap(), None);
        assert!(matches!(
            session
                .channel_for(&alice_ctx, hr.id())
                .await
                .expect_err("not cleared")
                .downcast_ref::<SessionError>(),
            Some(SessionError::ChannelNotFound { .. })
        ));
        let listed: Vec<ChannelId> = manager
            .views(Viewer::new(alice_ctx.clone()))
            .session(session.id())
            .await
            .unwrap()
            .channels()
            .await
            .iter()
            .map(|c| c.id())
            .collect();
        assert!(!listed.contains(&hr.id()) && listed.contains(&team.id()), "listed without it");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_participant_added_later_sees_the_history_granted_and_is_announced() {
        use crate::channel::ChannelError;
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let owner = match ctx.initiator_id {
            ActorId::User(id) => id,
            _ => unreachable!(),
        };
        let writer = channel.user_actor(owner).expect("the creator takes part");
        for text in ["one", "two", "three"] {
            writer.send_message(Some(text.to_string()), None, Vec::new()).await.unwrap();
        }
        let before = history_of(&channel, 3).await;
        let mut rx = runtime.event_bus().subscribe_all();

        channel
            .add_participant(&ctx, other, HistoryAccess::LastMessages { count: 1 })
            .await
            .expect("a user of the session joins");

        assert!(channel.participants().iter().any(|m| m.id == other), "a participant now");
        let other_user = match other {
            ActorId::User(id) => id,
            _ => unreachable!(),
        };
        assert!(channel.user_actor(other_user).is_some(), "with an actor of their own");
        // The history records what was shared with the newcomer, for it
        // alone and in the system's name, right before it joined - once its
        // actor runs (issue 417); the founder shared nothing (issue 420).
        let shared = |e: &HistoryEntry| {
            matches!(e, HistoryEntry::HistoryShared { .. } | HistoryEntry::ParticipantJoined { .. })
        };
        let recorded: Vec<_> = time::timeout(Duration::from_secs(5), async {
            loop {
                let recorded: Vec<_> =
                    channel.history_entries().await.into_iter().filter(shared).collect();
                if recorded.len() == 3 {
                    break recorded;
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("recorded");
        let from =
            channel.memberships(other).await.first().map(|range| range.start).expect("a member");
        assert!(from > Position::START, "from the last message");
        assert_eq!(
            channel.memberships(ctx.initiator_id).await.first().map(|range| range.start),
            Some(Position::START),
            "all of it: a founder"
        );
        assert!(
            matches!(recorded.as_slice(), [HistoryEntry::ParticipantJoined { participant: founder, .. }, HistoryEntry::HistoryShared { participant, from: f, by, .. }, HistoryEntry::ParticipantJoined { participant: joined, .. }]
            if *founder == ctx.caller_id && *participant == other && *f == from && *by == ctx.caller_id && *joined == other),
            "after the founder's own join (issue 420): {recorded:?}"
        );
        for (reader, seen) in [(other, 1), (ctx.caller_id, 0), (ActorId::User(UserId::new()), 0)] {
            let shared = channel
                .visible_entries(reader)
                .await
                .into_iter()
                .filter(|e| matches!(e, HistoryEntry::HistoryShared { .. }))
                .count();
            assert_eq!(shared, seen, "to the newcomer alone: {reader}");
        }
        let visible: Vec<_> =
            channel.visible_messages(other).await.into_iter().map(|m| m.content).collect();
        assert_eq!(visible, vec![Some("three".to_string())], "the last message and no more");
        assert_eq!(
            channel.visible_messages(ctx.initiator_id).await.len(),
            before.len(),
            "the others still see everything"
        );
        let stored = runtime.entity_stores().channels.get(&ctx, channel.id()).await.unwrap();
        assert!(
            stored.participants.iter().any(|p| p.actor_id() == other),
            "stored, not only in memory"
        );
        let events = drain(&mut rx);
        assert!(events.iter().any(|e| matches!(e.about::<Channel>(), Some(((_, cid), change)) if cid == channel.id() && change.data::<ActorId>(JOINED) == Some(other))),
                "the join is announced: {events:?}");

        // What is written after joining is visible.
        writer.send_message(Some("four".to_string()), None, Vec::new()).await.unwrap();
        history_of(&channel, 4).await;
        assert_eq!(channel.visible_messages(other).await.len(), 2);

        let refusal = |error: Error| error.downcast::<ChannelError>().expect("a ChannelError");
        let again =
            channel.add_participant(&ctx, other, HistoryAccess::All).await.expect_err("already in");
        assert!(matches!(refusal(again), ChannelError::AlreadyParticipant { .. }));
        let stranger = ActorId::User(UserId::new());
        let outside = channel
            .add_participant(&ctx, stranger, HistoryAccess::All)
            .await
            .expect_err("not in the session");
        assert!(matches!(refusal(outside), ChannelError::NotInSession { .. }));
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn who_joins_or_leaves_is_recorded_where_it_happened_and_shown_also_to_who_made_it() {
        use caelumora_model::actor::ActorEvent;
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let writer = channel
            .user_actor(ctx.initiator_id.as_user_id().unwrap())
            .expect("the creator takes part");
        let mut mine = writer.watch();
        writer.send_message(Some("before".to_string()), None, Vec::new()).await.unwrap();
        history_of(&channel, 1).await;
        async fn recorded(
            channel: &ChannelLoop,
            wanted: fn(&HistoryEntry) -> bool,
        ) -> Vec<HistoryEntry> {
            time::timeout(Duration::from_secs(5), async {
                loop {
                    let entries = channel.history_entries().await;
                    if entries.last().is_some_and(wanted) {
                        break entries;
                    }
                    time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("recorded")
        }
        async fn shown(watch: &mut ChannelWatch, wanted: fn(&ActorEvent) -> bool) -> Envelope {
            time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Ok(Shown::Envelope(envelope)) = watch.next().await {
                        if wanted(&envelope.event) {
                            break envelope;
                        }
                    }
                }
            })
            .await
            .expect("shown")
        }

        channel.add_participant(&ctx, other, HistoryAccess::None).await.expect("added");

        let entries =
            recorded(&channel, |e| matches!(e, HistoryEntry::ParticipantJoined { .. })).await;
        assert!(matches!(&entries[0], HistoryEntry::ParticipantJoined { participant, .. } if *participant == ctx.caller_id),
            "first the founder, who joined as anyone does, given nothing before (issue 420): {entries:?}");
        assert!(
            entries[1].said().is_some_and(|m| m.content.as_deref() == Some("before")),
            "after what was said before: {entries:?}"
        );
        assert!(
            matches!(&entries[2], HistoryEntry::ParticipantJoined { participant, name, by, by_name, .. }
            if *participant == other && name == "other" && *by == ctx.caller_id && by_name == "owner"),
            "who, and by whose act, named as they are: {entries:?}"
        );
        assert_eq!(
            entries.len(),
            3,
            "granted nothing before its join, nothing shared is recorded (issue 417): {entries:?}"
        );
        let visible = channel.visible_entries(other).await;
        assert!(
            matches!(visible.as_slice(), [HistoryEntry::ParticipantJoined { participant, .. }] if *participant == other),
            "the newcomer sees its own join, and nothing before it: {visible:?}"
        );
        let told = shown(&mut mine, |e| matches!(e, ActorEvent::ParticipantJoined { .. })).await;
        assert_eq!(
            told.context.sender_id, ctx.caller_id,
            "shown to who added them, in whose name it is told"
        );

        channel.remove_participant(&ctx, other).await.expect("removed");

        let entries =
            recorded(&channel, |e| matches!(e, HistoryEntry::ParticipantLeft { .. })).await;
        assert!(
            matches!(entries.last(), Some(HistoryEntry::ParticipantLeft { participant, by, .. }) if *participant == other && *by == ctx.caller_id),
            "{entries:?}"
        );
        let told = shown(&mut mine, |e| matches!(e, ActorEvent::ParticipantLeft { .. })).await;
        assert!(
            matches!(told.event, ActorEvent::ParticipantLeft { participant, ref name, .. } if participant == other && name == "other")
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_users_followers_are_told_their_actor_ended_when_they_leave_or_the_channel_is_deleted(
    ) {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let owner = ctx.initiator_id.as_user_id().unwrap();
        async fn ended(watch: &mut ChannelWatch, within: Duration) -> bool {
            time::timeout(within, async {
                while !matches!(watch.next().await, Ok(Shown::Ended)) {}
            })
            .await
            .is_ok()
        }

        // Leaving ends it.
        let channel = session.create_channel(&ctx, spec("side", vec![other])).await.unwrap();
        let mut theirs = channel.user_actor(other.as_user_id().unwrap()).unwrap().watch();
        let mut mine = channel.user_actor(owner).unwrap().watch();
        channel.remove_participant(&ctx, other).await.expect("removed");
        assert!(
            ended(&mut theirs, Duration::from_secs(5)).await,
            "whoever left is told their actor ended (issue 386)"
        );
        assert!(!ended(&mut mine, Duration::from_millis(50)).await, "everybody else's goes on");

        // So does deleting the channel, for everybody.
        session.delete_channel(&ctx, channel.id()).await.expect("deleted");
        assert!(ended(&mut mine, Duration::from_secs(5)).await, "a deleted channel runs no actor");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_removed_participant_is_kept_as_a_former_one_and_announced_but_the_approver_stays() {
        use crate::channel::ChannelError;
        use caelumora_model::approval::ApprovalStrategy;
        use caelumora_model::channel::InternalActor;
        use caelumora_model::template::InternalActorTemplate;
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let with_approver = ChannelSpec {
            internal_actors: vec![InternalActorTemplate::Approver {
                strategy: ApprovalStrategy::Any,
            }],
            ..spec("side", vec![other])
        };
        let channel = session.create_channel(&ctx, with_approver).await.unwrap();
        let mut rx = runtime.event_bus().subscribe_all();

        channel.remove_participant(&ctx, other).await.expect("removed");

        assert!(!channel.participants().iter().any(|m| m.id == other), "no longer a participant");
        let stored = runtime.entity_stores().channels.get(&ctx, channel.id()).await.unwrap();
        assert!(
            !stored.participants.iter().any(|p| p.actor_id() == other),
            "stored, not only in memory"
        );
        let former = stored
            .former
            .iter()
            .find(|f| f.participant.actor_id() == other)
            .expect("kept as a former participant (issue 164)");
        assert_eq!(former.left.by, ctx.caller_id, "with who removed it");
        let events = drain(&mut rx);
        assert!(events.iter().any(|e| matches!(e.about::<Channel>(), Some(((_, cid), change)) if cid == channel.id() && change.data::<ActorId>(LEFT).is_some())),
                "the leave is announced: {events:?}");

        let refusal = |error: Error| error.downcast::<ChannelError>().expect("a ChannelError");
        let gone = channel.remove_participant(&ctx, other).await.expect_err("not in it any more");
        assert!(matches!(refusal(gone), ChannelError::NotAParticipant { .. }));
        let approver = channel
            .channel()
            .internal_actors
            .iter()
            .map(InternalActor::actor_id)
            .next()
            .expect("an approver");
        assert!(!channel.participant_ids().contains(&approver), "no participant (issue 420)");
        let stays =
            channel.remove_participant(&ctx, approver).await.expect_err("nor removed as one");
        assert!(matches!(refusal(stays), ChannelError::NotAParticipant { .. }));
        assert!(channel.actor(approver).is_some(), "and runs on");

        // Added again at once, once its old actor has ended (issue 171).
        channel.add_participant(&ctx, other, HistoryAccess::All).await.expect("added again");
        assert!(channel.user_actor(other.as_user_id().unwrap()).is_some(), "with a new actor");
        let stored = runtime.entity_stores().channels.get(&ctx, channel.id()).await.unwrap();
        assert!(stored.former.is_empty(), "and no longer a former participant");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn only_who_may_change_a_channel_adds_or_removes_its_participants() {
        let (_, manager) = owned_runtime(Default::default());
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let alice_id = ActorId::User(alice.id);
        let bob_id = ActorId::User(bob.id);
        let carol_id = ActorId::User(carol.id);
        let as_user = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(&manager, &[&alice, &bob, &carol]).await;
        session.security_attributes = session
            .security_attributes
            .clone()
            .granting(Subject::Actor(bob_id), Grant::Read)
            .granting(Subject::Actor(carol_id), Grant::Read);
        let running = create(&manager, &as_user(alice_id), session).await.unwrap();
        let channel =
            running.create_channel(&as_user(alice_id), spec("side", vec![bob_id])).await.unwrap();

        let refused = channel
            .add_participant(&as_user(bob_id), carol_id, HistoryAccess::All)
            .await
            .expect_err("a participant may not");
        assert!(is_forbidden(&refused), "{refused}");
        let refused = channel
            .remove_participant(&as_user(bob_id), bob_id)
            .await
            .expect_err("nor remove anyone");
        assert!(is_forbidden(&refused), "{refused}");
        assert_eq!(channel.participants().len(), 2, "alice and bob, unchanged");

        channel
            .add_participant(&as_user(alice_id), carol_id, HistoryAccess::None)
            .await
            .expect("the owner may");
        channel.remove_participant(&as_user(alice_id), bob_id).await.expect("the owner may");
        assert!(
            channel.participants().iter().any(|m| m.id == carol_id)
                && !channel.participants().iter().any(|m| m.id == bob_id)
        );
        manager.stop(&SecurityContext::system(), running.id()).await.unwrap();
    }
}
