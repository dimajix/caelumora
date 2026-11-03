//! A channel of a session (`Channel`): its name, language and classification, who takes part in it
//! and what of its history they read, how it is stored and asked for (`ChannelField`), and the
//! events of its changes (`ChannelUpdateEvent`). How much of its history a participant is given is
//! `history_access`'s, who takes part `participant`'s, and what it holds while it runs `state`'s.

mod history_access;
mod participant;
mod state;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use history_access::{HistoryAccess, HistoryStart};
pub use participant::{FormerParticipant, InternalActor, Participant};
pub use state::{ChannelState, ChannelStateChange};

use crate::actor::ActorId;
use crate::approval::ApprovalRule;
use crate::component::{ComponentConfig, ComponentDescriptor};
use crate::confidentiality::Classification;
use crate::group::GroupId;
use crate::query::Field;
use crate::removal::Removal;
use crate::scope::{Scope, Scoped};
use crate::security::{Grant, Secured, SecuredMut, SecurityAttributes, Subject};
use crate::session::{Session, SessionId};
use crate::store::{EntityKind, Revision, Stored, Updatable};
use crate::value::language::Language;
use crate::value::name::{Name, ScopedName};
use crate::value::text::{Briefing, Summary};
use crate::value::ValueError;
use serde::{Deserialize, Serialize};
use std::ops::Not;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelUpdateEvent {
    /// Renamed, as it was asked: the name is made from it as the change is
    /// applied (issue 248), and so is a language.
    NameChanged(String),
    /// Reclassified (issue 133).
    ClassificationChanged(Classification),
    /// Its conversation is to be held in another language, or with `None`
    /// in its session's (issues 56, 425).
    LanguageChanged(Option<String>),
    /// Its rules on tool calls changed (issue 13), as they now stand.
    ApprovalRulesChanged(Vec<ApprovalRule>),
    /// What it is for changed (issue 245), as it now stands.
    DescriptionChanged { summary: Option<String>, briefing: Option<String> },
    /// Whether it uses the workspace of its session (issue 251).
    UsesWorkspaceChanged(bool),
}

define_id!(ChannelId);

/// A channel was started: its actors run (`EntityChange::Custom`, issue
/// 335).
pub const STARTED: &str = "started";

/// A channel was stopped: its actors ended.
pub const STOPPED: &str = "stopped";

/// Someone joined the channel - an agent or a user, as the `ActorId` in
/// the change's data says (issue 46).
pub const JOINED: &str = "joined";

/// Someone left the channel, as the `ActorId` in the change's data says.
pub const LEFT: &str = "left";

/// The state of an agent in the channel changed - its notes or private
/// notes (issue 65), the `AgentId` in the change's data. Published with the
/// channel's attributes once it is stored, before the agent announces the
/// end of its turn; without content, since part of it is private.
pub const AGENT_STATE_CHANGED: &str = "agent_state_changed";

/// Something new was said in the channel (issue 404): a message that others
/// hear (`Message::is_heard`), in the change's data as it was said.
/// Published with the channel's attributes and classification, so that it
/// reaches who may read the channel and is cleared for it - who would read
/// the message in its history -, once the message is stored.
pub const SAID: &str = "said";

/// Whether a channel uses its session's workspace unless it says
/// otherwise: it does (issue 251, decision 11).
pub(crate) fn uses_workspace_by_default() -> bool {
    true
}

