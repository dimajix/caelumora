//! The access policy of a process (issues 11, 23), deciding by who owns
//! an entity and whom its access list names.

use crate::actor::ActorId;
use crate::store::EntityKind;
use async_trait::async_trait;
use std::fmt::Debug;
use std::sync::Arc;
use super::{AccessPolicy, ActionType, Admission, Grant, Memberships, NoMemberships, Reclassified, Requirement, SecurityAttributes, SecurityContext, Verb};

/// The ownership policy (issues 11, 23): what is yours you may do
/// everything with, what an access list grants you you may read and use,
/// what belongs to the system everybody may read and use - and nobody acts
/// in someone else's name.
///
/// - The system (`ActorId::System`) may do everything.
/// - Reading - which also covers using, e.g. starting a session from a
///   template, or working on a workspace outside a session - is allowed to
///   the owner, to whom an entry of the access list grants it, and to
///   everybody for entities owned by the system or by no one. An agent
///   reads what its owner may read, and what it takes part in itself - a
///   participant as the caller (issue 264); everything else it does, it does as
///   its owner.
/// - Being told of a deletion is allowed to whom reading it was until then;
///   a deleted group grants nothing else.
/// - Creating is allowed only with oneself as owner.
/// - Changing, deleting and stopping are the owner's alone, and so is reading what an
///   agent saw: its context holds more than the conversation - its system
///   prompt, internal steps, tool results.
/// - Cancelling or waking a channel is allowed to whoever may read it.
/// - Joining a session or channel (`JOIN`) is allowed to whom an entry
///   grants `Join`, asked in the context of the one who joins.
/// - What may flow where, and who is cleared for what, is not its to decide:
///   the `ConfidentialityPolicy` decides it beside it (issue 304, reversing
///   issue 138).
/// - A channel may be reclassified by its owner, a message by the channel's
///   owner or its author, a note or memory entry by its author.
/// - A tool call of an agent is approved by the agent's owner and whom its
///   access list lets read it, of an agent of the system by the channel's
///   owner (issue 14).
/// - Searching and listing are allowed; their results are narrowed to what
///   the caller may read. Tool calls, secrets and commands are governed
///   elsewhere for now (issues 12 and 23).
///
/// What it decides of an entity, it decides by the `Admission` it gives for
/// the verb (issue 332), which a store narrows a list by; the other actions
/// are decided by an admission as well, reading or owning.
///
/// Who "you" are is the context's principal: the person whose rights its
/// caller exercises - a user themselves, an agent its owner, whoever asked
/// it (issue 255). An agent is its owner's delegate: the owner checks the
/// request where a call is admitted, and the asker lends it nothing.
#[derive(Debug)]
pub struct OwnershipPolicy {
    memberships: Arc<dyn Memberships>,
}

impl Default for OwnershipPolicy {
    fn default() -> Self {
        Self { memberships: Arc::new(NoMemberships) }
    }
}

impl OwnershipPolicy {
    /// The policy knowing of no memberships.
    pub fn new() -> Self {
        Self::default()
    }

    /// The policy, learning who belongs to a group from `memberships`.
    pub fn with_memberships(memberships: Arc<dyn Memberships>) -> Self {
        Self { memberships }
    }

    /// Whether the context answers for the agent with `agent` attributes in
    /// the channel with `channel` attributes - approves its calls, runs the
    /// commands that are its owners' (issues 14, 269): who is responsible
    /// for the agent - its owner, or whom its access list lets read it -,
    /// also in someone else's channel; for an agent of the system, whose
    /// channel it is.
    async fn answers_for(&self, agent: &SecurityAttributes, channel: &SecurityAttributes, context: &SecurityContext) -> bool {
        if agent.unowned() {
            self.owns(channel, context).await
        } else {
            self.granted(agent, Grant::Read, context).await
        }
    }

    /// Whether the principal owns an entity with `attributes`.
    async fn owns(&self, attributes: &SecurityAttributes, context: &SecurityContext) -> bool {
        Self::owning(context).admits(attributes, self.memberships.as_ref()).await
    }

