//! The history of a channel as an agent keeps it (issue 06): the chain of
//! what happened in it - messages said, each of the agent's own with what it
//! was produced from, reclassifications (issue 133), changes to what was
//! said (issue 234), where the steps of a plan began (issue 263), how
//! turns ended (issue 258) and who joined or left (issue 408) - read into
//! the messages as they stand (issue 197). Every entry is recorded with whom
//! it was sent by and whom it went to (`Recorded`, issue 417), and with the
//! head of the envelope it came in, so that it can be delivered again from
//! here (`Recorded::envelope`, issue 223); and read only for a reader, who is
//! given what of it they may see (`Recorded::as_seen_by`), and who stands
//! somewhere in it (`Heard`).

use crate::actor::{ActorEvent, ActorId, Envelope, EventContext, Rejection, RequestReason, RetryTarget};
use crate::agent::RetryRefused;
use crate::approval::{ApprovalId, ApprovalRequest, ApprovalResponse, ApprovalStatus};
use crate::question::{QuestionId, QuestionRequest};
use crate::chain::{Chain, Node, Position};
use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::message::{Message, MessageId};
use crate::security::SecurityContext;
use crate::turn::GivenUpReason;
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::{Not, Range};
use crate::digest;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

/// What happened in a `History`; its node carries the digest of the
/// history up to and including it.
// Nearly every entry is a message said: boxing it would cost an allocation
// per message and save nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryEntry {
    /// A message said, with what it was produced from if a turn of its
    /// sender produced it (`Message::produced_from`, issue 417).
    Said { message: Message },
    /// A message an agent was given to read (issues 22, 417) - a skill
    /// loaded for it, what a compaction gave again -: what the agent read,
    /// not what anyone said, so no message said in the channel, found by
    /// no id and changed by nobody. Gone to nobody: its record, read by
    /// whom may read what the agent saw.
    Given { message: Message },
    /// The message `message` - on whichever branch - is classified
    /// `classification` from here on (issue 133).
    Reclassified { message: MessageId, classification: Classification },
    /// The message `message.id` - on whichever branch - reads as `message`
    /// from here on (issue 234, `Message::modify`).
    Modified { message: Message },
    /// The message `message` - on whichever branch - was taken back by its
    /// author at `at` (issue 234, `Message::revoke`).
    Revoked { message: MessageId, at: DateTime<Utc> },
    /// The channel was classified `channel`, and every message said so far
    /// fits into it from here on (`Classification::fitted_into`).
    Fitted { channel: Classification },
    /// The turn `turn_id` of `actor` ended at `at` and was taken over
    /// (issue 258).
    TurnFinished { turn_id: TurnId, actor: ActorId, at: DateTime<Utc> },
    /// The turn `turn_id` of `actor` was given up at `at`, for `reason`
    /// (issue 258): what it said before stays said. Whose turn it was is
    /// recorded, since a turn may end before it said anything.
    TurnGivenUp { turn_id: TurnId, actor: ActorId, reason: GivenUpReason, at: DateTime<Utc> },
    /// Step `step_index` of `total_steps` of the plan of `actor`'s turn
    /// `turn_id` began at `at` (issue 263): what the turn says from here on
    /// is said in that step, until the next one begins or the turn gives
    /// its final answer. `description` is the step as the plan words it.
    PlanStepStarted { turn_id: TurnId, actor: ActorId, step_index: usize, total_steps: usize, description: String, at: DateTime<Utc> },
    /// `actor` carried out the command `command` with `args` at `at`, on the
    /// word of `issuer`, and answered `output` (issues 269, 417) - to `to`
    /// alone if it is private, to every user otherwise: an agent in its turn
    /// `turn_id`, recorded before the turn's end; the session (`/wakeup`),
    /// without a turn, in the name of whoever asked.
    CommandExecuted { turn_id: Option<TurnId>, actor: ActorId, issuer: ActorId, command: String, args: String, output: String, to: Option<ActorId>, at: DateTime<Utc> },
    /// `actor`, an agent, replaced its context in its turn `turn_id` at
    /// `at` (issue 16): `from`, the transcript as it stood, by `to`, the
    /// head of a branch of its own that begins anew - what replaces the
    /// older messages, then those kept. Every reference recorded before
    /// names the old branch, which stays; what is said afterwards is
    /// produced from the new one. What replaced the older messages follows
    /// it, as messages given to the agent. Kept in the agent's history
    /// alone: the transcript's digests do not record where a branch came
    /// from.
    Compacted { turn_id: TurnId, actor: ActorId, from: Digest, to: Digest, at: DateTime<Utc> },
    /// `actor`, an agent, ended `activity` - something of its own accord,
    /// besides answering - in its turn `turn_id` at `at` (issue 284), and
    /// `output` says what came of it: recorded where it happened, with what
    /// it said, which is everybody's.
    ActivityFinished { turn_id: TurnId, actor: ActorId, activity: String, output: String, at: DateTime<Utc> },
    /// `participant`, called `name` then, joined the channel at `at`, by
    /// the act of `by`, called `by_name` then - the participant itself if
    /// it joined by itself, `ActorId::System` if the runtime added it
    /// (issue 408): recorded where it happened among what was said, with
    /// the names as they were, so that it reads the same after a renaming.
    ParticipantJoined { participant: ActorId, name: String, by: ActorId, by_name: String, at: DateTime<Utc> },
    /// `participant` left the channel, as `ParticipantJoined` says - `by`
    /// who removed it, the participant itself if it left by itself. It was
    /// given nothing from the position `until` on (issue 417): where the
    /// channel stopped delivering to it, which may lie before this entry -
    /// what it still sent until its task ended comes first (issue 262).
    ParticipantLeft { participant: ActorId, name: String, by: ActorId, by_name: String, until: Position, at: DateTime<Utc> },
    /// `participant` was given the history from the position `from` on as
    /// it joined at `at`, by the act of `by`, called `by_name` then (issues
    /// 46, 417): how much `by` granted (`HistoryAccess`), fixed as a
    /// position. Recorded in the system's name right before its
    /// `ParticipantJoined`, for it alone, and only if it shares something -
    /// if `from` lies before the join; otherwise what it is given begins
    /// with its join.
    HistoryShared { participant: ActorId, from: Position, by: ActorId, by_name: String, at: DateTime<Utc> },
    /// `actor`, an agent, did not carry out the command `command` asked by
    /// `to`, for `reason` (issues 269, 417), told to `to` alone at `at`;
    /// `turn_id` if it had begun it. Nothing it did stays.
    CommandFailed { turn_id: Option<TurnId>, actor: ActorId, command: String, reason: String, to: ActorId, at: DateTime<Utc> },
    /// `actor`, an agent, answers again instead of `target` (issues 234,
    /// 417), told to the users at `at`.
    RetryAccepted { target: RetryTarget, actor: ActorId, at: DateTime<Utc> },
    /// `actor`, an agent, does not answer again instead of `target`, for
    /// `reason` (issues 234, 417), told to the users at `at`.
    RetryRejected { target: RetryTarget, reason: RetryRefused, actor: ActorId, at: DateTime<Utc> },
    /// A chain between agents reached its limit at `at` (issues 200, 417):
    /// the response ending in `message` asked nobody to respond, as more
    /// than `max_agent_hops` hops since the last user's message; `not_asked`
    /// are the agents it mentioned. Told by the moderator, in the system's
    /// name, to whom it names.
    ChainCutOff { message: MessageId, max_agent_hops: u32, not_asked: Vec<ActorId>, at: DateTime<Utc> },
    /// What its sender sent - an event of the kind `rejected`, about
    /// `message` if it is one - was not let into the channel at `at`, for
    /// `reason` (issues 304, 417): told to the sender alone, in the system's
    /// name. What was rejected entered nothing and is not recorded.
    MessageRejected { rejected: String, message: Option<MessageId>, reason: Rejection, at: DateTime<Utc> },
    /// Whom it went to was asked to respond at `at` (issue 417) - to what
    /// `by` said or nudged -, as the moderator decided and for `reason`
    /// (issue 439): in the system's name, to those asked.
    ResponseRequested { by: ActorId, reason: RequestReason, at: DateTime<Utc> },
    /// `by`, a user, asked the agent `agent` at `at` to carry out the
    /// command `command` with `args` (issues 269, 223). Like every entry
    /// that follows, recorded to be delivered again from the history - to
    /// an agent whose inbox fell behind, or that starts again before it
    /// stored what it took in -, and no item: shown live alone.
    CommandRequested { agent: ActorId, command: String, args: String, by: ActorId, at: DateTime<Utc> },
    /// `by`, a user, asked `author`, an agent, at `at` to answer again
    /// instead of `target` (issues 234, 223).
    RetryRequested { target: RetryTarget, author: ActorId, by: ActorId, at: DateTime<Utc> },
    /// `by` asked at `at` for a decision on `request` (issues 81, 223): an
    /// agent, of the approvers; an approver, of the users.
    ApprovalRequested { request: ApprovalRequest, by: ActorId, at: DateTime<Utc> },
    /// `by` answered the request `response.id` with `status` at `at`
    /// (issues 81, 419, 223): a user, to the approvers; an approver, its
    /// decision, to the users and the agent that asked.
    ApprovalDecided { response: ApprovalResponse, status: ApprovalStatus, by: ActorId, at: DateTime<Utc> },
    /// `by` withdrew the request `id` at `at` (issues 148, 223): the agent
    /// that asked, of the approvers; an approver, of the users.
    ApprovalRequestWithdrawn { id: ApprovalId, by: ActorId, at: DateTime<Utc> },
    /// `by`, an agent, asked the questions `request` at `at` (issues 165,
    /// 223). The answers are no entry: they go to the tool alone, never
    /// into a history.
    QuestionRequested { request: QuestionRequest, by: ActorId, at: DateTime<Utc> },
    /// `by`, an agent, no longer waits for the answers to its questions
    /// `id` since `at` (issues 165, 223), told to whom they were shown.
    QuestionClosed { id: QuestionId, shown_to: Option<Vec<ActorId>>, by: ActorId, at: DateTime<Utc> },
}

