use crate::actor::ActorId;
use crate::agent::AgentId;
use crate::approval::{ApprovalRule, ApprovalStrategy, Approver, ApproverId};
use crate::chain::Stored;
use crate::component::{ComponentConfig, ComponentDescriptor};
use crate::confidentiality::Classification;
use crate::value::language::Language;
use crate::message::MessageId;
use crate::message_history::{HistoryEntry, MessageHistory};
use crate::scope::{Scope, Scoped};
use crate::value::name::{Name, ScopedName};
use crate::value::text::{Briefing, Summary};
use crate::value::ValueError;
use crate::security::{Grant, Membership, Secured, SecurityAttributes, Subject};
use crate::session::SessionId;
use crate::store::Journaled;
use crate::store::query::Field;
use crate::user::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelUpdateEvent {
    /// Renamed, as it was asked: the name is made from it as the change is
    /// applied (issue 248), and so is a language.
    NameChanged(String),
    /// Reclassified (issue 133).
    ClassificationChanged(Classification),
    /// Its conversation is to be held in another language, or in none in
    /// particular (issue 56).
    LanguageChanged(Option<String>),
    /// Its rules on tool calls changed (issue 13), as they now stand.
    ApprovalRulesChanged(Vec<ApprovalRule>),
    /// What it is for changed (issue 245), as it now stands.
    DescriptionChanged { summary: Option<String>, briefing: Option<String> },
    /// Whether it uses the workspace of its session (issue 251).
    UsesWorkspaceChanged(bool),
}

define_id!(ChannelId);

/// Whether a channel uses its session's workspace unless it says
/// otherwise: it does (issue 251, decision 11).
pub(crate) fn uses_workspace_by_default() -> bool {
    true
}

/// The components of a channel whose moderator and the policy for the
/// moderator's slot are named by their kinds, as a request names them
/// (issue 174). Each one left out is its default.
pub fn moderation_components(moderator: Option<&str>, responder_policy: Option<&str>) -> ComponentConfig {
    let component_nodes = [("moderator", moderator), ("responder_policy", responder_policy)].into_iter()
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
    /// `channel_briefing_prompt_provider`; `None` if nobody wrote one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing: Option<Briefing>,
    /// The channel's components (issue 174): its moderator and the policy
    /// that fills the moderator's slot. A facet nobody here provides is
    /// built from its default.
    #[serde(default)]
    pub components: ComponentConfig,
    pub participants: Vec<Participant>,
    pub security_attributes: SecurityAttributes,
    /// The classification of the channel's contents (issue 133): what may
    /// flow into it, and who may read it. Set by its owner when creating it.
    pub classification: Classification,
    /// The language its conversation is held in (issue 56): its agents are
    /// told to write in it. `None` if it is held in none in particular.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    /// Where the view of the history begins for participants who joined
    /// later and may not see all of it (issue 46); everybody else sees all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history_limits: Vec<HistoryLimit>,
    /// Who took part and left (issue 164): removed participants, with who
    /// removed them and when. Not among `participants`, which is who takes
    /// part now; one added again is taken out of here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former: Vec<FormerParticipant>,
    /// What the channel restricts of the tool calls made in it (issue 13):
    /// rules that ask or deny, set by whoever may change the channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_rules: Vec<ApprovalRule>,
    /// Whether its members work on the workspace of its session here (issue
    /// 251, decision 11): what it is for, set by whoever may change it. It
    /// permits nothing the classifications refuse - the workspace is used
    /// here only where it and the channel flow both ways.
    #[serde(default = "uses_workspace_by_default")]
    pub uses_workspace: bool,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<crate::removal::Removal>,
}

/// How much of a channel's history a new participant may see, as asked for
/// when adding them (issue 46). Data only: the `ChannelLoop` turns it into a
/// `HistoryStart` and applies it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryAccess {
    #[default]
    All,
    None,
    LastDays { count: u32 },
    LastMessages { count: u32 },
}

/// Where a participant's view of a channel's history begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStart {
    /// From this message on.
    From(MessageId),
    /// After this message: what came before the participant joined is not
    /// visible.
    After(MessageId),
}

/// The view of the history granted to one participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryLimit {
    pub participant: ActorId,
    pub start: HistoryStart,
}

