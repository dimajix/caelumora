use crate::actor::ActorId;
use crate::agent::AgentId;
use crate::approval::{ApprovalRule, Approver};
use crate::component::{ComponentConfig, ComponentDescriptor};
use crate::confidentiality::Classification;
use crate::group::GroupId;
use crate::chain::Position;
use crate::history::{History, WINDOW};
use crate::message::{Message, MessageId};
use crate::removal::{Deletable, Removal};
use crate::scope::{Scope, Scoped};
use crate::security::{Grant, Secured, SecurityAttributes, Subject};
use crate::session::{Session, SessionId};
use crate::store::query::Field;
use crate::store::{EntityKind, Journaled, Stored, Updatable};
use crate::user::UserId;
use crate::value::language::Language;
use crate::value::name::{Name, ScopedName};
use crate::value::text::{Briefing, Summary};
use crate::value::ValueError;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

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
}

/// How much of a channel's history a new participant may see, as asked for
/// when adding them (issue 46). The `ChannelLoop` fixes it as a position
/// when they join (`start_in`, `HistoryStart::from`) and applies that.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryAccess {
    #[default]
    All,
    None,
    LastDays { count: u32 },
    LastMessages { count: u32 },
}

impl HistoryAccess {
    /// Whether `newest` - a channel's messages, newest first, as far as they
    /// were read - is enough to tell where the view begins (`start_in`): the
    /// history is read from its end only as far back as the grant reaches
    /// (issue 379).
    pub fn read_enough(&self, newest: &[Message], now: DateTime<Utc>) -> bool {
        match *self {
            HistoryAccess::All => true,
            HistoryAccess::None => !newest.is_empty(),
            HistoryAccess::LastMessages { count } => newest.len() > count as usize,
            HistoryAccess::LastDays { count } => {
                let since = now - Duration::days(i64::from(count));
                newest.last().is_some_and(|m| m.timestamp < since)
            }
        }
    }

    /// Where the view of a channel's history begins for a participant
    /// granted this, fixed `now` - so that "the last three days" does not
    /// move day by day. `newest` are its messages, newest first, as many as
    /// `read_enough` asks for, or all there are. `None` if all of it is
    /// visible.
    pub fn start_in(&self, newest: &[Message], now: DateTime<Utc>) -> Option<HistoryStart> {
        let last = || newest.first().map(|m| HistoryStart::After(m.id));
        // From the `n`-th newest, if an older one is there; all of it if not.
        let from = |n: usize| newest.get(n).map(|_| HistoryStart::From(newest[n - 1].id));
        match *self {
            HistoryAccess::All => None,
            HistoryAccess::None | HistoryAccess::LastMessages { count: 0 } => last(),
            HistoryAccess::LastMessages { count } => from(count as usize),
            HistoryAccess::LastDays { count } => {
                let since = now - Duration::days(i64::from(count));
                match newest.iter().take_while(|m| m.timestamp >= since).count() {
                    0 => last(),
                    recent => from(recent),
                }
            }
        }
    }
}

/// Where a participant's view of a channel's history begins, as a grant
/// reads it off the messages (`HistoryAccess::start_in`); fixed as a
/// position as they join (`from`), which the `HistoryShared` they are given
/// records (issue 417).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryStart {
    /// From this message on.
    From(MessageId),
    /// After this message: what came before the participant joined is not
    /// visible.
    After(MessageId),
}

impl HistoryStart {
    /// The message the view begins at or after.
    pub fn message(&self) -> MessageId {
        match *self {
            HistoryStart::From(id) | HistoryStart::After(id) => id,
        }
    }

    /// The position in a channel's history from which on it is visible from
    /// here, `said_at` being the position of the entry that said `message`
    /// (issue 379).
    pub fn from(&self, said_at: Position) -> Position {
        match self {
            HistoryStart::From(_) => said_at,
            HistoryStart::After(_) => said_at.next(),
        }
    }
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
            ChannelUpdateEvent::ClassificationChanged(classification) => self.classification = classification.clone(),
            ChannelUpdateEvent::LanguageChanged(language) => self.language = Language::optional(language.as_deref())?,
            ChannelUpdateEvent::ApprovalRulesChanged(rules) => self.approval_rules = rules.clone(),
            ChannelUpdateEvent::DescriptionChanged { summary, briefing } => self.describe(summary.as_deref(), briefing.as_deref())?,
            ChannelUpdateEvent::UsesWorkspaceChanged(uses) => self.uses_workspace = *uses,
        }
        Ok(())
    }
}

impl Deletable for Channel {
    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }
}

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
    /// A new channel, owned by `owner`, as every channel is built - from a
    /// template or on request (issue 09) -, read by its participants (issue
    /// 23): every user and agent in it, whoever they are at the time - the
    /// group of its participants (issue 304), which whoever stores the
    /// channel stores with it (`Group::of_channel`). Held in `language`, if
    /// any. Built empty (issue 420): its founders join by the one way in,
    /// as anyone does, once it is stored; and it has no internal actors -
    /// whoever builds it gives it those that are configured, and no others,
    /// so that without an approver nothing in it is approved.
    pub fn build(session_id: SessionId, owner: ActorId, name: Name, components: ComponentConfig, classification: Classification, language: Option<Language>) -> Self {
        let id = ChannelId::new();
        let security_attributes = SecurityAttributes::owned_by(owner).granting(Subject::Group(GroupId::of_channel(id)), Grant::Read);
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
            uses_workspace: true,
            deleted: None,
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
    pub fn describe(&mut self, summary: Option<&str>, briefing: Option<&str>) -> Result<(), ValueError> {
        (self.summary, self.briefing) = (Summary::optional(summary)?, Briefing::optional(briefing)?);
        Ok(())
    }
}

