//! A session (issue 88): the container of one interaction - who takes part, its channels, how
//! private it is, the workspace it is done in and the project its results go to - and its changes.

use crate::actor::ActorId;
use crate::approval::ApprovalRule;
pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
use crate::component::ComponentConfig;
use crate::group::GroupId;
use crate::privacy::PrivacyConfig;
use crate::project::{Project, ProjectId};
use crate::removal::Removal;
use crate::security::{AclEntry, Grant, Secured, SecurityAttributes, Subject};
use crate::store::query::{Field, Filter};
use crate::store::{EntityKind, NoEntity, Stored, Updatable};
use crate::user::UserId;
use crate::value::language::Language;
use crate::value::name::Name;
use crate::value::text::{Briefing, Summary};
use crate::value::time_zone::TimeZone;
use crate::value::ValueError;
use crate::workspace::WorkspaceId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    /// It was put into another project, or taken out into its own (issue
    /// 467): the project its results go to from now on.
    ProjectChanged(ProjectId),
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
            SessionUpdateEvent::LanguageChanged(language) => {
                self.language = Language::optional(language.as_deref())?
            }
            SessionUpdateEvent::TimeZoneChanged(zone) => {
                self.time_zone = TimeZone::optional(zone.as_deref())?
            }
            SessionUpdateEvent::PrivacyChanged(privacy) => self.privacy = *privacy,
            SessionUpdateEvent::ApprovalRulesChanged(rules) => self.approval_rules = rules.clone(),
            SessionUpdateEvent::DescriptionChanged { summary, briefing } => {
                self.describe(summary.as_deref(), briefing.as_deref())?
            }
            SessionUpdateEvent::WorkspaceChanged(workspace) => self.workspace = *workspace,
            SessionUpdateEvent::ProjectChanged(project) => self.project = *project,
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
        let attributes = SecurityAttributes::owned_by(ActorId::User(owner))
            .granting(Subject::Group(GroupId::of_session(id)), Grant::Read);
        SecurityAttributes {
            acl: attributes.acl.into_iter().chain(access.iter().copied()).collect(),
            ..attributes
        }
    }

    /// A new own project of the session (issue 467): its owner's, in its scope, read by its
    /// participants (`Project::own_of`) - `None` if its owner is no user. Each call makes another,
    /// with an id of its own, and stores nothing: the one stored is found by its name
    /// (`Project::own_name`).
    pub fn new_own_project(&self) -> Option<Project> {
        self.owner().map(|owner| Project::own_of(self.id, owner))
    }

    /// Says what the session is for (issue 245): `summary` and `briefing`
    /// replace what it said - a missing or empty one removes it.
    ///
    /// # Errors
    /// `ValueError` for the first that does not fit; the session is then
    /// unchanged.
    pub fn describe(
        &mut self,
        summary: Option<&str>,
        briefing: Option<&str>,
    ) -> Result<(), ValueError> {
        (self.summary, self.briefing) =
            (Summary::optional(summary)?, Briefing::optional(briefing)?);
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
        self.security_attributes.acl.retain(|entry| {
            !(entry.grant == Grant::Join && entry.subject == Subject::Actor(actor))
        });
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
    /// The project its results go to (issue 467).
    Project,
}

impl Field for SessionField {
    fn path(self) -> &'static [&'static str] {
        match self {
            SessionField::Participants => &["participants"],
            SessionField::Participant => &[],
            SessionField::Project => &["project"],
        }
    }
}

/// The conditions a list of sessions is asked by.
impl Filter<SessionField> {
    /// The sessions `actor` takes part in (`Session::takes_part`).
    pub fn with_participant(actor: ActorId) -> Self {
        Filter::any(SessionField::Participants, Filter::eq(SessionField::Participant, actor))
    }

