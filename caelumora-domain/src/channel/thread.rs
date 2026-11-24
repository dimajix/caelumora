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

    /// Takes `delegation` in: its briefs behind those of the delegation held, or as its delegation
    /// if it has none. `false`, and nothing taken, if it is delegated otherwise
    /// (`delegates_alike`).
    pub fn delegate(&mut self, delegation: Delegation) -> bool {
        if !self.delegates_alike(delegation.giver, delegation.recipient) {
            return false;
        }
        match &mut self.delegation {
            Some(held) => held.briefs.extend(delegation.briefs),
            None => self.delegation = Some(delegation),
        }
        true
    }
}

/// What was delegated into a thread (issue 557): who gives, who works on it, the root of the chain
/// that delegated - what both directions carry as their initiator -, and the briefs, the first the
/// one open. The thread's delegate carries it out; the channel alone writes it, following what
/// the delegate gives as the channel records it (`follow`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delegation {
    pub giver: ActorId,
    pub recipient: ActorId,
    pub initiator: ActorId,
    pub briefs: VecDeque<Brief>,
}

/// A brief of a delegation: the message given to read - its sender the giver, its id its own -,
/// and where it was given; `None` while it waits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Brief {
    pub message: Message,
    pub given_at: Option<Position>,
}

impl Delegation {
    /// Whether giving `recipient` `message` to read gives the open brief: to the recipient, the
    /// brief as it was delegated, not given yet.
    pub fn gives_brief(&self, recipient: ActorId, message: &Message) -> bool {
        recipient == self.recipient
            && self.briefs.front().is_some_and(|open| {
                open.given_at.is_none()
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
    /// `at`: the open brief given there; what came of it given back, and the next brief open.
    /// Whether it moved on.
    pub fn follow(&mut self, recipient: ActorId, message: &Message, at: Position) -> bool {
        if self.gives_brief(recipient, message) {
            if let Some(open) = self.briefs.front_mut() {
                open.given_at = Some(at);
            }
            true
        } else {
            self.gives_back(recipient, message) && self.briefs.pop_front().is_some()
        }
    }

    /// Follows the channel refusing its delegate to give `recipient` `message` to read: what came
    /// of the open brief, which cannot reach its giver - gone from where the thread was opened -,
    /// is dropped with the brief, and the next brief is open. A brief refused stays open: its
    /// delegate gives back that the recipient could not be asked. Whether it moved on.
    pub fn refused(&mut self, recipient: ActorId, message: &Message) -> bool {
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
            given_at: None,
        };
        let (first, second) = (brief("first"), brief("second"));
        let briefs = VecDeque::from([first.clone(), second.clone()]);
        let mut delegation = Delegation { giver, recipient, initiator: giver, briefs };
        let answer = Message { sender_id: recipient, ..Message::new(Classification::PUBLIC) };

        assert!(delegation.gives_back(giver, &answer), "what came of the open brief, given or not");
        assert!(!delegation.gives_brief(recipient, &second.message), "the open brief first");
        assert!(!delegation.gives_brief(giver, &first.message), "to its recipient");
        let altered = Message { content: Some("other".to_string()), ..first.message.clone() };
        assert!(!delegation.gives_brief(recipient, &altered), "as it was delegated");
        assert!(delegation.gives_brief(recipient, &first.message));
        delegation.follow(recipient, &first.message, Position::new(3));
        assert_eq!(delegation.briefs[0].given_at, Some(Position::new(3)), "given there");
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
                given_at: None,
            }]),
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

    /// Issue 557: a channel's line is named by the channel alone - the same for the same channel,
    /// another for another.
    #[test]
    fn the_line_of_a_channel_is_named_by_the_channel() {
        let (one, other) = (ChannelId::new(), ChannelId::new());
        assert_eq!(ThreadId::line(one), ThreadId::line(one), "the same channel, the same line");
        assert_ne!(ThreadId::line(one), ThreadId::line(other), "another channel, another line");
    }
}
