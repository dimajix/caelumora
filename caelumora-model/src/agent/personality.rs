use super::PersonalityId;
use crate::actor::{ActorEvent, ActorId, Envelope, Pick};
use crate::chain::Position;
use crate::component::ComponentConfig;
use crate::confidentiality::Visibility;
use crate::message::Message;
use crate::removal::Removal;
use crate::role::RoleId;
use crate::scope::{ScopeField, Scoped};
use crate::security::{Secured, SecurityAttributes};
use crate::store::{EntityKind, NoEntity, Stored};
use crate::value::name::ScopedName;
use crate::value::text::{Description, Introduction};
use crate::work::Work;
use serde::{Deserialize, Serialize};

/// What kind of agent a personality makes (issue 179), and so which actor
/// runs it in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonalityKind {
    /// An agent of ours: its model, tools and turns are run here, by an
    /// `AgentActor`.
    #[default]
    Internal,
    /// An agent that runs elsewhere, reached through the one component of
    /// the personality that provides `ExternalAgent`, by an
    /// `ExternalAgentActor`. Whatever else the personality wires in is not
    /// read.
    External,
}

impl PersonalityKind {
    /// Whether an agent of this kind can be asked to answer again instead
    /// of one of its messages (issue 234): not one that runs elsewhere,
    /// whose remote side keeps what it was given (`RetryRefused::Elsewhere`).
    pub fn can_retry(self) -> bool {
        match self {
            Self::Internal => true,
            Self::External => false,
        }
    }
}

/// How the agents of a personality hear between the steps of a turn (issue 431), rather than
/// once the turn has ended: whose messages (`from`), and whether also what is said in another's
/// authority (`others`). Written by hand too: what is left out is the default, `from: none`
/// and `others: wait`. A personality can only narrow what the core allows, never widen it: a
/// request to respond in another's authority, and what is above the turn's classification, wait
/// for the turn's end whatever this says (`takes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HearsDuringTurn {
    pub from: HearsFrom,
    pub others: OthersWords,
}

/// Whose messages an agent takes in between the steps of a turn (issue 431).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HearsFrom {
    /// Nobody's: what is said while a turn runs is heard once it has ended.
    #[default]
    None,
    /// The people's, not those of other agents.
    Users,
    /// Everyone's, other agents' too.
    All,
}

/// Whether a turn takes in, between its steps, what is said in another's authority than its own
/// (issue 431) - by someone other than whoever began the chain it answers, or an agent working
/// for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OthersWords {
    /// It waits for the turn's end, with everything said after it: nobody steers what a turn
    /// does in someone else's authority, but the agent hears nothing more in that turn once
    /// another has spoken.
    #[default]
    Wait,
    /// It waits as with `Wait`, until the turn's own initiator asks the agent again after it
    /// (issue 435) - by mentioning it, or as the moderator asks it otherwise: what was said
    /// before that request is heard then, as between turns, where a turn hears what was said
    /// before the request it answers. A request of another still waits.
    WhenAsked,
    /// It is heard, as between turns: the agent stays able to hear, and another's words may
    /// steer what the turn does. A request of theirs still waits.
    Hear,
}

impl HearsFrom {
    /// Whether a message of `sender` is taken in, as far as whose it is goes.
    pub fn hears(self, sender: ActorId) -> bool {
        match self {
            Self::None => false,
            Self::Users => matches!(sender, ActorId::User(_)),
            Self::All => true,
        }
    }
}

impl HearsDuringTurn {
    /// Whether the piece of work `work`, under way, takes in `envelope` between its steps, rather
    /// than leaving it, and everything behind it, for its end (issue 431). What was said: if
    /// `from` hears its sender, if the work may see it (`Work::may_see`) - what it saw above its
    /// classification would be written below its level -, and, unless `others` hears it, if it
    /// is said in the work's authority: by whoever began the chain the work answers, or an agent
    /// working for them (`EventContext::initiator_id`). A request to respond: only in the
    /// work's authority, which then answers it - another's is answered in a turn in theirs. Who
    /// joined or left: unless it hears nobody. Nothing else: a change to a message, a request to
    /// answer again, a command, an approval are taken in an order of their own.
    pub fn takes(&self, work: &Work, envelope: &Envelope) -> bool {
        let in_its_authority = envelope.context.initiator_id == work.event_context.initiator_id;
        match &envelope.event {
            ActorEvent::CompleteMessageGenerated(message) => {
                self.hears_said(work, message)
                    && (self.others == OthersWords::Hear || in_its_authority)
            }
            ActorEvent::ResponseRequested { .. } => {
                self.from != HearsFrom::None && in_its_authority
            }
            ActorEvent::ParticipantJoined { .. } | ActorEvent::ParticipantLeft { .. } => {
                self.from != HearsFrom::None
            }
            _ => false,
        }
    }