    /// Whether the principal may do with an entity with `attributes` what
    /// `needed` asks for, as `granting` admits.
    async fn granted(&self, attributes: &SecurityAttributes, needed: Grant, context: &SecurityContext) -> bool {
        Self::granting(context, needed, &[context.principal_id]).admits(attributes, self.memberships.as_ref()).await
    }

    /// The admission for `verb` of a context other than the system's, which
    /// may do everything.
    fn admission_for(context: &SecurityContext, verb: Verb) -> Admission {
        match verb {
            // Reading: what the principal may read, or the caller itself - an agent what it takes
            // part in (issue 264).
            Verb::Get => Self::granting_as_caller(context, Grant::Read),
            // Told of a deletion: whom reading admitted until then, also by
            // the groups that went with it.
            Verb::HearDeleted => Admission { deleted_groups: true, ..Self::granting_as_caller(context, Grant::Read) },
            // Creating is only allowed with oneself as owner; changing,
            // deleting, and what was deleted, only for the owner.
            Verb::Create | Verb::Update | Verb::Delete | Verb::ReadDeleted | Verb::Restore => Self::owning(context),
            // Until there is an administrator (issue 125): the system
            // alone, as the retention policy (issue 07) acts.
            Verb::Purge => Admission::default(),
            // What a kind's own verb requires (issue 323): using a skill
            // is granted as reading it - what the owner may read the
            // agent may follow (issue 22) -, changing a workspace's files
            // or adding an artifact's version as writing (issue 23); to
            // the principal, or an agent taking part itself.
            Verb::Custom(custom) => match custom.requires {
                Requirement::Grant(grant) => Self::granting_as_caller(context, grant),
                Requirement::Owner => Self::owning(context),
            },
        }
    }

    /// What the principal owns - nobody's for an empty context.
    fn owning(context: &SecurityContext) -> Admission {
        Admission { actors: Self::named(&[context.principal_id]), ..Admission::default() }
    }

    /// What grants `needed` to the principal or to the caller itself - an
    /// agent for what it takes part in (issue 264).
    fn granting_as_caller(context: &SecurityContext, needed: Grant) -> Admission {
        Self::granting(context, needed, &[context.principal_id, context.caller_id])
    }

    /// What is the system's, what one of `actors` owns, or what an entry of
    /// its access list grants `needed` - to one of them, by the roles that
    /// count for the context, and, if one of them is a user, to everyone.
    fn granting(context: &SecurityContext, needed: Grant, actors: &[ActorId]) -> Admission {
        let actors = Self::named(actors);
        Admission {
            everyone: actors.iter().any(|actor| matches!(actor, ActorId::User(_))),
            actors,
            unowned: true,
            grant: Some(needed),
            roles: context.counting_roles(),
            deleted_groups: false,
        }
    }

    /// The actors an owner or an entry may name of `actors`: each once, and
    /// nobody for `ActorId::None`.
    fn named(actors: &[ActorId]) -> Vec<ActorId> {
        let mut named: Vec<ActorId> = Vec::new();
        for actor in actors {
            if *actor != ActorId::None && !named.contains(actor) {
                named.push(*actor);
            }
        }
        named
    }
}

