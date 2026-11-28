//! The threads of a channel (issue 557): the conversations within it, each named by its
//! `ThreadId`, the channel's own line among them.

use super::ChannelId;
use crate::actor::ActorId;
use crate::agent::AgentId;
use crate::chain::Position;
use crate::message::{Message, MessageId};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

define_id!(ThreadId);

impl ThreadId {
    /// The thread of `channel`'s own line, its id derived from the channel's: what every event
    /// made for the channel happens in, unless it was made for another thread.
    pub fn line(channel: ChannelId) -> Self {
        Self(channel.0)
    }
}

/// Where a thread hangs: a message of another thread of the same channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadOrigin {
    pub thread: ThreadId,
    pub message: MessageId,
}

/// How a thread was opened: where it hangs, who opened it, whether who takes part in it may grow,
/// and the agents it owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opening {
    pub origin: ThreadOrigin,
    pub by: ActorId,
    pub circle: Circle,
    /// The agents it was founded with, made for it alone (issue 598), which end with it: let go
    /// and deleted as it is closed (`procedure::thread::Close`). None unless its opener asked
    /// (`ThreadSpec`).
    pub owned: Vec<AgentId>,
}

/// Whether who takes part in a thread may grow (issues 557, 602), fixed as it is opened: the one
/// thing that tells an open thread from a closed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Circle {
    /// Whoever says something in it, and whom something said in it mentions, take part in it from
    /// then on; so does the author of the message it hangs at, from its opening on.
    Open,
    /// Those it was opened with take part in it, and nobody else: nobody outside says anything in
    /// it, and nobody is taken into it - a thread delegated into, a sub-agent's (issue 558).
    Closed,
}

/// A thread of a channel, as the channel keeps it (`ChannelState::threads`): what it builds the
/// thread from as it starts, whether it was closed, and what its delegate carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadState {
    /// How it was opened; `None` for the channel's line, which hangs nowhere and is there from the
    /// channel's start.
    pub opening: Option<Opening>,
    /// Whether it was closed for good: nothing more is said in it. The line never is.
    pub closed: bool,
    /// What was delegated into it, if anything was (issue 557): kept, its briefs answered or not,
    /// until the thread is closed.
    pub delegation: Option<Delegation>,
}

impl ThreadState {
    /// The state of a channel's line.
    pub const LINE: ThreadState = ThreadState { opening: None, closed: false, delegation: None };

    /// Whether `giver` may delegate into it to `recipient`: it is delegated into by them, or not
    /// yet at all - a thread has one delegation, which brings back to its giver what its
    /// recipient made of it.
    pub fn delegates_alike(&self, giver: ActorId, recipient: ActorId) -> bool {
        self.delegation
            .as_ref()
            .is_none_or(|held| (held.giver, held.recipient) == (giver, recipient))
    }

    /// Takes `delegation` in: its briefs behind those of the delegation held - its giver's word on
    /// what came of the last one given so (issue 598) -, or as its delegation if it has none.
    /// `false`, and nothing taken, if it is delegated otherwise (`delegates_alike`), or its
    /// delegation was let go.
    pub fn delegate(&mut self, delegation: Delegation) -> bool {
        if !self.delegates_alike(delegation.giver, delegation.recipient) {
            return false;
        }
        match &mut self.delegation {
            Some(held) if held.is_let_go() => return false,
            Some(held) => {
                held.briefs.extend(delegation.briefs);
                held.word = Word::NotDue;
            }
            None => self.delegation = Some(delegation),
        }
        true
    }

    /// Whether its delegation is to an agent it owns (issue 598, `Opening::owned`): one its giver's
    /// word lets go, since closing the thread then ends the agent too.
    pub(crate) fn owns_its_recipient(&self) -> bool {
        let recipient = self.delegation.as_ref().and_then(|held| held.recipient.as_agent_id());
        let owned = self.opening.as_ref().map(|opening| opening.owned.as_slice());
        recipient.zip(owned).is_some_and(|(recipient, owned)| owned.contains(&recipient))
    }
}