/// What happened in a channel, recorded (issue 417): the entry, the actor
/// whose envelope it was - the system for what the channel says itself -,
/// whom it went to, resolved as it was recorded into what a store filters by
/// (`Recipients`), and the head of the envelope it came in (issue 223): its
/// context and when it was sent - what it is delivered again with from the
/// history (`envelope`). The chain's entry of a `History`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recorded {
    pub entry: HistoryEntry,
    pub sender: ActorId,
    pub recipients: Recipients,
    /// The context of the envelope as it entered the channel, without a
    /// position, which is the chain's. Its sender is the envelope's - for a
    /// request to respond, whose context it carries, while `sender` is the
    /// system, in whose name it is recorded.
    pub context: EventContext,
    pub timestamp: DateTime<Utc>,
}

/// What a range of a history - as recorded, or as a reader is given it -
/// folds by (`fold`).
impl AsRef<HistoryEntry> for Recorded {
    fn as_ref(&self) -> &HistoryEntry {
        &self.entry
    }
}

impl AsRef<HistoryEntry> for HistoryEntry {
    fn as_ref(&self) -> &HistoryEntry {
        self
    }
}

/// Whom something in a channel goes to, as a moderator decides it (issues
/// 417, 418).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    /// Everyone it concerns, by the rule every channel delivers by
    /// (`Envelope::recipients`, read off an entry as `HistoryEntry::recipients`).
    #[default]
    Everybody,
    /// These alone; nobody if empty.
    Only(Vec<ActorId>),
}

/// Every position of a history: the range of a member from its start on.
pub const EVERY_POSITION: Range<Position> = Position::START..Position::MAX;

/// Whom something in a channel goes to, resolved as it is recorded (issue
/// 417): every member of the kinds it names at the time - whoever's range
/// holds the entry's position -, the channel's approvers, and whom it names,
/// member or not. What a store filters by - for a database, columns of the
/// row and a `WHERE` over them and the reader's ranges (`Recorded::seen_by`);
/// and, read off an envelope, whom a channel delivers it to
/// (`Envelope::recipients`). Nobody if it names nobody.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipients {
    /// Every user that was a member at the time.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub users: bool,
    /// Every agent that was a member at the time.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub agents: bool,
    /// The channel's approvers: its own actors, no participants (issue
    /// 420), and so of no range - there all along.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub approvers: bool,
    /// These by name, member or not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub named: Vec<ActorId>,
}

impl Recipients {
    /// Nobody.
    pub const NOBODY: Recipients = Recipients { users: false, agents: false, approvers: false, named: Vec::new() };

    /// Every user that was a member at the time, and every agent too if
    /// `agents`.
    pub fn members(agents: bool) -> Self {
        Self { users: true, agents, ..Self::NOBODY }
    }

    /// The channel's approvers.
    pub const APPROVERS: Recipients = Recipients { users: false, agents: false, approvers: true, named: Vec::new() };

    /// `named` alone.
    pub fn only(named: Vec<ActorId>) -> Self {
        Self { named, ..Self::NOBODY }
    }

    /// Every user that was a member at the time, or, if `shown_to` names
    /// some, the users among them alone - by name, member or not.
    pub fn users(shown_to: Option<Vec<ActorId>>) -> Self {
        match shown_to {
            None => Self::members(false),
            Some(shown_to) => Self::only(shown_to.into_iter().filter(|actor| matches!(actor, ActorId::User(_))).collect()),
        }
    }

    /// Whether it goes to `actor` for what `actor` is or by its name -
    /// whether a user or an agent was a member at the time is the ranges'
    /// to say (`reaches`).
    pub fn names(&self, actor: ActorId) -> bool {
        self.named.contains(&actor) || self.of_kind(actor)
    }

    /// Whether it goes to `actor` for what it is: a user or an agent of the
    /// kinds it names, or an approver.
    fn of_kind(&self, actor: ActorId) -> bool {
        match actor {
            ActorId::User(_) => self.users,
            ActorId::Agent(_) => self.agents,
            ActorId::Approver(_) => self.approvers,
            ActorId::System | ActorId::None => false,
        }
    }

    /// Whether it reached `actor` at the position `seq`, `ranges` being
    /// where `actor` was a member: by name; as an approver; as a user or an
    /// agent while it was a member.
    fn reaches(&self, actor: ActorId, seq: Position, ranges: &[Range<Position>]) -> bool {
        self.named.contains(&actor)
            || (self.approvers && matches!(actor, ActorId::Approver(_)))
            || (self.of_kind(actor) && ranges.iter().any(|range| range.contains(&seq)))
    }
}

/// Where a reader of a history stands in it (issue 223): `next`, the first
/// position not yet delivered to it in order, and `taken`, the positions
/// after it delivered ahead of their turn - an approval taken while a turn
/// waits, the rest kept for later (`EventReceiver::recv_matching`). What a
/// reader that falls behind reads on from, and what an agent stores with
/// what it took in, so that a restart delivers what the stored state lacks,
/// nothing twice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heard {
    next: Position,
    /// Each after `next`, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    taken: Vec<Position>,
}

impl Heard {
    /// Nothing delivered from `next` on.
    pub fn at(next: Position) -> Self {
        Self { next, taken: Vec::new() }
    }

    /// The first position not delivered in order.
    pub fn next(&self) -> Position {
        self.next
    }

    /// Whether what is at `seq` was delivered.
    pub fn has(&self, seq: Position) -> bool {
        seq < self.next || self.taken.binary_search(&seq).is_ok()
    }

    /// What is at `seq` was delivered in order: everything before it was
    /// delivered, or is not for the reader - and so was what was taken
    /// ahead right after it.
    pub fn took(&mut self, seq: Position) {
        self.next = self.next.max(seq.next());
        let next = self.next;
        self.taken.retain(|taken| *taken >= next);
        while self.taken.first() == Some(&self.next) {
            self.taken.remove(0);
            self.next = self.next.next();
        }
    }

    /// What is at `seq` was delivered ahead of its turn - in order, if it
    /// is the next.
    pub fn took_ahead(&mut self, seq: Position) {
        if seq == self.next {
            return self.took(seq);
        }
        if let Err(at) = self.taken.binary_search(&seq) {
            if seq > self.next {
                self.taken.insert(at, seq);
            }
        }
    }
}

/// Who reads a history, and so what of an entry they are given
/// (`Recorded::as_seen_by`, issue 417). There is no read of a history
/// without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reader {
    /// A participant, now or formerly - or whom may read what an agent saw,
    /// reading as that agent: what went to them and what they sent, the
    /// latter in full.
    Participant(ActorId),
    /// Someone who may read the channel but takes no part in it - its
    /// session's owner, a reader from another channel: what went to
    /// everybody, as everybody was given it.
    Outsider,
    /// The channel itself, for its own checks - whether a message can be
    /// changed: everything, as it was recorded.
    Channel,
}

impl Recorded {
    /// `entry`, sent by `sender` to `audience` - to everybody resolved into
    /// whom it concerns by the rule every channel delivers by
    /// (`HistoryEntry::recipients`) -, recorded by its holder itself: of no
    /// envelope, its head names the sender alone, now.
    pub fn new(entry: HistoryEntry, sender: ActorId, audience: Audience) -> Self {
        let context = EventContext { sender_id: sender, initiator_id: sender, ..EventContext::default() };
        Self::with_head(entry, sender, audience, context, Utc::now())
    }

    /// What a channel records of `envelope` (issue 223): the entry it
    /// records (`Envelope::entry`), sent by `sender` - the system for a
    /// request to respond, else the envelope's sender - to `audience`,
    /// with the envelope's context and time; `None` if nothing of it is
    /// recorded.
    pub fn of(envelope: &Envelope, sender: ActorId, audience: Audience) -> Option<Self> {
        let context = EventContext { position: None, ..envelope.context.clone() };
        Some(Self::with_head(envelope.entry()?, sender, audience, context, envelope.timestamp))
    }

    fn with_head(entry: HistoryEntry, sender: ActorId, audience: Audience, context: EventContext, timestamp: DateTime<Utc>) -> Self {
        let recipients = match audience {
            Audience::Everybody => entry.recipients(),
            Audience::Only(only) => Recipients::only(only),
        };
        Self { entry, sender, recipients, context, timestamp }
    }

    /// `entry` as every channel delivers it, sent by whom it names
    /// (`HistoryEntry::named_sender`): what a holder records of what it was
    /// told, or did itself.
    fn delivered(entry: HistoryEntry) -> Self {
        let sender = entry.named_sender();
        Self::new(entry, sender, Audience::Everybody)
    }

    /// The envelope it came in, delivered again from the history at the
    /// position `seq` (issue 223): its event (`HistoryEntry::event`), its
    /// context with the position, its time, and `security`, the sender's
    /// security context as the channel knows it now - the history does not
    /// record it. `None` for an entry of no event. What is delivered of it
    /// is the channel's to stamp, as it stamps what it delivers live.
    pub fn envelope(&self, seq: Position, security: SecurityContext) -> Option<Envelope> {
        let event = self.entry.event()?;
        Some(Envelope { timestamp: self.timestamp, security, context: EventContext { position: Some(seq), ..self.context.clone() }, event })
    }

