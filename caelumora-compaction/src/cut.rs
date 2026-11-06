//! Where an agent's transcript is cut for a compaction: the part replaced and the part kept.
//!
//! The cut lies before a message, never between a tool's call and its result (issue 284), never
//! into a turn given up because its channel was lowered below it, and within the turn under way
//! before a call of its model but the first. The prompt parts given into the transcript that stand
//! before it are not replaced but carried over (issue 537): they count as kept. What was given in
//! the part replaced and is not still there in the part kept is what is given again, as whoever
//! first gave it; whose the replacement is and how it is classified follows from what it replaces.

use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::KnowledgeSource;
use caelumora_domain::digest::Digest;
use caelumora_domain::message::{Message, MessageRole};
use caelumora_domain::store::EntityKind;
use caelumora_domain::transcript::{as_they_stand, Transcript, TranscriptEntry};
use caelumora_domain::work::TurnId;
use std::collections::HashSet;
use uuid::Uuid;

/// What the agent was given: an entity of a kind (`KnowledgeSource::Entity`).
pub(crate) type Given = (EntityKind, Uuid);

/// An entry of the active path of a transcript, as a compaction reads it: its digest, what it gives
/// if it is one given (`TranscriptEntry::Given`), and its message as it stands, if it holds one.
type Entry = (Digest, Option<Given>, Option<Message>);

/// An agent's transcript cut for a compaction: the part replaced, without the prompt parts carried
/// over, and the part kept as it stands.
#[derive(Debug)]
pub(crate) struct Cut {
    replaced: Vec<Entry>,
    kept: Vec<Entry>,
    /// How many prompt parts before the cut are carried over (`Transcript::carried_parts`).
    carried: usize,
    /// The last entry before the cut: up to it the part replaced is read.
    until: Digest,
    /// The turn under way, for which it was cut.
    under_way: TurnId,
}

impl Cut {
    /// `transcript`'s active path cut so that at most `keep` tokens are kept as they stand, its
    /// entries standing in the context at `positions` (`Situation::token_positions`); `under_way`
    /// is the turn under way. The oldest cut allowed behind which at most `keep` tokens stand, the
    /// prompt parts carried over counted with them; the newest allowed if none is. `None` if there
    /// is none, or nothing before it to replace.
    pub(crate) fn of(
        transcript: &Transcript,
        positions: &[u64],
        keep: u64,
        under_way: TurnId,
    ) -> Option<Self> {
        let whole = kept_whole(transcript.entries().map(|node| &node.entry));
        let carried = transcript.carried_parts();
        let mut path: Vec<Entry> = as_they_stand(transcript.entries())
            .map(|(node, message)| (node.digest, node.entry.knowledge().and_then(given), message))
            .collect();
        let at = cut_at(&path, positions, keep, under_way, &whole, &carried)?;
        let kept = path.split_off(at);
        let (until, _, _) = *path.last()?;
        path.retain(|(digest, _, _)| !carried.contains(digest));
        let carried = at - path.len();
        Some(Self { until, under_way, replaced: path, kept, carried })
    }

    /// The last entry before the cut, up to which the part replaced is read: a part carried over,
    /// maybe, which what is read of it leaves out (`ContextResolver::conversation_until`).
    pub(crate) fn until(&self) -> Digest {
        self.until
    }

    /// The first entry kept; `None` if nothing is.
    pub(crate) fn keep_from(&self) -> Option<Digest> {
        self.kept.first().map(|(digest, _, _)| *digest)
    }

    /// How many messages the part replaced holds.
    pub(crate) fn messages_replaced(&self) -> usize {
        messages(&self.replaced)
    }

    /// How many messages stay as they stand: those of the part kept, and the prompt parts carried
    /// over from before it.
    pub(crate) fn messages_kept(&self) -> usize {
        messages(&self.kept) + self.carried
    }