/// What was delegated into a thread (issue 557): who gives, who works on it, the root of the chain
/// that delegated - what both directions carry as their initiator -, the briefs, the first the
/// one open, and the giver's word on what came of the last (issue 598, `Word`). The thread's
/// delegate carries it out; the channel alone writes it, following what the delegate gives as the
/// channel records it (`follow`), its recipient's turn ending with its answer deferred
/// (`follow_deferred`, issue 591), and what the giver does with what came back (`follow_read`,
/// `follow_turn_end`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delegation {
    pub giver: ActorId,
    pub recipient: ActorId,
    pub initiator: ActorId,
    pub briefs: VecDeque<Brief>,
    /// Its giver's word on what came of the last brief (issue 598).
    pub word: Word,
}

/// A giver's word on what came of its delegation's last brief (issue 598): due once it is given
/// back with no brief open, and let go if it was none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Word {
    /// None is due: a brief is open, or the last brief's answer came with a further brief.
    NotDue,
    /// Due on `answer`, what came of the last brief, given back; `read` once the giver read it.
    Due { answer: MessageId, read: bool },
    /// It was none, or the giver left where the delegation was opened: let go, it takes no brief.
    LetGo,
}

/// A brief of a delegation: the message given to read - its sender the giver, its id its own -,
/// and where it stands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Brief {
    pub message: Message,
    pub stage: BriefStage,
}

/// Where a brief of a delegation stands (issues 557, 591).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BriefStage {
    /// Delegated, not given yet.
    Waiting,
    /// Given to its recipient at `at`.
    Given { at: Position },
    /// Given at `at`; the recipient's turn answering it ended with its answer deferred (issue 591,
    /// `deferred` of `TurnFinished`) - `told` once the giver was given back that it follows. What
    /// is given back after that is the answer.
    Deferred { at: Position, told: bool },
}

impl Brief {
    /// Where it was given, if it was.
    pub fn given_at(&self) -> Option<Position> {
        match self.stage {
            BriefStage::Waiting => None,
            BriefStage::Given { at } | BriefStage::Deferred { at, .. } => Some(at),
        }
    }
}

impl Delegation {
    /// Whether giving `recipient` `message` to read gives the open brief: to the recipient, the
    /// brief as it was delegated, not given yet.
    pub fn gives_brief(&self, recipient: ActorId, message: &Message) -> bool {
        recipient == self.recipient
            && self.briefs.front().is_some_and(|open| {
                open.stage == BriefStage::Waiting
                    && (open.message.id, open.message.sender_id) == (message.id, message.sender_id)
                    && open.message.content == message.content
            })
    }

    /// Whether giving `recipient` `message` to read gives back what came of the open brief: to
    /// the giver, of the recipient - an answer once the brief was given, that the recipient could
    /// not be asked or left if it was not.
    pub fn gives_back(&self, recipient: ActorId, message: &Message) -> bool {
        recipient == self.giver && message.sender_id == self.recipient && !self.briefs.is_empty()
    }

    /// Follows its delegate giving `recipient` `message` to read, as the channel records it at
    /// `at`: the open brief given there; what came of it given back, and the next brief open - or,
    /// if none is, the giver's word on it due (issue 598). What is given back of a brief whose
    /// answer was deferred, and of which the giver was not told yet, tells the giver that the
    /// answer follows (issue 591): the brief stays open, and no word is due. Whether it moved on.
    pub fn follow(&mut self, recipient: ActorId, message: &Message, at: Position) -> bool {
        if self.gives_brief(recipient, message) {
            if let Some(open) = self.briefs.front_mut() {
                open.stage = BriefStage::Given { at };
            }
            true
        } else if !self.gives_back(recipient, message) {
            false
        } else if let Some(BriefStage::Deferred { told: told @ false, .. }) =
            self.briefs.front_mut().map(|open| &mut open.stage)
        {
            *told = true;
            true
        } else if self.briefs.pop_front().is_some() {
            if self.briefs.is_empty() {
                self.word = Word::Due { answer: message.id, read: false };
            }
            true
        } else {
            false
        }
    }