#[async_trait]
impl AccessPolicy for OwnershipPolicy {
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool {
        if context.principal_id == ActorId::System {
            return true;
        }
        match action {
            ActionType::Entity { verb, attributes: a, .. } => Self::admission_for(context, verb).admits(a, self.memberships.as_ref()).await,
            ActionType::List(_) => true,
            ActionType::CancelChannel(a)
            | ActionType::WakeChannel(a)
            | ActionType::RetryMessage(a)
            | ActionType::AnswerQuestion(a) => self.granted(a, Grant::Read, context).await,
            // What an agent can do is read by whoever reads the agent, and by
            // every participant of the channel it works in with them.
            ActionType::ReadAgentTools { agent, channel } => {
                self.granted(agent, Grant::Read, context).await || self.granted(channel, Grant::Read, context).await
            }
            ActionType::ApproveToolCall { agent, channel } => self.answers_for(agent, channel, context).await,
            // A command of an agent is run by whoever may write in the
            // channel - its participants -, unless the command is for the agent's
            // owners alone, as approving its calls is (issue 269).
            ActionType::ShowAgentCommand { command, agent, channel } | ActionType::ExecuteAgentCommand { command, agent, channel } => {
                if command.owners_only { self.answers_for(agent, channel, context).await } else { self.granted(channel, Grant::Read, context).await }
            }
            // A command of the session alike (issue 304): its participants, or
            // for the owners alone, whoever may change the channel.
            ActionType::ShowCommand { command, channel } | ActionType::ExecuteCommand { command, channel } => {
                if command.owners_only { self.owns(channel, context).await } else { self.granted(channel, Grant::Read, context).await }
            }
            ActionType::Reclassify(target) => match target {
                Reclassified::Channel { attributes } => self.owns(attributes, context).await,
                Reclassified::Message { author, attributes } => author == context.caller_id || self.owns(attributes, context).await,
                Reclassified::Note { author, .. } | Reclassified::Memory { author, .. } => author == context.caller_id,
            },
            // A message is its author's to change: who said it, not whom
            // they acted for.
            ActionType::ModifyMessage { author, .. } | ActionType::RevokeMessage { author, .. } => author == context.caller_id && author != ActorId::None,
            ActionType::ReadAgentContext(a)
            | ActionType::ReadAgentComponents(a)
            | ActionType::StopSession(a) => self.owns(a, context).await,
            // Hooks for another policy (issue 24): what decides a call
            // today is the tools offered and the place's rules; secrets
            // have no owner to decide by yet (issue 310).
            ActionType::CallTool(_)
            | ActionType::ReadSecret(_) => true,
        }
    }