    /// What was given in the part replaced and is not still there, as it was given, in the part
    /// kept: each once, in the order it was first given, with the message it was first given in -
    /// whose sender gave it (issue 539). Read as the entries stand: a gift taken back is neither
    /// given again nor still there (issue 234).
    pub(crate) fn wanted(&self) -> Vec<(Given, &Message)> {
        let still_there: Vec<Given> = given_in(&self.kept).into_iter().map(|(g, _)| g).collect();
        given_in(&self.replaced).into_iter().filter(|(g, _)| !still_there.contains(g)).collect()
    }

    /// Whose the replacement is and how it is classified. One that holds messages of the turn under
    /// way holds what the turn read: it is the turn's, classified as its `label`, and goes with the
    /// turn if it is given up as `Lowered`. One of what precedes the turn alone is no turn's,
    /// classified by what it replaces - `label` where that has no audience in common -, since the
    /// call writing it was given the transcript alone (issue 284).
    pub(crate) fn replaced_as(&self, label: Classification) -> (Option<TurnId>, Classification) {
        let under_way = Some(self.under_way);
        let of_turn = self
            .replaced
            .iter()
            .any(|(_, _, message)| message.as_ref().is_some_and(|m| m.turn_id == under_way));
        if of_turn {
            return (under_way, label);
        }
        let replaced = self
            .replaced
            .iter()
            .filter_map(|(_, _, message)| message.as_ref().map(|m| &m.classification));
        (None, Classification::join_all(replaced).unwrap_or(label))
    }
}

/// How many messages `entries` hold.
fn messages(entries: &[Entry]) -> usize {
    entries.iter().filter(|(_, _, m)| m.is_some()).count()
}

/// What `source` gave, if it is knowledge of an entity.
pub(crate) fn given(source: &KnowledgeSource) -> Option<Given> {
    match *source {
        KnowledgeSource::Entity { kind, id, .. } => Some((kind, id)),
        _ => None,
    }
}

/// Where the entries `path` are cut, standing in the context at `positions`: the index of the first
/// entry kept. Before a message only, so that a tool's result stays with its call: within the turn
/// under way `under_way` before one of its model's messages but the first - before that, its input
/// would be summarised and the turn kept -, before it before any message that is no tool's result,
/// up to the turn's input, and none in a turn of `whole`. The oldest of them behind which at most
/// `keep` tokens stand, those of the entries `carried` before it counted with them - the prompt
/// parts a compaction carries over -; the newest if none is. `None` if there is none, or nothing
/// before it to replace but what is carried.
fn cut_at(
    path: &[Entry],
    positions: &[u64],
    keep: u64,
    under_way: TurnId,
    whole: &HashSet<TurnId>,
    carried: &HashSet<Digest>,
) -> Option<usize> {
    let end = positions.last().copied().unwrap_or(0);
    let of_turn = |m: &Message| m.turn_id == Some(under_way);
    let turn_start = path.iter().position(|(_, _, m)| m.as_ref().is_some_and(of_turn));
    let first_call = path.iter().position(|(_, _, m)| {
        m.as_ref().is_some_and(|m| of_turn(m) && m.role == MessageRole::Assistant)
    });
    let candidates: Vec<usize> = path
        .iter()
        .enumerate()
        .filter(|(at, (_, _, message))| match message {
            None => false,
            Some(m) if of_turn(m) => m.role == MessageRole::Assistant && Some(*at) != first_call,
            Some(m) => {
                m.role != MessageRole::Tool
                    && turn_start.is_none_or(|start| *at < start)
                    && m.turn_id.is_none_or(|turn| !whole.contains(&turn))
            }
        })
        .map(|(at, _)| at)
        .collect();
    let position = |at: usize| positions.get(at).copied().unwrap_or(end);
    // How many tokens the entries carried before each entry hold, which stay.
    let mut carried_before = vec![0; path.len() + 1];
    for (at, (digest, _, _)) in path.iter().enumerate() {
        let size = if carried.contains(digest) {
            position(at + 1).saturating_sub(position(at))
        } else {
            0
        };
        carried_before[at + 1] = carried_before[at] + size;
    }
    let cut = candidates
        .iter()
        .copied()
        .find(|&at| end.saturating_sub(position(at)) + carried_before[at] <= keep)
        .or(candidates.last().copied())?;
    let replaced = |(digest, _, message): &Entry| message.is_some() && !carried.contains(digest);
    path[..cut].iter().any(replaced).then_some(cut)
}