crate::impl_deletable!(Channel);

/// A channel's name is unique in its session (issue 250).
impl Scoped for Channel {
    fn scoped_name(&self) -> ScopedName {
        ScopedName { name: self.name.clone(), scope: Scope::SessionScope { session_id: self.session_id } }
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

impl Channel {
    /// A new channel of `participants`, owned by `owner`, as every channel is
    /// built - from a template or on request (issue 09): with an approver
    /// added if there is none, and read by its members (issue 23): every
    /// user and agent in it, whoever they are at the time. Held in
    /// `language`, if any - by default the language its creator prefers.
    pub fn build(session_id: SessionId, owner: ActorId, name: Name, components: ComponentConfig, classification: Classification, language: Option<Language>, mut participants: Vec<Participant>) -> Self {
        if !participants.iter().any(|p| matches!(p, Participant::Approver(_))) {
            participants.push(Participant::Approver(Approver {
                id: ApproverId(Uuid::new_v4()),
                strategy: ApprovalStrategy::Any,
            }));
        }
        let id = ChannelId::new();
        let security_attributes = SecurityAttributes::owned_by(owner).granting(Subject::MemberOf(Membership::Channel(id)), Grant::Read);
        Self {
            id,
            session_id,
            name,
            summary: None,
            briefing: None,
            components,
            participants,
            security_attributes,
            classification,
            language,
            history_limits: Vec::new(),
            former: Vec::new(),
            approval_rules: Vec::new(),
            uses_workspace: true,
            deleted: None,
        }
    }

    /// Says what the channel is for (issue 245): `summary` and `briefing`
    /// replace what it said - a missing or empty one removes it.
    ///
    /// # Errors
    /// `ValueError` for the first that does not fit; the channel is then
    /// unchanged.
    pub fn describe(&mut self, summary: Option<&str>, briefing: Option<&str>) -> Result<(), ValueError> {
        (self.summary, self.briefing) = (Summary::optional(summary)?, Briefing::optional(briefing)?);
        Ok(())
    }
}

/// A participant that left a channel (issue 164): removed, by whom and
/// when. Its history limit is not kept - one added again is given a new
/// one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormerParticipant {
    pub participant: Participant,
    pub left: crate::removal::Removal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelState {
    pub channel_id: ChannelId,
    pub history: MessageHistory,
}

impl ChannelState {
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            history: MessageHistory::default(),
        }
    }
}

/// A change to a [`ChannelState`] since it was last stored (issue 193). The
/// channel's id is what the state is stored under, not a part that changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelStateChange {
    History(Stored<HistoryEntry>),
}

impl Journaled for ChannelState {
    type Change = ChannelStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        self.history.changes().map(ChannelStateChange::History).into_iter().collect()
    }

    fn mark_stored(&mut self) {
        self.history.mark_stored();
    }

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            ChannelStateChange::History(change) => self.history.apply(change),
        }
    }
}

/// Who takes part in a session or channel. Users and agents by reference:
/// their records live in the `EntityStore` and are read from there, so
/// that no session keeps a copy that goes stale or shows more of them than
/// their name (issue 120). An approver has no record of its own; it is
/// kept here whole.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Participant {
    User(UserId),
    Agent(AgentId),
    Approver(Approver),
}

impl Participant {
    pub fn actor_id(&self) -> ActorId {
        match self {
            Participant::User(id) => ActorId::User(*id),
            Participant::Agent(id) => ActorId::Agent(*id),
            Participant::Approver(approver) => ActorId::Approver(approver.id),
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::{replayed, stored};
    use crate::confidentiality::Confidentiality;
    use crate::message::tests::{id, make_message};

    #[test]
    fn what_changed_in_a_channels_state_replays_onto_what_was_stored() {
        let mut state = ChannelState::new(ChannelId::new());
        state.history.extend(vec![make_message(id(1))]);
        let mut copy = stored(&mut state);
        assert!(state.changes().is_empty());

        state.history.extend(vec![make_message(id(2))]);
        state.history.reclassify(id(1), Classification::general(Confidentiality::Public));
        assert_eq!(state.changes().len(), 1, "what the history records, as one");
        replayed(&state, &mut copy);
        assert_eq!(copy.channel_id, state.channel_id);
    }
}
