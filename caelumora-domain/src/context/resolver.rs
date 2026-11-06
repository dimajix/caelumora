//! How an agent's data in a channel is resolved into what a receiver is given (issues 47, 387,
//! `ContextResolver`): the context a reference names, for its model's next call as for a message
//! produced earlier, the conversation alone for a model to work on, and the transcript's messages
//! for a remote agent that keeps its own conversation.

use super::{ContextItem, ContextRef, FrameRef, ModelProfile, PromptAt, SenderStyle};
use crate::actor::ActorId;
use crate::agent::AgentChannelState;
use crate::chain::Node;
use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::kept::Kept;
use crate::message::{Message, MessageId, MessageRole};
use crate::prompt_catalogue::{context, PromptCatalogue, PromptError, PromptTexts, KERNEL};
use crate::tool::ToolSet;
use crate::transcript::{as_they_stand, Transcript, TranscriptEntry};
use crate::turn::GivenUpReason;
use crate::value::time_zone::TimeZone;
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

/// A context resolved (issues 06, 47): what the model of an agent was given
/// for a message, in the order it was given it, and the tools it was offered
/// (issue 137), with the reference it was resolved from (issue 119).
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    /// What the history recorded for the message.
    pub reference: ContextRef,
    pub items: Vec<ContextItem>,
    /// The tools offered, in their sets (issue 194).
    pub tool_sets: Vec<ToolSet>,
}

/// Resolves an agent's data in a channel - its transcript, the texts of its
/// prompt parts and the sets of tools it was offered - into what a receiver
/// is given: its model, the context a reference names; the remote agent of
/// an external agent, the transcript's messages (issue 529). One
/// implementation for the model call, for telling afterwards what the model
/// saw, for measuring how much of its context it fills (issue 284), for the
/// conversation a model works on - a summary of it (issue 537) -, and for the
/// remote agent.
#[derive(Debug, Clone, Copy)]
pub struct ContextResolver<'a> {
    transcript: TranscriptSource<'a>,
    prompts: &'a Kept<String>,
    offered_tools: &'a Kept<Vec<ToolSet>>,
    /// The texts of the notices it gives the model (issue 57), each worded
    /// in the variant of the context it resolves.
    catalogue: &'a Arc<PromptCatalogue>,
}

/// What a `ContextResolver` reads the transcript a reference names from
/// (issue 380): the part of it an agent's state holds - its active branch -,
/// or a path loaded from the store.
#[derive(Debug, Clone, Copy)]
enum TranscriptSource<'a> {
    Held(&'a Transcript),
    Path(&'a [Node<TranscriptEntry>]),
}

impl<'a> TranscriptSource<'a> {
    /// All of its entries.
    fn entries(self) -> Vec<&'a Node<TranscriptEntry>> {
        match self {
            TranscriptSource::Held(transcript) => transcript.entries().collect(),
            TranscriptSource::Path(path) => path.iter().collect(),
        }
    }

    /// The entries of the path `digest` names, up to and including its
    /// entry; `None` if it holds no such entry.
    fn prefix(self, digest: &Digest) -> Option<Vec<&'a Node<TranscriptEntry>>> {
        match self {
            TranscriptSource::Held(transcript) => transcript.prefix(digest),
            TranscriptSource::Path(path) => path
                .iter()
                .position(|node| node.digest == *digest)
                .map(|at| path[..=at].iter().collect()),
        }
    }
}

/// What the model is given of a transcript, entry by entry, and of the frame
/// around it.
struct GivenPath<'a> {
    /// What it is given before the first entry: the parts at `PromptAt::Start`.
    leading: Vec<ContextItem>,
    /// For each entry what it is given of it - its message, where a turn
    /// given up ended, the prompt parts placed after it.
    entries: Vec<(&'a Node<TranscriptEntry>, Vec<ContextItem>)>,
    /// What it is given after everything: the parts at `PromptAt::End`.
    trailing: Vec<ContextItem>,
}

impl GivenPath<'_> {
    /// All of it, in the order the model is given it, without the entries
    /// `left_out` leaves out.
    fn items(self, left_out: impl Fn(&TranscriptEntry) -> bool) -> Vec<ContextItem> {
        let mut items = self.leading;
        let entries = self.entries.into_iter().filter(|(node, _)| !left_out(&node.entry));
        items.extend(entries.flat_map(|(_, given)| given));
        items.extend(self.trailing);
        items
    }
}

impl<'a> ContextResolver<'a> {
    /// The resolver over the data of `state`, its transcript as the state
    /// holds it: what the next call of its model and how full its context
    /// is are resolved against - the head of its active branch. Its
    /// notices are worded by `catalogue`.
    pub fn of(state: &'a AgentChannelState, catalogue: &'a Arc<PromptCatalogue>) -> Self {
        Self {
            transcript: TranscriptSource::Held(&state.transcript),
            prompts: &state.prompts,
            offered_tools: &state.offered_tools,
            catalogue,
        }
    }

    /// The resolver over what was loaded from the store for an older
    /// message's context - which may lie on a branch the state does not
    /// hold, and name texts it no longer holds -: the path `path` of its
    /// transcript (`TranscriptStore::path`, issue 380), the `prompts` and
    /// the `offered_tools` it names (`TranscriptStore::named_by`, issue
    /// 383).
    /// Its notices are worded by `catalogue`.
    pub fn on_path(
        path: &'a [Node<TranscriptEntry>],
        prompts: &'a Kept<String>,
        offered_tools: &'a Kept<Vec<ToolSet>>,
        catalogue: &'a Arc<PromptCatalogue>,
    ) -> Self {
        Self { transcript: TranscriptSource::Path(path), prompts, offered_tools, catalogue }
    }