    /// What the piece of work `work`, under way, does with `envelope` it looks at between its
    /// steps (issues 431, 435). What it `takes` it takes. Before `asked` - the position of a
    /// request in its authority it took ahead (`pick_request`) - it takes too what waits only
    /// for the authority it was said in, as between turns a turn hears what was said before
    /// the request it answers, and passes over another's request, which waits for a turn in
    /// theirs. At anything else it stops.
    pub fn pick(&self, work: &Work, envelope: &Envelope, asked: Option<Position>) -> Pick {
        if self.takes(work, envelope) {
            return Pick::Take;
        }
        let before_asked =
            asked.zip(envelope.context.position).is_some_and(|(asked, at)| at < asked);
        match &envelope.event {
            ActorEvent::ResponseRequested { .. } if before_asked => Pick::PassOver,
            _ if before_asked && self.waits_for_authority(work, envelope) => Pick::Take,
            _ => Pick::Stop,
        }
    }

    /// What `work` does with `envelope` looking past what waits for the authority it was said
    /// in, for a request in its own authority that lets it hear what came before (issue 435):
    /// with `others: when_asked`, it takes such a request, passes over what it would take
    /// before it (`pick`) - what was said, by anyone, another's request, who joined or left -,
    /// and stops at anything else. Otherwise it stops at once. A request not recorded cannot
    /// say what came before it: it is stopped at too.
    pub fn pick_request(&self, work: &Work, envelope: &Envelope) -> Pick {
        if self.others != OthersWords::WhenAsked {
            return Pick::Stop;
        }
        let in_its_authority = envelope.context.initiator_id == work.event_context.initiator_id;
        match &envelope.event {
            ActorEvent::ResponseRequested { .. } if in_its_authority => {
                if self.takes(work, envelope) && envelope.context.position.is_some() {
                    Pick::Take
                } else {
                    Pick::Stop
                }
            }
            ActorEvent::ResponseRequested { .. } => Pick::PassOver,
            _ if self.takes(work, envelope) || self.waits_for_authority(work, envelope) => {
                Pick::PassOver
            }
            _ => Pick::Stop,
        }
    }

    /// Whether `envelope` is something said that `work` would take but for the authority it is
    /// said in: what a request in the work's own authority after it lets it hear, with
    /// `others: when_asked`.
    fn waits_for_authority(&self, work: &Work, envelope: &Envelope) -> bool {
        match &envelope.event {
            ActorEvent::CompleteMessageGenerated(message) => {
                self.others == OthersWords::WhenAsked
                    && self.hears_said(work, message)
                    && envelope.context.initiator_id != work.event_context.initiator_id
            }
            _ => false,
        }
    }

    /// Whether `work` hears `message` whoever's authority it is said in: if `from` hears its
    /// sender, and if the work may see it (`Work::may_see`) - what it saw above its
    /// classification would be written below its level.
    fn hears_said(&self, work: &Work, message: &Message) -> bool {
        self.from.hears(message.sender_id) && work.may_see(&message.classification)
    }
}

