//! The participants of a running channel: their actors started and ended, taken into further
//! threads and let go from one (issue 602), following whether their agents exist, their mentions,
//! and finding them by name.

use super::{
    ActorChange, Actors, Answer, ChannelActor, ChannelError, ChannelLoop, ChannelParticipant,
    ResolvedParticipant, ThreadChange, LEAVE_GRACE,
};
use anyhow::{Error, Result};
use arc_swap::ArcSwap;
use caelumora_api::actor::Actor;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::chain::Position;
use caelumora_domain::channel::Channel;
use caelumora_domain::channel::Circle;
use caelumora_domain::channel::Participant;
use caelumora_domain::channel::ThreadId;
use caelumora_domain::command::CommandDescription;
use caelumora_domain::mention::MentionDirectory;
use caelumora_domain::question::{QuestionError, Questions};
use caelumora_domain::security::SecurityAttributes;
use caelumora_domain::value::name::{named, Name};
use caelumora_security::{ActionType, SecurityContext};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use tokio::time;
use tracing::{info, warn};

impl ChannelLoop {
    /// Ends what `participant` takes part in of what runs in the channel, on behalf of `by`, for a
    /// procedure that lets them go (`procedure::participant::let_go_from_channel`, or
    /// `let_go_from_session` for every running channel of the session), which stored it, or takes
    /// their start back: its actor stopped - an agent at once, without finishing a
    /// turn; the approver discards its pending approvals -, and its task's end waited for (`left`,
    /// issue 171): the procedure holds the participant meanwhile, so that nobody takes them in
    /// again before. `run` announces the leave with a `ParticipantLeft` in the name of `by`'s
    /// caller, which the history records (issue 408), once the actor's task has ended: what it
    /// still sends until then - the step it was in, the end of its turn - enters the channel as its
    /// own, and nothing of it follows the announcement (issues 162, 262). Its state is kept, for
    /// the record and for when it joins again. Asks nothing, and the document is the caller's.
    ///
    /// A participant without an actor - a deleted agent resting in the channel, one that cannot
    /// run - has nothing to stop; where its attendance in the history is still open - it was left
    /// out as the channel started rather than stopped while it ran -, its leave is recorded at
    /// once (issue 590). An internal actor of the channel - its approver - is none (issue 420).
    pub(crate) async fn remove_actor(&self, participant: ActorId, by: &SecurityContext) {
        let running = self.actors.load().get(&participant).cloned();
        let left = match running {
            Some(running) => Some(self.leave_actor(participant, running, by)),
            // Without an actor - left out as the channel started - but taking part in its history
            // all the same: its leave is recorded at once, so that its attendance ends here.
            None => match self.attending().await.remove(&participant) {
                Some(threads) => {
                    let name = self.name_of(participant).await;
                    let threads = Arc::new(ArcSwap::from_pointee(threads));
                    let leave = ActorChange::Leave(participant, name, by.clone(), threads);
                    Some(self.request(leave))
                }
                None => None,
            },
        };
        if let Some(left) = left {
            self.left(participant, left).await;
        }
        info!("{} left channel {}, removed by {}", participant, self.id, by.caller_id);
    }

    /// Has the actors of the channel's agents follow whether the agents exist (issue 314) - of
    /// `only`, or of every agent taking part if `None`: a deleted agent rests in the channel, its
    /// actor stopped in the kernel's name and the document unchanged; a restored one has its actor
    /// built and started again, given the part of the history it may see, as when it joined. Either
    /// is announced as a leave or a join, in the kernel's name - also for an agent listed without
    /// an actor that did not rest and for which no leave was announced: left out as the channel
    /// started, since what it runs with - its personality, its components - was withdrawn, or one
    /// whose start failed as it was added and whose rollback failed too. For it this join is one
    /// too many on the bus; the history records it, since its actor starts only now, and keeps the
    /// range of attendance it has open (`Attendance`, issue 568). Rare - it needs what it runs with
    /// given back while the session runs, then one of the occasions that have the agents followed;
    /// telling it apart would need the channel to keep why it left out whom (issue 564). One that
    /// cannot be read or run is logged and left as it is. Of a running one only whether it exists
    /// is asked, not again whether it may run. For a procedure that holds the agents
    /// (`participant::Follow`), or a session it starts (`procedure::session::open`): nobody takes
    /// them in or lets them go meanwhile (issue 593).
    pub(crate) async fn follow_agents(&self, only: Option<&HashSet<AgentId>>) {
        let agents: Vec<Participant> = self
            .channel()
            .participants
            .iter()
            .filter(|p| matches!(p, Participant::Agent(id) if only.is_none_or(|only| only.contains(id))))
            .cloned()
            .collect();
        for participant in agents {
            let Participant::Agent(id) = participant else { continue };
            let actor = participant.actor_id();
            match self.actors.load().get(&actor).cloned() {
                // Running: only whether it still exists matters.
                Some(running) => match self.controllers.agent_exists(id).await {
                    Ok(true) => {}
                    Ok(false) => {
                        let system = SecurityContext::system();
                        let left = self.leave_actor(actor, running, &system);
                        // For a procedure that holds the agent (`participant::Follow`), or a
                        // session it starts, whose channels have no task to wait for (`left`).
                        self.left(actor, left).await;
                        self.channel.announce_resting(id, true);
                        info!("{} was deleted and rests in channel {}", actor, self.id);
                    }
                    Err(e) => warn!(
                        "Whether {} of channel {} exists cannot be read: {}",
                        actor, self.id, e
                    ),
                },
                None => {
                    let rejoined = async {
                        let Some(resolved) = self.resolve(&participant).await? else {
                            return Ok(None);
                        };
                        self.participation.require_runnable(&resolved).await?;
                        let (from, threads) = self.rejoining(actor).await?;
                        let threads = self.taking_part(Some(&threads));
                        let joined = SecurityContext::system();
                        let from = Some(from);
                        let started = self.add_actor(resolved, threads, from, Some(&joined));
                        Ok::<_, Error>(Some(started.await?))
                    }
                    .await;
                    match rejoined {
                        Ok(Some(started)) => {
                            // Started once `run` answers, as taking in waits for it.
                            started.carried_out().await;
                            self.channel.announce_resting(id, false);
                            info!(
                                "{} was restored and takes part in channel {} again",
                                actor, self.id
                            );
                        }
                        Ok(None) => {}
                        Err(e) => warn!(
                            "{} of channel {} cannot be run and is left out: {:#}",
                            actor, self.id, e
                        ),
                    }
                }
            }
        }
    }

