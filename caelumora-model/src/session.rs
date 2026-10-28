pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
use crate::approval::ApprovalRule;
use crate::component::ComponentConfig;
use crate::privacy::PrivacyConfig;
use crate::actor::ActorId;
use crate::group::GroupId;
use crate::security::{AclEntry, Grant, Secured, SecurityAttributes, Subject};
use crate::user::UserId;
use crate::store::query::{Field, Filter};
use chrono::{DateTime, Utc};
use crate::value::language::Language;
use crate::value::name::Name;
use crate::value::time_zone::TimeZone;
use crate::value::text::{Briefing, Summary};
use crate::value::ValueError;
use crate::project::ProjectId;
use crate::workspace::WorkspaceId;
use serde::{Deserialize, Serialize};
use crate::removal::Removal;
use crate::store::{EntityKind, NoEntity, Stored, Updatable};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionUpdateEvent {
    /// Renamed, as it was asked: the name is made from it as the change is
    /// applied (issue 248), as a description is.
    NameChanged(String),
    /// Its channels are to be held in another language, or in none in
    /// particular, where they hold none of their own (issue 425); made into
    /// a language as the change is applied, as a name is.
    LanguageChanged(Option<String>),
    /// It is to be held in another time zone, or with `None` in UTC (issue
    /// 424); made into a time zone as the change is applied.
    TimeZoneChanged(Option<String>),
    /// The session's privacy policy changed (issue 134).
    PrivacyChanged(PrivacyConfig),
    /// Its rules on tool calls changed (issue 13), as they now stand.
    ApprovalRulesChanged(Vec<ApprovalRule>),
    /// What it is for changed (issue 245), as it now stands.
    DescriptionChanged { summary: Option<String>, briefing: Option<String> },
    /// A workspace was attached, or the one attached taken away (issue
    /// 253), as it now stands.
    WorkspaceChanged(Option<WorkspaceId>),
    /// Someone was taken in after it was built (issue 221): by whoever may
    /// change it, or by themselves, where an entry let them join.
    ParticipantJoined(Participant),
}

define_id!(SessionId);

/// A session was started: its channels run (`EntityChange::Custom`, issue
/// 323).
pub const STARTED: &str = "started";

/// A session was stopped for everyone in it (`EntityChange::Custom`).
pub const STOPPED: &str = "stopped";


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    /// What it is called (issue 248): a name it could be looked up by.
    pub name: Name,
    /// What it is for, in a line (issue 245); `None` if nobody said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    /// What an agent should know before it works in any of
    /// the session's channels (issue 245), given to
    /// the agents whose personality wires in the
    /// `channel_briefing_prompt_provider`; `None` if nobody wrote one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing: Option<Briefing>,
    /// The language its channels are held in where they hold none of their
    /// own (issue 425, `Channel::effective_language`); `None` if it is held
    /// in none in particular.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    /// The time zone the times its agents are given are written in (issue
    /// 424, `effective_time_zone`); `None` for UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<TimeZone>,
    pub participants: Vec<Participant>,
    /// The session's channels by id, in order; each is a document of its own
    /// (issue 70).
    pub channels: Vec<ChannelId>,
    /// The session's own user commands, copied from its template when it was
    /// created, so that a running session keeps its set (issue 28).
    pub commands: ComponentConfig,
    /// Whom the session lets read what was said in it (issue 134).
    #[serde(default)]
    pub privacy: PrivacyConfig,
    /// What the session restricts of the tool calls made in any of its
    /// channels (issue 13): rules that ask or deny, copied from its
    /// template when it was created and set by whoever may change it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_rules: Vec<ApprovalRule>,
    /// The workspace its people and agents share in all its channels (issue
    /// 253), at most one; attached by whoever may change the session. Which
    /// of its channels may use it, the workspace's classification decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    /// The project its results go to (issue 467): its own, lying in its
    /// scope, or a shared one it lies in.
    pub project: ProjectId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub security_attributes: SecurityAttributes,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}

impl Stored for Session {
    const KIND: EntityKind = EntityKind::new("session");
    type Id = SessionId;
    type Within = NoEntity;
    type Field = SessionField;

    fn id(&self) -> SessionId {
        self.id
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }
}

/// What a change of the session says is made into its values here (issue
/// 248).
impl Updatable for Session {
    type Update = SessionUpdateEvent;