/// An agent's traits: its components and how confidential its reasoning is.
/// Written by hand, too (a seed file, issue 105): the fields with a default
/// may be left out there (issue 108).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Personality {
    pub id: PersonalityId,
    /// What it is found by: its name and where it lies (issue 177), the
    /// global scope unless named.
    #[serde(flatten)]
    pub name: ScopedName,
    /// What kind of agent it makes: `internal` unless named.
    #[serde(default)]
    pub kind: PersonalityKind,
    /// The roles its agents are limited to (issue 179); see
    /// [`Personality::limiting_roles`]. Not their roles: an agent is
    /// cleared by its owner's, these can only narrow them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<RoleId>>,
    /// What the personality is, for whoever chooses one: a catalogue line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    /// How its agents are introduced (issue 176) - to the others in their
    /// channel and to themselves: who they are, what they are there for.
    /// Public, like an agent's name. Not `description`, which says what the
    /// personality is, to whoever chooses one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introduction: Option<Introduction>,
    /// Whether the agent first analyzes the request and proposes a
    /// structured plan requiring user approval before executing it, instead
    /// of answering directly, is derived from whether `components` wires in
    /// the `create_plan_tool` component - not a separate flag here, since
    /// extended planning is just another tool a personality opts into.
    #[serde(default)]
    pub components: ComponentConfig,
    #[serde(default = "SecurityAttributes::systems")]
    pub security_attributes: SecurityAttributes,
    /// Who sees the reasoning of its agents (issues 61, 133): `owner` keeps
    /// it from the channel always; otherwise what a turn saw decides.
    /// `channel` unless named.
    #[serde(default)]
    pub reasoning_visibility: Visibility,
    /// How its agents hear between the steps of a turn (issue 431):
    /// nobody unless named.
    #[serde(default)]
    pub hears_during_turn: HearsDuringTurn,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}

impl Personality {
    /// The roles that limit what its agents are cleared for, besides their
    /// owner's (issue 179): an agent joins a channel only if its owner is
    /// cleared for it and, where this is `Some`, these roles are too. An
    /// internal personality that names none limits nothing; an external
    /// one that names none is limited to no roles - what is `public` - since
    /// what its agents are given leaves the system. Like every clearance,
    /// it counts only where roles are configured.
    pub fn limiting_roles(&self) -> Option<&[RoleId]> {
        match (&self.roles, self.kind) {
            (Some(roles), _) => Some(roles),
            (None, PersonalityKind::External) => Some(&[]),
            (None, PersonalityKind::Internal) => None,
        }
    }
}

impl Stored for Personality {
    const KIND: EntityKind = EntityKind::new("personality");
    type Id = PersonalityId;
    type Within = NoEntity;
    type Field = ScopeField;

    fn id(&self) -> PersonalityId {
        self.id
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
    }
}

impl Secured for Personality {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Scoped for Personality {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{EventContext, RequestReason};
    use crate::agent::AgentId;
    use crate::confidentiality::Classification;
    use crate::security::SecurityContext;
    use crate::user::UserId;

    #[test]
    fn a_personality_is_internal_unless_named_external() {
        let id = PersonalityId::new();
        let written = |kind: &str| serde_json::json!({ "id": id, "name": "plain", "kind": kind });
        let plain: Personality =
            serde_json::from_value(serde_json::json!({ "id": id, "name": "plain" })).unwrap();
        assert_eq!(plain.kind, PersonalityKind::Internal, "left out");
        let external: Personality = serde_json::from_value(written("external")).unwrap();
        assert_eq!(external.kind, PersonalityKind::External);
        assert!(serde_json::from_value::<Personality>(written("remote")).is_err(), "no other kind");
    }