    /// Whether `reader` is given anything of the entry at the position
    /// `seq` (issue 417), `ranges` being the reader's as a participant -
    /// where it was given what went to every member (`Memberships`): the
    /// channel everything; a participant what it sent, and, if anything of
    /// it is published, what went to it by name or what went to every
    /// member of its kind while it was one; an outsider what was published
    /// to every member. The one rule by which a history is read, a predicate
    /// over what a store keeps of the entry - its sender, its recipients,
    /// whether it is published, its position - and the reader's ranges, so
    /// that a database answers it as a `WHERE`: the model gives it as a
    /// filter too (`Filter::seen_by`, issue 451), which decides the same.
    pub fn seen_by(&self, seq: Position, reader: &Reader, ranges: &[Range<Position>]) -> bool {
        match reader {
            Reader::Channel => true,
            Reader::Participant(actor) if *actor == self.sender => true,
            Reader::Participant(actor) => self.entry.is_published() && self.recipients.reaches(*actor, seq, ranges),
            Reader::Outsider => self.entry.is_published() && self.recipients.users,
        }
    }

    /// What `reader` may see of the entry at the position `seq`, with
    /// `ranges` as `seen_by` takes them: nothing unless it sees it; then the
    /// entry as it was recorded for its sender and the channel, and as
    /// everybody was given it for anyone else (`HistoryEntry::published`).
    pub fn as_seen_by(&self, seq: Position, reader: &Reader, ranges: &[Range<Position>]) -> Option<HistoryEntry> {
        if !self.seen_by(seq, reader, ranges) {
            return None;
        }
        match reader {
            Reader::Channel => Some(self.entry.clone()),
            Reader::Participant(actor) if *actor == self.sender => Some(self.entry.clone()),
            Reader::Participant(_) | Reader::Outsider => self.entry.published(),
        }
    }
}

/// The conversation as it happened - a chain like a `Transcript`. What it
/// records is read from start to end into the messages as they stand: a
/// reclassification, a modification or a revocation applies to a message on
/// any branch, as a change made in the channel does, so it is folded over
/// every branch in the order things happened (`fold`). A message taken back
/// stays, without what it said (`Message::revoked`), so that whatever names
/// it by id finds it.
pub type History = Chain<Recorded>;

/// The messages `nodes` say - a range of a history, in the order its
/// entries were added - each with the node that said it, as they stand
/// after every later node of the range (`HistoryEntry::fold_into`), in the
/// order they were said. A change always comes after what it changes, so a
/// range that goes on to the end of its history gives every message in it
/// as it stands; a change of a message before the range is left out.
pub fn fold<'a, E: AsRef<HistoryEntry> + 'a>(nodes: impl IntoIterator<Item = &'a Node<E>>) -> Vec<(&'a Node<E>, Message)> {
    let mut messages: Vec<(&Node<E>, Message)> = Vec::new();
    let mut at: HashMap<MessageId, usize> = HashMap::new();
    for node in nodes {
        match node.entry.as_ref() {
            HistoryEntry::Said { message, .. } => {
                at.insert(message.id, messages.len());
                messages.push((node, message.clone()));
            }
            HistoryEntry::Fitted { .. } => {
                for (_, message) in &mut messages {
                    node.entry.as_ref().fold_into(message);
                }
            }
            entry => {
                if let Some(&i) = entry.changed().and_then(|id| at.get(&id)) {
                    entry.fold_into(&mut messages[i].1);
                }
            }
        }
    }
    messages
}

/// How many entries of a history its holder keeps in memory at least (issue
/// 379): the rest is read from the store.
pub const WINDOW: u64 = 200;

/// A page of a history's items (issues 218, 379): the last ones before a
/// position, as `items` gives them, where the page before it ends, and which
/// history it was read from.
#[derive(Debug, Clone, Default)]
pub struct HistoryPage {
    /// The items, oldest first, each at the position of the entry that
    /// added it (issue 417): what a client tells an item by, and the live
    /// event that announced it (`EventContext::position`).
    pub items: Vec<(Position, HistoryEntry)>,
    /// The position of the oldest of `items`: what the page before this one
    /// is read before (`HistoryStore::page`); `None` if there is no older
    /// item to be had.
    pub earlier: Option<Position>,
    /// The digest of the history when the page was read: up to where it was
    /// read.
    pub digest: Option<Digest>,
}

impl HistoryPage {
    /// The items without their positions, oldest first.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &HistoryEntry> {
        self.items.iter().map(|(_, entry)| entry)
    }
}

impl Chain<Recorded> {
    /// The turn of `author` given up, and why, if its end is the last thing
    /// of the conversation on the active path (issue 259,
    /// `HistoryEntry::is_conversation`): what may be asked for anew as long
    /// as nothing followed it - a notice or a request does not count.
    pub fn turn_given_up_last(&self, author: ActorId) -> Option<(TurnId, GivenUpReason)> {
        match self.entries().rev().map(|node| &node.entry.entry).find(|entry| entry.is_conversation()) {
            Some(HistoryEntry::TurnGivenUp { turn_id, actor, reason, .. }) if *actor == author => Some((*turn_id, reason.clone())),
            _ => None,
        }
    }

    /// The message `id` as it stands - on whichever branch - or `None` if
    /// it was never said here: as it was said, with every entry recorded
    /// after it folded into it (`HistoryEntry::fold_into`), which only
    /// changes it if it names it or fits the channel. Read from where it
    /// was said on, not from the start, and nothing but it is cloned.
    pub fn message(&self, id: MessageId) -> Option<Message> {
        let said = self.all_entries().rev().find(|node| node.entry.entry.said().is_some_and(|m| m.id == id))?;
        let mut message = said.entry.entry.said()?.clone();
        for node in self.from(said.seq.next()) {
            node.entry.entry.fold_into(&mut message);
        }
        Some(message)
    }

    /// Appends `msgs`, each as `push` does.
    pub fn extend(&mut self, msgs: Vec<Message>) {
        for m in msgs {
            self.push(m);
        }
    }

    /// Appends `msg`, said - with what it was produced from, if it says.
    pub fn push(&mut self, msg: Message) {
        self.record(HistoryEntry::Said { message: msg });
    }

    /// Records that message `id` - on whichever branch - is classified
    /// `classification` from now on (issue 133). `false`, and nothing is
    /// recorded, if there is no such message or it was taken back
    /// (`Message::reclassify`); one before what it holds is recorded
    /// (`is_changed`).
    pub fn reclassify(&mut self, id: MessageId, classification: Classification) -> bool {
        self.enter(Recorded::delivered(HistoryEntry::Reclassified { message: id, classification }))
    }

    /// Records that the message `modified.id` - on whichever branch -
    /// reads as `modified` from now on (issue 234). `false`, and nothing is
    /// recorded, if there is no such message or it cannot be modified so as
    /// it stands (`Message::modify`): taken back, or by another sender; one
    /// before what it holds is recorded (`is_changed`).
    pub fn modify(&mut self, modified: Message) -> bool {
        self.enter(Recorded::delivered(HistoryEntry::Modified { message: modified }))
    }

    /// Records that the message `id` - on whichever branch - was taken back
    /// at `at` (issue 234). `false`, and nothing is recorded, if there is no
    /// such message or it was taken back already; one before what it holds
    /// is recorded (`is_changed`).
    pub fn revoke(&mut self, id: MessageId, at: DateTime<Utc>) -> bool {
        self.enter(Recorded::delivered(HistoryEntry::Revoked { message: id, at }))
    }

    /// Whether what `change` does to the message `id` is a change to
    /// record: whether it changes the message as it stands, if it holds
    /// it. Held from a position on (issue 379), it may not hold a message
    /// said before: the change is recorded then, and the store applies it to
    /// the message if it holds it - a change of a message it never held
    /// changes nothing there either.
    fn is_changed(&self, id: MessageId, change: impl FnOnce(&mut Message) -> bool) -> bool {
        match self.message(id) {
            Some(mut standing) => change(&mut standing),
            None => self.base() > Position::START,
        }
    }

    /// Records that the channel is classified `channel` now, which every
    /// message said so far - on every branch: all of them were said in the
    /// channel - fits into from now on (`Classification::fitted_into`).
    /// Recorded whether or not it changes a message: telling which would
    /// fold the whole history.
    pub fn fit_into(&mut self, channel: &Classification) {
        self.enter(Recorded::delivered(HistoryEntry::Fitted { channel: channel.clone() }));
    }

    /// Records `item`, what happened that is no change of a message (`is_item`):
    /// a message said - as `push` records it, with what it was produced
    /// from -, how a turn ended (issue 258), after its messages, where a step
    /// of its plan began (issue 263), before the messages of the step, a
    /// command carried out (issue 269) or an activity ended (issue 284),
    /// before the turn's end, a compaction (issue 16), before what replaced
    /// the older messages, someone added or removed (issue 408). A change is recorded by its own method -
    /// `modify`, `revoke`, `reclassify`, `fit_into` -, which says whether it
    /// applies.
    pub fn record(&mut self, item: HistoryEntry) {
        debug_assert!(item.is_item(), "a change is recorded by its own method");
        self.enter(Recorded::delivered(item));
    }