    /// Follows a turn of `ender` ending with its answer deferred, as the channel records it after
    /// what it recorded before (issue 591): the open brief deferred, if `ender` is its recipient
    /// and it was given. Whether it moved on.
    pub(crate) fn follow_deferred(&mut self, ender: ActorId) -> bool {
        if ender != self.recipient {
            return false;
        }
        let Some(open) = self.briefs.front_mut() else { return false };
        let BriefStage::Given { at } = open.stage else { return false };
        open.stage = BriefStage::Deferred { at, told: false };
        true
    }

    /// Whether it was let go (issue 598): it takes no brief.
    pub fn is_let_go(&self) -> bool {
        self.word == Word::LetGo
    }

    /// Follows `reader` reading `message` (issue 598): the answer its giver's word is due on, read
    /// by the giver, once. Whether it moved on.
    pub(crate) fn follow_read(&mut self, reader: ActorId, message: MessageId) -> bool {
        let reads = reader == self.giver
            && matches!(self.word, Word::Due { answer, read: false } if answer == message);
        if reads {
            self.word = Word::Due { answer: message, read: true };
        }
        reads
    }

    /// Follows a turn of `ender` ending in the thread it was opened at, as the channel records it
    /// after what it recorded before (issue 598): let go if it is the giver's, ending once the
    /// giver read the answer its word was due on, with no brief given since - its word was none.
    /// Whether it was let go.
    pub(crate) fn follow_turn_end(&mut self, ender: ActorId) -> bool {
        let lets_go = ender == self.giver && matches!(self.word, Word::Due { read: true, .. });
        if lets_go {
            self.word = Word::LetGo;
        }
        lets_go
    }

    /// Follows `participant` leaving the thread it was opened at, alone or with the channel (issue
    /// 598): let go if it is the giver, to whom nothing comes back any more. Whether it was let go.
    pub(crate) fn follow_leave(&mut self, participant: ActorId) -> bool {
        let lets_go = participant == self.giver;
        if lets_go {
            self.word = Word::LetGo;
        }
        lets_go
    }