    #[test]
    fn a_personality_hears_nobody_during_a_turn_unless_named_and_waits_with_others_words() {
        let id = PersonalityId::new();
        let written = |hears: serde_json::Value| {
            serde_json::from_value::<Personality>(
                serde_json::json!({ "id": id, "name": "plain", "hears_during_turn": hears }),
            )
        };
        let plain: Personality =
            serde_json::from_value(serde_json::json!({ "id": id, "name": "plain" })).unwrap();
        assert_eq!(
            plain.hears_during_turn,
            HearsDuringTurn { from: HearsFrom::None, others: OthersWords::Wait },
            "left out"
        );
        let users = written(serde_json::json!({ "from": "users" })).unwrap();
        assert_eq!(
            users.hears_during_turn,
            HearsDuringTurn { from: HearsFrom::Users, others: OthersWords::Wait },
            "others' words wait unless named"
        );
        let all = written(serde_json::json!({ "from": "all", "others": "hear" })).unwrap();
        assert_eq!(
            all.hears_during_turn,
            HearsDuringTurn { from: HearsFrom::All, others: OthersWords::Hear }
        );
        let asked =
            written(serde_json::json!({ "from": "users", "others": "when_asked" })).unwrap();
        assert_eq!(
            asked.hears_during_turn,
            HearsDuringTurn { from: HearsFrom::Users, others: OthersWords::WhenAsked }
        );
        assert!(written(serde_json::json!({ "from": "agents" })).is_err(), "nothing else");
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    fn a_turn_takes_in_between_its_steps_what_its_personality_hears_and_never_a_request_in_anothers_authority(
    ) {
        use crate::confidentiality::Confidentiality;
        let alice = ActorId::User(UserId::new());
        let bob = ActorId::User(UserId::new());
        let agent = ActorId::Agent(AgentId::new());
        let internal = Classification::general(Confidentiality::Internal);
        let secret = Classification::general(Confidentiality::StrictlyConfidential);
        let work = Work::new(
            EventContext { initiator_id: alice, ..EventContext::default() },
            None,
            SecurityContext::default(),
            internal.clone(),
            Default::default(),
        );
        let in_authority_of = |initiator: ActorId, event: ActorEvent| {
            Envelope::new(
                SecurityContext::default(),
                EventContext { initiator_id: initiator, ..EventContext::default() },
                event,
            )
        };
        // Said by `sender`, in the authority of `initiator`, classified `classification`.
        let said = |sender: ActorId, initiator: ActorId, classification: &Classification| {
            in_authority_of(
                initiator,
                ActorEvent::CompleteMessageGenerated(Message {
                    sender_id: sender,
                    ..Message::new(classification.clone())
                }),
            )
        };
        let hears = |from, others| HearsDuringTurn { from, others };
        let (wait, hear) = (OthersWords::Wait, OthersWords::Hear);
        let cases = [
            ("nobody's", hears(HearsFrom::None, hear), said(alice, alice, &internal), false),
            ("hers", hears(HearsFrom::Users, wait), said(alice, alice, &internal), true),
            (
                "an agent's, hearing users",
                hears(HearsFrom::Users, hear),
                said(agent, alice, &internal),
                false,
            ),
            (
                "an agent's, hearing all",
                hears(HearsFrom::All, wait),
                said(agent, alice, &internal),
                true,
            ),
            (
                "his, waiting with others' words",
                hears(HearsFrom::All, wait),
                said(bob, bob, &internal),
                false,
            ),
            (
                "his, hearing others' words",
                hears(HearsFrom::Users, hear),
                said(bob, bob, &internal),
                true,
            ),
            (
                "his, until she asks",
                hears(HearsFrom::Users, OthersWords::WhenAsked),
                said(bob, bob, &internal),
                false,
            ),
            (
                "an agent's for him, waiting",
                hears(HearsFrom::All, wait),
                said(agent, bob, &internal),
                false,
            ),
            (
                "above the turn's classification",
                hears(HearsFrom::All, hear),
                said(alice, alice, &secret),
                false,
            ),
            (
                "her request",
                hears(HearsFrom::Users, wait),
                in_authority_of(
                    alice,
                    ActorEvent::ResponseRequested { reason: RequestReason::Mentioned },
                ),
                true,
            ),
            (
                "his request, hearing others' words",
                hears(HearsFrom::All, hear),
                in_authority_of(
                    bob,
                    ActorEvent::ResponseRequested { reason: RequestReason::Mentioned },
                ),
                false,
            ),
            (
                "her request, hearing nobody",
                hears(HearsFrom::None, hear),
                in_authority_of(
                    alice,
                    ActorEvent::ResponseRequested { reason: RequestReason::Mentioned },
                ),
                false,
            ),
            (
                "a command",
                hears(HearsFrom::All, hear),
                in_authority_of(
                    alice,
                    ActorEvent::CommandRequested {
                        command: "mark".to_string(),
                        args: String::new(),
                        agent: ActorId::None,
                    },
                ),
                false,
            ),
        ];
        for (what, hears, envelope, taken) in cases {
            assert_eq!(hears.takes(&work, &envelope), taken, "{what}");
        }
    }

    /// Issue 435: with `others: when_asked`, a request in the work's authority lets it hear
    /// another's words said before it - and nothing else that waits; another's request lifts
    /// nothing, and is passed over.
    #[test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    fn a_request_in_the_works_authority_lets_it_hear_anothers_words_said_before_it() {
        use crate::confidentiality::Confidentiality;
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let agent = ActorId::Agent(AgentId::new());
        let internal = Classification::general(Confidentiality::Internal);
        let secret = Classification::general(Confidentiality::StrictlyConfidential);
        let work = Work::new(
            EventContext { initiator_id: alice, ..EventContext::default() },
            None,
            SecurityContext::default(),
            internal.clone(),
            Default::default(),
        );
        let at = |seq: u64, initiator: ActorId, event: ActorEvent| {
            Envelope::new(
                SecurityContext::default(),
                EventContext {
                    initiator_id: initiator,
                    position: Some(Position::new(seq)),
                    ..EventContext::default()
                },
                event,
            )
        };
        let said = |seq, sender: ActorId, classification: &Classification| {
            at(
                seq,
                sender,
                ActorEvent::CompleteMessageGenerated(Message {
                    sender_id: sender,
                    ..Message::new(classification.clone())
                }),
            )
        };
        let command = ActorEvent::CommandRequested {
            command: "mark".to_string(),
            args: String::new(),
            agent: ActorId::None,
        };
        let hears = |others| HearsDuringTurn { from: HearsFrom::Users, others };
        let (asked, wait) = (hears(OthersWords::WhenAsked), hears(OthersWords::Wait));

        let looking_for_a_request = [
            (
                "her request",
                asked,
                at(5, alice, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }),
                Pick::Take,
            ),
            (
                "her request, unrecorded",
                asked,
                Envelope::new(
                    SecurityContext::default(),
                    EventContext { initiator_id: alice, ..EventContext::default() },
                    ActorEvent::ResponseRequested { reason: RequestReason::Mentioned },
                ),
                Pick::Stop,
            ),
            ("his words", asked, said(5, bob, &internal), Pick::PassOver),
            ("her words", asked, said(5, alice, &internal), Pick::PassOver),
            (
                "his request",
                asked,
                at(5, bob, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }),
                Pick::PassOver,
            ),
            ("an agent's words, hearing users", asked, said(5, agent, &internal), Pick::Stop),
            ("his words above the work's classification", asked, said(5, bob, &secret), Pick::Stop),
            ("a command", asked, at(5, alice, command.clone()), Pick::Stop),
            (
                "her request, waiting",
                wait,
                at(5, alice, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }),
                Pick::Stop,
            ),
        ];
        for (what, hears, envelope, picked) in looking_for_a_request {
            assert_eq!(hears.pick_request(&work, &envelope), picked, "{what}");
        }

        let asked_at = Some(Position::new(5));
        let looking_before_it = [
            ("his words before", asked, said(3, bob, &internal), asked_at, Pick::Take),
            ("his words after", asked, said(7, bob, &internal), asked_at, Pick::Stop),
            ("his words, nobody asked", asked, said(3, bob, &internal), None, Pick::Stop),
            ("his words before, waiting", wait, said(3, bob, &internal), asked_at, Pick::Stop),
            (
                "his request before",
                asked,
                at(3, bob, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }),
                asked_at,
                Pick::PassOver,
            ),
            (
                "his request after",
                asked,
                at(7, bob, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }),
                asked_at,
                Pick::Stop,
            ),
            ("her words after", asked, said(7, alice, &internal), asked_at, Pick::Take),
            (
                "his words above the work's classification before",
                asked,
                said(3, bob, &secret),
                asked_at,
                Pick::Stop,
            ),
            (
                "an agent's words before, hearing users",
                asked,
                said(3, agent, &internal),
                asked_at,
                Pick::Stop,
            ),
            ("a command before", asked, at(3, alice, command.clone()), asked_at, Pick::Stop),
        ];
        for (what, hears, envelope, asked, picked) in looking_before_it {
            assert_eq!(hears.pick(&work, &envelope, asked), picked, "{what}");
        }
    }

    #[test]
    fn only_an_agent_run_here_can_retry() {
        assert!(PersonalityKind::Internal.can_retry());
        assert!(!PersonalityKind::External.can_retry(), "its remote side keeps what it was given");
    }

    #[test]
    fn a_personality_limits_its_agents_only_where_it_names_roles_unless_external() {
        let mut personality: Personality = serde_json::from_value(
            serde_json::json!({ "id": PersonalityId::new(), "name": "plain" }),
        )
        .unwrap();
        assert_eq!(personality.limiting_roles(), None, "internal, none named: the owner's alone");
        personality.kind = PersonalityKind::External;
        assert_eq!(
            personality.limiting_roles(),
            Some(&[][..]),
            "external, none named: public only"
        );
        let staff = RoleId::new();
        personality.roles = Some(vec![staff]);
        assert_eq!(personality.limiting_roles(), Some(&[staff][..]));
        personality.kind = PersonalityKind::Internal;
        assert_eq!(personality.limiting_roles(), Some(&[staff][..]), "named, internal too");
    }
}