    /// What the model of `agent` is given, or was given, for the message
    /// `reference` is for, produced in the turn `turn` - the next one of the
    /// turn under way (`Work::context` of its `TurnState`) or one recorded in
    /// the history: the transcript it names, whose messages of `turn` are
    /// the turn's own (`Message::turn_id`, issue 241), with the prompt parts
    /// where they were placed, and the tools it was offered. `None` if what
    /// it names is not here - a transcript that is not a prefix of this
    /// one, a prompt or a set of tools that was never recorded, or a prompt
    /// whose entry is not in the prefix.
    ///
    /// # Errors
    /// `PromptError` if the text of a notice cannot be had.
    pub fn resolve(
        &self,
        reference: &ContextRef,
        turn: TurnId,
        agent: ActorId,
    ) -> Result<Option<ResolvedContext>, PromptError> {
        let Some(given) = self.given_path(reference, turn, agent)? else {
            return Ok(None);
        };
        // The system prompt: the parts at the start in the role of one - a part placed there for
        // want of an entry to follow is none of it.
        let system_prompt =
            given.leading.iter().take_while(|part| part.role() == MessageRole::System).count();
        let mut items = given.items(|_| false);
        reference.frame.profile.place_system_messages(&mut items[system_prompt..]);
        let tool_sets = match &reference.frame.tools {
            Some(digest) => match self.offered_tools.get(digest) {
                Some(sets) => sets.clone(),
                None => return Ok(None),
            },
            None => Vec::new(),
        };
        Ok(Some(ResolvedContext { reference: reference.clone(), items, tool_sets }))
    }

    /// The transcript up to and including the entry `last`, as the model of `agent` is given it in
    /// the turn `turn` - each message framed by `profile` at `time_zone` -, without any prompt
    /// part: none of a frame, and none given into the transcript (`TranscriptEntry::part_of`,
    /// issue 537), of which a compaction keeps those that still stand. What a model is handed to
    /// work on the conversation itself, a summary say (issue 16). `None` if the transcript holds
    /// no entry `last`.
    ///
    /// # Errors
    /// `PromptError` if the text of a notice cannot be had.
    pub fn conversation_until(
        &self,
        last: Digest,
        profile: ModelProfile,
        time_zone: TimeZone,
        turn: TurnId,
        agent: ActorId,
    ) -> Result<Option<Vec<ContextItem>>, PromptError> {
        let reference = ContextRef {
            transcript: Some(last),
            frame: FrameRef { profile, time_zone, ..FrameRef::default() },
        };
        let Some(given) = self.given_path(&reference, turn, agent)? else {
            return Ok(None);
        };
        let mut items = given.items(|entry| entry.part_of().is_some());
        // No system prompt: every message is the conversation's.
        profile.place_system_messages(&mut items);
        Ok(Some(items))
    }

    /// Where each entry of the transcript `reference` names stands in what
    /// the model of `agent` is given for it, in turn `turn`, in tokens (issue
    /// 284): how many it is given before the entry, and as a last element how
    /// many in all; and whether a call of its model measured any of it.
    /// `None` as for `resolve`.
    ///
    /// Measured before a message of the model of `agent` added where it
    /// stands - after the compaction that began the path's branch
    /// (`TranscriptEntry::Compacted`, issue 387), whose copies and summary
    /// carry the measures of calls made on another branch: its call's
    /// `prompt_tokens`, which count everything the call was given before it,
    /// the system prompt as well. Read from the transcript alone: what is
    /// given to the model is built from it, never from the history.
    /// Elsewhere estimated
    /// (`ContextItem::estimated_tokens`) from what the model is given of
    /// each entry, as `resolve` gives it, a message's reasoning only where
    /// its rule gives it back: on from each measured position, back from
    /// the first; from nothing if none is.
    ///
    /// # Errors
    /// As `resolve`.
    pub fn token_positions(
        &self,
        reference: &ContextRef,
        turn: TurnId,
        agent: ActorId,
    ) -> Result<Option<(Vec<u64>, bool)>, PromptError> {
        let Some(GivenPath { leading, entries, trailing }) =
            self.given_path(reference, turn, agent)?
        else {
            return Ok(None);
        };
        let size = |items: &[ContextItem]| {
            items.iter().map(|item| item.estimated_tokens(&reference.frame.profile)).sum::<u64>()
        };
        let added_from = entries
            .iter()
            .rposition(|(node, _)| matches!(node.entry, TranscriptEntry::Compacted { .. }))
            .map_or(0, |at| at + 1);
        let measured: Vec<Option<u64>> = entries
            .iter()
            .enumerate()
            .map(|(at, (node, _))| match &node.entry {
                TranscriptEntry::Message(message)
                    if at >= added_from && message.sender_id == agent =>
                {
                    message.prompt_tokens
                }
                _ => None,
            })
            .collect();
        let mut positions = vec![size(&leading); entries.len() + 1];
        let first = measured.iter().position(Option::is_some);
        if let Some(first) = first {
            positions[first] = measured[first].unwrap_or_default();
            for at in (0..first).rev() {
                positions[at] = positions[at + 1].saturating_sub(size(&entries[at].1));
            }
        }
        for at in first.unwrap_or(0)..entries.len() {
            if let Some(before) = measured[at] {
                positions[at] = before;
            }
            positions[at + 1] = positions[at] + size(&entries[at].1);
        }
        // What stands after everything counts in all, behind every entry. A
        // call's measure counts what stood at its end then, which is not on
        // the path: an estimate the larger by that.
        if let Some(all) = positions.last_mut() {
            *all += size(&trailing);
        }
        Ok(Some((positions, first.is_some())))
    }

    /// What a receiver that keeps its own conversation - the remote agent
    /// of an external agent - is given of the transcript it reads (the
    /// state's active path, or the path it was made on), for `agent`:
    /// its messages as they stand (`Transcript::messages`) and, where
    /// someone joined or left the channel, what it is told of that (issue
    /// 408), each framed by `profile` at `time_zone` and worded in its
    /// variant.
    ///
    /// # Errors
    /// `PromptError` if the text of a notice cannot be had.
    pub fn given_messages(
        &self,
        profile: &ModelProfile,
        agent: ActorId,
        time_zone: TimeZone,
    ) -> Result<Vec<Message>, PromptError> {
        let texts = profile.texts(self.catalogue);
        let mut given = Vec::new();
        for (node, message) in as_they_stand(self.transcript.entries()) {
            let message = match message {
                Some(message) => {
                    framed(profile, &texts, &node.entry, message, agent, false, time_zone)?
                }
                None => match participant_notice(
                    &texts,
                    &node.entry,
                    &node.digest,
                    agent,
                    profile.sender_style,
                )? {
                    Some(notice) => profile.as_said(notice, agent, false, time_zone),
                    None => continue,
                },
            };
            given.push(message);
        }
        Ok(given)
    }

