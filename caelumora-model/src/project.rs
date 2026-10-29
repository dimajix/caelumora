//! A project (issue 467): the bracket above a session that holds what
//! outlasts one piece of work - its results - and the name space they are
//! found in.
//!
//! A project holds a scope of its own (`Scope::ProjectScope`): what lies in
//! it belongs to the project. It lies in a scope itself, where it is found
//! by name: a shared project in the global scope or its owner's, a
//! session's own project in that session's scope, named `own` - a name no
//! other project has. A session's own becomes a shared one by being lifted
//! into another scope, under another name. Its members are users who work on
//! it; their group (`GroupId::of_project`) is what its access list names.

use crate::actor::ActorId;
use crate::group::GroupId;
use crate::removal::Removal;
use crate::scope::{Scope, ScopeField, Scoped};
use crate::security::{Grant, Secured, SecurityAttributes, Subject};
use crate::session::SessionId;
use crate::store::query::{FieldValue, Filter};
use crate::store::{EntityKind, NoEntity, Stored, Updatable};
use crate::user::UserId;
use crate::value::name::{Name, ScopedName};
use crate::value::text::Description;
use crate::value::ValueError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

define_id!(ProjectId);

/// A change of a project, as it was asked: what it says is made into the
/// project's values as it is applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProjectUpdateEvent {
    /// Renamed where it lies.
    NameChanged(String),
    /// What it is for, as its owner describes it, as it now stands; `None`
    /// takes the description away.
    DescriptionChanged(Option<String>),
    /// Who works on it, as it now stands; the group of its members follows.
    /// Told to who reads the project, who may read its members (unlike a
    /// team's, whose members only its readers see).
    MembersChanged(Vec<UserId>),
    /// Its access list changed. Without the list: only its owner sees it,
    /// while everybody who may read the project hears of this.
    AccessChanged,
    /// Lifted into `scope` under `name`: a session's own made a shared one.
    Lifted { name: String, scope: Scope },
}

/// A project: its name where it lies, what it is for, who works on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    /// What it is found by and where it lies: the global scope or its
    /// owner's for a shared project, a session's for the session's own -
    /// named `OWN` there. Its name is unique where it lies.
    #[serde(flatten)]
    pub name: ScopedName,
    /// What it is for, as its owner describes it; `None` if nobody said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    /// The users who work on it; none for a session's own, whose results
    /// are read by who reads the session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<UserId>,
    /// Owned by the user it was made for; its list names the group of its
    /// members for reading - and, for a session's own, the session's
    /// participants -, and whom its owner shares it with.
    pub security_attributes: SecurityAttributes,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Set when it was deleted: it stays stored, hidden from every read but
    /// those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}

impl Project {
    /// The name of every session's own project, in the session's scope.
    pub const OWN: &'static str = "own";

    /// A new shared project of `owner`, of `name` where it lies, without
    /// members or a description yet; read by the group of its members.
    pub fn new(name: ScopedName, owner: UserId) -> Self {
        Self::of(name, owner, &[])
    }

    /// The own project of session `session_id` of `owner`: named `OWN` in
    /// the session's scope, read by the session's participants - who read
    /// the session read its results - and by the group of its members,
    /// none until it is lifted.
    pub fn own_of(session_id: SessionId, owner: UserId) -> Self {
        Self::of(Self::own_name(session_id), owner, &[Subject::Group(GroupId::of_session(session_id))])
    }

    /// What the own project of session `session_id` is found by: `OWN`, in
    /// the session's scope.
    pub fn own_name(session_id: SessionId) -> ScopedName {
        ScopedName { name: Name::fixed(Self::OWN), scope: Scope::SessionScope { session_id } }
    }

    /// A new project of `owner`, of `name`, read by the group of its members
    /// and by `readers`.
    fn of(name: ScopedName, owner: UserId, readers: &[Subject]) -> Self {
        let (id, now) = (ProjectId::new(), Utc::now());
        let members = SecurityAttributes::owned_by(ActorId::User(owner)).granting(Subject::Group(GroupId::of_project(id)), Grant::Read);
        let security_attributes = readers.iter().fold(members, |attributes, reader| attributes.granting(*reader, Grant::Read));
        Self { id, name, description: None, members: Vec::new(), security_attributes, created_at: now, updated_at: now, deleted: None }
    }

    /// The scope it holds: what lies in it belongs to the project.
    pub fn holds(&self) -> Scope {
        Scope::ProjectScope { project_id: self.id }
    }

