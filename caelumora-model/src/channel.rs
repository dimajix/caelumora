use crate::actor::ActorId;
use crate::agent::AgentId;
use crate::approval::{ApprovalStrategy, Approver, ApproverId};
use crate::chain::ChainChange;
use crate::component::{ComponentConfig, ComponentDescriptor};
use crate::confidentiality::Classification;
use crate::message::MessageId;
use crate::message_history::{HistoryEntry, MessageHistory};
use crate::name::Name;
use crate::security::{Secured, SecurityAttributes};
use crate::session::SessionId;
use crate::store::Journaled;
use crate::user::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelUpdateEvent {
    NameChanged(Name),
    /// Reclassified (issue 133).
    ClassificationChanged(Classification),
}

define_id!(ChannelId);

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
    /// Where the view of the history begins for participants who joined
    /// later and may not see all of it (issue 46); everybody else sees all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history_limits: Vec<HistoryLimit>,
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

impl Secured for Channel {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Channel {
    /// A new channel of `participants`, owned by `owner`, as every channel is
    /// built - from a template or on request (issue 09): with an approver
    /// added if there is none, and with every user and agent in it as a
    /// contributor (see `participant_attributes`).
    pub fn build(session_id: SessionId, owner: ActorId, name: Name, components: ComponentConfig, classification: Classification, mut participants: Vec<Participant>) -> Self {
        if !participants.iter().any(|p| matches!(p, Participant::Approver(_))) {
            participants.push(Participant::Approver(Approver {
                id: ApproverId(Uuid::new_v4()),
                strategy: ApprovalStrategy::Any,
            }));
        }
        let security_attributes = participant_attributes(owner, &participants);
        Self {
            id: ChannelId::new(),
            session_id,
            name,
            components,
            participants,
            security_attributes,
            classification,
            history_limits: Vec::new(),
        }
    }
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
    History(ChainChange<HistoryEntry>),
}

impl Journaled for ChannelState {
    type Change = ChannelStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        self.history.changes().into_iter().map(ChannelStateChange::History).collect()
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

/// The attributes of a session or channel owned by `owner` with these
/// participants: its users and agents contribute - they read it and act in
/// it - while only the owner may change or delete it (issue 11). Approvers
/// act on behalf of users and need no access of their own.
pub fn participant_attributes(owner: ActorId, participants: &[Participant]) -> SecurityAttributes {
    let mut contributors: Vec<ActorId> = Vec::new();
    for participant in participants {
        let id = participant.actor_id();
        if matches!(participant, Participant::User(_) | Participant::Agent(_)) && id != owner && !contributors.contains(&id) {
            contributors.push(id);
        }
    }
    SecurityAttributes { owner_id: owner, contributors }
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
        assert_eq!(state.changes().len(), 2, "appended and amended");
        replayed(&state, &mut copy);
        assert_eq!(copy.channel_id, state.channel_id);
    }
}