    /// Starts the actor of `resolved` to take part in `threads` (issues 557, 602) - the one way
    /// every actor of the channel is started: as the channel is built (`ChannelLoop::new`), as a
    /// participant is added, joins or founds (`procedure::participant::take_into_channel`), as an
    /// agent is restored (`follow_agents`). Built, taken in (`adopt`) and handed to `run` to start,
    /// which gives it the part of the history it may see - from the position `from` on, nothing if
    /// `None`: what is said from its start on -, records what it was given and announces its join
    /// in the name of `joined`'s caller (issues 408, 417, 423) - nothing if `None`, for an actor
    /// that starts again with its channel. The actor it had before has ended: letting it go waited
    /// for that (`left`). What `run` answers once it started it (issue 593). Asks nothing: whoever
    /// takes them in decided that they may.
    ///
    /// # Errors
    /// What building the actor reports; it is not taken in then.
    pub(crate) async fn add_actor(
        &self,
        resolved: ResolvedParticipant,
        threads: BTreeSet<ThreadId>,
        from: Option<Position>,
        joined: Option<&SecurityContext>,
    ) -> Result<Answer> {
        let threads = Arc::new(ArcSwap::from_pointee(threads));
        let actor = self.create_actor(&resolved, &threads).await?;
        Ok(self.adopt(Arc::from(actor), resolved, threads, from, joined.cloned()).await)
    }

    /// Takes the running actor of `participant` into `thread` as well, on behalf of `by` (issue
    /// 602), for a procedure that holds the participant and the thread
    /// (`procedure::participant::take_into_thread`): `run` has it take part there - the thread's
    /// moderator may ask it, and what happens there is delivered to it - as it announces its join
    /// there in the name of `by`'s caller (`ParticipantJoined`). Of an agent, its branch of the
    /// thread forks at the message the thread hangs at, as the agent takes its join in.
    /// What `run` answers once it announced it, or found it drawn in meanwhile (`draw_in`). Asks
    /// nothing of `by`: the procedure did.
    ///
    /// # Errors
    /// `ChannelError::ThreadNotOpen`; `ClosedCircle` for a thread whose circle is closed;
    /// `NotAParticipant` if no actor of theirs runs in the channel; `AlreadyInThread`;
    /// `InOneThreadAlone` for an actor that takes part in one thread alone
    /// (`Actor::takes_part_in_several_threads`).
    pub(crate) async fn take_into_thread(
        &self,
        participant: ActorId,
        thread: ThreadId,
        by: &SecurityContext,
    ) -> Result<Answer> {
        let channel = self.id;
        let there = self.thread(thread).ok_or(ChannelError::ThreadNotOpen { thread, channel })?;
        if there.circle() == Circle::Closed {
            return Err(ChannelError::ClosedCircle { thread, channel }.into());
        }
        let running = self.actors.load().get(&participant).cloned();
        let running = running.ok_or(ChannelError::NotAParticipant { participant, channel })?;
        if running.takes_part_in(thread) {
            return Err(ChannelError::AlreadyInThread { participant, thread, channel }.into());
        }
        if !running.actor.takes_part_in_several_threads() {
            return Err(ChannelError::InOneThreadAlone { participant, channel }.into());
        }
        let name = running.actor.name().to_string();
        let joins = ActorChange::Thread(participant, name, thread, ThreadChange::Joins, by.clone());
        Ok(self.request(joins))
    }

    /// Lets the running actor of `participant` go from `thread` alone, on behalf of `by` (issue
    /// 602), for a procedure that holds the participant and the thread
    /// (`procedure::participant::let_go_from_thread`): `run` has it no longer take part there - its
    /// turn running there ends (`Actor::cancel`), the thread's moderator no longer asks it, nothing
    /// of the thread is delivered to it - as it announces its leave there
    /// in the name of `by`'s caller (`ParticipantLeft`), which reaches the actor all the same; it
    /// goes on in its other threads. Of an agent, its part and its branch of the thread go, as the
    /// agent takes its leave in. What `run` answers once it announced it. Asks nothing of `by`:
    /// the procedure did.
    ///
    /// # Errors
    /// `ChannelError::NotAParticipant` if no actor of theirs runs in the channel; `NotInThread`;
    /// `LastThread` for the one thread it takes part in, which it leaves by leaving the channel.
    pub(crate) fn let_go_from_thread(
        &self,
        participant: ActorId,
        thread: ThreadId,
        by: &SecurityContext,
    ) -> Result<Answer> {
        let channel = self.id;
        let running = self.actors.load().get(&participant).cloned();
        let running = running.ok_or(ChannelError::NotAParticipant { participant, channel })?;
        // One reading of where it takes part, for whether it does there and whether only there.
        let threads = running.threads.load();
        if !threads.contains(&thread) {
            return Err(ChannelError::NotInThread { participant, thread, channel }.into());
        }
        if threads.len() == 1 {
            return Err(ChannelError::LastThread { participant, thread, channel }.into());
        }
        let name = running.actor.name().to_string();
        let leaves =
            ActorChange::Thread(participant, name, thread, ThreadChange::Leaves, by.clone());
        Ok(self.request(leaves))
    }

    /// Has the running actor of `participant` take part in `thread`, or no longer, as `change`
    /// says (issue 602): in its threads, which its runtime shares, and with the thread's moderator;
    /// leaving, its turn running there ends (`Actor::cancel`). For `run`, which announces it right
    /// after; whether it changed anything - not for an actor that took part so already -, `None`
    /// for an actor no longer running or a thread no longer open.
    pub(super) fn take_part(
        &self,
        participant: ActorId,
        thread: ThreadId,
        change: ThreadChange,
    ) -> Option<bool> {
        let there = self.thread(thread)?;
        let running = self.actors.load().get(&participant).cloned()?;
        if running.takes_part_in(thread) == (change == ThreadChange::Joins) {
            return Some(false);
        }
        if change == ThreadChange::Leaves {
            running.actor.cancel(Some(thread));
        }
        running.threads.rcu(|threads| {
            let mut threads = BTreeSet::clone(threads);
            match change {
                ThreadChange::Joins => threads.insert(thread),
                ThreadChange::Leaves => threads.remove(&thread),
            };
            threads
        });
        let taking = match change {
            ThreadChange::Joins => {
                there.add_actor(Arc::clone(&running.actor));
                "takes part in"
            }
            ThreadChange::Leaves => {
                there.remove_actor(participant);
                "no longer takes part in"
            }
        };
        info!("{} {} thread {} of channel {}", participant, taking, thread, self.id);
        Some(true)
    }

    /// Makes `running`, the actor of `participant`, leave (`Actor::leave`),
    /// and takes it out of the channel's actors, its threads and the
    /// mentions; `run` announces it in the name of `remover` once its task
    /// has ended, and answers then (`left`). The document is the caller's.
    fn leave_actor(
        &self,
        participant: ActorId,
        running: ChannelActor,
        remover: &SecurityContext,
    ) -> Answer {
        let ChannelActor { actor, threads, .. } = running;
        let name = actor.name().to_string();
        self.drop_actor(participant, &actor, &threads.load());
        self.request(ActorChange::Leave(participant, name, remover.clone(), threads))
    }