    /// Nothing narrowed for the system, which may do everything; the
    /// admission it decides `check` by for everybody else.
    fn admission(&self, context: &SecurityContext, _kind: EntityKind, verb: Verb) -> Option<Admission> {
        (context.principal_id != ActorId::System).then(|| Self::admission_for(context, verb))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::testing::*;
    use crate::security::{AllowAllPolicy, CustomVerb, Subject, JOIN, WRITE};
    use crate::channel::{Channel, ChannelId};
    use crate::command::Command;
    use crate::group::{Group, GroupId};
    use crate::role::RoleId;
    use crate::scope::Scope;
    use crate::session::{Session, SessionId};
    use crate::store::{EntityKind, Stored};
    use crate::template::SessionTemplate;
    use crate::value::name::ScopedName;
    use crate::workspace::Workspace;
    use crate::agent::AgentId;

    /// A plugin's kind (issue 323), as skills are.
    const SKILL: EntityKind = EntityKind::new("skill");

    fn knowing(known: Known) -> OwnershipPolicy {
        OwnershipPolicy::with_memberships(Arc::new(known))
    }

    #[tokio::test]
    async fn a_message_is_its_authors_to_modify_and_take_back() {
        let (alice, bob) = (user(), user());
        let bobs = shared(bob, &[alice]);
        for (author, who, allowed) in [(alice, alice, true), (alice, bob, false)] {
            assert_eq!(allows(who, ActionType::ModifyMessage { author, channel: &bobs }).await, allowed, "modified by {who}: the author's, not the channel owner's");
            assert_eq!(allows(who, ActionType::RevokeMessage { author, channel: &bobs }).await, allowed, "taken back by {who}");
        }
        let agent = ActorId::Agent(AgentId::new());
        let for_alice = context(alice).next(agent, alice, alice);
        let access_policy = OwnershipPolicy::new();
        assert!(access_policy.check(&for_alice, ActionType::RevokeMessage { author: agent, channel: &bobs }).await, "an agent its own, acting for alice");
        assert!(!access_policy.check(&for_alice, ActionType::RevokeMessage { author: alice, channel: &bobs }).await, "but not alice's: who said it, not whom it acts for");
    }

    #[tokio::test]
    async fn an_agent_is_judged_by_its_owner_not_by_whoever_asked_it() {
        let (alice, bob) = (user(), user());
        let agent = ActorId::Agent(AgentId::new());
        let hers = SecurityAttributes::owned_by(alice);
        let access_policy = OwnershipPolicy::new();
        let asked_by_bob = context(bob).next(agent, alice, alice);
        assert!(access_policy.check(&asked_by_bob, ActionType::get(Session::KIND, &hers)).await, "with alice's rights, whoever asked");
        assert!(access_policy.check(&asked_by_bob, ActionType::update(Session::KIND, &hers)).await, "all of them");
        let asked_by_alice = context(alice).next(agent, bob, bob);
        assert!(!access_policy.check(&asked_by_alice, ActionType::get(Session::KIND, &hers)).await, "asking lends nothing, not even the owner's own rights to another's agent");
    }

    #[tokio::test]
    async fn owners_do_everything_whom_the_list_grants_read_strangers_nothing() {
        let (alice, bob, carol) = (user(), user(), user());
        let hers = shared(alice, &[bob]);

        for action in [ActionType::get(Session::KIND, &hers), ActionType::update(Session::KIND, &hers), ActionType::delete(Session::KIND, &hers)] {
            assert!(allows(alice, action).await, "{action:?}");
            assert!(!allows(carol, action).await, "{action:?}");
        }
        assert!(allows(bob, ActionType::get(Session::KIND, &hers)).await);
        assert!(allows(bob, ActionType::CancelChannel(&hers)).await, "a reader acts in it");
        assert!(allows(alice, ActionType::ReadAgentContext(&hers)).await);
        assert!(!allows(bob, ActionType::ReadAgentContext(&hers)).await, "what an agent saw is its owner's alone");
        assert!(allows(alice, ActionType::ReadAgentComponents(&hers)).await);
        assert!(!allows(bob, ActionType::ReadAgentComponents(&hers)).await, "and what it was built from, which may have been shared with its owner alone");
        let theirs = SecurityAttributes::owned_by(carol);
        let tools = |agent, channel| ActionType::ReadAgentTools { agent, channel };
        assert!(allows(bob, tools(&hers, &theirs)).await, "who reads the agent sees what it can do");
        assert!(allows(carol, tools(&hers, &theirs)).await, "and every member of the channel");
        assert!(!allows(carol, tools(&hers, &hers)).await, "but nobody else");
        let approve = |agent, channel| ActionType::ApproveToolCall { agent, channel };
        assert!(allows(alice, approve(&hers, &theirs)).await, "the agent's owner approves its calls, in any channel");
        assert!(allows(bob, approve(&hers, &theirs)).await, "and so do its readers");
        assert!(!allows(carol, approve(&hers, &theirs)).await, "not whose channel it is");
        let systems = SecurityAttributes::owned_by(ActorId::System);
        assert!(allows(carol, approve(&systems, &theirs)).await, "an agent of the system: whose channel it is");
        assert!(!allows(alice, approve(&systems, &theirs)).await, "nobody else");
        assert!(!allows(bob, approve(&SecurityAttributes::default(), &hers)).await, "not even a reader of the channel");
        let command = |owners_only| Command { name: "x".to_string(), description: String::new(), usage: None, owners_only };
        let (members, owners) = (command(false), command(true));
        let run = |command, agent, channel| ActionType::ExecuteAgentCommand { command, agent, channel };
        let show = |command, agent, channel| ActionType::ShowAgentCommand { command, agent, channel };
        assert!(allows(carol, run(&members, &hers, &theirs)).await, "a command for the members: whoever may write in the channel");
        assert!(allows(carol, show(&members, &hers, &theirs)).await);
        assert!(!allows(bob, run(&members, &hers, &theirs)).await, "not the agent's reader outside the channel");
        assert!(allows(alice, run(&owners, &hers, &theirs)).await, "a command for the owners: the agent's owner");
        assert!(allows(bob, run(&owners, &hers, &theirs)).await, "and its readers");
        assert!(!allows(carol, run(&owners, &hers, &theirs)).await, "not whose channel it is");
        assert!(!allows(carol, show(&owners, &hers, &theirs)).await, "who may not run it does not see it");
        assert!(allows(carol, run(&owners, &systems, &theirs)).await, "an agent of the system: whose channel it is");
        // A command of the session is decided alike (issue 304), by the
        // channel alone.
        let run = |command, channel| ActionType::ExecuteCommand { command, channel };
        let show = |command, channel| ActionType::ShowCommand { command, channel };
        assert!(allows(carol, run(&members, &theirs)).await && allows(carol, show(&members, &theirs)).await, "a command for the members: whoever reads the channel");
        assert!(!allows(bob, run(&members, &theirs)).await, "nobody else");
        assert!(allows(carol, run(&owners, &theirs)).await, "a command for the owners: whose channel it is");
        assert!(!allows(alice, run(&owners, &theirs)).await && !allows(alice, show(&owners, &theirs)).await, "not a member who does not own it");
        assert!(!allows(bob, ActionType::update(Session::KIND, &hers)).await);
        assert!(!allows(bob, ActionType::delete(Session::KIND, &hers)).await);
        // A plugin's verb that requires reading - using a skill, issue 22 -
        // is granted as reading; changing stays the owner's.
        const USE: CustomVerb = CustomVerb { name: "Use", requires: Requirement::Grant(Grant::Read) };
        assert!(allows(bob, ActionType::custom(SKILL, &USE, &hers)).await, "a reader uses it");
        assert!(!allows(carol, ActionType::custom(SKILL, &USE, &hers)).await, "nobody else");
        assert!(allows(carol, ActionType::custom(SKILL, &USE, &systems)).await, "the system's, everybody");
        assert!(allows(bob, ActionType::get(SKILL, &hers)).await && !allows(bob, ActionType::update(SKILL, &hers)).await, "read, not changed");
    }

    #[tokio::test]
    async fn an_agent_reads_what_its_owner_may_read_or_it_takes_part_in_and_changes_as_its_owner() {
        let (alice, bob) = (user(), user());
        let hers = ActorId::Agent(AgentId::new());
        let his = ActorId::Agent(AgentId::new());
        let member = ActorId::Agent(AgentId::new());
        let place = GroupId::of_channel(ChannelId::new());
        let alices = SecurityAttributes::owned_by(alice).granting(Subject::Group(place), Grant::Read);
        let access_policy = knowing(Known { members: vec![(member, place)], ..Known::default() });
        assert!(access_policy.check(&context(bob).next(hers, alice, alice), ActionType::get(Channel::KIND, &alices)).await, "her agent, whoever asked");
        assert!(!access_policy.check(&context(bob).next(his, bob, bob), ActionType::get(Channel::KIND, &alices)).await, "not his: he may not read it");
        assert!(access_policy.check(&context(bob).next(member, bob, bob), ActionType::get(Channel::KIND, &alices)).await, "but an agent taking part in it");
        assert!(access_policy.check(&context(bob).next(member, bob, bob), ActionType::get(Session::KIND, &alices)).await, "so for every read");
        assert!(!access_policy.check(&context(bob).next(member, bob, bob), ActionType::update(Channel::KIND, &alices)).await, "changing stays its owner's");
        assert!(!access_policy.check(&context(bob).next(member, bob, bob), ActionType::CancelChannel(&alices)).await, "and so does acting on it");
    }

    #[tokio::test]
    async fn a_channel_is_reclassified_by_its_owner_a_message_also_by_its_author() {
        let access_policy = OwnershipPolicy::new();
        let (owner, author, stranger) = (user(), user(), user());
        let attributes = shared(owner, &[author, stranger]);
        let may = |who, target| {
            let access_policy = &access_policy;
            async move { access_policy.check(&context(who), ActionType::Reclassify(target)).await }
        };
        let channel_itself = Reclassified::Channel { attributes: &attributes };
        assert!(may(owner, channel_itself).await, "the owner reclassifies the channel");
        assert!(!may(author, channel_itself).await, "a participant does not");

        let message = Reclassified::Message { author, attributes: &attributes };
        assert!(may(author, message).await, "the author their message");
        assert!(may(owner, message).await, "so does the channel's owner");
        assert!(!may(stranger, message).await, "nobody else");

        let agent = ActorId::Agent(AgentId::new());
        for own in [Reclassified::Note { author: agent, attributes: &attributes }, Reclassified::Memory { author: agent, attributes: &attributes }] {
            assert!(may(agent, own).await, "an agent what it wrote");
            assert!(!may(owner, own).await, "not even the channel's owner another's");
        }
    }

    #[tokio::test]
    async fn what_belongs_to_the_system_everybody_uses_and_only_the_system_changes() {
        let alice = user();
        for shared in [SecurityAttributes::default(), SecurityAttributes::owned_by(ActorId::System)] {
            assert!(allows(alice, ActionType::get(SessionTemplate::KIND, &shared)).await, "stored without an owner, or the system's");
            assert!(!allows(alice, ActionType::update(SessionTemplate::KIND, &shared)).await);
            assert!(allows(ActorId::System, ActionType::update(SessionTemplate::KIND, &shared)).await);
        }
    }

    #[tokio::test]
    async fn nobody_creates_in_someone_elses_name_and_no_one_at_all_creates() {
        let (alice, bob) = (user(), user());
        assert!(allows(alice, ActionType::create(Session::KIND, &SecurityAttributes::owned_by(alice))).await);
        assert!(!allows(bob, ActionType::create(Session::KIND, &SecurityAttributes::owned_by(alice))).await);
        assert!(!allows(ActorId::None, ActionType::create(Session::KIND, &SecurityAttributes::default())).await, "an empty context has no rights");
        assert!(!allows(ActorId::None, ActionType::update(Session::KIND, &SecurityAttributes::default())).await);
    }

    #[tokio::test]
    async fn members_of_a_group_read_while_they_are_members_wherever_they_stand() {
        let (alice, bob, carol) = (user(), user(), user());
        let place = GroupId::new();
        let hers = SecurityAttributes::owned_by(alice).granting(Subject::Group(place), Grant::Read);
        let access_policy = knowing(Known { members: vec![(bob, place)], ..Known::default() });
        let elsewhere = SecurityContext::new(bob, SessionId::new(), ChannelId::new());
        assert!(access_policy.check(&elsewhere, ActionType::get(Session::KIND, &hers)).await, "a member, though he acts somewhere else");
        assert!(!access_policy.check(&context(carol), ActionType::get(Session::KIND, &hers)).await, "not who is no member");
        assert!(!access_policy.check(&context(bob), ActionType::update(Session::KIND, &hers)).await, "reading, not changing");
        let other = SecurityAttributes::owned_by(alice).granting(Subject::Group(GroupId::new()), Grant::Read);
        assert!(!access_policy.check(&context(bob), ActionType::get(Session::KIND, &other)).await, "a member of the group named, not of any");
    }

    #[tokio::test]
    async fn a_deleted_group_grants_nothing_but_its_members_hear_of_what_went_with_it() {
        let (alice, bob) = (user(), user());
        let place = GroupId::new();
        let hers = SecurityAttributes::owned_by(alice).granting(Subject::Group(place), Grant::Read);
        let access_policy = knowing(Known { members: vec![(bob, place)], deleted: vec![place] });
        assert!(!access_policy.check(&context(bob), ActionType::get(Session::KIND, &hers)).await, "read by nobody through it");
        assert!(access_policy.check(&context(bob), ActionType::hear_deleted(Session::KIND, &hers)).await, "but told of the deletion");
        assert!(!access_policy.check(&context(user()), ActionType::hear_deleted(Session::KIND, &hers)).await, "who was no member is not");
        assert!(allows(alice, ActionType::hear_deleted(Session::KIND, &hers)).await, "its owner is");
    }

    /// Issue 332: the admission a store narrows a list by is what `check`
    /// decides an entity by - for whom, by which entries.
    #[tokio::test]
    async fn the_admission_of_a_verb_names_who_and_what_admits() {
        let alice = user();
        let role = RoleId::new();
        let agent = ActorId::Agent(AgentId::new());
        let policy = OwnershipPolicy::new();
        let hers = context(alice).with_roles(vec![role], None);
        let admission = |context: &SecurityContext, verb| policy.admission(context, Session::KIND, verb).expect("an admission");
        let reading = Admission { actors: vec![alice], unowned: true, grant: Some(Grant::Read), roles: vec![role], everyone: true, deleted_groups: false };
        assert_eq!(admission(&hers, Verb::Get), reading, "reading: owned, the system's, or an entry covering it");
        assert_eq!(admission(&hers, Verb::HearDeleted), Admission { deleted_groups: true, ..reading.clone() }, "told of a deletion: by deleted groups as well");
        let as_agent = context(alice).next(agent, alice, alice);
        assert_eq!(admission(&as_agent, Verb::Get).actors, [alice, agent], "an agent reads as its owner and as itself");
        assert!(admission(&as_agent, Verb::Get).everyone, "an entry for everyone admits it by its owner");
        assert_eq!(admission(&as_agent, Verb::Update), Admission { actors: vec![alice], ..Admission::default() }, "changing: the principal's own");
        assert_eq!(admission(&context(ActorId::None), Verb::Update), Admission::default(), "nobody's for an empty context");
        assert_eq!(admission(&hers, Verb::Purge), Admission::default(), "purging: nobody");
        assert_eq!(admission(&hers, Verb::Custom(&WRITE)).grant, Some(Grant::Write), "a kind's verb by what it requires");
        assert_eq!(policy.admission(&SecurityContext::system(), Session::KIND, Verb::Get), None, "nothing narrowed for the system");
        assert_eq!(AllowAllPolicy.admission(&hers, Session::KIND, Verb::Get), None, "nor by a policy that does not narrow");
    }

    #[tokio::test]
    async fn a_group_is_read_by_its_owner_and_its_members_and_changed_by_its_owner() {
        let (alice, bob, carol) = (user(), user(), user());
        let ActorId::User(alice_id) = alice else { unreachable!() };
        let team = Group::team(ScopedName::new("project", Scope::GlobalScope).unwrap(), alice_id);
        let access_policy = knowing(Known { members: vec![(bob, team.id)], ..Known::default() });
        let theirs = &team.security_attributes;
        assert!(access_policy.check(&context(alice), ActionType::get(Group::KIND, theirs)).await, "its owner");
        assert!(access_policy.check(&context(bob), ActionType::get(Group::KIND, theirs)).await, "a member, by the group's own list");
        assert!(!access_policy.check(&context(carol), ActionType::get(Group::KIND, theirs)).await, "not a stranger");
        assert!(access_policy.check(&context(alice), ActionType::update(Group::KIND, theirs)).await && access_policy.check(&context(alice), ActionType::delete(Group::KIND, theirs)).await);
        assert!(!access_policy.check(&context(bob), ActionType::update(Group::KIND, theirs)).await, "a member does not change it");
    }

    #[tokio::test]
    async fn a_role_grants_to_whoever_holds_it_an_agent_narrowed_by_its_personality() {
        let (alice, bob) = (user(), user());
        let (hr, staff) = (RoleId::new(), RoleId::new());
        let agent = ActorId::Agent(AgentId::new());
        let hers = SecurityAttributes::owned_by(alice).granting(Subject::Role(hr), Grant::Read);
        // Bob holds hr; his agent is limited to staff, so of his roles only
        // that counts for it - as its context says (issue 304).
        let access_policy = OwnershipPolicy::new();
        let his_context = context(bob).with_roles(vec![hr, staff], None);
        let his_agents = context(bob).next(agent, bob, bob).with_roles(vec![hr, staff], Some(vec![staff]));
        assert!(access_policy.check(&his_context, ActionType::get(SKILL, &hers)).await, "who holds the role");
        assert!(!access_policy.check(&his_agents, ActionType::get(SKILL, &hers)).await, "not his agent, limited below it");
        assert!(!access_policy.check(&context(bob).next(agent, bob, bob), ActionType::get(SKILL, &hers)).await, "nor a context continued without roles: the initiator lends none");
        let his = SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Read);
        assert!(access_policy.check(&his_agents, ActionType::get(SKILL, &his)).await, "what he is granted as a person, his agent is too");
    }

