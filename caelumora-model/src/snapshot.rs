//! Sessions and channels as one viewer sees them at one moment (issue 80):
//! what a client lists and offers, taken by the runtime's views
//! (`caelumora_runtime::view`, issue 163) for running and stopped sessions
//! alike, and only of what the viewer may read.
//!
//! A snapshot answers the access questions that decide what to show or
//! offer - may the viewer change the channel, read an agent's context - at
//! the time it is taken. It is data for listing and offering: an operation
//! still checks its own permission when it acts, never by a snapshot.
//!
//! How much a snapshot holds is the caller's choice (`Detail`): what a
//! detail costs - reading every agent's state, say - is paid only where it
//! is shown. A detail is taken where it is asked for and its part is
//! there, and nothing else: `USAGE` without `MEMBERS` gives no usage, since
//! there are no members to carry it. A list not asked for is empty. What
//! a member's details read - its usage, notes and pending approvals - comes
//! from its agent's state, read once for all of them; who it is
//! (`PROFILE`) from its personality, its owner and a user's profile.

use crate::actor::{ActorId, ActorState};
use crate::agent::PersonalityKind;
use crate::approval::{ApprovalRequest, ApprovalRule};
use crate::confidentiality::Classification;
use crate::value::language::Language;
use crate::value::text::{Briefing, Summary};
use crate::digest::Digest;
use crate::message::TokenUsage;
use crate::note::Note;
use crate::privacy::PrivacyConfig;
use crate::session::{ChannelId, SessionId};
use crate::work::TurnId;

bitflags::bitflags! {
    /// What a snapshot holds beyond what it always does, each detail in the
    /// part it belongs to; combined freely, without one implying another.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Detail: u8 {
        /// Of a session: the channels the viewer may read.
        const CHANNELS = 1 << 0;
        /// Of a channel: its members.
        const MEMBERS = 1 << 1;
        /// Of a member: what its model calls used (issue 209).
        const USAGE = 1 << 2;
        /// Of a member: the notes it published (issue 17).
        const NOTES = 1 << 3;
        /// Of a member: the approvals its turn waits for (issue 172), and the
        /// questions (issue 165).
        const APPROVALS = 1 << 4;
        /// Of a member: who it is - its introduction, and of an agent its
        /// personality and owner (issue 244).
        const PROFILE = 1 << 5;
        /// Of a session or channel: its briefing (issue 245), which may be
        /// long - not where they are listed.
        const BRIEFING = 1 << 6;
    }
}

/// A session with its participants and the channels the viewer may read.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub name: String,
    /// What it is for, in a line (issue 245); `None` if nobody said.
    pub summary: Option<Summary>,
    /// What an agent should know before it works here (issue 245) - with
    /// `Detail::BRIEFING`.
    pub briefing: Option<Briefing>,
    /// Whom the session lets read what was said in it (issue 134) - shown
    /// to everyone who takes part.
    pub privacy: PrivacyConfig,
    /// What the session restricts of tool calls (issue 13) - shown to
    /// everyone who takes part, whose agents it binds.
    pub approval_rules: Vec<ApprovalRule>,
    /// Whether the viewer may change the session - rename it, change its
    /// privacy (`UpdateSession`).
    pub can_change: bool,
    /// Everyone who takes part in the session - whom a new channel can be
    /// made of (issue 09).
    pub participants: Vec<ParticipantSnapshot>,
    /// The channels the viewer may read, in the order of the session -
    /// with `Detail::CHANNELS`.
    pub channels: Vec<ChannelSnapshot>,
    /// The workspace attached to it (issue 253), if the viewer may read it:
    /// its owner, who attached it. Who uses it in a channel learns of it
    /// there (`ChannelView::workspace`).
    pub workspace: Option<WorkspaceSnapshot>,
}

/// A participant of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticipantSnapshot {
    pub id: ActorId,
    /// `None` for a user or agent that no longer exists.
    pub name: Option<String>,
}