/// The turns of `entries` a compaction must not cut apart: those given up because the channel was
/// lowered below them - its model is given only the tools they called, which the summary of a part
/// of one without its end would not know (issues 141, 258).
fn kept_whole<'a>(entries: impl Iterator<Item = &'a TranscriptEntry>) -> HashSet<TurnId> {
    entries.filter_map(TranscriptEntry::lowered_turn).map(|(turn, _)| turn).collect()
}

/// What was given in `entries`, each once, in the order it was first given
/// (`TranscriptEntry::Given`), with the message it was first given in: a skill a user gave and one
/// the model loaded alike (issue 389). A gift taken back holds no message any more, and gives
/// nothing.
fn given_in(entries: &[Entry]) -> Vec<(Given, &Message)> {
    let mut given: Vec<(Given, &Message)> = Vec::new();
    for (what, message) in entries.iter().filter_map(|(_, gives, m)| gives.zip(m.as_ref())) {
        if !given.iter().any(|(g, _)| *g == what) {
            given.push((what, message));
        }
    }
    given
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::confidentiality::Confidentiality;
    use caelumora_domain::digest;
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::message::MessageId;
    use caelumora_domain::transcript::GivenSource;
    use caelumora_domain::turn::GivenUpReason;
    use caelumora_domain::user::UserId;
    use caelumora_domain::workspace::{Workspace, WorkspaceId};
    use chrono::Utc;

    /// A message saying `content` in `role`, of the turn `turn` if any, classified
    /// `classification`.
    fn message(
        content: &str,
        role: MessageRole,
        turn: Option<TurnId>,
        classification: Classification,
    ) -> Message {
        Message {
            content: Some(content.to_string()),
            role,
            turn_id: turn,
            classification,
            ..make_message(MessageId::new())
        }
    }

    /// The entry of a public message saying `content`, as `message` makes it.
    fn entry(content: &str, role: MessageRole, turn: Option<TurnId>) -> Entry {
        let message = message(content, role, turn, Classification::PUBLIC);
        (digest::content(content.as_bytes()), None, Some(message))
    }

    fn end(turn: &str) -> Entry {
        (digest::content(turn.as_bytes()), None, None)
    }

    #[test]
    fn the_cut_lies_before_a_message_and_keeps_a_result_with_its_call() {
        let (earlier, under_way) = (TurnId::new(), TurnId::new());
        let path = vec![
            entry("u1", MessageRole::User, None),
            entry("call", MessageRole::Assistant, Some(earlier)),
            entry("result", MessageRole::Tool, Some(earlier)),
            end("end"),
            entry("u2", MessageRole::User, None),
        ];
        let positions = [0, 10, 20, 50, 50, 60];
        let none = HashSet::new();
        let no_parts = HashSet::new();
        assert_eq!(
            cut_at(&path, &positions, 10, under_way, &none, &no_parts),
            Some(4),
            "the newest part that fits"
        );
        assert_eq!(
            cut_at(&path, &positions, 45, under_way, &none, &no_parts),
            Some(4),
            "never before a result, which would part it from its call"
        );
        assert_eq!(
            cut_at(&path, &positions, 50, under_way, &none, &no_parts),
            Some(1),
            "a finished turn may be cut apart"
        );
        assert_eq!(
            cut_at(&path, &positions, 5, under_way, &none, &no_parts),
            Some(4),
            "the newest if nothing fits"
        );
        assert_eq!(
            cut_at(&path, &positions, 60, under_way, &none, &no_parts),
            None,
            "nothing to summarise if everything fits"
        );
        assert_eq!(
            cut_at(&path, &positions, 50, under_way, &HashSet::from([earlier]), &no_parts),
            Some(4),
            "not into a turn kept whole"
        );
    }

    #[test]
    fn within_the_turn_under_way_the_cut_lies_before_a_call_of_its_model_but_the_first() {
        let under_way = TurnId::new();
        let path = vec![
            entry("u1", MessageRole::User, None),
            entry("a1", MessageRole::Assistant, Some(under_way)),
            entry("r1", MessageRole::Tool, Some(under_way)),
            entry("a2", MessageRole::Assistant, Some(under_way)),
            entry("r2", MessageRole::Tool, Some(under_way)),
            entry("a3", MessageRole::Assistant, Some(under_way)),
            entry("r3", MessageRole::Tool, Some(under_way)),
        ];
        let positions = [0, 10, 20, 40, 50, 70, 80, 100];
        let none = HashSet::new();
        let no_parts = HashSet::new();
        assert_eq!(
            cut_at(&path, &positions, 60, under_way, &none, &no_parts),
            Some(3),
            "before its second call"
        );
        assert_eq!(
            cut_at(&path, &positions, 85, under_way, &none, &no_parts),
            Some(3),
            "not before its first: its input would go and the turn stay"
        );
        assert_eq!(
            cut_at(&path, &positions, 10, under_way, &none, &no_parts),
            Some(5),
            "its newest call and results stay"
        );
        assert_eq!(cut_at(&path, &positions, 100, under_way, &none, &no_parts), None);
    }

    /// Issue 537: a prompt part given into the transcript that stands before the cut is carried
    /// over, so its tokens stay, and it is nothing to replace.
    #[test]
    fn a_part_carried_over_counts_as_kept() {
        let under_way = TurnId::new();
        let part = entry("[Rules]", MessageRole::System, None);
        let carried = HashSet::from([part.0]);
        let none = HashSet::new();
        let path = vec![
            part.clone(),
            entry("u1", MessageRole::User, None),
            entry("u2", MessageRole::User, None),
            entry("u3", MessageRole::User, None),
        ];
        let positions = [0, 20, 30, 40, 50];
        assert_eq!(
            cut_at(&path, &positions, 40, under_way, &none, &carried),
            Some(2),
            "what is carried stays beside what is kept"
        );
        assert_eq!(
            cut_at(&path, &positions, 40, under_way, &none, &HashSet::new()),
            Some(1),
            "without the part carried, the cut lies earlier"
        );
        let path = vec![part, entry("u1", MessageRole::User, None)];
        assert_eq!(
            cut_at(&path, &[0, 20, 30], 10, under_way, &none, &carried),
            None,
            "nothing to summarise but what is carried"
        );
    }

    /// Issue 537: what a compaction replaces holds no part carried over - it is not counted as
    /// replaced, nor classifies the replacement.
    #[test]
    fn a_part_carried_over_is_neither_counted_nor_classified_as_replaced() {
        let internal = Classification::general(Confidentiality::Internal);
        let mut transcript = Transcript::default();
        transcript.give(
            message("[Rules]", MessageRole::System, None, internal),
            Some(GivenSource::Part("rules".to_string())),
        );
        transcript.push(message("u1", MessageRole::User, None, Classification::PUBLIC));
        transcript.push(message("u2", MessageRole::User, None, Classification::PUBLIC));

        let cut = Cut::of(&transcript, &[0, 10, 20, 30], 10, TurnId::new()).expect("cut");
        assert_eq!(
            (cut.messages_replaced(), cut.messages_kept()),
            (1, 2),
            "the part not replaced, but kept as it stands"
        );
        assert_eq!(
            cut.replaced_as(Classification::PUBLIC),
            (None, Classification::PUBLIC),
            "classified by what it replaces, without the part"
        );
    }

    /// Issue 539: what is given again is wanted with the message it was first given in - a skill
    /// alice gave and the model loaded again is alice's -, each once, and only if the part kept
    /// does not hold it.
    #[test]
    fn what_is_wanted_is_named_with_the_message_it_was_first_given_in() {
        let (release_notes, review) = (WorkspaceId::new(), WorkspaceId::new());
        let skill = |id| {
            let source = KnowledgeSource::entity::<Workspace>(id, None);
            Some(GivenSource::Knowledge(source))
        };
        let given = |by: ActorId, name: &str, id| {
            let message = Message {
                sender_id: by,
                sender_name: name.to_string(),
                ..message("[Skill]", MessageRole::System, None, Classification::PUBLIC)
            };
            (message, skill(id))
        };
        let (alice, agent) = (ActorId::User(UserId::new()), ActorId::Agent(AgentId::new()));
        let mut transcript = Transcript::default();
        for (gift, source) in [
            given(alice, "alice", release_notes),
            given(agent, "Helper", release_notes),
            given(agent, "Helper", review),
        ] {
            transcript.give(gift, source);
        }
        transcript.push(message("u1", MessageRole::User, None, Classification::PUBLIC));
        let (gift, source) = given(agent, "Helper", review);
        transcript.give(gift, source);
        transcript.push(message("u2", MessageRole::User, None, Classification::PUBLIC));

        let cut = Cut::of(&transcript, &[0, 10, 20, 30, 40, 50, 60], 20, TurnId::new());
        let wanted = cut.as_ref().expect("cut").wanted();
        let wanted: Vec<_> = wanted.iter().map(|((_, id), m)| (*id, m.sender_id)).collect();
        assert_eq!(
            wanted,
            [(Uuid::from(release_notes), alice)],
            "alice's, once; not what the part kept holds"
        );
    }

    /// Issue 234: a gift taken back is read as it stands - the model is no longer given it -, so it
    /// is not given again, nor does it stand for a gift in the part replaced.
    #[test]
    fn a_gift_taken_back_is_neither_wanted_nor_still_there() {
        let (release_notes, review) = (WorkspaceId::new(), WorkspaceId::new());
        let given = |id, sender_name: &str| {
            let message = Message {
                sender_name: sender_name.to_string(),
                ..message("[Skill]", MessageRole::System, None, Classification::PUBLIC)
            };
            let source =
                Some(GivenSource::Knowledge(KnowledgeSource::entity::<Workspace>(id, None)));
            (message, source)
        };
        let said = |text| message(text, MessageRole::User, None, Classification::PUBLIC);
        let mut transcript = Transcript::default();
        let mut taken_back = Vec::new();
        for (id, sender_name, back) in [(release_notes, "alice", true), (review, "alice", false)] {
            let (gift, source) = given(id, sender_name);
            taken_back.extend(back.then_some(gift.id));
            transcript.give(gift, source);
        }
        transcript.push(said("u1"));
        transcript.push(said("u2"));
        let (gift, source) = given(review, "Helper");
        taken_back.push(gift.id);
        transcript.give(gift, source);
        transcript.push(said("u3"));
        for id in taken_back {
            assert!(transcript.revoke(id, Utc::now()));
        }

        let positions = [0, 10, 20, 30, 40, 50, 60, 60, 60];
        let cut = Cut::of(&transcript, &positions, 30, TurnId::new()).expect("cut");
        assert_eq!(cut.keep_from(), Some(transcript.entries().nth(3).unwrap().digest), "before u2");
        let wanted: Vec<_> =
            cut.wanted().iter().map(|((_, id), m)| (*id, m.sender_name.clone())).collect();
        assert_eq!(
            wanted,
            [(Uuid::from(review), "alice".to_string())],
            "not the one taken back; the one taken back in the part kept does not stand for it"
        );
    }

    #[test]
    fn a_turn_given_up_for_a_lowered_channel_is_kept_whole() {
        let (lowered, failed) = (TurnId::new(), TurnId::new());
        let given_up =
            |turn_id, reason| TranscriptEntry::TurnGivenUp { turn_id, reason, at: Utc::now() };
        let entries = [
            given_up(lowered, GivenUpReason::Lowered { channel: Classification::PUBLIC }),
            given_up(failed, GivenUpReason::Left),
            TranscriptEntry::TurnFinished { turn_id: TurnId::new(), at: Utc::now() },
        ];
        assert_eq!(kept_whole(entries.iter()), HashSet::from([lowered]));
    }
}