/// A participant that left a channel (issue 164): removed, by whom and
/// when. What it saw of the history is the history's to say (issue 417).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormerParticipant {
    pub participant: Participant,
    pub left: Removal,
}

/// What a channel holds while it runs: its history, and from where it holds
/// it. Its document - what `Journaled` writes - is the small part alone; the
/// history is a chain a store keeps apart (issue 379), written with the state
/// and read on its own (`HistoryStore`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelState {
    pub channel_id: ChannelId,
    /// The channel's history, from `history_from` on: as a store loads it.
    #[serde(skip)]
    pub history: History,
    /// The position from which on the channel keeps its history in memory;
    /// a store loads the history from there.
    pub history_from: Position,
}

impl ChannelState {
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            history: History::default(),
            history_from: Position::START,
        }
    }

    /// Lets go of what its history holds before its window - the last
    /// `WINDOW` entries - once it is stored, and records where the window
    /// begins, to be stored with the next put (issue 379). The channel reads
    /// no further back than that in memory; everything else is the store's.
    pub fn keep_window(&mut self) {
        self.history.keep_from(self.history.end().before(WINDOW));
        self.history_from = self.history.base();
    }
}

/// A change to a [`ChannelState`] since it was last stored (issue 193). The
/// channel's id is what the state is stored under, not a part that changes;
/// its history is a chain a store keeps apart (issue 379).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelStateChange {
    /// From where the history is held, whole.
    HistoryFrom(Position),
}

impl Journaled for ChannelState {
    type Change = ChannelStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        vec![ChannelStateChange::HistoryFrom(self.history_from)]
    }

    /// The history is marked stored by the store that wrote it.
    fn mark_stored(&mut self) {}

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            ChannelStateChange::HistoryFrom(from) => self.history_from = from,
        }
        Ok(())
    }
}

/// Who takes part in a session or channel: users and agents, by reference.
/// Their records live in the `EntityStore` and are read from there, so
/// that no session keeps a copy that goes stale or shows more of them than
/// their name (issue 120).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Participant {
    User(UserId),
    Agent(AgentId),
}

impl Participant {
    pub fn actor_id(&self) -> ActorId {
        match self {
            Participant::User(id) => ActorId::User(*id),
            Participant::Agent(id) => ActorId::Agent(*id),
        }
    }
}

/// An actor that belongs to a channel itself, as its moderator does (issue
/// 420): none of its participants, nor in their group. It is never added,
/// removed or announced, and has no range of the history - what is recorded
/// to it goes to it by name. Otherwise it is an actor like any other: the
/// channel starts it with the others, delivers to it by the one rule
/// (`Envelope::concerns`) and takes in what it sends. It has no record of
/// its own and is kept here whole.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InternalActor {
    /// The approver, which collects the users' decisions on the tool calls
    /// that need approval and decides by its strategy (see APPROVALS.md).
    Approver(Approver),
}