    /// Records `recorded`, as the channel records what enters it (issue
    /// 417): an item after the last; a change - a modification, a
    /// revocation, a reclassification - only if it applies, as `modify`,
    /// `revoke` and `reclassify` say; a fitting always. Returns whether it
    /// recorded it.
    pub fn enter(&mut self, recorded: Recorded) -> bool {
        let applies = match &recorded.entry {
            HistoryEntry::Modified { message: modified } => self.is_changed(modified.id, |m| m.modify(modified)),
            HistoryEntry::Revoked { message, at } => self.is_changed(*message, |m| m.revoke(*at)),
            HistoryEntry::Reclassified { message, classification } => self.is_changed(*message, |m| m.reclassify(classification)),
            HistoryEntry::Fitted { .. } | HistoryEntry::Said { .. } | HistoryEntry::Given { .. } | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. } | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. } | HistoryEntry::ParticipantLeft { .. } | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. } | HistoryEntry::RetryAccepted { .. } | HistoryEntry::RetryRejected { .. } | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::MessageRejected { .. } | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. } | HistoryEntry::RetryRequested { .. } | HistoryEntry::ApprovalRequested { .. } | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. } | HistoryEntry::QuestionRequested { .. } | HistoryEntry::QuestionClosed { .. } => true,
        };
        if applies {
            self.push_with(recorded, digest::history_chain);
        }
        applies
    }
}