    /// What the model of `agent` is given of the transcript `reference`
    /// names, in turn `turn`, entry by entry, with the prompt parts where
    /// they were placed - before `ModelProfile::place_system_messages`.
    /// `None` as for `resolve`, and if a part's entry is not on the path.
    fn given_path(
        &self,
        reference: &ContextRef,
        turn: TurnId,
        agent: ActorId,
    ) -> Result<Option<GivenPath<'a>>, PromptError> {
        let prefix = match reference.transcript.as_ref() {
            Some(digest) => match self.transcript.prefix(digest) {
                Some(prefix) => prefix,
                None => return Ok(None),
            },
            None => Vec::new(),
        };
        let path: Vec<_> = as_they_stand(prefix).collect();
        // The turns given up because the channel was lowered below them:
        // what they read may not flow into it, and nobody released it (issue
        // 141), so the model is given which tools they called and nothing
        // else (issue 258).
        let lowered: HashMap<TurnId, &Classification> =
            path.iter().filter_map(|(node, _)| node.entry.lowered_turn()).collect();
        let profile = &reference.frame.profile;
        // As the context was recorded: in the variant its model was given.
        let texts = profile.texts(self.catalogue);
        // The parts at `at`.
        let placed_at = |at: PromptAt| -> Option<Vec<ContextItem>> {
            reference
                .frame
                .prompts
                .iter()
                .filter(|p| p.at == at)
                .map(|p| {
                    Some(ContextItem::Prompt {
                        digest: p.text,
                        role: p.role,
                        text: self.prompts.get(&p.text)?.clone(),
                        classification: p.classification.clone(),
                    })
                })
                .collect()
        };
        let Some(leading) = placed_at(PromptAt::Start) else { return Ok(None) };
        let Some(trailing) = placed_at(PromptAt::End) else { return Ok(None) };
        let mut placed = leading.len() + trailing.len();
        let time_zone = reference.frame.time_zone;
        let mut entries = Vec::with_capacity(path.len());
        for (node, message) in path {
            let in_turn = message.as_ref().is_some_and(|m| m.turn_id == Some(turn));
            let message = message.and_then(|m| match m.turn_id.and_then(|t| lowered.get(&t)) {
                Some(channel) => m.names_only(channel),
                None => Some(m),
            });
            let mut given = Vec::new();
            // The end of a turn is no message; one given up is told where it
            // stands, and so is who joined or left. A part may have been
            // placed after any of them.
            if let Some(message) = message {
                given.push(ContextItem::Conversation {
                    message: framed(
                        profile,
                        &texts,
                        &node.entry,
                        message,
                        agent,
                        in_turn,
                        time_zone,
                    )?,
                });
            }
            if let TranscriptEntry::TurnGivenUp { turn_id, reason, at } = &node.entry {
                given.push(ContextItem::Conversation {
                    message: profile.as_said(
                        given_up_notice(&texts, *turn_id, reason, *at)?,
                        agent,
                        false,
                        time_zone,
                    ),
                });
            }
            if let Some(notice) =
                participant_notice(&texts, &node.entry, &node.digest, agent, profile.sender_style)?
            {
                given.push(ContextItem::Conversation {
                    message: profile.as_said(notice, agent, false, time_zone),
                });
            }
            let Some(parts) = placed_at(PromptAt::After(node.digest)) else { return Ok(None) };
            placed += parts.len();
            given.extend(parts);
            entries.push((node, given));
        }
        // Every part was placed, none after what is not there.
        Ok((placed == reference.frame.prompts.len()).then_some(GivenPath {
            leading,
            entries,
            trailing,
        }))
    }
}

/// `message`, which `entry` records, as `profile` gives it to the model of `agent` in `time_zone`,
/// `in_turn` if the turn under way produced it: what was given to read as such
/// (`ModelProfile::given_to_read`, worded by `texts`, issue 539), what was said as said
/// (`ModelProfile::as_said`).
///
/// # Errors
/// `PromptError` if a text cannot be had.
fn framed(
    profile: &ModelProfile,
    texts: &PromptTexts,
    entry: &TranscriptEntry,
    message: Message,
    agent: ActorId,
    in_turn: bool,
    time_zone: TimeZone,
) -> Result<Message, PromptError> {
    match entry {
        TranscriptEntry::Given { .. } => profile.given_to_read(message, agent, in_turn, texts),
        _ => Ok(profile.as_said(message, agent, in_turn, time_zone)),
    }
}

/// What the model is told of the turn `turn`, given up at `at` for `reason` (issue 258), where its
/// end stands: why, and that what its tools did stays done, worded by `texts` (`kernel.given_up`).
/// A fixed text of the kernel's, which says nothing of the conversation; its id is the turn's, so
/// that it is the same each time it is resolved.
fn given_up_notice(
    texts: &PromptTexts,
    turn: TurnId,
    reason: &GivenUpReason,
    at: DateTime<Utc>,
) -> Result<Message, PromptError> {
    let cause = texts.render(&reason.cause_key(), ())?;
    let lowered = matches!(reason, GivenUpReason::Lowered { .. });
    Ok(Message {
        id: MessageId(turn.0),
        timestamp: at,
        role: MessageRole::System,
        content: Some(texts.render(&format!("{KERNEL}.given_up"), context! { cause, lowered })?),
        internal: true,
        turn_id: Some(turn),
        ..Message::new(Classification::PUBLIC)
    })
}