/// A channel as the viewer sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSnapshot {
    pub id: ChannelId,
    pub name: String,
    /// What it is for, in a line (issue 245); `None` if nobody said.
    pub summary: Option<Summary>,
    /// What an agent should know before it works here (issue 245) - with
    /// `Detail::BRIEFING`.
    pub briefing: Option<Briefing>,
    /// The kind of the channel's moderator, e.g. `policy_moderator` (issue
    /// 174); `None` if it is a preconfigured component, whose kind is not
    /// known without reading it.
    pub moderator: Option<String>,
    /// The kind of the policy configured for the moderator's slot, e.g.
    /// `round_robin_policy` (issues 44, 174): whom it asks to respond.
    /// `None` like `moderator`.
    pub responder_policy: Option<String>,
    /// The classification of the channel's contents (issue 133).
    pub classification: Classification,
    /// The language its conversation is held in (issue 56), if any.
    pub language: Option<Language>,
    /// What the channel restricts of tool calls (issue 13) - shown to
    /// everyone who reads it, whose agents it binds.
    pub approval_rules: Vec<ApprovalRule>,
    /// Whether its members work on the workspace of its session here (issue
    /// 251), where the classifications let them.
    pub uses_workspace: bool,
    /// Whether the viewer may rename the channel and add or remove its
    /// participants (`UpdateChannel`, issue 46).
    pub can_change: bool,
    /// Its participants, in the order of the channel - with
    /// `Detail::MEMBERS`.
    pub members: Vec<MemberSnapshot>,
    /// Who took part and left (issue 164), in the order they left, each
    /// with `left` - with `Detail::MEMBERS`, without the other details.
    pub former_members: Vec<MemberSnapshot>,
}

/// The notes one agent published in a channel.
#[derive(Debug, Clone, PartialEq)]
pub struct PublishedNotes {
    pub agent: ActorId,
    pub name: String,
    pub notes: Vec<Note>,
    /// The digest of the agent's note chain: the state of knowledge read.
    pub digest: Option<Digest>,
}