    /// Takes `actor`, of `id`, out of the channel's actors, of the `threads` it takes part in and
    /// of the mentions, and makes it leave (`Actor::leave`): what letting it go means for the
    /// running channel, a participant's actor (`leave_actor`) or a thread's delegate
    /// (`let_delegate_go`).
    pub(super) fn drop_actor(
        &self,
        id: ActorId,
        actor: &Arc<dyn Actor>,
        threads: &BTreeSet<ThreadId>,
    ) {
        self.actors.rcu(|actors| {
            let mut actors = HashMap::clone(actors);
            actors.remove(&id);
            actors
        });
        for taking_part in threads.iter().filter_map(|thread| self.thread(*thread)) {
            taking_part.remove_actor(id);
        }
        self.rebuild_mentions();
        actor.leave();
    }

    /// Waits until `run` carried out the leave of `participant` it answers with `left` (issue
    /// 593): its actor's task has ended and its leave is announced - or the channel stopped, its
    /// actor with it. Until then the actor may still store the participant's state - a turn given
    /// up - which a new actor loads as it is built, and what it sends would pass as the new
    /// actor's (issue 171): whoever lets a participant go waits for it, holding the participant,
    /// so that nobody takes them in again meanwhile - not at all where `run` has not started, which
    /// never starts an actor removed before (issue 171). At most `LEAVE_GRACE`: by then `run` has
    /// aborted the actor; one that has not ended even so - it blocks without ever reaching an
    /// `await` - is logged.
    async fn left(&self, participant: ActorId, left: Answer) {
        // Before `run` started, no actor of the channel had a task: one removed is never started.
        if !self.has_run() {
            return;
        }
        if time::timeout(LEAVE_GRACE, left.carried_out()).await.is_err() {
            warn!("{} left channel {}, but its actor has not ended", participant, self.id);
        }
    }

    /// Builds the handles of the participants anew from the names of the actors that run in the
    /// channel: as one joins or leaves, and for a session that lost the announcements of renames
    /// (issue 585).
    pub(crate) fn rebuild_mentions(&self) {
        self.store_mentions(None);
    }

    /// Takes in that `participant` is called `name` now, as announced: mentions resolve by the
    /// handle of its new name from now on. The name is the announcement's, as its actor may not
    /// tell it yet - the controller the actor reads takes over a rename made elsewhere in a task of
    /// its own (issue 585). The next rebuild reads the actors again: one before that controller
    /// took the rename over - as another participant joins or leaves meanwhile - brings the old
    /// handle back, until the rebuild after. Changes nothing where it takes no part.
    pub(crate) fn participant_renamed(&self, participant: ActorId, name: &Name) {
        if self.actors.load().contains_key(&participant) {
            self.store_mentions(Some((participant, name)));
        }
    }

    /// Builds the handles of the participants from the names of the actors that run in the
    /// channel - `renamed` called by the name given - and keeps them, warning of a handle that is
    /// not one participant's.
    fn store_mentions(&self, renamed: Option<(ActorId, &Name)>) {
        let _building =
            self.building_mentions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mentions = Self::mention_directory(&self.actors.load(), &self.channel(), renamed);
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
                let handle = name.handle();
                ChannelParticipant { id: a.id(), name, handle }
            })
            .collect()
    }

    /// The agents of the channel `text` names, as a user names one in a command (issue 277),
    /// found as `named` finds participants: several if it is ambiguous, none if it names no
    /// agent.
    pub fn agents_named(&self, text: &str) -> Vec<ChannelParticipant> {
        named(
            self.participants().into_iter().filter(|m| m.id.as_agent_id().is_some()).collect(),
            text,
            |m| &m.name,
        )
    }

    /// The agents of the channel that carry out commands themselves (issue
    /// 269), each with the commands `issuer` may see of it, by handle; an
    /// agent with none is left out.
    pub(crate) async fn agent_commands(
        &self,
        issuer: &SecurityContext,
    ) -> Vec<(ChannelParticipant, Vec<CommandDescription>)> {
        let mut found = Vec::new();
        for participant in self.participants().into_iter().filter(|m| m.id.as_agent_id().is_some())
        {
            let commands = self.commands_of(participant.id, issuer).await;
            if !commands.is_empty() {
                found.push((participant, commands));
            }
        }
        found.sort_by(|(l, _), (r, _)| l.handle.cmp(&r.handle));
        found
    }

    /// The commands the agent `agent` carries out itself that `issuer` may see in this channel
    /// (`ShowAgentCommand`, issue 516), by name: asked with the agent's attributes and the
    /// channel's, as a command is admitted at the door (`admit_command`). None if `agent` is no
    /// agent taking part.
    pub(crate) async fn commands_of(
        &self,
        agent: ActorId,
        issuer: &SecurityContext,
    ) -> Vec<CommandDescription> {
        let Some((commands, attributes)) = self.commands_held(agent).await else {
            return Vec::new();
        };
        let channel = self.security_attributes();
        let mut visible = Vec::new();
        for command in commands {
            let show = ActionType::ShowAgentCommand {
                command: &command,
                agent: &attributes,
                channel: &channel,
            };
            if self.policies.access_policy().check(issuer, show).await {
                visible.push(command);
            }
        }
        visible
    }

    /// The commands the agent `agent` carries out in this channel, and its attributes, by which
    /// who may see and ask for them is decided (`ShowAgentCommand`, `ExecuteAgentCommand`); `None`
    /// if it is no agent taking part, or no longer exists.
    pub(super) async fn commands_held(
        &self,
        agent: ActorId,
    ) -> Option<(Vec<CommandDescription>, SecurityAttributes)> {
        let actor = self.agent_actor(agent)?;
        let attributes = self.controllers.agent_attributes(agent.as_agent_id()?).await.ok()?;
        Some((actor.commands(), attributes))
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
        questions.resolve(&self.mentions.load(), &users, context.owner_id, context.initiator_id)
    }

    /// Whom of `actors`, those of `channel`, a mention can name: its participants, not its
    /// internal actors (issue 420), by their names - that of `renamed` the one given.
    fn mention_directory(
        actors: &Actors,
        channel: &Channel,
        renamed: Option<(ActorId, &Name)>,
    ) -> MentionDirectory {
        let name_of = |running: &ChannelActor| match renamed {
            Some((participant, name)) if participant == running.actor.id() => name.clone(),
            _ => running.actor.name(),
        };
        let names: Vec<(ActorId, Name)> = actors
            .values()
            .filter(|running| !channel.is_internal(running.actor.id()))
            .map(|running| (running.actor.id(), name_of(running)))
            .collect();
        MentionDirectory::new(names.iter().map(|(id, name)| (*id, name.as_str())))
    }

    /// The handles of the participants that run in the channel, by which it resolves the
    /// mentions of a message as it enters (issue 29) - for `@all` or `@channel` those taking part
    /// in the message's thread (issues 401, 594, `taking_part_in`).
    pub fn mentions(&self) -> Arc<MentionDirectory> {
        self.mentions.load_full()
    }
}