/// The components of a channel whose moderator and the policy for the
/// moderator's slot are named by their kinds, as a request names them
/// (issue 174). Each one left out is its default.
pub fn moderation_components(
    moderator: Option<&str>,
    responder_policy: Option<&str>,
) -> ComponentConfig {
    let component_nodes = [("moderator", moderator), ("responder_policy", responder_policy)]
        .into_iter()
        .filter_map(|(id, kind)| kind.map(|kind| ComponentDescriptor::new(id, kind)))
        .collect();
    ComponentConfig { component_nodes }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub session_id: SessionId,
    /// What it is found by in its session (issue 177).
    pub name: Name,
    /// What it is for, in a line (issue 245); `None` if nobody said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    /// What an agent should know before it works here (issue 245), given to
    /// the agents whose personality wires in the
    /// component `channel_briefing`; `None` if nobody wrote one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing: Option<Briefing>,
    /// The channel's components (issue 174): its moderator and the policy
    /// that fills the moderator's slot. A facet nobody here provides is
    /// built from its default.
    #[serde(default)]
    pub components: ComponentConfig,
    pub participants: Vec<Participant>,
    /// The actors that belong to the channel itself (issue 420): its
    /// approver, if it is given one.
    pub internal_actors: Vec<InternalActor>,
    pub security_attributes: SecurityAttributes,
    /// The classification of the channel's contents (issue 133): what may
    /// flow into it, and who may read it. Set by its owner when creating it.
    pub classification: Classification,
    /// The language its conversation is held in (issue 56): its agents are
    /// told to write in it. `None` if it holds none of its own: then it is
    /// held in its session's, if that has one (issue 425,
    /// `effective_language`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    /// Who took part and left (issue 164): removed participants, with who
    /// removed them and when. Not among `participants`, which is who takes
    /// part now; one added again is taken out of here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former: Vec<FormerParticipant>,
    /// What the channel restricts of the tool calls made in it (issue 13):
    /// rules that ask or deny, set by whoever may change the channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_rules: Vec<ApprovalRule>,
    /// Whether its participants work on the workspace of its session here (issue
    /// 251, decision 11): what it is for, set by whoever may change it. It
    /// permits nothing the classifications refuse - the workspace is used
    /// here only where it and the channel flow both ways.
    #[serde(default = "uses_workspace_by_default")]
    pub uses_workspace: bool,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
    /// Its revision (issue 492), counted up by every write of it.
    #[serde(default)]
    pub revision: Revision,
    /// Whether its purge has begun (issue 492): set by the store alone.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub purging: bool,
}

impl Stored for Channel {
    const KIND: EntityKind = EntityKind::new("channel");
    type Id = ChannelId;
    type Within = Session;
    type Field = ChannelField;

    fn id(&self) -> ChannelId {
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

    fn revision(&self) -> Revision {
        self.revision
    }

    fn set_revision(&mut self, revision: Revision) {
        self.revision = revision;
    }

    fn purging(&self) -> bool {
        self.purging
    }

    fn set_purging(&mut self) {
        self.purging = true;
    }

    fn classification(&self) -> Option<&Classification> {
        Some(&self.classification)
    }
}

/// What a change of the channel says is made into its values here (issue
/// 248).
impl Updatable for Channel {
    type Update = ChannelUpdateEvent;

    fn apply(&mut self, event: &ChannelUpdateEvent) -> Result<(), ValueError> {
        match event {
            ChannelUpdateEvent::NameChanged(name) => self.name = Name::new(name)?,
            ChannelUpdateEvent::ClassificationChanged(classification) => {
                self.classification = classification.clone()
            }
            ChannelUpdateEvent::LanguageChanged(language) => {
                self.language = Language::optional(language.as_deref())?
            }
            ChannelUpdateEvent::ApprovalRulesChanged(rules) => self.approval_rules = rules.clone(),
            ChannelUpdateEvent::DescriptionChanged { summary, briefing } => {
                self.describe(summary.as_deref(), briefing.as_deref())?
            }
            ChannelUpdateEvent::UsesWorkspaceChanged(uses) => self.uses_workspace = *uses,
        }
        Ok(())
    }
}

/// A channel's name is unique in its session (issue 250).
impl Scoped for Channel {
    fn scoped_name(&self) -> ScopedName {
        ScopedName {
            name: self.name.clone(),
            scope: Scope::SessionScope { session_id: self.session_id },
        }
    }
}

/// What a query of channels may name (issue 293).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelField {
    /// The session it belongs to.
    Session,
}

impl Field for ChannelField {
    fn path(self) -> &'static [&'static str] {
        match self {
            ChannelField::Session => &["session_id"],
        }
    }
}

impl Secured for Channel {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl SecuredMut for Channel {
    fn security_attributes_mut(&mut self) -> &mut SecurityAttributes {
        &mut self.security_attributes
    }
}

impl Channel {
    /// The attributes of a new channel `id` of `owner`: read by its participants, whoever they are
    /// at the time (issue 23) - the group of its participants, stored with it
    /// (`Group::of_channel`).
    pub fn attributes_of(id: ChannelId, owner: ActorId) -> SecurityAttributes {
        SecurityAttributes::owned_by(owner)
            .granting(Subject::Group(GroupId::of_channel(id)), Grant::Read)
    }

