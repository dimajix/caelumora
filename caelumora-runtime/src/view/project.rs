//! A project as a viewer reached it (issue 467): one they own, work on or are shared, or the one
//! the results of a session they read go to. It is read alone: whoever changes it asks again
//! (`ProjectController`).

use super::{SessionView, ViewResult, Views};
use caelumora_model::actor::ActorId;
use caelumora_model::project::{Project, ProjectId};
use caelumora_model::security::AccessQuestions;
use caelumora_model::snapshot::ProjectSnapshot;
use caelumora_model::store::query::{Filter, Query};

#[derive(Clone)]
pub struct ProjectView {
    views: Views,
    project: Project,
}

impl ProjectView {
    pub(super) fn of(views: Views, project: Project) -> Self {
        Self { views, project }
    }

    pub fn id(&self) -> ProjectId {
        self.project.id
    }

    /// The views it was reached through, which an extension of a plugin reads from (issue 328).
    pub fn views(&self) -> &Views {
        &self.views
    }

    /// The sessions that lie in it - whose results go to it - that the viewer may read, in the
    /// order the store lists them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn sessions(&self) -> ViewResult<Vec<SessionView>> {
        self.views.sessions_of(&Query::filtered(Filter::in_project(self.project.id))).await
    }

    /// Whether the viewer may change it (`UpdateProject`): its owner.
    pub async fn may_change(&self) -> bool {
        self.views
            .access_policy()
            .may_change_project(self.views.context(), &self.project.security_attributes)
            .await
    }

    /// The project as the viewer sees it; its members called by their names, read in the
    /// runtime's name - who works on a project one reads is no secret -, its access list for
    /// whoever may change it.
    pub async fn snapshot(&self) -> ProjectSnapshot {
        let mut members = Vec::with_capacity(self.project.members.len());
        for member in &self.project.members {
            members.push((*member, self.views.name_of(ActorId::User(*member)).await));
        }
        let can_change = self.may_change().await;
        let access = match can_change {
            true => self.views.access_snapshots(&self.project.security_attributes.acl).await,
            false => Vec::new(),
        };
        ProjectSnapshot {
            id: self.project.id,
            name: self.project.name.to_string(),
            scope: self.views.scope_snapshot(&self.project.name.scope).await,
            description: self.project.description.clone(),
            members,
            can_change,
            access,
        }
    }
}