#[cfg(test)]
mod tests {
    use crate::actor::user::ChannelWatch;
    use crate::actor::Shown;
    use crate::channel::testing::Policy;
    use crate::channel::testing::Stuck;
    use crate::channel::tests::*;
    use crate::channel::{Answer, ChannelError, ChannelLoop};
    use crate::error::is_forbidden;
    use crate::session::testing::*;
    use crate::session::{ChannelSpec, SessionError};
    use crate::testing::{attributes, Script, ScriptedAgent, ScriptedAgents, World};
    use crate::view::Viewer;
    use anyhow::Error;
    use caelumora_api::actor::{agent_factory, Actor};
    use caelumora_api::library::{Library, LibraryBuilder};
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::actor::Envelope;
    use caelumora_domain::agent::{Personality, PersonalityKind};
    use caelumora_domain::chain::Position;
    use caelumora_domain::channel::{Channel, ChannelId, HistoryAccess, ThreadId, JOINED, LEFT};
    use caelumora_domain::confidentiality::Classification;
    use caelumora_domain::history::{HistoryEntry, Reader};
    use caelumora_domain::security::{Grant, Subject};
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::store::StoreError;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::UserId;
    use caelumora_domain::value::name::Name;
    use caelumora_security::{ActionType, SecurityContext};
    use caelumora_store::testing::record_users;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::oneshot;
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
    async fn all_and_channel_mention_every_participant_of_the_line_of_the_moment() {
        let (channel, alice, sleepers) = channel_with_sleepers(&["Sassy", "All"]).await;
        let everyone: HashSet<ActorId> =
            [ActorId::User(alice), sleepers[0].id(), sleepers[1].id()].into();

        for text in ["@all, what do you think?", "@channel?"] {
            let line = channel.taking_part_in(ThreadId::line(channel.id));
            let (named, addressed) = channel.mentions().resolve(text, &line);
            assert_eq!(addressed.into_iter().collect::<HashSet<_>>(), everyone, "{text}");
            assert!(named.is_empty(), "{text}: nobody by name");
        }
        assert_eq!(
            channel.mentions().resolve("@sassy", &[]).0,
            [sleepers[0].id()],
            "a handle still names one"
        );
    }

    /// Issue 585: a participant renamed, as announced, is mentioned by the handle of its new name -
    /// before its actor tells it -, until the handles are built anew from the actors; one that
    /// takes no part changes nothing.
    #[tokio::test]
    async fn a_participant_renamed_is_mentioned_by_the_name_announced() {
        let (channel, _, sleepers) = channel_with_sleepers(&["Sassy"]).await;
        let sassy = sleepers[0].id();
        channel.participant_renamed(sassy, &Name::new("Saucy").unwrap());
        assert_eq!(channel.mentions().resolve("@saucy", &[]).0, [sassy], "by the name announced");
        assert!(channel.mentions().resolve("@sassy", &[]).0.is_empty(), "not by the old one");

        channel.participant_renamed(ActorId::User(UserId::new()), &Name::new("Sassy").unwrap());
        assert_eq!(channel.mentions().resolve("@saucy", &[]).0, [sassy], "nobody taking part");
        channel.rebuild_mentions();
        assert_eq!(channel.mentions().resolve("@sassy", &[]).0, [sassy], "by what its actor tells");
    }