impl HistoryEntry {
    /// Whether it is an item of its history - a message said or given, the
    /// start of a plan's step, a command carried out or not, an activity
    /// ended, a compaction, a turn's end, a change of who takes part, a
    /// notice, a request - rather than a change of what was said before:
    /// what a reader of the history is given as it stands
    /// (`StandingItems`), a change being in the message it changes. What is
    /// recorded only to be delivered again (issue 223) - a command or a
    /// request to answer again asked of an agent, approvals and questions -
    /// is neither: never in a page, shown live alone.
    pub fn is_item(&self) -> bool {
        match self {
            HistoryEntry::Said { .. } | HistoryEntry::Given { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::CommandExecuted { .. } | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. } | HistoryEntry::ParticipantLeft { .. } | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. } | HistoryEntry::RetryAccepted { .. } | HistoryEntry::RetryRejected { .. } | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::MessageRejected { .. } | HistoryEntry::ResponseRequested { .. } => true,
            HistoryEntry::Reclassified { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Fitted { .. } => false,
            // Recorded to be delivered, not read (issue 223): neither an item
            // nor a change.
            HistoryEntry::CommandRequested { .. } | HistoryEntry::RetryRequested { .. } | HistoryEntry::ApprovalRequested { .. } | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. } | HistoryEntry::QuestionRequested { .. } | HistoryEntry::QuestionClosed { .. } => false,
        }
    }

    /// Whether it is part of the conversation - something said, given or
    /// done in it, a change of it, a turn's end, a change of who takes
    /// part -, rather than what was only told or asked about it: a notice,
    /// a request to respond, what a newcomer was given. What "the last
    /// thing" in a channel is read among (issue 417).
    pub fn is_conversation(&self) -> bool {
        match self {
            HistoryEntry::Said { .. } | HistoryEntry::Given { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::CommandExecuted { .. } | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. } | HistoryEntry::ParticipantLeft { .. }
            | HistoryEntry::Reclassified { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Fitted { .. } => true,
            HistoryEntry::HistoryShared { .. } | HistoryEntry::CommandFailed { .. } | HistoryEntry::RetryAccepted { .. } | HistoryEntry::RetryRejected { .. }
            | HistoryEntry::ChainCutOff { .. } | HistoryEntry::MessageRejected { .. } | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. } | HistoryEntry::RetryRequested { .. } | HistoryEntry::ApprovalRequested { .. } | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. } | HistoryEntry::QuestionRequested { .. } | HistoryEntry::QuestionClosed { .. } => false,
        }
    }

    /// Whether it ends a turn of `actor`, finished or given up.
    pub fn ends_turn_of(&self, actor: ActorId) -> bool {
        matches!(self, HistoryEntry::TurnFinished { actor: of, .. } | HistoryEntry::TurnGivenUp { actor: of, .. } if *of == actor)
    }

    /// The message, if this is one said.
    pub fn said(&self) -> Option<&Message> {
        match self {
            HistoryEntry::Said { message } => Some(message),
            HistoryEntry::Given { .. } | HistoryEntry::Reclassified { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Fitted { .. }
            | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. } | HistoryEntry::ParticipantJoined { .. } | HistoryEntry::ParticipantLeft { .. } | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. } | HistoryEntry::RetryAccepted { .. } | HistoryEntry::RetryRejected { .. } | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::MessageRejected { .. } | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. } | HistoryEntry::RetryRequested { .. } | HistoryEntry::ApprovalRequested { .. } | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. } | HistoryEntry::QuestionRequested { .. } | HistoryEntry::QuestionClosed { .. } => None,
        }
    }

    /// Applies what it changes to `standing`, a message said before it as it
    /// stands, and says whether that changed it: a modification, a
    /// revocation or a reclassification of that message
    /// (`Message::modify`, `revoke`, `reclassify`), and a fitting of the
    /// channel's, which applies to every message before it
    /// (`Classification::fitted_into`). Every other entry changes nothing.
    /// The one rule by which what stands is read from a history.
    pub fn fold_into(&self, standing: &mut Message) -> bool {
        match self {
            HistoryEntry::Modified { message } if message.id == standing.id => standing.modify(message),
            HistoryEntry::Revoked { message, at } if *message == standing.id => standing.revoke(*at),
            HistoryEntry::Reclassified { message, classification } if *message == standing.id => standing.reclassify(classification),
            HistoryEntry::Fitted { channel } => {
                let fitted = standing.classification.fitted_into(channel);
                let changed = fitted != standing.classification;
                standing.classification = fitted;
                changed
            }
            HistoryEntry::Said { .. } | HistoryEntry::Given { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Reclassified { .. }
            | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. } | HistoryEntry::ParticipantJoined { .. } | HistoryEntry::ParticipantLeft { .. } | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. } | HistoryEntry::RetryAccepted { .. } | HistoryEntry::RetryRejected { .. } | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::MessageRejected { .. } | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. } | HistoryEntry::RetryRequested { .. } | HistoryEntry::ApprovalRequested { .. } | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. } | HistoryEntry::QuestionRequested { .. } | HistoryEntry::QuestionClosed { .. } => false,
        }
    }

    /// The message it modifies, takes back or reclassifies, if it does.
    pub fn changed(&self) -> Option<MessageId> {
        match self {
            HistoryEntry::Modified { message } => Some(message.id),
            HistoryEntry::Revoked { message, .. } | HistoryEntry::Reclassified { message, .. } => Some(*message),
            HistoryEntry::Said { .. } | HistoryEntry::Given { .. } | HistoryEntry::Fitted { .. } | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. } | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. } | HistoryEntry::ParticipantLeft { .. } | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. } | HistoryEntry::RetryAccepted { .. } | HistoryEntry::RetryRejected { .. } | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::MessageRejected { .. } | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. } | HistoryEntry::RetryRequested { .. } | HistoryEntry::ApprovalRequested { .. } | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. } | HistoryEntry::QuestionRequested { .. } | HistoryEntry::QuestionClosed { .. } => None,
        }
    }

    /// Whom the entry goes to when it goes to everybody, by the rule every
    /// channel delivers by (issue 417, `Envelope::recipients`): a message to
    /// every member who is a user, and to every agent too if there is
    /// something to hear in it; a change of a message and who joined or left
    /// to every user and agent; how a turn went, what it carried out and
    /// what came of what it did of its own accord to the users; what
    /// answers a command to the user it answers; the part of the history
    /// shared with a newcomer to the newcomer alone; what an agent was given
    /// and a compaction to nobody; a command and a request to answer again
    /// to the agent they name (issues 234, 269); an approval through the
    /// approvers - an agent's request to them, theirs to the users, a user's
    /// answer to them, their decision to the users and the agent that asked
    /// (issues 81, 148); an agent's questions and their closing to the users,
    /// to those asked alone if they are sensitive (issue 165). Whether it
    /// went back to its sender is not this rule's: a reader is given what it
    /// sent itself.
    pub fn recipients(&self) -> Recipients {
        match self {
            HistoryEntry::HistoryShared { participant, .. } => Recipients::only(vec![*participant]),
            HistoryEntry::CommandExecuted { to: Some(to), .. } | HistoryEntry::CommandFailed { to, .. } => {
                Recipients::only(vec![*to].into_iter().filter(|to| matches!(to, ActorId::User(_))).collect())
            }
            HistoryEntry::Said { message } => Recipients::members(message.is_heard()),
            HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Reclassified { .. } | HistoryEntry::Fitted { .. }
            | HistoryEntry::ParticipantJoined { .. } | HistoryEntry::ParticipantLeft { .. } => Recipients::members(true),
            HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { to: None, .. }
            | HistoryEntry::ActivityFinished { .. } | HistoryEntry::RetryAccepted { .. } | HistoryEntry::RetryRejected { .. } => Recipients::members(false),
            HistoryEntry::Compacted { .. } | HistoryEntry::Given { .. } | HistoryEntry::ChainCutOff { .. } | HistoryEntry::MessageRejected { .. }
            | HistoryEntry::ResponseRequested { .. } => Recipients::NOBODY,
            HistoryEntry::CommandRequested { agent: named, .. } | HistoryEntry::RetryRequested { author: named, .. } => {
                Recipients::only(Some(*named).filter(|named| matches!(named, ActorId::Agent(_))).into_iter().collect())
            }
            HistoryEntry::ApprovalRequested { by, .. } | HistoryEntry::ApprovalRequestWithdrawn { by, .. } => match by {
                ActorId::Agent(_) => Recipients::APPROVERS,
                ActorId::Approver(_) => Recipients::members(false),
                _ => Recipients::NOBODY,
            },
            HistoryEntry::ApprovalDecided { response, status, by, .. } => match by {
                // A timeout is the approver's alone.
                ActorId::User(_) if *status != ApprovalStatus::Timeout => Recipients::APPROVERS,
                ActorId::Approver(_) => Recipients { named: vec![ActorId::Agent(response.agent_id)], ..Recipients::members(false) },
                _ => Recipients::NOBODY,
            },
            HistoryEntry::QuestionRequested { by: ActorId::Agent(_), request, .. } => Recipients::users(request.shown_to()),
            HistoryEntry::QuestionClosed { by: ActorId::Agent(_), shown_to, .. } => Recipients::users(shown_to.clone()),
            HistoryEntry::QuestionRequested { .. } | HistoryEntry::QuestionClosed { .. } => Recipients::NOBODY,
        }
    }

    /// The event the entry records, as it is delivered again from the
    /// history (issue 223): the inverse of `Envelope::entry`, whose
    /// envelope gives the rest - who sent it, when, who caused it. A message
    /// said is an intermediate one if it is internal, a step of a plan, as
    /// an agent sends it, and a complete one otherwise; a turn's end comes
    /// without its answer, which the history holds as the messages said
    /// before it (`HistoryStore::answer`). `None` for a fitting of the
    /// channel's, which is no event.
    pub fn event(&self) -> Option<ActorEvent> {
        Some(match self.clone() {
            HistoryEntry::Said { message } if message.internal => ActorEvent::IntermediateMessageGenerated(message),
            HistoryEntry::Said { message } => ActorEvent::CompleteMessageGenerated(message),
            HistoryEntry::Given { message } => ActorEvent::MessageGiven { message },
            HistoryEntry::Reclassified { message, classification } => ActorEvent::MessageReclassified { message, classification },
            HistoryEntry::Modified { message } => ActorEvent::MessageModified(message),
            HistoryEntry::Revoked { message, at } => ActorEvent::MessageRevoked { message, at },
            HistoryEntry::TurnFinished { turn_id, .. } => ActorEvent::TurnFinished { turn_id, messages: Vec::new() },
            HistoryEntry::TurnGivenUp { turn_id, reason, .. } => ActorEvent::TurnGivenUp { turn_id, reason },
            HistoryEntry::PlanStepStarted { turn_id, step_index, total_steps, description, .. } => ActorEvent::PlanStepStarted { turn_id, step_index, total_steps, description },
            HistoryEntry::CommandExecuted { turn_id, command, args, output, to, .. } => ActorEvent::CommandExecuted { command, args, output, to, turn_id },
            HistoryEntry::Compacted { turn_id, from, to, .. } => ActorEvent::ContextCompacted { turn_id, from, to },
            HistoryEntry::ActivityFinished { turn_id, activity, output, .. } => ActorEvent::ActivityFinished { turn_id, activity, output },
            HistoryEntry::ParticipantJoined { participant, name, .. } => ActorEvent::ParticipantJoined { participant, name },
            HistoryEntry::ParticipantLeft { participant, name, until, .. } => ActorEvent::ParticipantLeft { participant, name, until },
            HistoryEntry::HistoryShared { participant, from, by, by_name, .. } => ActorEvent::HistoryShared { participant, from, by, by_name },
            HistoryEntry::CommandFailed { turn_id, command, reason, to, .. } => ActorEvent::CommandFailed { command, reason, to, turn_id },
            HistoryEntry::RetryAccepted { target, .. } => ActorEvent::RetryAccepted { target },
            HistoryEntry::RetryRejected { target, reason, .. } => ActorEvent::RetryRejected { target, reason },
            HistoryEntry::ChainCutOff { message, max_agent_hops, not_asked, .. } => ActorEvent::ChainCutOff { message, max_agent_hops, not_asked },
            HistoryEntry::MessageRejected { rejected, message, reason, .. } => ActorEvent::MessageRejected { rejected, message, reason },
            HistoryEntry::ResponseRequested { reason, .. } => ActorEvent::ResponseRequested { reason },
            HistoryEntry::CommandRequested { agent, command, args, .. } => ActorEvent::CommandRequested { command, args, agent },
            HistoryEntry::RetryRequested { target, author, .. } => ActorEvent::RetryRequested { target, author },
            HistoryEntry::ApprovalRequested { request, .. } => ActorEvent::ApprovalRequested(request),
            HistoryEntry::ApprovalDecided { response, status: ApprovalStatus::Approved, .. } => ActorEvent::ApprovalGranted(response),
            HistoryEntry::ApprovalDecided { response, status: ApprovalStatus::Denied, .. } => ActorEvent::ApprovalDenied(response),
            HistoryEntry::ApprovalDecided { response, status: ApprovalStatus::Timeout, .. } => ActorEvent::ApprovalTimedOut(response),
            HistoryEntry::ApprovalRequestWithdrawn { id, .. } => ActorEvent::ApprovalRequestWithdrawn { id },
            HistoryEntry::QuestionRequested { request, .. } => ActorEvent::QuestionRequested(request),
            HistoryEntry::QuestionClosed { id, shown_to, .. } => ActorEvent::QuestionClosed { id, shown_to },
            HistoryEntry::Fitted { .. } => return None,
        })
    }

    /// Whether anything of the entry is for others than its sender
    /// (`published`): what a store keeps of it as it stands, and filters by.
    pub fn is_published(&self) -> bool {
        self.published().is_some()
    }

    /// The entry as everybody but its sender may see it (issue 417): a
    /// message, said or modified, as it is published (`Message::published`),
    /// `None` if nothing of it is left to show; nothing of what an agent was
    /// given; every other entry as it is.
    pub fn published(&self) -> Option<HistoryEntry> {
        match self {
            HistoryEntry::Said { message } => message.published().map(|message| HistoryEntry::Said { message }),
            HistoryEntry::Modified { message } => message.published().map(|message| HistoryEntry::Modified { message }),
            HistoryEntry::Given { .. } => None,
            HistoryEntry::Revoked { .. } | HistoryEntry::Reclassified { .. } | HistoryEntry::Fitted { .. } | HistoryEntry::TurnFinished { .. }
            | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. } | HistoryEntry::Compacted { .. }
            | HistoryEntry::ActivityFinished { .. } | HistoryEntry::ParticipantJoined { .. } | HistoryEntry::ParticipantLeft { .. } | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. } | HistoryEntry::RetryAccepted { .. } | HistoryEntry::RetryRejected { .. } | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::MessageRejected { .. } | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. } | HistoryEntry::RetryRequested { .. } | HistoryEntry::ApprovalRequested { .. } | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. } | HistoryEntry::QuestionRequested { .. } | HistoryEntry::QuestionClosed { .. } => Some(self.clone()),
        }
    }

    /// Whom the entry names as having done what it records: the sender of a
    /// message said or modified, the actor of a turn's mark, whoever added
    /// or removed a participant; nobody known for a revocation, a
    /// reclassification or a fitting. What a holder records as the sender
    /// of what it was told or did itself (`Recorded::delivered`).
    fn named_sender(&self) -> ActorId {
        match self {
            HistoryEntry::Said { message } | HistoryEntry::Given { message } | HistoryEntry::Modified { message } => message.sender_id,
            HistoryEntry::TurnFinished { actor, .. } | HistoryEntry::TurnGivenUp { actor, .. } | HistoryEntry::PlanStepStarted { actor, .. }
            | HistoryEntry::CommandExecuted { actor, .. } | HistoryEntry::Compacted { actor, .. } | HistoryEntry::ActivityFinished { actor, .. }
            | HistoryEntry::CommandFailed { actor, .. } | HistoryEntry::RetryAccepted { actor, .. } | HistoryEntry::RetryRejected { actor, .. } => *actor,
            HistoryEntry::ParticipantJoined { by, .. } | HistoryEntry::ParticipantLeft { by, .. } | HistoryEntry::HistoryShared { by, .. }
            | HistoryEntry::CommandRequested { by, .. } | HistoryEntry::RetryRequested { by, .. } | HistoryEntry::ApprovalRequested { by, .. } | HistoryEntry::ApprovalDecided { by, .. }
            | HistoryEntry::ApprovalRequestWithdrawn { by, .. } | HistoryEntry::QuestionRequested { by, .. } | HistoryEntry::QuestionClosed { by, .. } => *by,
            HistoryEntry::ChainCutOff { .. } | HistoryEntry::MessageRejected { .. } | HistoryEntry::ResponseRequested { .. } => ActorId::System,
            HistoryEntry::Revoked { .. } | HistoryEntry::Reclassified { .. } | HistoryEntry::Fitted { .. } => ActorId::None,
        }
    }

    /// What the message it says was produced from - the digests of the
    /// transcript before it and of the frame around it (issue 426) -, and in
    /// which turn, if this is one said and both are known: whether what its
    /// author saw can be shown (issue 207).
    pub fn context(&self) -> Option<(Option<Digest>, Digest, TurnId)> {
        match self {
            HistoryEntry::Said { message } => {
                let (transcript, frame) = message.produced_from()?;
                Some((transcript, frame, message.turn_id?))
            }
            HistoryEntry::Given { .. } | HistoryEntry::Reclassified { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Fitted { .. }
            | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. } | HistoryEntry::ParticipantJoined { .. } | HistoryEntry::ParticipantLeft { .. } | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. } | HistoryEntry::RetryAccepted { .. } | HistoryEntry::RetryRejected { .. } | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::MessageRejected { .. } | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. } | HistoryEntry::RetryRequested { .. } | HistoryEntry::ApprovalRequested { .. } | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. } | HistoryEntry::QuestionRequested { .. } | HistoryEntry::QuestionClosed { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FrameRef;
    use crate::confidentiality::Confidentiality;
    use crate::digest::message_chain;
    use crate::message::testing::{id, make_message};
    use crate::agent::AgentId;
    use crate::message::MessageRole;
    use crate::user::UserId;
    use super::testing::{items_of, messages_of, recorded_context};
    use crate::actor::ActorState;
    use crate::security::SecurityContext;
    use uuid::Uuid;

    /// Issue 417: a reader is given an entry only if it went to them or
    /// they sent it - an outsider what went to everybody, the channel
    /// everything -, in full only if they sent it.
    #[test]
    fn a_reader_is_given_what_went_to_them_and_what_they_sent_in_full() {
        use crate::tool::{ToolCall, ToolVisibility};
        // Every participant a member all along.
        const ALL: &[Range<Position>] = &[EVERY_POSITION];
        let (agent, other, user, approver) = (ActorId::Agent(AgentId::new()), ActorId::Agent(AgentId::new()), ActorId::User(UserId::new()), ActorId::Approver(Default::default()));
        let hidden = ToolCall { id: Some("1".to_string()), function_name: "look".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::HIDDEN, presentation: Default::default() };
        let answer = Message { sender_id: agent, role: MessageRole::Assistant, content: Some("found it".to_string()), tool_calls: vec![hidden], turn_id: Some(TurnId::new()), ..make_message(id(1)) }.produced_from_context(None, digest::frame(&FrameRef::default()));
        let said = Recorded::new(HistoryEntry::Said { message: answer }, agent, Audience::Everybody);
        let calls = |entry: Option<HistoryEntry>| entry.as_ref().and_then(HistoryEntry::said).map(|m| (m.tool_calls.len(), m.content.clone()));

        assert_eq!(calls(said.as_seen_by(Position::START, &Reader::Participant(agent), ALL)), Some((1, Some("found it".to_string()))), "its sender, in full");
        assert!(said.as_seen_by(Position::START, &Reader::Participant(agent), ALL).and_then(|e| e.context().map(|_| ())).is_some(), "with what it was produced from");
        for reader in [Reader::Participant(user), Reader::Participant(other), Reader::Outsider] {
            let seen = said.as_seen_by(Position::START, &reader, ALL);
            assert_eq!(calls(seen.clone()), Some((0, Some("found it".to_string()))), "{reader:?}: as it was published");
            assert!(seen.and_then(|e| e.context().map(|_| ())).is_none(), "{reader:?}: without what it was produced from");
        }
        assert!(said.as_seen_by(Position::START, &Reader::Participant(approver), ALL).is_none(), "it concerns no approver");
        assert_eq!(calls(said.as_seen_by(Position::START, &Reader::Channel, ALL)), Some((1, Some("found it".to_string()))), "the channel, as it was recorded");

        let step = Recorded::new(HistoryEntry::Said { message: Message { sender_id: agent, internal: true, ..make_message(id(2)) } }, agent, Audience::Everybody);
        assert!(step.as_seen_by(Position::START, &Reader::Participant(user), ALL).is_some() && step.as_seen_by(Position::START, &Reader::Participant(other), ALL).is_none(), "a step of a plan: the users', no other agent's");
        let ended = Recorded::new(HistoryEntry::TurnFinished { turn_id: TurnId::new(), actor: agent, at: Utc::now() }, agent, Audience::Everybody);
        assert!(ended.as_seen_by(Position::START, &Reader::Participant(agent), ALL).is_some() && ended.as_seen_by(Position::START, &Reader::Participant(other), ALL).is_none(), "a turn's end: its own and the users'");

        let to_one = Recorded::new(HistoryEntry::Said { message: make_message(id(3)) }, ActorId::System, Audience::Only(vec![user]));
        assert!(to_one.as_seen_by(Position::START, &Reader::Participant(user), ALL).is_some(), "whom it went to");
        assert!(to_one.as_seen_by(Position::START, &Reader::Participant(ActorId::User(UserId::new())), ALL).is_none(), "no other user");
        assert!(to_one.as_seen_by(Position::START, &Reader::Outsider, ALL).is_none(), "nor an outsider: it did not go to everybody");
        let to_nobody = Recorded::new(HistoryEntry::Said { message: Message { sender_id: agent, ..make_message(id(4)) } }, agent, Audience::Only(Vec::new()));
        assert!(to_nobody.as_seen_by(Position::START, &Reader::Participant(agent), ALL).is_some() && to_nobody.as_seen_by(Position::START, &Reader::Participant(user), ALL).is_none(), "to nobody: its sender alone");
    }

    /// Every event the history records, each with its sender - `agent`
    /// unless it is a user's or an approver's -, about `user`.
    fn recorded_events(agent: ActorId, user: ActorId) -> Vec<(ActorId, ActorEvent)> {
        use crate::approval::ApproverId;
        use crate::question::{Addressee, Question, Questions};
        use crate::tool::ToolCall;
        let turn_id = TurnId::new();
        let heard = Message { sender_id: agent, role: MessageRole::Assistant, content: Some("hello".to_string()), ..make_message(id(1)) };
        let step = Message { internal: true, ..heard.clone() };
        let transcript = message_chain(None, &heard);
        let ActorId::Agent(agent_id) = agent else { panic!("{agent} is no agent") };
        let approver = ActorId::Approver(ApproverId::new());
        let call = ToolCall { id: None, function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() };
        let approval = ApprovalRequest { suggested: None, explicit: false, id: Default::default(), session_id: Default::default(), channel_id: Default::default(), agent_id, tool_call: call.clone(), description: "test".to_string() };
        let response = ApprovalResponse::new(approval.id, agent_id);
        let questions = |sensitive| QuestionRequest {
            id: QuestionId::new(), session_id: Default::default(), channel_id: Default::default(), agent_id, tool_call: call.clone(),
            questions: Questions { questions: vec![Question { header: "Name".to_string(), question: "Who?".to_string(), choices: Vec::new(), multi_select: false }], ask: vec![Addressee::Actor(user)], sensitive },
        };
        let by_agent = [
            ActorEvent::CompleteMessageGenerated(heard.clone()),
            ActorEvent::IntermediateMessageGenerated(step),
            ActorEvent::MessageGiven { message: heard.clone() },
            ActorEvent::ContextCompacted { turn_id, from: transcript, to: transcript },
            ActorEvent::TurnFinished { turn_id, messages: Vec::new() },
            ActorEvent::TurnGivenUp { turn_id, reason: GivenUpReason::Left },
            ActorEvent::PlanStepStarted { turn_id, step_index: 0, total_steps: 1, description: "first".to_string() },
            ActorEvent::CommandExecuted { command: "compact".to_string(), args: String::new(), output: "done".to_string(), to: None, turn_id: Some(turn_id) },
            ActorEvent::ActivityFinished { turn_id, activity: "compacting".to_string(), output: "done".to_string() },
            ActorEvent::ParticipantJoined { participant: user, name: "user".to_string() },
            ActorEvent::ParticipantLeft { participant: user, name: "user".to_string(), until: Position::new(3) },
            ActorEvent::MessageModified(heard.clone()),
            ActorEvent::MessageRevoked { message: heard.id, at: Utc::now() },
            ActorEvent::MessageReclassified { message: heard.id, classification: Classification::PUBLIC },
            ActorEvent::CommandExecuted { command: "mark".to_string(), args: String::new(), output: "yours".to_string(), to: Some(user), turn_id: Some(turn_id) },
            ActorEvent::CommandExecuted { command: "wakeup".to_string(), args: String::new(), output: "woke".to_string(), to: None, turn_id: None },
            ActorEvent::CommandFailed { command: "mark".to_string(), reason: "no".to_string(), to: user, turn_id: None },
            ActorEvent::RetryAccepted { target: RetryTarget::Turn(turn_id) },
            ActorEvent::RetryRejected { target: RetryTarget::Turn(turn_id), reason: RetryRefused::NotLast },
            ActorEvent::ChainCutOff { message: heard.id, max_agent_hops: 3, not_asked: vec![agent] },
            ActorEvent::MessageRejected { rejected: "MessageModified".to_string(), message: Some(heard.id), reason: Rejection::Invalid },
            ActorEvent::ResponseRequested { reason: RequestReason::Mentioned },
            ActorEvent::HistoryShared { participant: user, from: Position::new(3), by: agent, by_name: "agent".to_string() },
            ActorEvent::ApprovalRequested(approval.clone()),
            ActorEvent::ApprovalRequestWithdrawn { id: approval.id },
            ActorEvent::QuestionRequested(questions(false)),
            ActorEvent::QuestionRequested(questions(true)),
            ActorEvent::QuestionClosed { id: QuestionId::new(), shown_to: None },
            ActorEvent::QuestionClosed { id: QuestionId::new(), shown_to: Some(vec![user]) },
        ];
        let mut events: Vec<(ActorId, ActorEvent)> = by_agent.into_iter().map(|event| (agent, event)).collect();
        events.extend([
            (user, ActorEvent::CommandRequested { command: "mark".to_string(), args: "it".to_string(), agent }),
            (user, ActorEvent::RetryRequested { target: RetryTarget::Turn(turn_id), author: agent }),
            (user, ActorEvent::ApprovalGranted(response.clone())),
            (user, ActorEvent::ApprovalDenied(response.clone())),
            (user, ActorEvent::ApprovalTimedOut(response.clone())),
            (approver, ActorEvent::ApprovalRequested(approval.clone())),
            (approver, ActorEvent::ApprovalRequestWithdrawn { id: approval.id }),
            (approver, ActorEvent::ApprovalGranted(response.clone())),
            (approver, ActorEvent::ApprovalDenied(response.clone())),
            (approver, ActorEvent::ApprovalTimedOut(response)),
        ]);
        events
    }

    /// Issues 417, 223: what the history records of an envelope goes to
    /// whom the envelope went to - one rule, read off the event and,
    /// resolved as it is recorded, off the entry.
    #[test]
    fn an_entry_goes_to_whom_its_envelope_went_to() {
        let (agent, user) = (ActorId::Agent(AgentId::new()), ActorId::User(UserId::new()));
        for (sender, event) in recorded_events(agent, user) {
            let kind = event.kind();
            let envelope = Envelope::new(SecurityContext::default(), EventContext { sender_id: sender, ..EventContext::default() }, event);
            let recorded = Recorded::of(&envelope, ActorId::System, Audience::Everybody).unwrap_or_else(|| panic!("{kind} is recorded"));
            assert!(!envelope.event.is_transient(), "{kind}: what is recorded is delivered from the history, never lost");
            for participant in [user, agent, ActorId::Agent(AgentId::new()), ActorId::User(UserId::new()), ActorId::Approver(Default::default())] {
                let member_all_along = [EVERY_POSITION];
                assert_eq!(recorded.seen_by(Position::START, &Reader::Participant(participant), &member_all_along), envelope.recipients().names(participant), "{kind} of {sender} for {participant}");
            }
        }
    }

    /// Issue 223: an envelope recorded comes back from the history as it
    /// came in - its event, its context with the position of its entry, its
    /// time -, with the security context the channel gives it; a turn's end
    /// without its answer, which the history holds before it.
    #[test]
    fn an_envelope_recorded_comes_back_as_it_came_in() {
        let (agent, user) = (ActorId::Agent(AgentId::new()), ActorId::User(UserId::new()));
        let context = EventContext { sender_id: agent, sender_name: "agent".to_string(), correlation_id: Uuid::new_v4(), span_depth: 2, agent_hops: 1, initiator_id: user, ..EventContext::new(Default::default(), Default::default(), agent, "agent") };
        for (sender, event) in recorded_events(agent, user) {
            let kind = event.kind();
            let context = EventContext { sender_id: sender, position: Some(Position::new(4)), ..context.clone() };
            let envelope = Envelope::new(SecurityContext::default().next(sender, ActorId::None, ActorId::None), context, event);
            let recorded = Recorded::of(&envelope, ActorId::System, Audience::Everybody).unwrap_or_else(|| panic!("{kind} is recorded"));
            let again = recorded.envelope(Position::new(7), SecurityContext::default()).unwrap_or_else(|| panic!("{kind} comes back"));
            assert_eq!(serde_json::to_value(&again.event).unwrap(), serde_json::to_value(&envelope.event).unwrap(), "{kind}: the event");
            assert_eq!((again.context.position, again.timestamp), (Some(Position::new(7)), envelope.timestamp), "{kind}: where its entry is, when it was sent");
            assert_eq!(EventContext { position: None, ..again.context }, EventContext { position: None, ..envelope.context.clone() }, "{kind}: the context it came in");
            assert_eq!(again.security, SecurityContext::default(), "{kind}: the context given");
        }
        let answer = Message { sender_id: agent, turn_id: Some(TurnId::new()), ..make_message(id(1)) };
        let ended = Envelope::new(SecurityContext::default(), context, ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![answer] });
        let again = Recorded::of(&ended, agent, Audience::Everybody).and_then(|recorded| recorded.envelope(Position::START, SecurityContext::default()));
        assert!(matches!(again.map(|e| e.event), Some(ActorEvent::TurnFinished { messages, .. }) if messages.is_empty()), "without its answer");
        assert!(Recorded::new(HistoryEntry::Fitted { channel: Classification::PUBLIC }, ActorId::None, Audience::Everybody).envelope(Position::START, SecurityContext::default()).is_none(), "a fitting is no event");
    }

    /// Issue 223: of what the history does not record, the answers to
    /// questions are not lost on the way; the rest is transient.
    #[test]
    fn of_what_is_not_recorded_only_the_answers_are_kept() {
        use crate::question::QuestionId;
        let turn_id = TurnId::new();
        let message = make_message(id(1));
        for (event, transient) in [
            (ActorEvent::QuestionAnswered { id: QuestionId::new(), agent_id: AgentId::new(), responses: Vec::new() }, false),
            (ActorEvent::QuestionDeclined { id: QuestionId::new(), agent_id: AgentId::new() }, false),
            (ActorEvent::PartialMessageGenerated(message.clone()), true),
            (ActorEvent::IntermediatePartialGenerated(message), true),
            (ActorEvent::StateChanged(ActorState::Idle), true),
            (ActorEvent::ResponseStarted { turn_id }, true),
            (ActorEvent::CommandStarted { command: "mark".to_string(), args: String::new(), to: ActorId::None, turn_id }, true),
            (ActorEvent::ActivityStarted { turn_id, activity: "compacting".to_string() }, true),
            (ActorEvent::PlanStepFinished { turn_id, step_index: 0, total_steps: 1, description: "first".to_string() }, true),
            (ActorEvent::WakeupRequested, true),
            (ActorEvent::NudgeRequested { participant: ActorId::None }, true),
        ] {
            let kind = event.kind();
            assert!(Envelope::new(SecurityContext::default(), EventContext::default(), event.clone()).entry().is_none(), "{kind} is not recorded");
            assert_eq!(event.is_transient(), transient, "{kind}");
        }
    }

    /// Issue 223: what is recorded to be delivered again is neither an item
    /// nor part of the conversation: a page shows none of it, and a turn
    /// given up is still the last thing.
    #[test]
    fn what_is_recorded_to_be_delivered_again_is_no_item() {
        let (agent, user) = (ActorId::Agent(AgentId::new()), ActorId::User(UserId::new()));
        let turn = TurnId::new();
        let mut history = History::default();
        history.record(HistoryEntry::TurnGivenUp { turn_id: turn, actor: agent, reason: GivenUpReason::Left, at: Utc::now() });
        let delivered_alone: Vec<(ActorId, ActorEvent)> = recorded_events(agent, user).into_iter()
            .filter(|(sender, event)| Envelope::new(SecurityContext::default(), EventContext { sender_id: *sender, ..EventContext::default() }, event.clone()).entry().is_some_and(|entry| !entry.is_item() && entry.changed().is_none() && entry.event().is_some()))
            .collect();
        assert_eq!(delivered_alone.len(), 16, "the requests, approvals and questions: {:?}", delivered_alone.iter().map(|(_, event)| event.kind()).collect::<Vec<_>>());
        for (sender, event) in delivered_alone {
            let envelope = Envelope::new(SecurityContext::default(), EventContext { sender_id: sender, ..EventContext::default() }, event);
            assert!(history.enter(Recorded::of(&envelope, sender, Audience::Everybody).expect("recorded")));
        }
        assert_eq!(items_of(&history).len(), 1, "the turn given up alone");
        assert_eq!(history.turn_given_up_last(agent), Some((turn, GivenUpReason::Left)), "nothing of the conversation followed it");
    }

    /// Issue 223: where a reader stands - what was delivered in order, and
    /// what ahead of it, forgotten once passed.
    #[test]
    fn a_reader_stands_after_what_was_delivered_in_order_and_ahead() {
        let at = Position::new;
        let mut heard = Heard::at(at(3));
        assert!(heard.has(at(2)) && !heard.has(at(3)), "before where it begins");
        heard.took_ahead(at(6));
        heard.took_ahead(at(2));
        assert!(heard.has(at(6)) && !heard.has(at(5)), "taken ahead");
        heard.took(at(4));
        assert_eq!(heard.next(), at(5), "past what was delivered in order, what lies between not for it");
        assert!(heard.has(at(6)), "still taken ahead");
        heard.took(at(5));
        assert_eq!(heard, Heard::at(at(7)), "and what was taken ahead right after it");
        heard.took_ahead(at(9));
        heard.took(at(10));
        assert_eq!(heard, Heard::at(at(11)), "what was taken ahead is forgotten once passed");
        heard.took(at(1));
        assert_eq!(heard.next(), at(11), "never back");
        let stored: Heard = serde_json::from_value(serde_json::to_value(&heard).unwrap()).unwrap();
        assert_eq!(stored, heard);
    }

    #[test]
    fn a_turn_given_up_is_found_while_its_end_is_the_last_thing() {
        let (agent, other) = (ActorId::Agent(AgentId::new()), ActorId::Agent(AgentId::new()));
        let turn = TurnId::new();
        let mut history = History::default();
        history.push(make_message(id(1)));
        assert_eq!(history.turn_given_up_last(agent), None, "no turn ended");
        history.record(HistoryEntry::TurnGivenUp { turn_id: turn, actor: agent, reason: GivenUpReason::Left, at: Utc::now() });
        assert_eq!(history.turn_given_up_last(agent), Some((turn, GivenUpReason::Left)));
        assert_eq!(history.turn_given_up_last(other), None, "another agent's");
        // What was only told or asked of it is no part of the conversation
        // (issue 417).
        history.record(HistoryEntry::RetryRejected { target: RetryTarget::Turn(turn), reason: RetryRefused::NotLast, actor: other, at: Utc::now() });
        history.record(HistoryEntry::ResponseRequested { by: other, reason: RequestReason::Chosen, at: Utc::now() });
        assert_eq!(history.turn_given_up_last(agent), Some((turn, GivenUpReason::Left)), "a notice and a request do not count");
        history.push(make_message(id(2)));
        assert_eq!(history.turn_given_up_last(agent), None, "something followed it");
    }

    #[test]
    fn its_items_are_the_messages_as_they_stand_and_how_turns_ended() {
        let (finished, given_up) = (TurnId::new(), TurnId::new());
        let at = Utc::now();
        let mut history = History::default();
        history.push(make_message(id(1)));
        history.record(HistoryEntry::PlanStepStarted { turn_id: finished, actor: ActorId::default(), step_index: 0, total_steps: 1, description: "first".to_string(), at });
        history.push(Message { turn_id: Some(finished), ..make_message(id(2)) });
        history.record(HistoryEntry::TurnFinished { turn_id: finished, actor: ActorId::default(), at });
        history.modify(Message { content: Some("changed".to_string()), ..make_message(id(1)) });
        history.push(Message { turn_id: Some(given_up), ..make_message(id(3)) });
        history.record(HistoryEntry::TurnGivenUp { turn_id: given_up, actor: ActorId::default(), reason: GivenUpReason::Left, at });
        history.fit_into(&Classification::CHANNEL_DEFAULT);

        let items = items_of(&history);
        let kinds: Vec<String> = items.iter().map(|item| match item {
            HistoryEntry::Said { message, .. } => format!("said {}", message.content.clone().unwrap_or_default()),
            HistoryEntry::TurnFinished { turn_id, .. } => format!("finished {}", *turn_id == finished),
            HistoryEntry::TurnGivenUp { turn_id, .. } => format!("given up {}", *turn_id == given_up),
            HistoryEntry::PlanStepStarted { turn_id, description, .. } => format!("step {description} {}", *turn_id == finished),
            other => format!("{other:?}"),
        }).collect();
        let text = |n: u128| format!("said {}", make_message(id(n)).content.unwrap_or_default());
        assert_eq!(kinds, ["said changed".to_string(), "step first true".to_string(), text(2), "finished true".to_string(), text(3), "given up true".to_string()],
            "in the order they happened, the change folded into its message, the fitting in every one");
        assert_eq!(messages_of(&history).len(), 3, "the messages alone, as before");
    }

    #[test]
    fn a_change_of_a_message_before_what_is_held_is_recorded_for_the_store() {
        let mut history = History::default();
        history.extend(vec![make_message(id(1)), make_message(id(2)), make_message(id(3))]);
        assert!(!history.revoke(id(9), Utc::now()), "a message it never held, whole");
        history.mark_stored();
        history.keep_from(Position::new(2));
        assert!(history.revoke(id(1), Utc::now()), "before what it holds: recorded, the store applies it");
        assert!(history.modify(Message { content: Some("changed".to_string()), ..make_message(id(3)) }), "one it holds as before");
        assert!(history.revoke(id(3), Utc::now()) && !history.revoke(id(3), Utc::now()), "and taken back once");
        assert_eq!(history.end().get(), 6);
    }

    #[test]
    fn a_range_folded_gives_its_messages_as_the_whole_history_does() {
        let public = Classification::general(Confidentiality::Public);
        let mut history = History::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let from = history.end();
        history.extend(vec![make_message(id(3)), make_message(id(4))]);
        history.modify(Message { content: Some("changed".to_string()), ..make_message(id(3)) });
        history.revoke(id(1), Utc::now());
        history.fit_into(&public);
        history.reclassify(id(4), Classification::general(Confidentiality::Internal));

        let whole: Vec<Message> = fold(history.all_entries()).into_iter().map(|(_, m)| m).collect();
        let range: Vec<(Position, Message)> = fold(history.from(from)).into_iter().map(|(node, m)| (node.seq, m)).collect();
        assert_eq!(range.iter().map(|(seq, _)| seq.get()).collect::<Vec<_>>(), [2, 3], "the messages of the range, where they were said");
        assert_eq!(serde_json::to_value(range.into_iter().map(|(_, m)| m).collect::<Vec<_>>()).unwrap(), serde_json::to_value(&whole[2..]).unwrap(), "as they stand");
        assert_eq!(history.message(id(3)).and_then(|m| m.content).as_deref(), Some("changed"));
        assert_eq!(history.message(id(3)).map(|m| m.classification), Some(public), "fitted after it");
        assert!(history.message(id(1)).is_some_and(|m| m.revoked.is_some()));
    }

    #[test]
    fn an_entry_folds_into_the_message_it_changes_and_says_whether_it_did() {
        let at = Utc::now();
        let mut message = make_message(id(1));
        assert!(!HistoryEntry::Revoked { message: id(2), at }.fold_into(&mut message), "another message");
        assert!(!HistoryEntry::Said { message: make_message(id(1)) }.fold_into(&mut message), "says, changes nothing");
        assert!(HistoryEntry::Modified { message: Message { content: Some("changed".to_string()), ..make_message(id(1)) } }.fold_into(&mut message));
        assert_eq!(message.content.as_deref(), Some("changed"));
        let fitted = HistoryEntry::Fitted { channel: Classification::general(Confidentiality::Public) };
        assert!(fitted.fold_into(&mut message), "lowered to the channel's");
        assert!(!fitted.fold_into(&mut message), "fits already");
        assert!(HistoryEntry::Revoked { message: id(1), at }.fold_into(&mut message));
        assert!(!HistoryEntry::Revoked { message: id(1), at }.fold_into(&mut message), "taken back already");
    }

    #[test]
    fn a_history_keeps_what_its_messages_were_produced_from() {
        let transcript = Some(message_chain(None, &make_message(id(9))));
        let frame = digest::frame(&FrameRef::default());
        let turn = TurnId::new();
        let mut history = History::default();
        history.extend(vec![make_message(id(1))]);
        history.push(Message { turn_id: Some(turn), ..make_message(id(2)) }.produced_from_context(transcript, frame));

        let loaded: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        let entries: Vec<_> = loaded.all_entries().collect();
        assert_eq!(entries[0].entry.entry.context(), None, "said by someone else");
        assert_eq!(entries[1].entry.entry.context(), Some((transcript, frame, turn)), "with the turn it was produced in");
        assert_eq!(recorded_context(&loaded, id(2)), Some((transcript, frame, turn)), "found by its message");
        assert_eq!(recorded_context(&loaded, id(1)), None);
        assert_eq!(loaded.digest(), history.digest());
    }

    #[test]
    fn a_reclassification_is_recorded_and_read_into_the_messages_as_they_stand() {
        let public = Classification::general(Confidentiality::Public);
        let internal = Classification::general(Confidentiality::Internal);
        let said = |n, classification: &Classification| Message { classification: classification.clone(), ..make_message(id(n)) };
        let mut history = History::default();
        history.extend(vec![said(1, &internal)]);
        let fork = history.digest();
        history.extend(vec![said(2, &internal)]);
        history.move_head(fork.as_ref());
        history.extend(vec![said(3, &internal)]);
        let before = history.digest();

        assert!(history.reclassify(id(2), public.clone()), "on the branch no longer active");
        assert!(!history.reclassify(id(9), public.clone()), "never said here");
        let mut revoked = history.clone();
        revoked.revoke(id(1), Utc::now());
        assert!(!revoked.reclassify(id(1), public.clone()), "taken back: nothing left to release");
        assert_ne!(history.digest(), before, "recorded: the digest covers it");
        assert_eq!(history.all_entries().len(), 4, "one entry");
        assert_eq!(history.message(id(2)).map(|m| m.classification), Some(public.clone()), "read as it stands, on whichever branch");
        assert_eq!(messages_of(&history).iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(3)], "the active path says what was said");

        let entries = history.all_entries().len();
        history.fit_into(&public);
        history.fit_into(&public);
        assert_eq!(history.all_entries().len(), entries + 2, "recorded, also when nothing is left to fit");
        assert!(messages_of(&history).iter().chain(history.message(id(2)).iter()).all(|m| m.classification == public));

        let loaded: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(messages_of(&loaded).iter().map(|m| &m.classification).collect::<Vec<_>>(), [&public, &public]);
    }

    #[test]
    fn what_was_recorded_since_the_history_was_stored_replays_onto_it() {
        let public = Classification::general(Confidentiality::Public);
        let mut history = History::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let mut stored: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        history.mark_stored();

        history.reclassify(id(2), public.clone());
        history.fit_into(&public);
        let changes = history.changes().expect("changed");
        assert_eq!(changes.entries.len(), 2, "one entry each: {changes:?}");
        stored.apply(changes).unwrap();
        assert_eq!(stored.digest(), history.digest());
        assert!(messages_of(&stored).iter().all(|m| m.classification == public));
    }

    #[test]
    fn a_change_is_recorded_and_read_into_the_messages_as_they_stand() {
        let at = Utc::now();
        let modified = |n, content: &str| Message { content: Some(content.to_string()), timestamp: at, ..make_message(id(n)) };
        let mut history = History::default();
        history.extend(vec![make_message(id(1))]);
        let fork = history.digest();
        history.extend(vec![make_message(id(2))]);
        history.move_head(fork.as_ref());
        history.extend(vec![make_message(id(3))]);
        let before = history.digest();

        assert!(history.modify(modified(1, "corrected")), "on the active path");
        assert!(history.revoke(id(2), at), "on a branch no longer active");
        assert!(!history.modify(modified(9, "never said")), "never said here");
        assert!(!history.modify(modified(2, "too late")), "taken back already");
        assert!(!history.modify(Message { sender_id: ActorId::System, ..make_message(id(3)) }), "by another sender");
        assert_ne!(history.digest(), before, "recorded: the digest covers it");
        assert_eq!(history.all_entries().len(), 5, "one entry each, nothing rewritten");

        let messages = messages_of(&history);
        assert_eq!(messages.iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(3)], "in their places");
        assert_eq!((messages[0].content.as_deref(), messages[0].modified), (Some("corrected"), Some(at)));
        let revoked = history.message(id(2)).expect("a message taken back is still found");
        assert!(revoked.content.is_none() && revoked.revoked == Some(at));

        let loaded: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(messages_of(&loaded)[0].content.as_deref(), Some("corrected"));
        assert_eq!(loaded.digest(), history.digest());
    }

    #[test]
    fn a_change_since_the_history_was_stored_replays_onto_it() {
        let mut history = History::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let mut stored: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        history.mark_stored();

        history.revoke(id(2), Utc::now());
        stored.apply(history.changes().expect("changed")).unwrap();
        assert_eq!(stored.digest(), history.digest());
        assert!(stored.message(id(2)).is_some_and(|m| m.revoked.is_some()));
    }
}
