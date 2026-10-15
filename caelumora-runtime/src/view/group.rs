//! A group as a viewer reaches it (issue 286): a team its owner keeps or
//! someone belongs to, or the participants of a session or channel they
//! read. A node decides nothing: whoever changes it asks again.

use super::Views;
use caelumora_model::actor::ActorId;
use caelumora_model::group::{Group, GroupId};
use caelumora_model::scope::Scope;
use caelumora_model::security::{AccessQuestions, SecurityContext};
use caelumora_model::snapshot::GroupSnapshot;
use caelumora_model::store::StoreResultExt;

#[derive(Clone)]
pub struct GroupView {
    views: Views,
    group: Group,
}

impl GroupView {
    pub(super) fn of(views: Views, group: Group) -> Self {
        Self { views, group }
    }

    pub fn id(&self) -> GroupId {
        self.group.id
    }

    /// Where it lies: a team in the global scope or its owner's, the
    /// participants of a session or channel in its scope.
    pub fn scope(&self) -> &Scope {
        &self.group.name.scope
    }

    /// Whether the viewer may rename it, change its members and delete it
    /// (`UpdateGroup`): the owner of a team - never the participants of a
    /// session or channel, which change by taking part.
    pub async fn may_change(&self) -> bool {
        !self.group.is_participants()
            && self.views.access_policy().may_change_group(self.views.context(), &self.group.security_attributes).await
    }

    /// The group as the viewer sees it; its members called by their names,
    /// read in the runtime's name - who belongs to a group one reads is no
    /// secret.
    pub async fn snapshot(&self) -> GroupSnapshot {
        let system = SecurityContext::system();
        let mut members = Vec::with_capacity(self.group.members.len());
        for member in &self.group.members {
            let name = match member {
                ActorId::User(user) => self.views.stores().users.get(&system, *user).await.optional().ok().flatten().map(|u| u.username.to_string()),
                ActorId::Agent(agent) => self.views.stores().agents.get(&system, *agent).await.optional().ok().flatten().map(|a| a.name.to_string()),
                _ => None,
            };
            members.push((*member, name));
        }
        GroupSnapshot {
            id: self.group.id,
            name: self.group.name.name.to_string(),
            scope: self.group.name.scope.clone(),
            owner: self.group.security_attributes.owner_id,
            members,
            can_change: self.may_change().await,
        }
    }
}