    /// Whether `actor` is among the channel's participants - resting in it included.
    pub fn lists(&self, actor: ActorId) -> bool {
        self.participants.iter().any(|p| p.actor_id() == actor)
    }

    /// A new channel, owned by `owner`, as every channel is built - from a
    /// template or on request (issue 09) -, read by its participants (issue
    /// 23): every user and agent in it, whoever they are at the time - the
    /// group of its participants (issue 304), which whoever stores the
    /// channel stores with it (`Group::of_channel`). Held in `language`, if
    /// any. Built empty (issue 420): its founders join by the one way in,
    /// as anyone does, once it is stored; and it has no internal actors -
    /// whoever builds it gives it those that are configured, and no others,
    /// so that without an approver nothing in it is approved.
    pub fn build(
        session_id: SessionId,
        owner: ActorId,
        name: Name,
        components: ComponentConfig,
        classification: Classification,
        language: Option<Language>,
    ) -> Self {
        let id = ChannelId::new();
        let security_attributes = Self::attributes_of(id, owner);
        Self {
            id,
            session_id,
            name,
            summary: None,
            briefing: None,
            components,
            participants: Vec::new(),
            internal_actors: Vec::new(),
            security_attributes,
            classification,
            language,
            former: Vec::new(),
            approval_rules: Vec::new(),
            uses_workspace: uses_workspace_by_default(),
            deleted: None,
            revision: Default::default(),
            purging: false,
        }
    }

    /// The language the channel's conversation is held in (issue 425): its
    /// own, else that of `session`, the session it belongs to - which
    /// follows the session's as it changes. `None` if neither has one.
    pub fn effective_language<'a>(&'a self, session: &'a Session) -> Option<&'a Language> {
        self.language.as_ref().or(session.language.as_ref())
    }

    /// Whether `actor` is one of the channel's internal actors (issue 420),
    /// which take no part in it.
    pub fn is_internal(&self, actor: ActorId) -> bool {
        self.internal_actors.iter().any(|internal| internal.actor_id() == actor)
    }

    /// Says what the channel is for (issue 245): `summary` and `briefing`
    /// replace what it said - a missing or empty one removes it.
    ///
    /// # Errors
    /// `ValueError` for the first that does not fit; the channel is then
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::testing::make_session;
    use crate::user::UserId;

    #[test]
    fn a_change_is_made_into_the_channel_s_values_or_leaves_it_as_it_was() {
        let owner = ActorId::User(UserId::new());
        let mut channel = Channel::build(
            SessionId::new(),
            owner,
            Name::new("main").unwrap(),
            ComponentConfig::default(),
            Classification::CHANNEL_DEFAULT,
            None,
        );
        channel.apply(&ChannelUpdateEvent::NameChanged("planning".to_string())).unwrap();
        channel.apply(&ChannelUpdateEvent::LanguageChanged(Some("de".to_string()))).unwrap();
        channel.apply(&ChannelUpdateEvent::UsesWorkspaceChanged(false)).unwrap();
        assert_eq!(
            (
                channel.name.as_str(),
                channel.language.as_ref().map(|l| l.as_str()),
                channel.uses_workspace
            ),
            ("planning", Some("de"), false)
        );

        assert!(
            channel
                .apply(&ChannelUpdateEvent::LanguageChanged(Some("no language".to_string())))
                .is_err(),
            "no language tag"
        );
        assert_eq!(channel.language.as_ref().map(|l| l.as_str()), Some("de"), "unchanged");
    }

    #[test]
    fn a_channel_without_a_language_of_its_own_is_held_in_its_sessions() {
        let owner = UserId::new();
        let mut session = make_session("s", owner);
        let tag = |tag: &str| Some(Language::new(tag).unwrap());
        let mut channel = Channel::build(
            session.id,
            ActorId::User(owner),
            Name::new("main").unwrap(),
            ComponentConfig::default(),
            Classification::CHANNEL_DEFAULT,
            None,
        );
        assert_eq!(channel.effective_language(&session), None, "neither has one");
        session.language = tag("de");
        assert_eq!(channel.effective_language(&session), tag("de").as_ref(), "the session's");
        channel.language = tag("fr");
        assert_eq!(
            channel.effective_language(&session),
            tag("fr").as_ref(),
            "its own before the session's"
        );
    }
}