    /// Whether it is a session's own: it lies in the session's scope.
    pub fn is_own(&self) -> bool {
        matches!(self.name.scope, Scope::SessionScope { .. })
    }
}

impl Stored for Project {
    const KIND: EntityKind = EntityKind::new("project");
    type Id = ProjectId;
    type Within = NoEntity;
    type Field = ScopeField;

    fn id(&self) -> ProjectId {
        self.id
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }

    /// A project is a user's and lies in the global scope or its owner's -
    /// or, as a session's own, in that session's scope, named `OWN` and
    /// without members; never in a channel's, a group's - not yet a place
    /// anything is found from - or another project's. `OWN` is the name of a
    /// session's own alone, so that a lifted one is named anew.
    fn validate(&self) -> Result<(), String> {
        let Some(owner) = self.owner() else { return Err("a project belongs to a user".to_string()) };
        let own = self.name.name == Name::fixed(Self::OWN);
        match &self.name.scope {
            Scope::GlobalScope | Scope::UserScope { .. } if own => Err(format!("'{}' is the name of a session's own project", Self::OWN)),
            Scope::GlobalScope => Ok(()),
            Scope::UserScope { user_id } if *user_id == owner => Ok(()),
            Scope::UserScope { .. } => Err("a project lies in no other user's scope than its owner's".to_string()),
            Scope::SessionScope { .. } if !own => Err(format!("a project in a session's scope is its own, named '{}'", Self::OWN)),
            Scope::SessionScope { .. } if !self.members.is_empty() => Err("a session's own project has no members: who reads the session reads it".to_string()),
            Scope::SessionScope { .. } => Ok(()),
            Scope::ChannelScope { .. } | Scope::GroupScope { .. } | Scope::ProjectScope { .. } => Err("a project lies in the global scope, its owner's or a session's as its own".to_string()),
        }
    }
}

/// The conditions a list of projects is asked by.
impl Filter<ScopeField> {
    /// The shared projects: those that lie where a shared one lies - the global scope or a
    /// user's -, not a session's own (`Project::is_own`).
    pub fn shared_projects() -> Self {
        Filter::is_in(ScopeField::Scope, [FieldValue::variant("GlobalScope"), FieldValue::variant("UserScope")])
    }
}

/// What a change of the project says is made into its values here. Whether
/// it may lie where it is lifted is the store's to say as it is stored
/// (`Stored::validate`); a new access list is never announced: whoever takes
/// the change over reads it as stored.
impl Updatable for Project {
    type Update = ProjectUpdateEvent;

    fn apply(&mut self, event: &ProjectUpdateEvent) -> Result<(), ValueError> {
        match event {
            ProjectUpdateEvent::NameChanged(name) => self.name = ScopedName::new(name, self.name.scope.clone())?,
            ProjectUpdateEvent::DescriptionChanged(description) => self.description = Description::optional(description.as_deref())?,
            ProjectUpdateEvent::MembersChanged(members) => self.members.clone_from(members),
            ProjectUpdateEvent::AccessChanged => {}
            ProjectUpdateEvent::Lifted { name, scope } => self.name = ScopedName::new(name, scope.clone())?,
        }
        Ok(())
    }

    fn touch(&mut self, at: DateTime<Utc>) {
        self.updated_at = at;
    }
}

impl Secured for Project {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Scoped for Project {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::AclEntry;
    use crate::session::ChannelId;

    fn shared(scope: Scope, owner: UserId) -> Project {
        Project::new(ScopedName::new("report", scope).unwrap(), owner)
    }

    /// The condition of the shared projects meets exactly those that are no session's own.
    #[test]
    fn the_shared_projects_are_those_no_session_owns() {
        let owner = UserId::new();
        let projects = [
            Project::new(ScopedName::new("report", Scope::GlobalScope).unwrap(), owner),
            Project::new(ScopedName::new("report", Scope::UserScope { user_id: owner }).unwrap(), owner),
            Project::own_of(SessionId::new(), owner),
        ];
        for project in projects {
            assert_eq!(Filter::shared_projects().matches(&serde_json::to_value(&project).unwrap()), !project.is_own(), "{:?}", project.name.scope);
        }
    }

