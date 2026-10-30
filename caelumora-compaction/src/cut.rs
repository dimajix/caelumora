//! Where an agent's transcript is cut for a compaction: the part replaced and the part kept.
//!
//! The cut lies before a message, never between a tool's call and its result (issue 284), never
//! into a turn given up because its channel was lowered below it, and within the turn under way
//! before a call of its model but the first. What was given in the part replaced and is not still
//! there in the part kept is what is given again; whose the replacement is and how it is classified
//! follows from what it replaces.

use caelumora_model::confidentiality::Classification;
use caelumora_model::context::KnowledgeSource;
use caelumora_model::digest::Digest;
use caelumora_model::message::{Message, MessageRole};
use caelumora_model::store::EntityKind;
use caelumora_model::transcript::{as_they_stand, Transcript, TranscriptEntry};
use caelumora_model::turn::GivenUpReason;
use caelumora_model::work::TurnId;
use std::collections::HashSet;
use uuid::Uuid;

/// What the agent was given: an entity of a kind (`KnowledgeSource::Entity`).
pub(crate) type Given = (EntityKind, Uuid);

/// An entry of the active path of a transcript, as a compaction reads it: its digest, what it gives
/// if it is one given (`TranscriptEntry::Given`), and its message as it stands, if it holds one.
type Entry = (Digest, Option<Given>, Option<Message>);

/// An agent's transcript cut for a compaction: the part replaced, and the part kept as it stands.
#[derive(Debug)]
pub(crate) struct Cut {
    replaced: Vec<Entry>,
    kept: Vec<Entry>,
    /// The last entry of the part replaced: up to it the part is read.
    until: Digest,
    /// The turn under way, for which it was cut.
    under_way: TurnId,
}

impl Cut {
    /// `transcript`'s active path cut so that at most `keep` tokens are kept as they stand, its
    /// entries standing in the context at `positions` (`Situation::token_positions`); `under_way`
    /// is the turn under way. The oldest cut allowed behind which at most `keep` tokens stand; the
    /// newest allowed if none is. `None` if there is none, or nothing before it to replace.
    pub(crate) fn of(
        transcript: &Transcript,
        positions: &[u64],
        keep: u64,
        under_way: TurnId,
    ) -> Option<Self> {
        let whole = kept_whole(transcript.entries().map(|node| &node.entry));
        let mut path: Vec<Entry> = as_they_stand(transcript.entries())
            .map(|(node, message)| {
                let gives = match &node.entry {
                    TranscriptEntry::Given { source: Some(source), .. } => given(source),
                    _ => None,
                };
                (node.digest, gives, message)
            })
            .collect();
        let at = cut_at(&path, positions, keep, under_way, &whole)?;
        let kept = path.split_off(at);
        let (until, _, _) = path.last()?;
        Some(Self { until: *until, under_way, replaced: path, kept })
    }

    /// The last entry of the part replaced.
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

    /// How many messages the part kept holds.
    pub(crate) fn messages_kept(&self) -> usize {
        messages(&self.kept)
    }

    /// What was given in the part replaced and is not still there, as it was given, in the part
    /// kept: each once, in the order it was first given.
    pub(crate) fn wanted(&self) -> Vec<Given> {
        let still_there = given_in(&self.kept);
        given_in(&self.replaced).into_iter().filter(|g| !still_there.contains(g)).collect()
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
fn given(source: &KnowledgeSource) -> Option<Given> {
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
/// `keep` tokens stand; the newest if none is. `None` if there is none, or nothing before it to
/// replace.
fn cut_at(
    path: &[Entry],
    positions: &[u64],
    keep: u64,
    under_way: TurnId,
    whole: &HashSet<TurnId>,
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
    let cut = candidates
        .iter()
        .copied()
        .find(|&at| end.saturating_sub(positions.get(at).copied().unwrap_or(end)) <= keep)
        .or(candidates.last().copied())?;
    path[..cut].iter().any(|(_, _, message)| message.is_some()).then_some(cut)
}

/// The turns of `entries` a compaction must not cut apart: those given up because the channel was
/// lowered below them - its model is given only the tools they called, which the summary of a part
/// of one without its end would not know (issues 141, 258).
fn kept_whole<'a>(entries: impl Iterator<Item = &'a TranscriptEntry>) -> HashSet<TurnId> {
    entries
        .filter_map(|entry| match entry {
            TranscriptEntry::TurnGivenUp {
                turn_id, reason: GivenUpReason::Lowered { .. }, ..
            } => Some(*turn_id),
            _ => None,
        })
        .collect()
}

/// What was given in `entries`, each once, in the order it was first given
/// (`TranscriptEntry::Given`): a skill a user gave and one the model loaded alike (issue 389).
fn given_in(entries: &[Entry]) -> Vec<Given> {
    let mut given = Vec::new();
    for what in entries.iter().filter_map(|(_, gives, _)| gives.as_ref()) {
        if !given.contains(what) {
            given.push(*what);
        }
    }
    given
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::digest;
    use chrono::Utc;

    fn entry(content: &str, role: MessageRole, turn: Option<TurnId>) -> Entry {
        let message = Message {
            content: Some(content.to_string()),
            role,
            turn_id: turn,
            ..Message::new(Classification::PUBLIC)
        };
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
        assert_eq!(
            cut_at(&path, &positions, 10, under_way, &none),
            Some(4),
            "the newest part that fits"
        );
        assert_eq!(
            cut_at(&path, &positions, 45, under_way, &none),
            Some(4),
            "never before a result, which would part it from its call"
        );
        assert_eq!(
            cut_at(&path, &positions, 50, under_way, &none),
            Some(1),
            "a finished turn may be cut apart"
        );
        assert_eq!(
            cut_at(&path, &positions, 5, under_way, &none),
            Some(4),
            "the newest if nothing fits"
        );
        assert_eq!(
            cut_at(&path, &positions, 60, under_way, &none),
            None,
            "nothing to summarise if everything fits"
        );
        assert_eq!(
            cut_at(&path, &positions, 50, under_way, &HashSet::from([earlier])),
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
        assert_eq!(
            cut_at(&path, &positions, 60, under_way, &none),
            Some(3),
            "before its second call"
        );
        assert_eq!(
            cut_at(&path, &positions, 85, under_way, &none),
            Some(3),
            "not before its first: its input would go and the turn stay"
        );
        assert_eq!(
            cut_at(&path, &positions, 10, under_way, &none),
            Some(5),
            "its newest call and results stay"
        );
        assert_eq!(cut_at(&path, &positions, 100, under_way, &none), None);
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