    /// Issue 593: who lets a participant go waits until `run` answers that its old actor has
    /// ended (`left`), so that nobody takes them in again meanwhile (issue 171) - once `run` has
    /// started: before, no actor of the channel had a task. Asked of `left` itself, with an answer
    /// that comes late: the scripted agents end at once as they leave.
    #[tokio::test]
    async fn letting_a_participant_go_waits_until_its_old_actor_has_ended() {
        let Stuck { channel, agent, running, .. } = stuck_in_a_turn().await;
        let (ended_tx, ended_rx) = oneshot::channel();
        let ended = Arc::new(AtomicBool::new(false));
        let ending = Arc::clone(&ended);
        tokio::spawn(async move {
            time::sleep(Duration::from_millis(200)).await;
            ending.store(true, Ordering::SeqCst);
            let _ = ended_tx.send(());
        });

        channel.left(ActorId::Agent(agent), Answer(ended_rx)).await;
        assert!(ended.load(Ordering::SeqCst), "not before its old actor ended");
        running.abort();
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn only_who_is_cleared_takes_part_in_or_reads_a_channel() {
        use crate::channel::ChannelError;
        use caelumora_domain::confidentiality::{
            Clearance, Compartment, Compartments, Confidentiality,
        };
        use caelumora_domain::role::{RoleId, Roles};
        use caelumora_domain::user::User;
        use caelumora_security::confidentiality::ConfidentialityRules;

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
        let kernel = owned_kernel(ConfidentialityRules::new(compartments).with_roles(roles));
        let user = |name: &str, roles: &[&str]| User {
            roles: ids.ids_of(roles).unwrap(),
            ..make_user(name)
        };
        let (alice, bob) = (user("alice", &["staff", "hr"]), user("bob", &["staff"]));
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        // Her roles in her context, as her login gives them: she reaches the channels she is
        // cleared for (issue 593, reclassifying reaches the channel as `channel_for` does).
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default())
            .with_roles(alice.roles.clone(), None);
        let mut session = make_session("hers", alice.id);
        session.security_attributes =
            session.security_attributes.clone().granting(Subject::Actor(bob_id), Grant::Read);
        session.participants = record_users(kernel.entity_stores(), &[&alice, &bob]).await;
        let session = create(&kernel, &alice_ctx, session).await.unwrap();

        let hr_confidential =
            Classification::new(Confidentiality::Confidential, [Compartment::new("hr")]).unwrap();
        let with = |name: &str, classification: &Classification, participants| ChannelSpec {
            classification: classification.clone(),
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
            .reclassify_by_procedure(
                &alice_ctx,
                Classification::general(Confidentiality::Confidential),
            )
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
        let listed: Vec<ChannelId> = kernel
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
        kernel.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// How often `participant` joined `channel` by its history, once the first join is recorded and
    /// whatever else was under way had time to be recorded too.
    async fn joins_recorded(channel: &ChannelLoop, participant: ActorId) -> usize {
        let joins = |entries: Vec<HistoryEntry>| {
            entries
                .iter()
                .filter(|e| {
                    matches!(e, HistoryEntry::ParticipantJoined { participant: p, .. } if *p == participant)
                })
                .count()
        };
        time::timeout(Duration::from_secs(5), async {
            while joins(channel.history_entries().await) == 0 {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a join is recorded");
        time::sleep(Duration::from_millis(50)).await;
        joins(channel.history_entries().await)
    }

    /// Issue 564: two who add one participant at once are served one after the other - the
    /// first takes them in, the second finds them there; never two actors.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_who_add_one_participant_at_once_take_them_in_once() {
        let kernel = test_kernel();
        let (session, ctx, other) = running_session_of_a_user(&kernel).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let add = || {
            let (channel, ctx) = (Arc::clone(&channel), ctx.clone());
            tokio::spawn(
                async move { channel.add_participant(&ctx, other, HistoryAccess::All).await },
            )
        };

        let (first, second) = (add(), add());
        let outcomes = [first.await.unwrap(), second.await.unwrap()];

        let added = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
        assert_eq!(added, 1, "one takes them in: {outcomes:?}");
        assert!(
            outcomes.iter().filter_map(|outcome| outcome.as_ref().err()).all(|error| matches!(
                error.downcast_ref(),
                Some(ChannelError::AlreadyParticipant { .. })
            )),
            "the other finds them there: {outcomes:?}"
        );
        assert!(channel.channel().lists(other), "listed");
        assert_eq!(joins_recorded(&channel, other).await, 1, "one actor joined, not two");
        kernel.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_participant_added_later_sees_the_history_granted_and_is_announced() {
        use crate::channel::ChannelError;
        let kernel = test_kernel();
        let (session, ctx, other) = running_session_of_a_user(&kernel).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let owner = match ctx.initiator_id {
            ActorId::User(id) => id,
            _ => unreachable!(),
        };
        let writer = channel.user_actor(owner).expect("the creator takes part");
        for text in ["one", "two", "three"] {
            writer.send_message(None, Some(text.to_string()), None, Vec::new()).await.unwrap();
        }
        let before = history_of(&channel, 3).await;
        let mut rx = kernel.event_bus().subscribe_all();

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
        let from = channel
            .attendance(other)
            .await
            .first()
            .map(|at| at.range.start)
            .expect("a participant");
        assert!(from > Position::START, "from the last message");
        assert_eq!(
            channel.attendance(ctx.initiator_id).await.first().map(|at| at.range.start),
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
        let stored = kernel.entity_stores().channels.get(&ctx, channel.id()).await.unwrap();
        assert!(
            stored.participants.iter().any(|p| p.actor_id() == other),
            "stored, not only in memory"
        );
        let events = rx.drain().await;
        assert!(events.iter().any(|e| matches!(e.about::<Channel>(), Some(((_, cid), change)) if cid == channel.id() && change.data::<ActorId>(JOINED) == Some(other))),
                "the join is announced: {events:?}");

        // What is written after joining is visible.
        writer.send_message(None, Some("four".to_string()), None, Vec::new()).await.unwrap();
        history_of(&channel, 4).await;
        assert_eq!(channel.visible_messages(other).await.len(), 2);

        let refusal = |error: Error| error.downcast::<ChannelError>().expect("a ChannelError");
        let again =
            channel.add_participant(&ctx, other, HistoryAccess::All).await.expect_err("already in");
        assert!(matches!(refusal(again), ChannelError::AlreadyParticipant { .. }));
        let stranger = ActorId::User(UserId::new());
        let outside =
            channel.add_participant(&ctx, stranger, HistoryAccess::All).await.expect_err("nobody");
        let unknown = outside.downcast_ref::<StoreError>();
        assert!(matches!(unknown, Some(StoreError::NotFound { .. })), "read first: {outside}");
        kernel.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn who_joins_or_leaves_is_recorded_where_it_happened_and_shown_also_to_who_made_it() {
        use caelumora_domain::actor::ActorEvent;
        let kernel = test_kernel();
        let (session, ctx, other) = running_session_of_a_user(&kernel).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let writer = channel
            .user_actor(ctx.initiator_id.as_user_id().unwrap())
            .expect("the creator takes part");
        let mut mine = writer.watch();
        writer.send_message(None, Some("before".to_string()), None, Vec::new()).await.unwrap();
        history_of(&channel, 1).await;
        async fn recorded(
            channel: &ChannelLoop,
            wanted: fn(&HistoryEntry) -> bool,
        ) -> Vec<HistoryEntry> {
            channel.history_once(|entries| entries.last().is_some_and(wanted)).await
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
        kernel.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_users_followers_are_told_their_actor_ended_when_they_leave_or_the_channel_is_deleted(
    ) {
        let kernel = test_kernel();
        let (session, ctx, other) = running_session_of_a_user(&kernel).await;
        let owner = ctx.initiator_id.as_user_id().unwrap();

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
        assert!(session.delete_channel(&ctx, channel.id()).await.expect("deleted"));
        assert!(ended(&mut mine, Duration::from_secs(5)).await, "a deleted channel runs no actor");
        kernel.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_removed_participant_is_kept_as_a_former_one_and_announced_but_the_approver_stays() {
        use crate::channel::ChannelError;
        use caelumora_domain::approval::ApprovalStrategy;
        use caelumora_domain::channel::InternalActor;
        use caelumora_domain::template::InternalActorTemplate;
        let kernel = test_kernel();
        let (session, ctx, other) = running_session_of_a_user(&kernel).await;
        let with_approver = ChannelSpec {
            internal_actors: vec![InternalActorTemplate::Approver {
                strategy: ApprovalStrategy::Any,
            }],
            ..spec("side", vec![other])
        };
        let channel = session.create_channel(&ctx, with_approver).await.unwrap();
        let mut rx = kernel.event_bus().subscribe_all();

        channel.remove_participant(&ctx, other).await.expect("removed");

        assert!(!channel.participants().iter().any(|m| m.id == other), "no longer a participant");
        let stored = kernel.entity_stores().channels.get(&ctx, channel.id()).await.unwrap();
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
        let events = rx.drain().await;
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
        let stored = kernel.entity_stores().channels.get(&ctx, channel.id()).await.unwrap();
        assert!(stored.former.is_empty(), "and no longer a former participant");
        kernel.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn only_who_may_change_a_channel_adds_or_removes_its_participants() {
        let kernel = owned_kernel(Default::default());
        let (alice, bob, carol) = (make_user("alice"), make_user("bob"), make_user("carol"));
        let alice_id = ActorId::User(alice.id);
        let bob_id = ActorId::User(bob.id);
        let carol_id = ActorId::User(carol.id);
        let as_user = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(kernel.entity_stores(), &[&alice, &bob, &carol]).await;
        session.security_attributes = session
            .security_attributes
            .clone()
            .granting(Subject::Actor(bob_id), Grant::Read)
            .granting(Subject::Actor(carol_id), Grant::Read);
        let running = create(&kernel, &as_user(alice_id), session).await.unwrap();
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
        kernel.stop_session(&SecurityContext::system(), running.id()).await.unwrap();
    }

    /// Who takes part leaves by their own act, without the right to change the channel (issue
    /// 562): kept as removed by themselves, and announced in their name. Nobody else is taken out
    /// so; the owner leaves as anyone does, also the last one leaves, and the owner keeps the
    /// channel, to come back to - running, empty, as it was (issue 593).
    #[tokio::test]
    async fn who_takes_part_leaves_by_their_own_act_and_takes_nobody_else_out() {
        let kernel = owned_kernel(Default::default());
        let (alice, bob, carol) = (make_user("alice"), make_user("bob"), make_user("carol"));
        let alice_id = ActorId::User(alice.id);
        let bob_id = ActorId::User(bob.id);
        let carol_id = ActorId::User(carol.id);
        let as_user = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(kernel.entity_stores(), &[&alice, &bob, &carol]).await;
        session.security_attributes = session
            .security_attributes
            .clone()
            .granting(Subject::Actor(bob_id), Grant::Read)
            .granting(Subject::Actor(carol_id), Grant::Read);
        let running = create(&kernel, &as_user(alice_id), session).await.unwrap();
        let channel = running
            .create_channel(&as_user(alice_id), spec("side", vec![bob_id, carol_id]))
            .await
            .unwrap();

        for other in [carol_id, alice_id] {
            let refused =
                channel.leave(&as_user(bob_id), other).await.expect_err("nobody else's act");
            assert!(is_forbidden(&refused), "{refused}");
        }
        assert_eq!(channel.participants().len(), 3, "all of them, unchanged");

        channel.leave(&as_user(bob_id), bob_id).await.expect("his own act");

        assert!(!channel.participants().iter().any(|m| m.id == bob_id), "out of the channel");
        let stored = kernel.entity_stores().channels.get(&as_user(alice_id), channel.id()).await;
        let former =
            stored.unwrap().former.into_iter().find(|f| f.participant.actor_id() == bob_id);
        assert_eq!(former.expect("a former participant").left.by, bob_id, "removed by himself");
        let left = time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(HistoryEntry::ParticipantLeft { participant, by, .. }) =
                    channel.history_entries().await.last()
                {
                    break (*participant, *by);
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        });
        assert_eq!(left.await.expect("recorded"), (bob_id, bob_id), "in his name");

        channel.leave(&as_user(alice_id), alice_id).await.expect("the owner as anyone");
        channel.leave(&as_user(carol_id), carol_id).await.expect("the last one too");
        assert!(channel.participants().is_empty(), "nobody takes part");
        channel
            .add_participant(&as_user(alice_id), alice_id, HistoryAccess::All)
            .await
            .expect("she keeps the channel, and comes back");
        let back = channel.user_actor(alice.id).expect("her actor");
        back.send_message(None, Some("back".to_string()), None, Vec::new()).await.expect("said");
        let heard = time::timeout(Duration::from_secs(5), async {
            loop {
                let entries = channel.history_entries().await;
                let mut said = entries.iter().filter_map(HistoryEntry::said);
                if said.any(|m| m.sender_id == alice_id && m.content.as_deref() == Some("back")) {
                    break;
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        });
        heard.await.expect("the empty channel ran on, and her actor in it (issue 593)");
        kernel.stop_session(&SecurityContext::system(), running.id()).await.unwrap();
    }

    /// Leaving is what a policy refuses as it refuses everything (issue 562): asked as
    /// `LeaveChannel`, and who would leave stays.
    #[tokio::test]
    async fn a_policy_may_refuse_leaving() {
        let alice = make_user("alice");
        let refuse_leaving = Policy(|action| !matches!(action, ActionType::LeaveChannel(_)));
        let channel = channel_with(&[&alice], Arc::new(refuse_leaving)).await;
        let leaving = ActorId::User(alice.id);
        let context = SecurityContext::new(leaving, Default::default(), Default::default());
        let refused = channel.leave(&context, leaving).await.expect_err("leaving is refused");
        assert!(is_forbidden(&refused), "{refused}");
        assert!(channel.user_actor(alice.id).is_some(), "she stays");
    }

    /// Adding one whose actor cannot start is rolled back as the channel had them (issue 563):
    /// not listed, and not made a former participant they never were - here an agent, which a
    /// world without agents cannot run.
    #[tokio::test]
    async fn the_addition_of_one_who_cannot_start_is_rolled_back_as_the_channel_had_them() {
        let world = World::new().await;
        let plain = world.personality("plain", Some(world.alice), &[]).await;
        let waiting = world.agent("scout", world.alice, &plain).await;
        let scout = ActorId::Agent(waiting);
        let session = world.shared_session(&[], &[waiting]).await;
        let main = Arc::clone(&session.channels()[0]);

        let error = main
            .add_participant(&World::as_user(world.alice), scout, HistoryAccess::All)
            .await
            .expect_err("no agent runs here");
        assert!(
            matches!(error.downcast_ref(), Some(ChannelError::UnknownAgentKind { .. })),
            "stored, then refused as it starts: {error}"
        );
        let after = main.channel();
        assert!(!after.lists(scout), "taken back out");
        assert!(after.former.is_empty(), "made no former participant");
        world.kernel.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
    }

    /// How an agent is taken into a channel again while its range of attendance is still open
    /// (issue 568): listed, but left out as its channel started, and no leave recorded.
    #[derive(Debug, Clone, Copy)]
    enum Again {
        /// Left out since its personality was withdrawn, added by its owner once it is given back
        /// (`TakeIn`).
        Added,
        /// Left out so, followed by the channel once it is given back (`follow_agents`).
        Followed,
        /// Deleted while its session was stopped, and restored.
        Restored,
    }

    /// Where `channel` stopped delivering to `participant` by the leave its history records, at
    /// most five seconds after it was let go: its actor ends first.
    async fn left_until(channel: &ChannelLoop, participant: ActorId) -> Position {
        let until = |entries: Vec<HistoryEntry>| {
            entries.iter().find_map(|e| match e {
                HistoryEntry::ParticipantLeft { participant: p, until, .. }
                    if *p == participant =>
                {
                    Some(*until)
                }
                _ => None,
            })
        };
        time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(until) = until(channel.history_entries().await) {
                    break until;
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the leave is recorded")
    }

    /// Issue 568: an agent taken into a channel again while its range of attendance is still
    /// open, in each way `Again` names, joins again in the history, since its actor starts only
    /// now, but keeps the one range it has, which ends where it leaves.
    #[tokio::test]
    async fn one_taken_in_again_while_it_attends_attends_until_it_leaves() {
        for again in [Again::Added, Again::Followed, Again::Restored] {
            let answering = ScriptedAgents(Script::Answers("ok".to_string()));
            let world = World::with(&[], &[&answering], |_| {}).await;
            let alice = World::as_user(world.alice);
            let bob = World::as_user(world.bob);
            let mut personality =
                world.personality("shared", Some(world.bob), &[world.alice]).await;
            let helper = world.agent("helper", world.alice, &personality).await;
            let actor = ActorId::Agent(helper);
            let session = world.shared_session(&[helper], &[]).await;
            let founded = &session.channels()[0];
            assert_eq!(joins_recorded(founded, actor).await, 1, "a founder");
            let founding = founded.attendance(actor).await[0].range.start;
            assert!(founding > Position::START, "after the users' joins, so that a move shows");
            world.kernel.stop_session(&alice, session.id()).await.unwrap();
            let personalities = &world.kernel.entity_stores().personalities;
            match again {
                Again::Added | Again::Followed => {
                    personality.security_attributes = attributes(Some(world.bob), &[]);
                    personalities.update(&bob, &mut personality).await.unwrap();
                }
                Again::Restored => {
                    assert!(world.kernel.agent_manager().delete(&alice, helper).await.unwrap());
                }
            }

            let session = world.kernel.open_session(&alice, session.id()).await.unwrap();
            let main = Arc::clone(&session.channels()[0]);
            assert!(!main.participant_ids().contains(&actor), "{again:?}: left out as it starts");
            assert!(main.channel().lists(actor), "{again:?}: listed still");
            match again {
                Again::Added | Again::Followed => {
                    personality.security_attributes = attributes(Some(world.bob), &[world.alice]);
                    personalities.update(&bob, &mut personality).await.unwrap();
                }
                Again::Restored => {
                    world.kernel.agent_manager().restore(&alice, helper).await.unwrap();
                }
            }
            match again {
                Again::Added => {
                    main.add_participant(&alice, actor, HistoryAccess::None).await.unwrap();
                }
                Again::Followed => main.follow_agents(Some(&HashSet::from([helper]))).await,
                // The session follows the agent as it learns of the restore.
                Again::Restored => {}
            }
            time::timeout(Duration::from_secs(5), async {
                while !main.participant_ids().contains(&actor) {
                    time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("{again:?}: it runs again"));
            assert_eq!(joins_recorded(&main, actor).await, 2, "{again:?}: its actor joined again");

            main.remove_participant(&alice, actor).await.unwrap();
            let until = left_until(&main, actor).await;
            let ranges = main.attendance(actor).await;
            assert!(
                matches!(ranges.as_slice(), [at]
                    if at.range.start == founding && at.range.end == until),
                "{again:?}: one range, from its founding to where it left: {ranges:?}"
            );
            world.kernel.stop_session(&alice, session.id()).await.unwrap();
        }
    }

    /// That `newcomer`, added to `channel` granted nothing, is shared nothing and sees nothing
    /// recorded before its join, once that is recorded: `case` says where.
    async fn given_nothing_before_its_join(channel: &ChannelLoop, newcomer: ActorId, case: &str) {
        assert_eq!(joins_recorded(channel, newcomer).await, 1, "{case}: joined");
        let shared = channel.history_entries().await.into_iter().any(|e| {
            matches!(e, HistoryEntry::HistoryShared { participant, .. } if participant == newcomer)
        });
        assert!(!shared, "{case}: nothing shared is recorded");
        let visible = channel.visible_entries(newcomer).await;
        assert!(
            matches!(visible.first(), Some(HistoryEntry::ParticipantJoined { participant, .. })
                if *participant == newcomer),
            "{case}: its own join first: {visible:?}"
        );
    }

    /// Issue 568: a newcomer granted nothing is given nothing recorded before its join - in a
    /// channel where nothing was said yet, the joins of its founders; after an agent answered,
    /// how its turn ended, which follows the last message.
    #[tokio::test]
    async fn one_granted_nothing_sees_nothing_recorded_before_its_join() {
        let answering = ScriptedAgents(Script::Answers("ok".to_string()));
        let world = World::with(&[], &[&answering], |_| {}).await;
        let alice = World::as_user(world.alice);
        let plain = world.personality("plain", Some(world.alice), &[]).await;
        let helper = world.agent("helper", world.alice, &plain).await;
        let first = world.agent("first", world.alice, &plain).await;
        let second = world.agent("second", world.alice, &plain).await;
        let session = world.shared_session(&[helper], &[first, second]).await;
        let main = Arc::clone(&session.channels()[0]);
        assert_eq!(joins_recorded(&main, ActorId::Agent(helper)).await, 1, "a founder");

        main.add_participant(&alice, ActorId::Agent(first), HistoryAccess::None).await.unwrap();
        given_nothing_before_its_join(&main, ActorId::Agent(first), "nothing said yet").await;

        let writer = main.user_actor(world.alice).expect("a founder");
        writer
            .send_message(None, Some("@helper hello".to_string()), None, Vec::new())
            .await
            .unwrap();
        let turn_ended_last = |entries: Vec<HistoryEntry>| {
            let said = entries.iter().rposition(|e| e.said().is_some());
            let ended =
                entries.iter().rposition(|e| matches!(e, HistoryEntry::TurnFinished { .. }));
            said.is_some() && ended > said
        };
        time::timeout(Duration::from_secs(5), async {
            while !turn_ended_last(main.history_entries().await) {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("an answer, and its turn's end after it");
        main.add_participant(&alice, ActorId::Agent(second), HistoryAccess::None).await.unwrap();
        given_nothing_before_its_join(&main, ActorId::Agent(second), "after a turn ended").await;
        world.kernel.stop_session(&alice, session.id()).await.unwrap();
    }

    /// Issue 568: a grant counts messages alone - joins and other events are none -, so that one
    /// granted more messages than there are sees them all, from the oldest on, but not the joins
    /// recorded before it.
    #[tokio::test]
    async fn a_grant_begins_at_the_oldest_message_it_grants() {
        let kernel = test_kernel();
        let (session, ctx, other) = running_session_of_a_user(&kernel).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let ActorId::User(owner) = ctx.initiator_id else { unreachable!() };
        let writer = channel.user_actor(owner).expect("the creator takes part");
        writer.send_message(None, Some("one".to_string()), None, Vec::new()).await.unwrap();
        history_of(&channel, 1).await;

        let more = HistoryAccess::LastMessages { count: 9 };
        channel.add_participant(&ctx, other, more).await.expect("a user of the session joins");

        assert_eq!(joins_recorded(&channel, other).await, 1, "joined");
        let visible = channel.visible_entries(other).await;
        assert!(
            visible
                .first()
                .and_then(HistoryEntry::said)
                .is_some_and(|m| m.content.as_deref() == Some("one")),
            "from the message on, not the founder's join before it: {visible:?}"
        );
        assert!(
            visible.iter().any(|e| matches!(e, HistoryEntry::HistoryShared { .. })),
            "what it was given is recorded, since it lies before its join: {visible:?}"
        );
        kernel.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// Scripted agents that answer, until the flag it holds is set: then none can start.
    #[derive(Debug, Default)]
    struct Breakable(Arc<AtomicBool>);

    impl Library for Breakable {
        fn name(&self) -> &'static str {
            "breakable"
        }

        fn register(&self, library: &mut dyn LibraryBuilder) -> anyhow::Result<()> {
            let broken = Arc::clone(&self.0);
            let factory = agent_factory(move |runtime| {
                let broken = broken.load(Ordering::SeqCst);
                async move {
                    anyhow::ensure!(!broken, "it cannot start");
                    let answers = Script::Answers("ok".to_string());
                    Ok(Box::new(ScriptedAgent::new(runtime, answers)) as Box<dyn Actor>)
                }
            });
            library.register_agent_kind(PersonalityKind::Internal, &[], factory)
        }
    }

    /// Issue 568, as taking someone in is undone (issue 593): an agent left out as its channel
    /// started, its range of attendance open, that is taken in again and cannot start after all
    /// has the listing taken back and no leave recorded - it never started, and stays as it was,
    /// left out and attending.
    #[tokio::test]
    async fn one_taken_in_again_that_cannot_start_stays_left_out_without_a_leave() {
        let breakable = Breakable::default();
        let world = World::with(&[], &[&breakable], |_| {}).await;
        let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
        let helper = world.agent("helper", world.alice, &shared).await;
        let session = world.shared_session(&[helper], &[]).await;
        let main = session.channels()[0].id();
        let helper = ActorId::Agent(helper);
        let history = world.kernel.history_store();
        let open = async || {
            let attended = history.attendance(main, helper).await.unwrap();
            attended.iter().any(|at| at.range.end == Position::MAX)
        };
        time::timeout(Duration::from_secs(5), async {
            while !open().await {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("its join is recorded");
        let personalities = &world.kernel.entity_stores().personalities;
        let bob = World::as_user(world.bob);
        let attributes_withdrawn = attributes(Some(world.bob), &[]);
        let mut withdrawn = Personality { security_attributes: attributes_withdrawn, ..shared };
        personalities.update(&bob, &mut withdrawn).await.unwrap();
        let alice = World::as_user(world.alice);
        world.kernel.stop_session(&alice, session.id()).await.unwrap();
        let session = world.kernel.open_session(&alice, session.id()).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);
        assert!(main.actor(helper).is_none(), "left out as it started");
        let attributes_given = attributes(Some(world.bob), &[world.alice]);
        let mut given_back = Personality { security_attributes: attributes_given, ..withdrawn };
        personalities.update(&bob, &mut given_back).await.unwrap();
        breakable.0.store(true, Ordering::SeqCst);

        let adding = main.add_participant(&alice, helper, HistoryAccess::All);
        adding.await.expect_err("it cannot start");

        assert!(main.channel().lists(helper), "listed as before");
        assert!(main.actor(helper).is_none(), "left out still");
        assert!(open().await, "attending as before");
        // Time for its run to record what it would.
        time::sleep(Duration::from_millis(100)).await;
        let page = history.page(main.id(), &Reader::Channel, Position::START, None, 1000, None);
        let left = |entry: &&HistoryEntry| matches!(entry, HistoryEntry::ParticipantLeft { participant, .. } if *participant == helper);
        assert_eq!(page.await.unwrap().entries().filter(left).count(), 0, "no leave recorded");
        world.kernel.stop_session(&alice, session.id()).await.unwrap();
    }

    /// Issue 590: a participant left out as the channel started - its personality withdrawn, its
    /// leave never recorded - is recorded as left as it is removed, so that its attendance ends
    /// there, not where it would join again.
    #[tokio::test]
    async fn one_left_out_as_the_channel_started_is_recorded_as_left_as_it_is_removed() {
        // Agents that run, so that it takes part until its personality is withdrawn.
        let answering = ScriptedAgents(Script::Answers("ok".to_string()));
        let world = World::with(&[], &[&answering], |_| {}).await;
        let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
        let helper = world.agent("helper", world.alice, &shared).await;
        let session = world.shared_session(&[helper], &[]).await;
        let main = session.channels()[0].id();
        let helper = ActorId::Agent(helper);
        let history = world.kernel.history_store();
        let open = async || {
            let attended = history.attendance(main, helper).await.unwrap();
            attended.iter().any(|at| at.range.end == Position::MAX)
        };
        time::timeout(Duration::from_secs(5), async {
            while !open().await {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("its join is recorded");
        let attributes = attributes(Some(world.bob), &[]);
        let mut withdrawn = Personality { security_attributes: attributes, ..shared };
        let personalities = &world.kernel.entity_stores().personalities;
        personalities.update(&World::as_user(world.bob), &mut withdrawn).await.unwrap();
        let alice = World::as_user(world.alice);
        world.kernel.stop_session(&alice, session.id()).await.unwrap();
        let session = world.kernel.open_session(&alice, session.id()).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);
        assert!(main.actor(helper).is_none(), "left out as it started");
        assert!(open().await, "taking part in its history all the same");

        main.remove_participant(&alice, helper).await.expect("hers to remove");

        time::timeout(Duration::from_secs(5), async {
            while open().await {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("its leave is recorded");
        let leaves = async || {
            let page = history.page(main.id(), &Reader::Channel, Position::START, None, 1000, None);
            let page = page.await.unwrap();
            let left = |entry: &&HistoryEntry| matches!(entry, HistoryEntry::ParticipantLeft { participant, .. } if *participant == helper);
            page.entries().filter(left).count()
        };
        assert_eq!(leaves().await, 1, "once");
        world.kernel.stop_session(&alice, session.id()).await.unwrap();
        world.kernel.open_session(&alice, session.id()).await.unwrap();
        // Time for its run to record what it would.
        time::sleep(Duration::from_millis(100)).await;
        assert_eq!(leaves().await, 1, "and not again as the channel starts again");
        world.kernel.stop_session(&alice, session.id()).await.unwrap();
    }
}