    /// Follows the channel refusing its delegate to give `recipient` `message` to read: what came
    /// of the open brief, which cannot reach its giver - gone from where the thread was opened -,
    /// is dropped with the brief, and the next brief is open. A brief refused stays open: its
    /// delegate gives back that the recipient could not be asked. Whether it moved on.
    pub(crate) fn refused(&mut self, recipient: ActorId, message: &Message) -> bool {
        self.gives_back(recipient, message) && self.briefs.pop_front().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentId;
    use crate::confidentiality::Classification;
    use crate::user::UserId;

    /// Issue 557: a delegation gives its open brief, once, to its recipient; then takes back what
    /// came of it, of its recipient to its giver, and opens the next brief - nothing else.
    #[test]
    fn a_delegation_follows_its_briefs_as_they_are_given_and_answered() {
        let giver = ActorId::User(UserId::new());
        let recipient = ActorId::Agent(AgentId::new());
        let brief = |text: &str| Brief {
            message: Message {
                id: MessageId::new(),
                sender_id: giver,
                content: Some(text.to_string()),
                ..Message::new(Classification::CHANNEL_DEFAULT)
            },
            stage: BriefStage::Waiting,
        };
        let (first, second) = (brief("first"), brief("second"));
        let briefs = VecDeque::from([first.clone(), second.clone()]);
        let mut delegation =
            Delegation { giver, recipient, initiator: giver, briefs, word: Word::NotDue };
        let answer = Message { sender_id: recipient, ..Message::new(Classification::PUBLIC) };

        assert!(delegation.gives_back(giver, &answer), "what came of the open brief, given or not");
        assert!(!delegation.gives_brief(recipient, &second.message), "the open brief first");
        assert!(!delegation.gives_brief(giver, &first.message), "to its recipient");
        let altered = Message { content: Some("other".to_string()), ..first.message.clone() };
        assert!(!delegation.gives_brief(recipient, &altered), "as it was delegated");
        assert!(delegation.gives_brief(recipient, &first.message));
        delegation.follow(recipient, &first.message, Position::new(3));
        assert_eq!(delegation.briefs[0].given_at(), Some(Position::new(3)), "given there");
        assert!(!delegation.gives_brief(recipient, &first.message), "once");

        assert!(!delegation.gives_back(recipient, &answer), "back to its giver");
        let anyones = Message { sender_id: giver, ..answer.clone() };
        assert!(!delegation.gives_back(giver, &anyones), "of its recipient");
        assert!(delegation.gives_back(giver, &answer));
        delegation.follow(giver, &answer, Position::new(5));
        assert_eq!(delegation.briefs.len(), 1, "answered");
        assert!(delegation.gives_brief(recipient, &second.message), "and the next one open");

        delegation.refused(recipient, &second.message);
        assert_eq!(delegation.briefs.len(), 1, "a brief refused stays open, to be given back");
        delegation.refused(giver, &answer);
        assert!(delegation.briefs.is_empty(), "what cannot reach its giver goes with its brief");
    }

    /// Issue 557: a thread takes the briefs of the one giver and recipient it is delegated by and
    /// to, behind those it holds, and no other's.
    #[test]
    fn a_thread_is_delegated_into_by_one_giver_to_one_recipient() {
        let giver = ActorId::User(UserId::new());
        let recipient = ActorId::Agent(AgentId::new());
        let briefed = |giver, recipient| Delegation {
            giver,
            recipient,
            initiator: giver,
            briefs: VecDeque::from([Brief {
                message: Message::new(Classification::CHANNEL_DEFAULT),
                stage: BriefStage::Waiting,
            }]),
            word: Word::NotDue,
        };
        let mut thread = ThreadState { opening: None, closed: false, delegation: None };
        assert!(thread.delegate(briefed(giver, recipient)), "the first");
        assert!(
            thread.delegate(briefed(giver, recipient)),
            "the same giver, to the same recipient"
        );
        let other = ActorId::User(UserId::new());
        assert!(!thread.delegate(briefed(other, recipient)), "not another giver");
        assert!(!thread.delegates_alike(giver, other), "nor to another recipient");
        assert_eq!(thread.delegation.map(|held| held.briefs.len()), Some(2), "behind each other");
    }

    /// Issue 598: once what came of its last brief is given back, a delegation waits for its
    /// giver's word: read by the giver, a turn of the giver's ending after lets it go - not before
    /// the reading, not one of another, not once a brief came since; let go, it takes no brief;
    /// its giver leaving lets it go too.
    #[test]
    fn a_delegation_waits_for_its_givers_word_on_what_came_back() {
        let giver = ActorId::Agent(AgentId::new());
        let recipient = ActorId::Agent(AgentId::new());
        let brief = || Brief {
            message: Message {
                id: MessageId::new(),
                sender_id: giver,
                ..Message::new(Classification::CHANNEL_DEFAULT)
            },
            stage: BriefStage::Waiting,
        };
        let delegated = |briefs: usize| Delegation {
            giver,
            recipient,
            initiator: giver,
            briefs: (0..briefs).map(|_| brief()).collect(),
            word: Word::NotDue,
        };
        let answer = || Message {
            id: MessageId::new(),
            sender_id: recipient,
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let at = Position::new;

        let mut two = delegated(2);
        two.follow(giver, &answer(), at(2));
        assert_eq!(two.word, Word::NotDue, "another brief open: no word due");

        let mut delegation = delegated(1);
        let back = answer();
        delegation.follow(giver, &back, at(4));
        let due = Word::Due { answer: back.id, read: false };
        assert_eq!(delegation.word, due, "its giver's word due");
        assert!(!delegation.follow_turn_end(giver), "not read yet");
        assert!(!delegation.follow_read(recipient, back.id), "read by another");
        assert!(!delegation.follow_read(giver, MessageId::new()), "another message");
        assert!(delegation.follow_read(giver, back.id), "read by its giver");
        assert!(!delegation.follow_read(giver, back.id), "once");
        assert!(!delegation.follow_turn_end(recipient), "a turn of another");
        assert!(delegation.follow_turn_end(giver), "its giver's turn after, without a word");
        assert!(delegation.is_let_go(), "let go");
        let mut thread = ThreadState { opening: None, closed: false, delegation: Some(delegation) };
        assert!(!thread.delegate(delegated(1)), "let go, it takes no brief");

        let mut asked = delegated(1);
        let back = answer();
        asked.follow(giver, &back, at(4));
        asked.follow_read(giver, back.id);
        let mut thread = ThreadState { opening: None, closed: false, delegation: Some(asked) };
        assert!(thread.delegate(delegated(1)), "a brief before the turn's end: its word");
        let held = thread.delegation.as_mut().expect("delegated");
        assert_eq!(held.word, Word::NotDue, "no word due any more");
        assert!(!held.follow_turn_end(giver), "and its turn's end lets nothing go");

        let mut left = delegated(0);
        assert!(!left.follow_leave(recipient), "another leaving");
        assert!(left.follow_leave(giver) && left.is_let_go(), "its giver leaving");
    }

    /// Issue 591: a brief given whose answer its recipient's turn deferred - not one of another -
    /// stays open as what is given back first tells its giver that the
    /// answer follows, and no word is due; what is given back next answers it.
    #[test]
    fn a_brief_whose_answer_was_deferred_is_answered_after_its_giver_was_told() {
        let giver = ActorId::Agent(AgentId::new());
        let recipient = ActorId::Agent(AgentId::new());
        let brief = Brief {
            message: Message {
                id: MessageId::new(),
                sender_id: giver,
                ..Message::new(Classification::CHANNEL_DEFAULT)
            },
            stage: BriefStage::Waiting,
        };
        let briefs = VecDeque::from([brief.clone()]);
        let mut delegation =
            Delegation { giver, recipient, initiator: giver, briefs, word: Word::NotDue };
        let back = || Message {
            id: MessageId::new(),
            sender_id: recipient,
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let at = Position::new;

        assert!(!delegation.follow_deferred(recipient), "not given yet");
        delegation.follow(recipient, &brief.message, at(3));
        assert!(!delegation.follow_deferred(giver), "a turn of another");
        assert!(delegation.follow_deferred(recipient), "its recipient's");
        let deferred = BriefStage::Deferred { at: at(3), told: false };
        assert_eq!(delegation.briefs[0].stage, deferred, "deferred, given where it was");
        assert!(!delegation.follow_deferred(recipient), "once");

        assert!(delegation.follow(giver, &back(), at(6)), "its giver told");
        assert_eq!(delegation.briefs[0].stage, BriefStage::Deferred { at: at(3), told: true });
        assert_eq!(delegation.word, Word::NotDue, "the brief open: no word due");
        assert_eq!(delegation.briefs[0].given_at(), Some(at(3)), "read on from where it was given");

        let answer = back();
        assert!(delegation.follow(giver, &answer, at(9)), "then the answer");
        assert!(delegation.briefs.is_empty(), "answered");
        assert_eq!(delegation.word, Word::Due { answer: answer.id, read: false }, "the word due");
    }

    /// Issue 557: a channel's line is named by the channel alone - the same for the same channel,
    /// another for another.
    #[test]
    fn the_line_of_a_channel_is_named_by_the_channel() {
        let (one, other) = (ChannelId::new(), ChannelId::new());
        assert_eq!(ThreadId::line(one), ThreadId::line(one), "the same channel, the same line");
        assert_ne!(ThreadId::line(one), ThreadId::line(other), "another channel, another line");
    }
}