    #[test]
    fn a_project_lies_in_the_global_scope_its_owners_or_a_session_s_as_its_own() {
        let alice = UserId::new();
        for scope in [Scope::GlobalScope, Scope::UserScope { user_id: alice }] {
            assert!(shared(scope.clone(), alice).validate().is_ok(), "{scope:?}");
        }
        assert!(Project::own_of(SessionId::new(), alice).validate().is_ok(), "a session's own");
        let session_id = SessionId::new();
        for scope in [
            Scope::UserScope { user_id: UserId::new() },
            Scope::SessionScope { session_id },
            Scope::ChannelScope { session_id, channel_id: ChannelId::new() },
            Scope::GroupScope { group_id: GroupId::new() },
            Scope::ProjectScope { project_id: ProjectId::new() },
        ] {
            assert!(shared(scope.clone(), alice).validate().is_err(), "not in {scope:?}");
        }
        let named_own = Project::new(ScopedName::new(Project::OWN, Scope::UserScope { user_id: alice }).unwrap(), alice);
        assert!(named_own.validate().is_err(), "'own' is a session's own's name alone");
        let mut nobodys = shared(Scope::GlobalScope, alice);
        nobodys.security_attributes.owner_id = ActorId::System;
        assert!(nobodys.validate().is_err(), "a user's");
    }

    #[test]
    fn a_session_s_own_project_is_its_alone_and_read_by_its_participants() {
        let (session_id, alice) = (SessionId::new(), UserId::new());
        let own = Project::own_of(session_id, alice);
        assert!(own.is_own() && !shared(Scope::GlobalScope, alice).is_own());
        assert_eq!((own.name.name.as_str(), &own.name.scope, own.owner()), (Project::OWN, &Scope::SessionScope { session_id }, Some(alice)));
        assert_eq!(own.security_attributes.acl, [
            AclEntry { subject: Subject::Group(GroupId::of_project(own.id)), grant: Grant::Read },
            AclEntry { subject: Subject::Group(GroupId::of_session(session_id)), grant: Grant::Read },
        ], "read by the session's participants, and by its members once it is lifted");
        let mut with_members = own.clone();
        with_members.members = vec![UserId::new()];
        assert!(with_members.validate().is_err(), "who reads the session reads it");
    }

    #[test]
    fn a_shared_project_is_read_by_the_group_of_its_members() {
        let project = shared(Scope::GlobalScope, UserId::new());
        assert_eq!(project.security_attributes.acl, [AclEntry { subject: Subject::Group(GroupId::of_project(project.id)), grant: Grant::Read }]);
    }

    #[test]
    fn a_change_is_made_into_the_project_where_it_lies() {
        let (session_id, alice, bob) = (SessionId::new(), UserId::new(), UserId::new());
        let mut project = Project::own_of(session_id, alice);
        project.apply(&ProjectUpdateEvent::DescriptionChanged(Some("The figures of Q3".to_string()))).unwrap();
        assert_eq!(project.description.as_ref().map(ToString::to_string).as_deref(), Some("The figures of Q3"));
        assert!(project.apply(&ProjectUpdateEvent::NameChanged("report".to_string())).is_ok());
        assert_eq!(project.name.scope, Scope::SessionScope { session_id }, "renamed where it lies");
        assert!(project.validate().is_err(), "a session's own keeps its name");

        let lifted = ProjectUpdateEvent::Lifted { name: "report".to_string(), scope: Scope::UserScope { user_id: alice } };
        project.apply(&lifted).unwrap();
        project.apply(&ProjectUpdateEvent::MembersChanged(vec![bob])).unwrap();
        assert!(!project.is_own() && project.validate().is_ok(), "lifted, a shared one with members");
        assert!(project.apply(&ProjectUpdateEvent::Lifted { name: " ".to_string(), scope: Scope::GlobalScope }).is_err(), "under a name");
        let mut kept_its_name = Project::own_of(session_id, alice);
        kept_its_name.apply(&ProjectUpdateEvent::Lifted { name: Project::OWN.to_string(), scope: Scope::GlobalScope }).unwrap();
        assert!(kept_its_name.validate().is_err(), "named anew as it is lifted");
    }

    #[test]
    fn a_project_is_written_without_what_it_does_not_have() {
        let project = shared(Scope::GlobalScope, UserId::new());
        let written = serde_json::to_value(&project).unwrap();
        for absent in ["description", "members", "deleted"] {
            assert!(written.get(absent).is_none(), "{absent}: {written}");
        }
        let read: Project = serde_json::from_value(written).unwrap();
        assert_eq!(read.name, project.name);
    }
}