/// What the model of `agent` is told of `entry`, whose digest is `digest`, if someone joined or
/// left the channel (issue 408): who, and who made them, named in `style` as the senders of
/// messages are - the agent itself as "you" -, worded by `texts` (`kernel.participant_joined`,
/// `…_left`). A fixed text of the kernel's, marked as the channel's notice, so that it reads as no
/// participant's, given as a system message where the model keeps one
/// (`ModelProfile::place_system_messages`); its id is the entry's, so that it is the same each time
/// it is resolved. `None` for any other entry.
fn participant_notice(
    texts: &PromptTexts,
    entry: &TranscriptEntry,
    digest: &Digest,
    agent: ActorId,
    style: SenderStyle,
) -> Result<Option<Message>, PromptError> {
    let (key, participant, name, by, by_name, at) = match entry {
        TranscriptEntry::ParticipantJoined { participant, name, by, by_name, at } => {
            ("participant_joined", participant, name, by, by_name, at)
        }
        TranscriptEntry::ParticipantLeft { participant, name, by, by_name, at } => {
            ("participant_left", participant, name, by, by_name, at)
        }
        _ => return Ok(None),
    };
    let text = texts.render(
        &format!("{KERNEL}.{key}"),
        context! {
            participant => style.of(name),
            you => *participant == agent,
            by => style.of(by_name),
            by_you => *by == agent,
            by_self => by == participant,
            by_system => *by == ActorId::System,
        },
    )?;
    Ok(Some(Message {
        id: MessageId(digest.as_uuid()),
        timestamp: *at,
        role: MessageRole::System,
        content: Some(text),
        sender_id: ActorId::System,
        ..Message::new(Classification::PUBLIC)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentId;
    use crate::context::testing::text;
    use crate::context::{FrameRef, PromptRef, ReasoningHistory, SenderFormat, SystemMessages};
    use crate::digest;
    use crate::digest::prompt;
    use crate::message::TokenUsage;
    use crate::prompt_catalogue::{kernel_catalogue, PromptVariant, KERNEL_PROMPTS};
    use crate::tool::Tool;
    use crate::turn::TurnState;
    use crate::user::UserId;
    use crate::work::Work;
    use std::fs;

    fn contents(items: &[ContextItem]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                ContextItem::Prompt { text, .. } => format!("prompt:{text}"),
                ContextItem::Conversation { message } => {
                    message.content.clone().unwrap_or_default()
                }
            })
            .collect()
    }

    #[test]
    fn a_context_is_resolved_with_the_senders_marked_as_its_profile_says() {
        let agent = ActorId::Agent(AgentId::new());
        let alice = Message {
            sender_id: ActorId::User(UserId::new()),
            sender_name: "Alice".to_string(),
            role: MessageRole::User,
            ..text("question")
        };
        let mut state = AgentChannelState::default();
        state.transcript.push(alice.clone());
        state.transcript.push(Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            ..text("answer")
        });
        let reference = |sender_format| ContextRef {
            transcript: state.transcript.digest(),
            frame: FrameRef {
                profile: ModelProfile { sender_format, ..ModelProfile::default() },
                ..FrameRef::default()
            },
        };

        let resolver = ContextResolver::of(&state, kernel_catalogue());
        let seen = resolver
            .resolve(&reference(SenderFormat::Prefix), TurnId::new(), agent)
            .unwrap()
            .expect("resolved");
        assert_eq!(
            contents(&seen.items),
            ["@alice: question", "answer"],
            "what the model is given, and the context panel shows"
        );
        assert_eq!(
            state.transcript.messages()[0].content,
            alice.content,
            "the transcript keeps it as received"
        );
        let seen = resolver
            .resolve(&reference(SenderFormat::None), TurnId::new(), agent)
            .unwrap()
            .expect("resolved");
        assert_eq!(contents(&seen.items), ["question", "answer"]);
    }

    #[test]
    fn who_joined_or_left_is_told_where_it_happened_as_the_channel_s_notice() {
        let agent = ActorId::Agent(AgentId::new());
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let at = Utc::now();
        let mut state = AgentChannelState::default();
        state.transcript.push(Message {
            sender_id: alice,
            sender_name: "Alice".to_string(),
            role: MessageRole::User,
            ..text("hello")
        });
        let joined = |participant: ActorId, name: &str, by: ActorId, by_name: &str| {
            TranscriptEntry::ParticipantJoined {
                participant,
                name: name.to_string(),
                by,
                by_name: by_name.to_string(),
                at,
            }
        };
        let left = |participant: ActorId, name: &str, by: ActorId, by_name: &str| {
            TranscriptEntry::ParticipantLeft {
                participant,
                name: name.to_string(),
                by,
                by_name: by_name.to_string(),
                at,
            }
        };
        state.transcript.record(joined(bob, "Bob", alice, "Alice"));
        state.transcript.record(joined(agent, "Helper", alice, "Alice"));
        state.transcript.record(joined(bob, "Bob", bob, "Bob"));
        state.transcript.record(joined(agent, "Helper", ActorId::System, "system"));
        state.transcript.record(left(bob, "Bob", alice, "Alice"));
        state.transcript.record(left(bob, "Bob", bob, "Bob"));
        state.transcript.record(left(bob, "Bob", ActorId::System, "system"));
        let reference = ContextRef {
            transcript: state.transcript.digest(),
            frame: FrameRef {
                profile: ModelProfile {
                    sender_format: SenderFormat::Prefix,
                    ..ModelProfile::default()
                },
                ..FrameRef::default()
            },
        };

        let resolver = ContextResolver::of(&state, kernel_catalogue());
        let seen = resolver.resolve(&reference, TurnId::new(), agent).unwrap().expect("resolved");
        assert_eq!(
            contents(&seen.items),
            [
                "@alice: hello",
                "Channel notice: @alice added @bob to the channel.",
                "Channel notice: @alice added you to the channel.",
                "Channel notice: @bob joined the channel.",
                "Channel notice: You were added to the channel.",
                "Channel notice: @alice removed @bob from the channel.",
                "Channel notice: @bob left the channel.",
                "Channel notice: @bob was removed from the channel.",
            ],
            "in the order it happened, unmarked by any participant"
        );
        assert!(
            seen.items[1..].iter().all(|item| item.role() == MessageRole::User),
            "not as a system message, which the model would move (issue 49)"
        );
        let again = resolver.resolve(&reference, TurnId::new(), agent).unwrap().expect("resolved");
        assert_eq!(ids(&seen.items), ids(&again.items), "the same each time it is resolved");
    }

    #[test]
    fn a_remote_agent_is_given_the_messages_framed_and_the_notices_as_system_messages() {
        let agent = ActorId::Agent(AgentId::new());
        let alice = ActorId::User(UserId::new());
        let mut state = AgentChannelState::default();
        state.transcript.push(Message {
            sender_id: alice,
            sender_name: "Alice".to_string(),
            role: MessageRole::User,
            ..text("hello")
        });
        state.transcript.record(TranscriptEntry::ParticipantJoined {
            participant: agent,
            name: "Helper".to_string(),
            by: alice,
            by_name: "Alice".to_string(),
            at: Utc::now(),
        });
        state.transcript.push(Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            ..text("hi")
        });
        let profile =
            ModelProfile { sender_format: SenderFormat::Prefix, ..ModelProfile::default() };

        let told: Vec<_> = ContextResolver::of(&state, kernel_catalogue())
            .given_messages(&profile, agent, Default::default())
            .unwrap()
            .into_iter()
            .map(|message| (message.role, message.content.unwrap_or_default()))
            .collect();
        assert_eq!(
            told,
            [
                (MessageRole::User, "@alice: hello".to_string()),
                (
                    MessageRole::System,
                    "Channel notice: @alice added you to the channel.".to_string()
                ),
                (MessageRole::Assistant, "hi".to_string()),
            ],
            "framed as the model is given them, the notice kept a system message"
        );
    }

    /// Issue 539: what is given to read is framed by the resolver, whoever gave it - a system
    /// message where the model keeps one, never marked as said, who gave it worded if another
    /// participant did -, for the agent's model as for a remote agent.
    #[test]
    fn what_is_given_to_read_is_framed_as_given_whoever_gave_it() {
        let agent = ActorId::Agent(AgentId::new());
        let alice = ActorId::User(UserId::new());
        let mut state = AgentChannelState::default();
        state.transcript.push(Message {
            sender_id: alice,
            sender_name: "Alice".to_string(),
            role: MessageRole::User,
            ..text("hello")
        });
        // A skill alice gave with `/skill`, one the agent loaded itself, written as a user's.
        state.transcript.give(
            Message {
                sender_id: alice,
                sender_name: "Alice".to_string(),
                role: MessageRole::System,
                ..text("[Skill: notes]\nCollect.")
            },
            None,
        );
        state.transcript.give(
            Message { sender_id: agent, role: MessageRole::User, ..text("[Skill: plan]\nOrder.") },
            None,
        );
        let profile = |system_messages| ModelProfile {
            sender_format: SenderFormat::Prefix,
            system_messages,
            ..ModelProfile::default()
        };
        let resolved = |system_messages| {
            let reference = ContextRef {
                transcript: state.transcript.digest(),
                frame: FrameRef { profile: profile(system_messages), ..FrameRef::default() },
            };
            let resolver = ContextResolver::of(&state, kernel_catalogue());
            let items = resolver
                .resolve(&reference, TurnId::new(), agent)
                .unwrap()
                .expect("resolved")
                .items;
            let roles = items.iter().map(ContextItem::role);
            roles.zip(contents(&items)).collect::<Vec<_>>()
        };
        let given = |role| {
            [
                (MessageRole::User, "@alice: hello".to_string()),
                (role, "@alice gave you this to read:\n[Skill: notes]\nCollect.".to_string()),
                (role, "[Skill: plan]\nOrder.".to_string()),
            ]
        };
        assert_eq!(
            resolved(SystemMessages::Anywhere),
            given(MessageRole::System),
            "unmarked as said, who gave it worded"
        );
        assert_eq!(
            resolved(SystemMessages::OnlyAtStart),
            given(MessageRole::User),
            "as a user message where the model keeps no system message there"
        );

        let told: Vec<_> = ContextResolver::of(&state, kernel_catalogue())
            .given_messages(&profile(SystemMessages::OnlyAtStart), agent, Default::default())
            .unwrap()
            .into_iter()
            .map(|message| (message.role, message.content.unwrap_or_default()))
            .collect();
        assert_eq!(
            told,
            given(MessageRole::System),
            "worded so for a remote agent too, whose agent takes the text"
        );
    }

    /// The ids of the messages among `items`.
    fn ids(items: &[ContextItem]) -> Vec<MessageId> {
        items
            .iter()
            .filter_map(|item| match item {
                ContextItem::Conversation { message } => Some(message.id),
                ContextItem::Prompt { .. } => None,
            })
            .collect()
    }

    #[test]
    fn prompt_parts_are_resolved_where_they_were_placed() {
        let mut state = AgentChannelState::default();
        let (earlier, question) = (text("earlier"), text("question"));
        let after_earlier = state.transcript.push(earlier.clone());
        assert_eq!(after_earlier, None);
        let before_question = state.transcript.push(question.clone());
        let whole = state.transcript.digest();
        for part in ["system", "notes"] {
            state.prompts.keep(prompt(part), || part.to_string());
        }
        let reference = ContextRef {
            transcript: whole,
            frame: FrameRef {
                prompts: vec![
                    PromptRef {
                        text: prompt("system"),
                        role: MessageRole::System,
                        at: PromptAt::Start,
                        classification: None,
                    },
                    PromptRef {
                        text: prompt("notes"),
                        role: MessageRole::User,
                        at: PromptAt::after(before_question),
                        classification: None,
                    },
                ],
                tools: None,
                knowledge: Vec::new(),
                profile: ModelProfile::default(),
                time_zone: Default::default(),
            },
        };

        let resolver = ContextResolver::of(&state, kernel_catalogue());
        let turn = TurnId::new();
        let seen = resolver
            .resolve(&reference, turn, ActorId::None)
            .unwrap()
            .expect("everything it names is here");
        assert_eq!(contents(&seen.items), ["prompt:system", "earlier", "prompt:notes", "question"]);

        let mut elsewhere = reference.clone();
        elsewhere.frame.prompts[1].at = PromptAt::After(prompt("not an entry"));
        assert!(
            resolver.resolve(&elsewhere, turn, ActorId::None).unwrap().is_none(),
            "a part after an entry that is not in the prefix"
        );
        let mut forgotten = reference;
        forgotten.frame.prompts[0].text = prompt("never recorded");
        assert!(
            resolver.resolve(&forgotten, turn, ActorId::None).unwrap().is_none(),
            "a part whose text was not kept"
        );
    }

    #[test]
    fn the_context_is_measured_by_the_calls_made_on_it_and_estimated_as_the_model_is_given_it() {
        let agent = ActorId::Agent(AgentId::new());
        let measure = |state: &AgentChannelState| {
            let turn = state.turn.as_ref().unwrap();
            ContextResolver::of(state, kernel_catalogue())
                .token_positions(&turn.work.context, turn.work.turn_id, agent)
                .unwrap()
                .unwrap()
        };
        // Its reasoning: 100 tokens, as many as it produced in all.
        let call = |prompt: u64, content: &str| Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            prompt_tokens: Some(prompt),
            completion_tokens: Some(101),
            reasoning: Some("r".repeat(400)),
            ..text(content)
        };
        let mut state = AgentChannelState::default();
        state.transcript.push(text("hello"));
        begin(
            &mut state,
            ModelProfile { reasoning_history: ReasoningHistory::Never, ..ModelProfile::default() },
        );
        assert_eq!(measure(&state), (vec![0, 2], false), "nothing measured: \"hello\" estimated");

        let first = call(100, "call");
        state.said(first.clone());
        state.said(Message { sender_id: agent, role: MessageRole::Tool, ..text(&"x".repeat(40)) });
        assert_eq!(measure(&state), (vec![98, 100, 101, 111], true),
            "before the call as it measured, back from it and on from it estimated - the call as the model is given it, without its reasoning");
        state.turn.as_mut().unwrap().work.context.frame.profile.reasoning_history =
            ReasoningHistory::CurrentTurn;
        assert_eq!(measure(&state).0[2], 201, "with it where it is given back");

        let kept_from = state
            .transcript
            .entries()
            .find(|n| n.entry.message().is_some_and(|m| m.id == first.id))
            .map(|n| n.digest);
        let summary = Message {
            sender_id: agent,
            prompt_tokens: Some(500),
            completion_tokens: Some(8),
            ..text("summary")
        };
        state
            .compact(agent, vec![(summary, None)], kept_from, kernel_catalogue())
            .expect("compacted");
        assert_eq!(measure(&state), (vec![0, 2, 103, 113, 113], false), "neither the summary's call nor the copy measure the new branch, and where the compaction's part ends gives nothing");

        state.said(call(60, "done"));
        let (positions, measured) = measure(&state);
        assert!(measured, "a call on the new branch measures it");
        assert_eq!(
            positions[4..],
            [60, 161],
            "before it as it measured, then it as the model is given it"
        );

        let mut others = AgentChannelState::default();
        others.transcript.push(text("hello"));
        begin(&mut others, ModelProfile::default());
        others
            .transcript
            .push(Message { sender_id: ActorId::Agent(AgentId::new()), ..call(70, "theirs") });
        others.turn.as_mut().unwrap().work.context.transcript = others.transcript.digest();
        assert!(
            !measure(&others).1,
            "a call of another agent's model does not measure this one's context"
        );
    }

    /// A turn of `state` begun on its transcript as it stands.
    fn begin(state: &mut AgentChannelState, profile: ModelProfile) {
        let context = ContextRef {
            transcript: state.transcript.digest(),
            frame: FrameRef { profile, ..FrameRef::default() },
        };
        let work = Work { turn_id: TurnId::new(), context, ..Default::default() };
        state.turn = Some(TurnState { work, ..TurnState::default() });
    }

    /// A prompt part `text` in `role` after the end of `state`'s turn so
    /// far, its text kept.
    fn place(state: &mut AgentChannelState, role: MessageRole, text: &str) {
        state.prompts.keep(prompt(text), || text.to_string());
        let turn = state.turn.as_mut().unwrap();
        turn.work.context.frame.prompts.push(PromptRef {
            text: prompt(text),
            role,
            at: PromptAt::after(turn.work.context.transcript),
            classification: None,
        });
    }

    /// Issue 537: a part at the end stands after everything a call is given - after a part placed
    /// after the last entry too -, and moves on as the turn says more: what each message was
    /// produced from had it last. It counts in all, behind every entry.
    #[test]
    fn a_part_at_the_end_is_given_last_in_every_call() {
        let agent = ActorId::Agent(AgentId::new());
        let mut state = AgentChannelState::default();
        state.transcript.push(text("question"));
        begin(&mut state, ModelProfile::default());
        state.prompts.keep(prompt("[Time]"), || "[Time]".to_string());
        state.turn.as_mut().unwrap().work.context.frame.prompts.push(PromptRef {
            text: prompt("[Time]"),
            role: MessageRole::User,
            at: PromptAt::End,
            classification: None,
        });
        place(&mut state, MessageRole::User, "step 1");
        let resolved = |state: &AgentChannelState, context: &ContextRef| {
            let turn_id = state.turn.as_ref().unwrap().work.turn_id;
            let resolver = ContextResolver::of(state, kernel_catalogue());
            contents(&resolver.resolve(context, turn_id, agent).unwrap().expect("resolved").items)
        };
        let next = |state: &AgentChannelState| state.turn.as_ref().unwrap().work.context.clone();
        assert_eq!(
            resolved(&state, &next(&state)),
            ["question", "prompt:step 1", "prompt:[Time]"],
            "after the part placed after the last entry"
        );

        let first = state.said(Message { sender_id: agent, ..text("first") });
        assert_eq!(
            resolved(&state, &next(&state)),
            ["question", "prompt:step 1", "first", "prompt:[Time]"],
            "moved on with the turn"
        );
        let produced_from = state.context_of(&first).expect("sent with what it was produced from");
        assert_eq!(
            resolved(&state, &produced_from),
            ["question", "prompt:step 1", "prompt:[Time]"],
            "what the message was produced from had it last"
        );

        let turn_id = state.turn.as_ref().unwrap().work.turn_id;
        let positions = |context: &ContextRef| {
            let resolver = ContextResolver::of(&state, kernel_catalogue());
            resolver.token_positions(context, turn_id, agent).unwrap().expect("measured").0
        };
        let with = positions(&next(&state));
        let mut without_it = next(&state);
        without_it.frame.prompts.retain(|p| p.at != PromptAt::End);
        let without = positions(&without_it);
        let entries = without.len() - 1;
        assert_eq!(with[..entries], without[..entries], "behind every entry");
        assert!(with[entries] > without[entries], "counted in all");
    }

    #[test]
    fn a_turn_under_way_is_resolved_from_the_transcript_it_says_into() {
        let agent = ActorId::Agent(AgentId::new());
        let own = |content: &str| Message {
            sender_id: agent,
            reasoning: Some(format!("why {content}")),
            ..text(content)
        };
        let mut state = AgentChannelState::default();
        state.transcript.push(own("earlier"));
        state.transcript.push(text("question"));

        begin(&mut state, ModelProfile::default());
        // Instructions of the kernel among the turn's messages: one before them, one after the
        // first.
        place(&mut state, MessageRole::User, "step 1");
        let first = state.said(own("first"));
        place(&mut state, MessageRole::User, "step 2");
        let second = state.said(own("second"));

        let (next, turn_id) =
            state.turn.as_ref().map(|t| (t.work.context.clone(), t.work.turn_id)).unwrap();
        let current = ContextResolver::of(&state, kernel_catalogue())
            .resolve(&next, turn_id, agent)
            .unwrap()
            .expect("the transcript holds the turn's messages");
        assert_eq!(
            contents(&current.items),
            ["earlier", "question", "prompt:step 1", "first", "prompt:step 2", "second"]
        );
        let reasoning = current.items.iter().map(|item| matches!(item, ContextItem::Conversation { message } if message.reasoning.is_some())).collect::<Vec<_>>();
        assert_eq!(
            reasoning,
            [false, false, false, true, false, true],
            "the turn's own reasoning, not an earlier turn's"
        );

        // What each message was produced from, sent with it as it was said
        // (issues 260, 417), resolves to what the model was given for it -
        // while the turn is under way and once it ended.
        let seen = |state: &AgentChannelState, message: &Message| {
            let context = state.context_of(message).expect("sent with what it was produced from");
            contents(
                &ContextResolver::of(state, kernel_catalogue())
                    .resolve(&context, turn_id, agent)
                    .unwrap()
                    .expect("everything it names is here")
                    .items,
            )
        };
        assert_eq!(
            seen(&state, &second),
            ["earlier", "question", "prompt:step 1", "first", "prompt:step 2"]
        );
        assert_eq!(
            state.transcript.digest(),
            next.transcript,
            "the turn's context names the transcript's head"
        );
        state.turn = None;
        assert_eq!(seen(&state, &first), ["earlier", "question", "prompt:step 1"]);
        assert_eq!(
            seen(&state, &second),
            ["earlier", "question", "prompt:step 1", "first", "prompt:step 2"]
        );
    }

    #[test]
    fn usage_sums_the_agent_s_own_calls_on_every_branch_and_in_its_turn() {
        let agent = ActorId::Agent(AgentId::new());
        let call = |sender: ActorId, prompt: u64, completion: u64, size: Option<u64>| Message {
            sender_id: sender,
            prompt_tokens: Some(prompt),
            completion_tokens: Some(completion),
            context_size: size,
            ..text("answer")
        };
        let mut state = AgentChannelState::default();
        assert_eq!(state.used, None, "no call yet");

        begin(&mut state, ModelProfile::default());
        state.said(call(agent, 100, 10, Some(4096)));
        // Heard: it carries its sender's tokens.
        let heard = call(ActorId::Agent(AgentId::new()), 5000, 500, Some(8192));
        state.perceive(vec![heard]);
        let before_given = state.transcript.digest();
        state.said(call(agent, 200, 20, Some(4096)));
        state.transcript.record(TranscriptEntry::TurnGivenUp {
            turn_id: TurnId::new(),
            reason: GivenUpReason::Left,
            at: Utc::now(),
        });
        // A branch no longer active - as an external agent's earlier calls
        // are (`start_anew`) - counts as well.
        state.transcript.move_head(before_given.as_ref());
        begin(&mut state, ModelProfile::default());
        state.said(call(agent, 300, 30, None));

        assert_eq!(state.used, Some(TokenUsage {
            calls: 3,
            prompt_tokens: 600,
            completion_tokens: 60,
            context_size: None,
            context_tokens: 330,
        }), "not the other agent's call; the turn given up and the one under way count, the context as the last call measured it, its answer included");
    }

    /// Issue 57: a context is worded in the variant of the texts it was
    /// recorded with - for the next call of the model and for what it saw,
    /// asked afterwards, alike.
    #[test]
    fn a_context_is_worded_in_the_variant_it_was_recorded_with() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("kernel.yaml"), "given_up_cause:\n  left: you walked out\n")
            .unwrap();
        let mut catalogue = PromptCatalogue::new();
        catalogue.add(KERNEL, KERNEL_PROMPTS).unwrap();
        catalogue.add_variant("gemma4", dir.path()).unwrap();
        let catalogue = Arc::new(catalogue);
        let mut state = AgentChannelState::default();
        state.transcript.record(TranscriptEntry::TurnGivenUp {
            turn_id: TurnId::new(),
            reason: GivenUpReason::Left,
            at: Utc::now(),
        });
        let worded = |profile| {
            let reference = ContextRef {
                transcript: state.transcript.digest(),
                frame: FrameRef { profile, ..FrameRef::default() },
            };
            contents(
                &ContextResolver::of(&state, &catalogue)
                    .resolve(&reference, TurnId::new(), ActorId::None)
                    .unwrap()
                    .expect("resolved")
                    .items,
            )
        };
        let gemma = ModelProfile {
            prompt_texts: Some(PromptVariant::new("gemma4").unwrap()),
            ..ModelProfile::default()
        };
        assert!(worded(gemma)[0].contains("because you walked out."), "{:?}", worded(gemma));
        assert!(
            worded(ModelProfile::default())[0].contains("because you left the channel."),
            "the default where it was recorded without one"
        );
    }

    #[test]
    fn the_tools_offered_are_resolved_from_their_set() {
        use crate::tool::ToolParameters;
        let mut state = AgentChannelState::default();
        let echo = Tool {
            name: "echo".to_string(),
            description: "echoes".to_string(),
            parameters: ToolParameters::object(
                [("text", ToolParameters::string().described("what"))],
                ["text"],
            ),
            requires_approval: true,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        };
        let offered = vec![ToolSet::new("echo", "echoes", vec![echo])];
        let digest = digest::tool_sets(&offered);
        state.offered_tools.keep(digest, || offered.clone());

        let resolver = ContextResolver::of(&state, kernel_catalogue());
        let with = |tools| ContextRef {
            frame: FrameRef { tools, ..FrameRef::default() },
            ..ContextRef::default()
        };
        assert_eq!(
            resolver
                .resolve(&with(Some(digest)), TurnId::new(), ActorId::None)
                .unwrap()
                .expect("the set is kept")
                .tool_sets,
            offered
        );
        assert!(resolver
            .resolve(&with(None), TurnId::new(), ActorId::None)
            .unwrap()
            .expect("no tools offered")
            .tool_sets
            .is_empty());
        assert!(
            resolver
                .resolve(&with(Some(prompt("never recorded"))), TurnId::new(), ActorId::None)
                .unwrap()
                .is_none(),
            "a set that was not kept"
        );
    }

    #[test]
    fn a_system_message_stays_where_it_is_only_where_the_model_keeps_it() {
        let mut state = AgentChannelState::default();
        state.transcript.push(Message { role: MessageRole::User, ..text("question") });
        state.transcript.push(Message { role: MessageRole::System, ..text("an old instruction") });

        let roles = |system_messages| {
            let mut state = state.clone();
            state.prompts.keep(prompt("system prompt"), || "system prompt".to_string());
            begin(&mut state, ModelProfile { system_messages, ..ModelProfile::default() });
            state.turn.as_mut().unwrap().work.context.frame.prompts.push(PromptRef {
                text: prompt("system prompt"),
                role: MessageRole::System,
                at: PromptAt::Start,
                classification: None,
            });
            place(&mut state, MessageRole::System, "instruction");
            let next = state.turn.as_ref().unwrap().work.context.clone();
            ContextResolver::of(&state, kernel_catalogue())
                .resolve(&next, TurnId::new(), ActorId::None)
                .unwrap()
                .expect("everything it names is here")
                .items
                .iter()
                .map(ContextItem::role)
                .collect::<Vec<_>>()
        };
        use MessageRole::{System, User};
        assert_eq!(
            roles(SystemMessages::OnlyAtStart),
            [System, User, User, User],
            "only the system prompt stays one"
        );
        assert_eq!(roles(SystemMessages::Anywhere), [System, User, System, System]);
        assert_eq!(SystemMessages::default(), SystemMessages::OnlyAtStart);
    }

    /// Issue 539: the system prompt is the parts at the start alone - what begins the transcript,
    /// a summary given after a compaction, or a notice of who joined, is none of it, nor is what
    /// is placed after it -, so that a model that merges the system messages takes none of it for
    /// its instructions.
    #[test]
    fn what_begins_the_transcript_is_no_part_of_the_system_prompt() {
        let agent = ActorId::Agent(AgentId::new());
        let mut state = AgentChannelState::default();
        state.transcript.record(TranscriptEntry::ParticipantJoined {
            participant: agent,
            name: "Helper".to_string(),
            by: ActorId::System,
            by_name: "system".to_string(),
            at: Utc::now(),
        });
        state.transcript.give(text("[Summary of the conversation so far]\nAsked."), None);
        let roles = |system_messages| {
            let mut state = state.clone();
            state.prompts.keep(prompt("system prompt"), || "system prompt".to_string());
            begin(&mut state, ModelProfile { system_messages, ..ModelProfile::default() });
            state.turn.as_mut().unwrap().work.context.frame.prompts.push(PromptRef {
                text: prompt("system prompt"),
                role: MessageRole::System,
                at: PromptAt::Start,
                classification: None,
            });
            place(&mut state, MessageRole::System, "the plan");
            let next = state.turn.as_ref().unwrap().work.context.clone();
            let resolver = ContextResolver::of(&state, kernel_catalogue());
            let items = resolver.resolve(&next, TurnId::new(), agent).unwrap().expect("resolved");
            items.items.iter().map(ContextItem::role).collect::<Vec<_>>()
        };
        use MessageRole::{System, User};
        assert_eq!(
            roles(SystemMessages::OnlyAtStart),
            [System, User, User, User],
            "the notice, the summary and the plan after the system prompt as user messages"
        );
        assert_eq!(roles(SystemMessages::Anywhere), [System, System, System, System]);
    }

    /// Issue 539: what a model is handed to work on the conversation has no system prompt, so that
    /// a summary that begins it is a user message where the model keeps system messages only at
    /// the start.
    #[test]
    fn the_conversation_alone_has_no_system_prompt() {
        let agent = ActorId::Agent(AgentId::new());
        let mut state = AgentChannelState::default();
        state.transcript.give(text("[Summary of the conversation so far]\nAsked."), None);
        state.transcript.push(Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            ..text("ok")
        });
        let last = state.transcript.digest().expect("an entry");
        let roles = |system_messages| {
            let profile = ModelProfile { system_messages, ..ModelProfile::default() };
            let resolver = ContextResolver::of(&state, kernel_catalogue());
            let items = resolver
                .conversation_until(last, profile, TimeZone::UTC, TurnId::new(), agent)
                .unwrap()
                .expect("the transcript holds it");
            items.iter().map(ContextItem::role).collect::<Vec<_>>()
        };
        use MessageRole::{Assistant, System, User};
        assert_eq!(roles(SystemMessages::OnlyAtStart), [User, Assistant], "no system prompt");
        assert_eq!(roles(SystemMessages::Anywhere), [System, Assistant]);
    }

    #[test]
    fn a_context_is_resolved_with_the_messages_as_they_stood_when_it_was_taken() {
        let mut state = AgentChannelState::default();
        let question = text("question");
        let after_question = {
            state.transcript.push(question.clone());
            state.transcript.digest()
        };
        state.transcript.push(text("aside"));
        let then = ContextRef { transcript: state.transcript.digest(), ..ContextRef::default() };
        state.prompts.keep(prompt("notes"), || "notes".to_string());

        let at = Utc::now();
        state.transcript.modify(Message {
            content: Some("question, corrected".to_string()),
            timestamp: at,
            ..question.clone()
        });
        let aside = state.transcript.messages()[1].id;
        state.transcript.revoke(aside, at);
        let notes = PromptRef {
            text: prompt("notes"),
            role: MessageRole::User,
            at: PromptAt::after(after_question),
            classification: None,
        };
        let now = ContextRef {
            transcript: state.transcript.digest(),
            frame: FrameRef { prompts: vec![notes], ..FrameRef::default() },
        };

        let resolver = ContextResolver::of(&state, kernel_catalogue());
        assert_eq!(
            contents(
                &resolver
                    .resolve(&then, TurnId::new(), ActorId::None)
                    .unwrap()
                    .expect("resolved")
                    .items
            ),
            ["question", "aside"],
            "what the model saw then"
        );
        assert_eq!(
            contents(
                &resolver
                    .resolve(&now, TurnId::new(), ActorId::None)
                    .unwrap()
                    .expect("resolved")
                    .items
            ),
            ["question, corrected", "prompt:notes"],
            "what stands now"
        );
    }
}