    #[tokio::test]
    async fn a_grant_to_read_is_not_one_to_write_and_the_owner_needs_none() {
        let (alice, bob) = (user(), user());
        assert!(Grant::Write.covers(Grant::Read) && Grant::Write.covers(Grant::Write));
        assert!(Grant::Read.covers(Grant::Read) && !Grant::Read.covers(Grant::Write));
        let access_policy = OwnershipPolicy::new();
        let hers = shared(alice, &[]);
        assert!(access_policy.granted(&hers, Grant::Write, &context(alice)).await, "the owner, without an entry");
        let readable = shared(alice, &[bob]);
        assert!(access_policy.granted(&readable, Grant::Read, &context(bob)).await);
        assert!(!access_policy.granted(&readable, Grant::Write, &context(bob)).await, "read, not write");
        let writable = SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Write);
        assert!(access_policy.granted(&writable, Grant::Read, &context(bob)).await, "who may write may read");
    }

    #[tokio::test]
    async fn a_workspaces_files_are_read_and_changed_as_its_list_grants() {
        let (alice, bob, carol) = (user(), user(), user());
        let theirs = SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Read).granting(Subject::Actor(carol), Grant::Write);
        assert!(allows(alice, ActionType::custom(Workspace::KIND, &WRITE, &theirs)).await, "its owner");
        assert!(allows(bob, ActionType::get(Workspace::KIND, &theirs)).await && !allows(bob, ActionType::custom(Workspace::KIND, &WRITE, &theirs)).await, "a reader reads");
        assert!(allows(carol, ActionType::get(Workspace::KIND, &theirs)).await && allows(carol, ActionType::custom(Workspace::KIND, &WRITE, &theirs)).await, "a writer both");
        assert!(!allows(carol, ActionType::update(Workspace::KIND, &theirs)).await, "changing the workspace itself stays the owner's");
    }

    #[tokio::test]
    async fn who_may_join_reads_nothing_and_who_reads_may_not_join() {
        let (alice, bob, carol) = (user(), user(), user());
        let join = |attributes| ActionType::custom(Session::KIND, &JOIN, attributes);
        let invited = SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Join);
        assert!(allows(bob, join(&invited)).await, "whom an entry lets join");
        assert!(!allows(bob, ActionType::get(Session::KIND, &invited)).await, "reads nothing of it before they did");
        assert!(!allows(carol, join(&invited)).await, "nobody else");
        let read = shared(alice, &[bob]);
        assert!(!allows(bob, join(&read)).await, "reading lets nobody join");
        let written = SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Write);
        assert!(!allows(bob, join(&written)).await, "nor writing");
        let open = SecurityAttributes::owned_by(alice).granting(Subject::Everyone, Grant::Join);
        assert!(allows(carol, join(&open)).await, "an entry for everyone lets every user join");
        let agent = ActorId::Agent(AgentId::new());
        assert!(OwnershipPolicy::new().check(&context(carol).next(agent, carol, carol), join(&open)).await, "an agent by its owner");
        assert!(!allows(carol, ActionType::get(Session::KIND, &open)).await, "and reading it is another entry");
        assert!(allows(carol, ActionType::get(Session::KIND, &SecurityAttributes::owned_by(alice).granting(Subject::Everyone, Grant::Read))).await, "which may be for everyone, too");
    }
}