/// A participant of a channel as the viewer sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberSnapshot {
    pub id: ActorId,
    /// `None` for a user or agent that no longer exists.
    pub name: Option<String>,
    /// The handle the participant is `@`-mentioned by (see
    /// `crate::mention`), or `None` if it cannot be mentioned - approvers,
    /// participants that no longer exist, names that yield no handle.
    pub handle: Option<String>,
    /// Whether the viewer may see what this participant saw when it produced
    /// a message (`ReadAgentContext`, issue 41) - only ever for agents.
    pub can_read_context: bool,
    /// Whether the viewer may see which tools this participant has now
    /// (`ReadAgentTools`, issue 194) - only ever for agents.
    pub can_read_tools: bool,
    /// Whether the viewer may approve this participant's tool calls
    /// (`ApproveToolCall`, issue 14) - only ever for agents taking part.
    pub can_approve: bool,
    /// Whether the viewer may ask this participant to answer again instead
    /// of one of its messages or of its answer under way (`RetryMessage`,
    /// issues 234, 242) - only ever for agents taking part that answer
    /// again, not for one that runs elsewhere. Whether it does is the
    /// agent's own to decide.
    pub can_retry: bool,
    /// Whether the viewer may end this participant's answer under way
    /// (`CancelChannel`, issues 235, 243) - only ever for agents taking
    /// part, of either kind.
    pub can_cancel: bool,
    /// The turn of the answer the member is giving now (issue 240), read
    /// from the running channel: only an agent run here in a turn that can
    /// still be ended has one - not an external agent, whose answers are its
    /// remote side's. `None` in a channel that does not run. A snapshot of a
    /// channel's stream is kept up by the events that follow it; elsewhere
    /// it says how things stood when it was taken.
    pub turn_under_way: Option<TurnId>,
    /// Where the member stands now (issue 243), read from the running
    /// channel - only ever for agents taking part; `None` in a channel that
    /// does not run. Kept up by `ActorEvent::StateChanged`.
    pub state: Option<ActorState>,
    /// The commands an agent member carries out itself that the viewer may
    /// see (issue 269), read from the running channel - for completion and
    /// `/help`. Empty for any other member, and in a channel that does not
    /// run, where nothing could carry them out.
    pub commands: Vec<crate::command::Command>,
    /// What an agent member's model calls in the channel used (issue 209),
    /// with `Detail::USAGE`; `None` also for a member whose calls report
    /// no tokens (an external agent, a user), before its first call, and if
    /// the agent's state could not be read.
    pub usage: Option<TokenUsage>,
    /// The notes an agent member published in the channel (issue 17) - of
    /// a channel read entry by entry, those released to where the viewer
    /// reads from - with `Detail::NOTES`.
    pub notes: Vec<Note>,
    /// The approvals an agent member's turn waits for (issue 172), for a
    /// viewer who takes part in the channel as a user and reads all of it -
    /// with `Detail::APPROVALS`.
    pub pending_approvals: Vec<ApprovalRequest>,
    /// The questions an agent member's turn waits for the answers to (issue
    /// 165), for the same viewer - sensitive ones only if they are asked -
    /// with `Detail::APPROVALS`.
    pub pending_questions: Vec<crate::question::QuestionRequest>,
    /// Who removed a former member and when (issue 164); `None` for one that
    /// takes part.
    pub left: Option<crate::removal::Removal>,
    /// Whether the viewer may delete this member - only ever an agent, its
    /// owner (`DeleteAgent`, issue 164).
    pub can_delete: bool,
    /// How the member is introduced (issue 176) - an agent by its
    /// personality, a user by themselves - with `Detail::PROFILE`. Public,
    /// like the name; `None` also for an approver, a member that no longer
    /// exists, and one whose introduction could not be read.
    pub introduction: Option<String>,
    /// The name of an agent member's personality, without its scope (issue
    /// 244) - with `Detail::PROFILE`. Public, like the introduction; not
    /// the personality itself, which stays its readers'.
    pub personality: Option<String>,
    /// Whether an agent member runs here or elsewhere (issue 244) - with
    /// `Detail::PROFILE`: what an external agent is given leaves the
    /// system.
    pub personality_kind: Option<PersonalityKind>,
    /// Who owns an agent member (issue 244): whose authority it acts with
    /// and who answers for its tool calls - with `Detail::PROFILE`.
    pub owner: Option<ActorId>,
    /// The owner's name - also where they share no session with the
    /// viewer: who talks to the agent talks to them.
    pub owner_name: Option<String>,
}

/// Something deleted - a session, a channel - as the trash shows it to the
/// viewer (issue 164): what it was called, who deleted it and when, and
/// what the viewer may do about it.
#[derive(Debug, Clone, PartialEq)]
pub struct DeletedSnapshot<I> {
    pub id: I,
    pub name: String,
    pub deleted: crate::removal::Removal,
    /// Whether the viewer may restore it.
    pub can_restore: bool,
    /// Whether the viewer may remove it for good - never for a channel,
    /// which is purged with its session.
    pub can_purge: bool,
}

/// A workspace as the viewer sees it (issue 253): its own, in the list of
/// workspaces, or a session's where they may use it in a channel.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceSnapshot {
    pub id: crate::workspace::WorkspaceId,
    pub name: String,
    /// Where it lies (issue 251): a session's scope, its owner's or the
    /// global one.
    pub scope: crate::scope::Scope,
    /// The storage its files lie in, by its configured name.
    pub storage: String,
    pub description: Option<crate::value::text::Description>,
    pub classification: Classification,
    /// Whether its storage gives it a shell (issue 251).
    pub has_shell: bool,
    /// Whether the viewer may change it - rename, describe, reclassify -
    /// and attach it to a session (`UpdateWorkspace`).
    pub can_change: bool,
}

/// An agent as the viewer sees it in the list of agents (issue 164).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSnapshot {
    pub id: crate::agent::AgentId,
    pub name: String,
    /// Whether the viewer may delete it (`DeleteAgent`).
    pub can_delete: bool,
}