impl InternalActor {
    pub fn actor_id(&self) -> ActorId {
        match self {
            InternalActor::Approver(approver) => ActorId::Approver(approver.id),
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testing::{replayed, stored};
    use crate::message::testing::{id, make_message};

    /// Five messages, one a day, the last one `now`.
    fn daily(now: DateTime<Utc>) -> Vec<Message> {
        (0..5).rev()
            .map(|days| Message { id: MessageId::new(), timestamp: now - Duration::days(days), ..Message::new(Classification::CHANNEL_DEFAULT) })
            .collect()
    }

    fn ids(history: &[Message]) -> Vec<MessageId> {
        history.iter().map(|m| m.id).collect()
    }

    /// What `access` grants of `history`, oldest first, read from the end
    /// only as far as it asks: the ids from where the view begins.
    fn granted(access: HistoryAccess, history: &[Message], now: DateTime<Utc>) -> Vec<MessageId> {
        let mut newest: Vec<Message> = Vec::new();
        for message in history.iter().rev() {
            if access.read_enough(&newest, now) {
                break;
            }
            newest.push(message.clone());
        }
        let from = match access.start_in(&newest, now) {
            Some(start) => start.from(Position::new(history.iter().position(|m| m.id == start.message()).unwrap() as u64)).get() as usize,
            None => 0,
        };
        ids(&history[from.min(history.len())..])
    }

    #[test]
    fn a_change_is_made_into_the_channel_s_values_or_leaves_it_as_it_was() {
        let owner = ActorId::User(UserId::new());
        let mut channel = Channel::build(SessionId::new(), owner, Name::new("main").unwrap(), ComponentConfig::default(), Classification::CHANNEL_DEFAULT, None);
        channel.apply(&ChannelUpdateEvent::NameChanged("planning".to_string())).unwrap();
        channel.apply(&ChannelUpdateEvent::LanguageChanged(Some("de".to_string()))).unwrap();
        channel.apply(&ChannelUpdateEvent::UsesWorkspaceChanged(false)).unwrap();
        assert_eq!((channel.name.as_str(), channel.language.as_ref().map(|l| l.as_str()), channel.uses_workspace), ("planning", Some("de"), false));

        assert!(channel.apply(&ChannelUpdateEvent::LanguageChanged(Some("no language".to_string()))).is_err(), "no language tag");
        assert_eq!(channel.language.as_ref().map(|l| l.as_str()), Some("de"), "unchanged");
    }

    #[test]
    fn a_channel_without_a_language_of_its_own_is_held_in_its_sessions() {
        let (owner, id, now) = (UserId::new(), SessionId::new(), Utc::now());
        let mut session = Session {
            id, name: Name::new("s").unwrap(), summary: None, briefing: None, language: None, time_zone: None, participants: Vec::new(), channels: Vec::new(),
            commands: Default::default(), privacy: Default::default(), approval_rules: Vec::new(), workspace: None, created_at: now, updated_at: now,
            security_attributes: Session::attributes_of(id, owner, &[]), deleted: None,
        };
        let tag = |tag: &str| Some(Language::new(tag).unwrap());
        let mut channel = Channel::build(id, ActorId::User(owner), Name::new("main").unwrap(), ComponentConfig::default(), Classification::CHANNEL_DEFAULT, None);
        assert_eq!(channel.effective_language(&session), None, "neither has one");
        session.language = tag("de");
        assert_eq!(channel.effective_language(&session), tag("de").as_ref(), "the session's");
        channel.language = tag("fr");
        assert_eq!(channel.effective_language(&session), tag("fr").as_ref(), "its own before the session's");
    }

    #[test]
    fn history_access_all_and_none_grant_everything_and_nothing() {
        let now = Utc::now();
        let history = daily(now);
        assert_eq!(granted(HistoryAccess::All, &history, now), ids(&history), "All sees every message");
        assert!(granted(HistoryAccess::None, &history, now).is_empty(), "None sees no earlier message");
        assert_eq!(HistoryAccess::None.start_in(&[], now), None, "an empty channel has nothing to hide");
        assert!(HistoryAccess::All.read_enough(&[], now), "all of it is granted without reading any");
    }

    #[test]
    fn history_access_last_messages_and_days_cut_the_history() {
        let now = Utc::now();
        let history = daily(now);
        assert_eq!(granted(HistoryAccess::LastMessages { count: 2 }, &history, now), ids(&history[3..]));
        assert_eq!(granted(HistoryAccess::LastMessages { count: 9 }, &history, now), ids(&history),
                   "more than there is grants all of it");
        assert!(granted(HistoryAccess::LastMessages { count: 0 }, &history, now).is_empty());
        // Messages 1, 2 and 3 days ago plus today's - within the last three days.
        assert_eq!(granted(HistoryAccess::LastDays { count: 3 }, &history, now), ids(&history[1..]));
        assert!(granted(HistoryAccess::LastDays { count: 3 }, &history, now + Duration::days(10)).is_empty(),
                "nothing within the last days sees nothing");
        assert!(!HistoryAccess::LastDays { count: 3 }.read_enough(&history[3..].iter().rev().cloned().collect::<Vec<_>>(), now), "read back until one older than the grant");
    }

    #[test]
    fn a_history_start_is_the_position_of_its_message_or_the_one_after() {
        let at = id(1);
        assert_eq!((HistoryStart::From(at).from(Position::new(4)), HistoryStart::After(at).from(Position::new(4))), (Position::new(4), Position::new(5)));
        assert_eq!(HistoryStart::After(at).message(), at);
    }

    #[test]
    fn a_channel_keeps_the_last_entries_of_its_history_once_they_are_stored() {
        let mut state = ChannelState::new(ChannelId::new());
        state.history.extend((1..=250).map(|n| make_message(id(n))).collect());
        state.keep_window();
        assert_eq!((state.history.base().get(), state.history_from.get()), (0, 0), "nothing goes before it is stored");
        state.history.mark_stored();
        state.keep_window();
        assert_eq!((state.history.base().get(), state.history.end().get(), state.history_from.get()), (50, 250, 50), "the last entries");
    }

    #[test]
    fn a_channels_state_journals_its_small_part_and_leaves_its_history_to_the_store() {
        let mut state = ChannelState::new(ChannelId::new());
        state.history.extend(vec![make_message(id(1))]);
        let mut copy = stored(&mut state);
        assert!(copy.history.is_empty(), "the history is not in the document");
        assert!(state.history.changes().is_some(), "and is marked stored by the store that writes it");

        state.history_from = Position::new(1);
        assert_eq!(state.changes().len(), 1, "the small part, whole");
        replayed(&state, &mut copy);
        assert_eq!((copy.channel_id, copy.history_from.get()), (state.channel_id, 1));
    }
}