    /// The sessions whose results go to `project` (issue 467): those that lie in it.
    pub fn in_project(project: ProjectId) -> Self {
        Filter::eq(SessionField::Project, project)
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

    /// A new session of `owner` with `participants`, granting what `access` grants besides.
    fn session_of(owner: UserId, participants: Vec<Participant>, access: &[AclEntry]) -> Session {
        let id = SessionId::new();
        let now = Utc::now();
        Session {
            id,
            name: Name::new("s").unwrap(),
            summary: None,
            briefing: None,
            language: None,
            time_zone: None,
            participants,
            channels: Vec::new(),
            commands: Default::default(),
            privacy: Default::default(),
            approval_rules: Vec::new(),
            workspace: None,
            project: ProjectId::new(),
            created_at: now,
            updated_at: now,
            security_attributes: Session::attributes_of(id, owner, access),
            deleted: None,
        }
    }

    #[test]
    fn taking_a_user_in_uses_up_their_invitation_and_no_rule() {
        let alice = UserId::new();
        let bob = UserId::new();
        let carol = UserId::new();
        let access = [
            AclEntry { subject: Subject::Actor(ActorId::User(bob)), grant: Grant::Join },
            AclEntry { subject: Subject::Actor(ActorId::User(carol)), grant: Grant::Join },
            AclEntry { subject: Subject::Everyone, grant: Grant::Join },
        ];
        let mut session = session_of(alice, vec![Participant::User(alice)], &access);
        let id = session.id;
        session.take_in(Participant::User(bob));
        session.take_in(Participant::User(bob));
        session.take_in(Participant::Agent(AgentId::new()));
        assert_eq!(session.participants.len(), 3, "each once");
        let joins: Vec<Subject> = session
            .security_attributes
            .acl
            .iter()
            .filter(|e| e.grant == Grant::Join)
            .map(|e| e.subject)
            .collect();
        assert_eq!(
            joins,
            [Subject::Actor(ActorId::User(carol)), Subject::Everyone],
            "bob's invitation is used up, carol's and the rule stay"
        );
        assert!(
            session.security_attributes.acl.contains(&AclEntry {
                subject: Subject::Group(GroupId::of_session(id)),
                grant: Grant::Read
            }),
            "read by its members"
        );
    }

    /// The condition of the sessions an actor takes part in meets exactly
    /// those it is a participant of.
    #[test]
    fn the_sessions_with_a_participant_are_those_it_takes_part_in() {
        let alice = UserId::new();
        let agent = AgentId::new();
        let session =
            session_of(alice, vec![Participant::User(alice), Participant::Agent(agent)], &[]);
        let document = serde_json::to_value(&session).unwrap();
        for actor in [
            ActorId::User(alice),
            ActorId::Agent(agent),
            ActorId::User(UserId::new()),
            ActorId::Agent(AgentId::new()),
        ] {
            assert_eq!(
                Filter::with_participant(actor).matches(&document),
                session.takes_part(actor),
                "{actor}"
            );
        }
    }

    /// Issue 485: a session's new own project is its owner's and lies in its scope; a session of
    /// no user has none.
    #[test]
    fn a_new_own_project_is_the_owner_s_in_the_session_s_scope() {
        let alice = UserId::new();
        let mut session = session_of(alice, Vec::new(), &[]);
        let id = session.id;
        let own = session.new_own_project().expect("a session of a user");
        assert_eq!(own.name, Project::own_name(id));
        assert_eq!(own.owner(), Some(alice));
        assert!(
            own.security_attributes.acl.contains(&AclEntry {
                subject: Subject::Group(GroupId::of_session(id)),
                grant: Grant::Read
            }),
            "read by its participants"
        );
        session.security_attributes.owner_id = ActorId::Agent(AgentId::new());
        assert!(session.new_own_project().is_none(), "a session of no user");
    }

    /// Issue 467: the condition of the sessions of a project meets those
    /// whose results go to it, as it is changed.
    #[test]
    fn the_sessions_in_a_project_are_those_whose_results_go_to_it() {
        let mut session = session_of(UserId::new(), Vec::new(), &[]);
        let shared = ProjectId::new();
        assert!(!Filter::in_project(shared).matches(&serde_json::to_value(&session).unwrap()));
        session.apply(&SessionUpdateEvent::ProjectChanged(shared)).unwrap();
        assert_eq!(session.project, shared);
        assert!(Filter::in_project(shared).matches(&serde_json::to_value(&session).unwrap()));
    }

    #[test]
    fn a_change_is_made_into_the_session_s_values_or_leaves_it_as_it_was() {
        let alice = UserId::new();
        let mut session = session_of(alice, vec![Participant::User(alice)], &[]);
        session.apply(&SessionUpdateEvent::NameChanged("planning".to_string())).unwrap();
        session
            .apply(&SessionUpdateEvent::DescriptionChanged {
                summary: Some("What we plan".to_string()),
                briefing: None,
            })
            .unwrap();
        session.apply(&SessionUpdateEvent::ParticipantJoined(Participant::User(alice))).unwrap();
        assert_eq!(
            (session.name.as_str(), session.summary.as_ref().map(|s| s.as_str())),
            ("planning", Some("What we plan"))
        );
        assert_eq!(session.participants.len(), 1, "taken in as `take_in` does: once");
        session.apply(&SessionUpdateEvent::LanguageChanged(Some("de".to_string()))).unwrap();
        assert_eq!(
            session.language.as_ref().map(Language::as_str),
            Some("de"),
            "made into a language"
        );
        assert!(
            session
                .apply(&SessionUpdateEvent::LanguageChanged(Some("no language".to_string())))
                .is_err(),
            "no language tag"
        );
        assert_eq!(session.language.as_ref().map(Language::as_str), Some("de"), "unchanged");
        session.apply(&SessionUpdateEvent::LanguageChanged(None)).unwrap();
        assert_eq!(session.language, None, "taken back");

        assert_eq!(session.effective_time_zone(), TimeZone::UTC, "UTC where it names none");
        session
            .apply(&SessionUpdateEvent::TimeZoneChanged(Some("Europe/Berlin".to_string())))
            .unwrap();
        assert_eq!(session.effective_time_zone().as_str(), "Europe/Berlin");
        assert!(
            session
                .apply(&SessionUpdateEvent::TimeZoneChanged(Some("Europe/Atlantis".to_string())))
                .is_err(),
            "no time zone"
        );
        assert_eq!(session.effective_time_zone().as_str(), "Europe/Berlin", "unchanged");

        assert!(session.apply(&SessionUpdateEvent::NameChanged(String::new())).is_err(), "no name");
        assert_eq!(session.name.as_str(), "planning", "unchanged");
    }
}