    fn apply(&mut self, event: &SessionUpdateEvent) -> Result<(), ValueError> {
        match event {
            SessionUpdateEvent::NameChanged(name) => self.name = Name::new(name)?,
            SessionUpdateEvent::LanguageChanged(language) => self.language = Language::optional(language.as_deref())?,
            SessionUpdateEvent::TimeZoneChanged(zone) => self.time_zone = TimeZone::optional(zone.as_deref())?,
            SessionUpdateEvent::PrivacyChanged(privacy) => self.privacy = *privacy,
            SessionUpdateEvent::ApprovalRulesChanged(rules) => self.approval_rules = rules.clone(),
            SessionUpdateEvent::DescriptionChanged { summary, briefing } => self.describe(summary.as_deref(), briefing.as_deref())?,
            SessionUpdateEvent::WorkspaceChanged(workspace) => self.workspace = *workspace,
            SessionUpdateEvent::ParticipantJoined(participant) => self.take_in(participant.clone()),
        }
        Ok(())
    }

    fn touch(&mut self, at: DateTime<Utc>) {
        self.updated_at = at;
    }
}

impl Session {
    /// The time zone the session is held in (issue 424): its own, else UTC -
    /// never the server's, which is the deployment's and would make what a
    /// model is given depend on the machine.
    pub fn effective_time_zone(&self) -> TimeZone {
        self.time_zone.unwrap_or(TimeZone::UTC)
    }

    /// The attributes of a new session `id` of `owner`: read by its
    /// members, whoever they are at the time (issue 23) - the group of its
    /// participants, stored with it (`Group::of_session`) -, and granting
    /// what `access` grants besides, such as whom it lets join (issue 221).
    pub fn attributes_of(id: SessionId, owner: UserId, access: &[AclEntry]) -> SecurityAttributes {
        let attributes = SecurityAttributes::owned_by(ActorId::User(owner)).granting(Subject::Group(GroupId::of_session(id)), Grant::Read);
        SecurityAttributes { acl: attributes.acl.into_iter().chain(access.iter().copied()).collect(), ..attributes }
    }

    /// Says what the session is for (issue 245): `summary` and `briefing`
    /// replace what it said - a missing or empty one removes it.
    ///
    /// # Errors
    /// `ValueError` for the first that does not fit; the session is then
    /// unchanged.
    pub fn describe(&mut self, summary: Option<&str>, briefing: Option<&str>) -> Result<(), ValueError> {
        (self.summary, self.briefing) = (Summary::optional(summary)?, Briefing::optional(briefing)?);
        Ok(())
    }

    /// Takes `participant` in (issue 221), unless it takes part already;
    /// the group of its participants follows it where it is stored
    /// (`Group::of_session`). An entry that let a user join them alone - an
    /// invitation - is used up, so that one removed later cannot take
    /// themselves back in; one for a role, a group or everyone stays.
    /// Whether they may is asked before.
    pub fn take_in(&mut self, participant: Participant) {
        let actor = participant.actor_id();
        self.security_attributes.acl.retain(|entry| !(entry.grant == Grant::Join && entry.subject == Subject::Actor(actor)));
        if !self.takes_part(actor) {
            self.participants.push(participant);
        }
    }

    /// Whether `actor` is one of its participants.
    pub fn takes_part(&self, actor: ActorId) -> bool {
        self.participants.iter().any(|participant| participant.actor_id() == actor)
    }
}

/// What a query of sessions may name (issue 293). `Participant` is an
/// element of `Participants`, the whole of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionField {
    /// Its participants.
    Participants,
    /// A participant, within `Participants`.
    Participant,
}

impl Field for SessionField {
    fn path(self) -> &'static [&'static str] {
        match self {
            SessionField::Participants => &["participants"],
            SessionField::Participant => &[],
        }
    }
}

/// The conditions a list of sessions is asked by.
impl Filter<SessionField> {
    /// The sessions `actor` takes part in (`Session::takes_part`).
    pub fn with_participant(actor: ActorId) -> Self {
        Filter::any(SessionField::Participants, Filter::eq(SessionField::Participant, actor))
    }
}

impl Secured for Session {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentId;

    #[test]
    fn taking_a_user_in_uses_up_their_invitation_and_no_rule() {
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let id = SessionId::new();
        let access = [
            AclEntry { subject: Subject::Actor(ActorId::User(bob)), grant: Grant::Join },
            AclEntry { subject: Subject::Actor(ActorId::User(carol)), grant: Grant::Join },
            AclEntry { subject: Subject::Everyone, grant: Grant::Join },
        ];
        let now = Utc::now();
        let mut session = Session {
            id, name: Name::new("s").unwrap(), summary: None, briefing: None, language: None, time_zone: None, participants: vec![Participant::User(alice)], channels: Vec::new(),
            commands: Default::default(), privacy: Default::default(), approval_rules: Vec::new(), workspace: None, project: ProjectId::new(), created_at: now, updated_at: now,
            security_attributes: Session::attributes_of(id, alice, &access), deleted: None,
        };
        session.take_in(Participant::User(bob));
        session.take_in(Participant::User(bob));
        session.take_in(Participant::Agent(AgentId::new()));
        assert_eq!(session.participants.len(), 3, "each once");
        let joins: Vec<Subject> = session.security_attributes.acl.iter().filter(|e| e.grant == Grant::Join).map(|e| e.subject).collect();
        assert_eq!(joins, [Subject::Actor(ActorId::User(carol)), Subject::Everyone], "bob's invitation is used up, carol's and the rule stay");
        assert!(session.security_attributes.acl.contains(&AclEntry { subject: Subject::Group(GroupId::of_session(id)), grant: Grant::Read }), "read by its members");
    }

    /// The condition of the sessions an actor takes part in meets exactly
    /// those it is a participant of.
    #[test]
    fn the_sessions_with_a_participant_are_those_it_takes_part_in() {
        let (alice, agent) = (UserId::new(), AgentId::new());
        let id = SessionId::new();
        let now = Utc::now();
        let session = Session {
            id, name: Name::new("s").unwrap(), summary: None, briefing: None, language: None, time_zone: None,
            participants: vec![Participant::User(alice), Participant::Agent(agent)], channels: Vec::new(),
            commands: Default::default(), privacy: Default::default(), approval_rules: Vec::new(), workspace: None, project: ProjectId::new(), created_at: now, updated_at: now,
            security_attributes: Session::attributes_of(id, alice, &[]), deleted: None,
        };
        let document = serde_json::to_value(&session).unwrap();
        for actor in [ActorId::User(alice), ActorId::Agent(agent), ActorId::User(UserId::new()), ActorId::Agent(AgentId::new())] {
            assert_eq!(Filter::with_participant(actor).matches(&document), session.takes_part(actor), "{actor}");
        }
    }

    #[test]
    fn a_change_is_made_into_the_session_s_values_or_leaves_it_as_it_was() {
        let (alice, id, now) = (UserId::new(), SessionId::new(), Utc::now());
        let mut session = Session {
            id, name: Name::new("s").unwrap(), summary: None, briefing: None, language: None, time_zone: None, participants: vec![Participant::User(alice)], channels: Vec::new(),
            commands: Default::default(), privacy: Default::default(), approval_rules: Vec::new(), workspace: None, project: ProjectId::new(), created_at: now, updated_at: now,
            security_attributes: Session::attributes_of(id, alice, &[]), deleted: None,
        };
        session.apply(&SessionUpdateEvent::NameChanged("planning".to_string())).unwrap();
        session.apply(&SessionUpdateEvent::DescriptionChanged { summary: Some("What we plan".to_string()), briefing: None }).unwrap();
        session.apply(&SessionUpdateEvent::ParticipantJoined(Participant::User(alice))).unwrap();
        assert_eq!((session.name.as_str(), session.summary.as_ref().map(|s| s.as_str())), ("planning", Some("What we plan")));
        assert_eq!(session.participants.len(), 1, "taken in as `take_in` does: once");
        session.apply(&SessionUpdateEvent::LanguageChanged(Some("de".to_string()))).unwrap();
        assert_eq!(session.language.as_ref().map(Language::as_str), Some("de"), "made into a language");
        assert!(session.apply(&SessionUpdateEvent::LanguageChanged(Some("no language".to_string()))).is_err(), "no language tag");
        assert_eq!(session.language.as_ref().map(Language::as_str), Some("de"), "unchanged");
        session.apply(&SessionUpdateEvent::LanguageChanged(None)).unwrap();
        assert_eq!(session.language, None, "taken back");

        assert_eq!(session.effective_time_zone(), TimeZone::UTC, "UTC where it names none");
        session.apply(&SessionUpdateEvent::TimeZoneChanged(Some("Europe/Berlin".to_string()))).unwrap();
        assert_eq!(session.effective_time_zone().as_str(), "Europe/Berlin");
        assert!(session.apply(&SessionUpdateEvent::TimeZoneChanged(Some("Europe/Atlantis".to_string()))).is_err(), "no time zone");
        assert_eq!(session.effective_time_zone().as_str(), "Europe/Berlin", "unchanged");

        assert!(session.apply(&SessionUpdateEvent::NameChanged(String::new())).is_err(), "no name");
        assert_eq!(session.name.as_str(), "planning", "unchanged");
    }
}
