//! An agent in one channel as it is kept: its `AgentChannelState` - its transcript, its notes, its
//! part of each thread - and the changes by which a store records it (`AgentChannelStateChange`).

use super::{AgentThreadState, AttachmentIndex};
use crate::actor::{ActorEvent, ActorId};
use crate::approval::ApprovalRequest;
use crate::chain::Stored;
use crate::channel::{ThreadId, ThreadOrigin};
use crate::confidentiality::Classification;
use crate::content::ContentId;
use crate::context::{
    ContextResolver, FrameRef, KnowledgeRef, KnowledgeSource, PromptAt, PromptRef,
};
use crate::digest;
use crate::digest::Digest;
use crate::external_agent::RemoteState;
use crate::history::{Heard, HistoryEntry};
use crate::kept::Kept;
use crate::ledger::LedgerEntry;
use crate::message::{executed_calls, Message, MessageId, MessageRole, TokenUsage};
use crate::note::{NoteChain, NoteChange, NoteOrigin, Notes};
use crate::prompt_catalogue::PromptCatalogue;
use crate::question::QuestionRequest;
use crate::store;
use crate::store::Journaled;
use crate::tool::{ToolCallState, ToolSet};
use crate::transcript::{
    as_they_stand, given_as_they_stand, knowledge_as_seen_whole, parts_as_given, GivenSource,
    Transcript, TranscriptEntry,
};
use crate::turn::GivenUpReason;
use crate::turn::{AfterStep, CommandRequest, TurnState};
use crate::work::{TurnId, Work};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::mem;
use std::ops::Not;
use std::sync::Arc;
use tracing::warn;

/// An agent in one channel, of either kind (issue 206): what it heard and said, what it was given,
/// its part of each thread it took part in (issue 602), and the part only its kind keeps - the
/// turns under way of an agent run here, what an external agent keeps of the remote side. What an
/// external agent cannot have - notes, prompt parts, tool sets, a turn - stays empty.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentChannelState {
    /// The agent's part of each thread it took part in (issue 602): its turn there, where it stands
    /// in it, what its calls there used. Begun as the agent first does something there
    /// (`thread_mut`).
    pub threads: BTreeMap<ThreadId, AgentThreadState>,
    /// Whether the agent sleeps in its channel (issue 602): where it stands outside a turn, its
    /// own as a whole, not of a thread - awake it waits idle. The actor owns the live value and
    /// writes it in when it stores; a turn under way stands at a step of its own
    /// (`TurnState::step`).
    #[serde(default, skip_serializing_if = "Not::not")]
    pub asleep: bool,
    /// What the agent's model is given of the conversation: its own
    /// messages in full, internal steps included, and what others said
    /// (issue 50). Of an external agent, what the remote agent was given,
    /// in the order it was given (issue 179). A branch for every thread it
    /// takes part in (issue 602), each as a store loads it: not part of the
    /// state's document, but a tree a store keeps apart (issue 380), written
    /// with the state and read on its own (`TranscriptStore`).
    #[serde(skip)]
    pub transcript: Transcript,
    /// The notes the agent published in this channel (issue 17).
    pub notes: NoteChain,
    /// The agent's private notes in this channel, which only it reads - and
    /// only where the channel's confidentiality allows (issue 51).
    pub private_notes: NoteChain,
    /// The prompt parts the agent was given in this channel - the system
    /// prompt, notes, ... - by digest, each kept once: what the `FrameRef`s
    /// of its contexts point to (issues 06, 47). Written by the store apart
    /// from the document, before everything that names them, and read by
    /// digest (`TranscriptStore::prompts`, issue 383); a state loaded holds
    /// those its turns under way name.
    #[serde(skip)]
    pub prompts: Kept<String>,
    /// The tool sets the agent's model was offered in this channel, by
    /// digest, each offer kept once: what the `FrameRef`s of its contexts
    /// point to (issues 137, 194). Kept as the prompt parts are.
    #[serde(skip)]
    pub offered_tools: Kept<Vec<ToolSet>>,
    /// The frames of the contexts the agent's messages in this channel were
    /// produced from (`FrameRef`), by digest, each kept once: what the
    /// messages in the history point to beside their transcript (issue
    /// 426). Kept as the prompt parts are, but held only until stored: the
    /// turn under way holds its own whole (`Work::context`).
    #[serde(skip)]
    pub frames: Kept<FrameRef>,
    /// What an external agent keeps of the remote side (issue 179), empty
    /// for an agent run here.
    #[serde(default, skip_serializing_if = "RemoteState::is_empty")]
    pub remote: RemoteState,
    /// What the agent recorded that its channel is to be told of (issue 417), each with the thread
    /// whose branch it was recorded on, where it is told (issue 602): what it was given to read,
    /// where it stopped reading a part given (issue 537), a compaction of its context - for the
    /// channel's history to hold what the agent saw where it saw it. Taken by its actor after every
    /// step and sent (`take_told`); not stored: one cut off by a crash is in the transcript alone.
    #[serde(skip)]
    pub to_tell: Vec<(ThreadId, ActorEvent)>,
    /// The last of its channel's messages the agent took in - heard, said
    /// or was given (issue 417): where it stands in its channel's history,
    /// which it catches up with from there on, and where a turn's changes
    /// were made. Kept on its own, since its transcript holds the active
    /// branch alone, which begins anew with every call of an external agent
    /// and every compaction. `None` before the first.
    pub last_message: Option<MessageId>,
    /// Where the agent stands in its channel's history (issue 223): what
    /// of it was delivered to it and taken in. Written as it takes in, so
    /// that every put stores it with what it took in; what its channel
    /// delivers on from after a restart - what the stored state lacks,
    /// nothing twice.
    pub heard: Heard,
    /// The numbers its transcript gives the attachments it holds (issue
    /// 282): how its model names them.
    #[serde(default, skip_serializing_if = "AttachmentIndex::is_empty")]
    pub attachments: AttachmentIndex,
}

impl AgentChannelState {
    /// The agent's part of `thread`, `None` if it has none there yet.
    pub fn thread(&self, thread: ThreadId) -> Option<&AgentThreadState> {
        self.threads.get(&thread)
    }

    /// The agent's part of `thread`, to change it; begun empty if it has none yet.
    pub fn thread_mut(&mut self, thread: ThreadId) -> &mut AgentThreadState {
        self.threads.entry(thread).or_default()
    }

    /// The threads it holds a part or a branch of (issue 602).
    pub fn threads_held(&self) -> BTreeSet<ThreadId> {
        self.threads.keys().copied().chain(self.transcript.threads()).collect()
    }

    /// Takes in that it was taken into `thread` (issue 602): its branch there, new to it, begins
    /// as its branch of the thread `thread` hangs at stood at `origin`'s message, forked
    /// (`Transcript::fork`), or at the root without such a branch - or for the line, which hangs
    /// nowhere. Nothing for a thread it holds already.
    pub fn join_thread(&mut self, thread: ThreadId, origin: Option<ThreadOrigin>) {
        if self.threads_held().contains(&thread) {
            return;
        }
        if let Some(origin) = origin {
            self.transcript.fork(thread, origin);
        }
    }

    /// Takes in that it was let go from `thread` (issue 602): its part and its branch there go -
    /// what it held stays in its store, what its calls there used with it.
    pub fn leave_thread(&mut self, thread: ThreadId) {
        self.threads.remove(&thread);
        self.transcript.start_anew(thread);
    }

    /// The turn under way in `thread`, `None` between turns there.
    pub fn turn(&self, thread: ThreadId) -> Option<&TurnState> {
        self.thread(thread).and_then(|part| part.turn.as_ref())
    }

    /// As `turn`, to change it.
    pub fn turn_mut(&mut self, thread: ThreadId) -> Option<&mut TurnState> {
        self.threads.get_mut(&thread).and_then(|part| part.turn.as_mut())
    }

    /// The turns under way in all its threads.
    pub fn turns(&self) -> impl Iterator<Item = &TurnState> {
        self.threads.values().filter_map(|part| part.turn.as_ref())
    }

    /// What its calls used in all its threads (issue 602): the calls and tokens summed, the
    /// context of the thread whose context is fullest (`TokenUsage::filled`). `None` while no call
    /// reported its tokens.
    pub fn used(&self) -> Option<TokenUsage> {
        self.threads.values().filter_map(|part| part.used).reduce(|all, used| {
            let fuller = if used.filled() > all.filled() { used } else { all };
            TokenUsage {
                calls: all.calls + used.calls,
                prompt_tokens: all.prompt_tokens + used.prompt_tokens,
                completion_tokens: all.completion_tokens + used.completion_tokens,
                ..fuller
            }
        })
    }

    /// Lets go of what it holds only for its store, once it is stored
    /// (issue 380).
    ///
    /// Of the transcript, what no head reaches (`Transcript::keep_window`):
    /// every reader in memory reads the branch of a thread, and what lies
    /// elsewhere is read from the store (`TranscriptStore`). Of the prompt
    /// texts and the tool sets those its turns under way name (issue 383):
    /// their calls read them, and between turns nothing does; what an older
    /// message's context names is read from the store. Of the frames none
    /// (issue 426): a turn under way holds its own.
    pub fn keep_window(&mut self) {
        self.transcript.keep_window();
        let contexts: Vec<_> = self.turns().map(|turn| &turn.work.context).collect();
        let named: HashSet<Digest> = contexts
            .iter()
            .flat_map(|context| context.frame.prompts.iter().map(|prompt| prompt.text))
            .collect();
        let offered: HashSet<Digest> =
            contexts.iter().filter_map(|context| context.frame.tools).collect();
        self.prompts.keep_only(|digest| named.contains(digest));
        self.offered_tools.keep_only(|digest| offered.contains(digest));
        self.frames.keep_only(|_| false);
    }

    /// Makes a loaded state fit to run from (issue 75): its part of every thread
    /// (`AgentThreadState::prepare_resume`), each told what its model is given of a call not
    /// executed by `not_executed` of its turn. Returns how many calls were answered so.
    pub fn prepare_resume(&mut self, not_executed: impl Fn(&TurnState) -> String) -> usize {
        self.threads.values_mut().map(|part| part.prepare_resume(&not_executed)).sum()
    }

    /// Whether the agent has work under way that goes on by itself once its
    /// channel runs again (issue 388): a turn in one of its threads stored
    /// at a step it resumes from (`AgentThreadState::has_work_under_way`),
    /// or a remote task the remote agent works on. What a store keeps an
    /// index of, so that a restart finds the sessions to start.
    pub fn has_work_under_way(&self) -> bool {
        self.threads.values().any(AgentThreadState::has_work_under_way)
            || self.remote.has_work_under_way()
    }

    /// Takes in what it perceived of what others said (issues 196, 206), the same for both kinds of
    /// agent - what passed its perceive processors - onto the branch of `thread`, which a turn
    /// under way there has then seen (issue 168), their attachments numbered (issue 282). What was
    /// offered to it is in its channel's history.
    pub fn perceive(&mut self, thread: ThreadId, mut perceived: Vec<Message>) {
        perceived.iter_mut().for_each(|message| self.attachments.number(message));
        self.transcript.extend(thread, perceived.clone());
        // A turn under way in the thread has seen it, and its next message
        // is produced from the transcript with it (issue 260).
        let head = self.transcript.digest(thread);
        if let Some(turn) = self.turn_mut(thread) {
            perceived.iter().for_each(|m| turn.work.see_message(m));
            turn.work.context.transcript = head;
        }
    }

    /// Takes in `modified`, a change of a message it heard or said, into its
    /// transcript (`Transcript::modify`), the attachments it adds numbered
    /// (issue 282) - one it keeps keeps its number -, if the transcript
    /// takes it: what it does not take is given no number. `false` as for
    /// `Transcript::modify`.
    pub fn modify(&mut self, mut modified: Message) -> bool {
        if !self.transcript.takes(&modified) {
            return false;
        }
        self.attachments.number(&mut modified);
        self.record_change(TranscriptEntry::Modified { message: modified })
    }

    /// Records `change` - a modification, a taking back or a reclassification of a message, of the
    /// whole channel - on every branch that holds the message (`Transcript::holding`): at once,
    /// but on the branch of a turn whose step's calls have no results yet - one set aside, waiting
    /// for a person (issue 602) - after the step's tool message, since nothing may stand between
    /// calls and their results. `false`, and nothing is recorded, if no branch holds it.
    pub fn record_change(&mut self, change: TranscriptEntry) -> bool {
        let holding = self.transcript.holding(&change);
        for &thread in &holding {
            match self.turn_mut(thread).filter(|turn| !turn.tool_calls.is_empty()) {
                Some(turn) => turn.after_step.push(AfterStep::Noted { entry: change.clone() }),
                None => self.transcript.record_change(thread, change.clone()),
            }
        }
        !holding.is_empty()
    }

    /// Takes note of `entry` of the channel's history, which the agent is told of rather than
    /// hears - who joined or left (issue 408) -, the same for both kinds of agent: into the branch
    /// of `thread`, where its model is told of it (`TranscriptEntry::of_announced`). A turn under
    /// way there has seen it, as what is heard (`perceive`). Any other entry is not noted.
    ///
    /// While the calls of a step of that turn run, it is put aside and written after the step's
    /// tool message, as what the turn is given (`given`, issue 482).
    pub fn note_announced(&mut self, thread: ThreadId, entry: HistoryEntry) {
        let Some(told) = TranscriptEntry::of_announced(&entry) else {
            warn!("An entry of the history that is not announced was to be noted");
            return;
        };
        if let Some(turn) = self.turn_mut(thread).filter(|turn| !turn.tool_calls.is_empty()) {
            turn.after_step.push(AfterStep::Noted { entry: told });
            return;
        }
        self.note(thread, told);
    }

    /// Records `told`, who joined or left or a change of a message put aside after a step, into
    /// the branch of `thread`, and moves the turn under way there, if any, on past it.
    fn note(&mut self, thread: ThreadId, told: TranscriptEntry) {
        if told.changed().is_some() {
            self.transcript.record_change(thread, told);
        } else {
            self.transcript.record(thread, told);
        }
        let head = self.transcript.digest(thread);
        if let Some(turn) = self.turn_mut(thread) {
            turn.work.context.transcript = head;
        }
    }

    /// Records `message`, said by the turn under way in `thread` (issues 136, 260): the
    /// turn's from now on (`Message::turn_id`, issue 235), whoever built it,
    /// into the branch of `thread`, and into the turn's answer unless it is a step
    /// (`TurnState::answer`), and the context moved on past it. Kept as it
    /// is said, so that nothing of it waits for the turn's end. Once it ends
    /// a step - its calls have their results -, what came to the turn while
    /// they ran follows it (`TurnState::after_step`): what it was given
    /// (`given`, issue 389) and who joined or left (`note_announced`, issue
    /// 482). Returns the message
    /// as the agent sends it: with what it was produced from - the turn's
    /// context as it stood, its frame kept once by digest (issue 426) - for
    /// its channel's history to hold (issue 417); its attachments numbered
    /// on the transcript's copy alone (issue 282).
    /// Between turns, which says nothing, it is recorded as heard would be,
    /// without a context.
    pub fn said(&mut self, thread: ThreadId, mut message: Message) -> Message {
        let part = self.thread_mut(thread);
        part.used = TokenUsage::counted(part.used, &message);
        if let Some(turn) = &part.turn {
            message.turn_id = Some(turn.work.turn_id);
        }
        self.last_message = Some(message.id);
        let mut numbered = message.clone();
        self.attachments.number(&mut numbered);
        self.transcript.push(thread, numbered);
        let head = self.transcript.digest(thread);
        let Some(turn) = self.turn_mut(thread) else {
            tracing::warn!("A message was said by an agent without a turn under way");
            return message;
        };
        let context = turn.work.context.clone();
        if !message.internal {
            turn.answer.push(message.clone());
        }
        turn.work.context.transcript = head;
        if turn.tool_calls.is_empty() {
            for put_aside in mem::take(&mut turn.after_step) {
                match put_aside {
                    AfterStep::Given { message, knowledge } => {
                        self.given(thread, message, knowledge);
                    }
                    AfterStep::Noted { entry } => self.note(thread, entry),
                }
            }
        }
        let frame = self.keep_frame(&context.frame);
        message.produced_from_context(context.transcript, frame)
    }

    /// Records `message`, given to the agent to read in the turn under way in `thread` (issue 22) -
    /// a skill loaded for it - as the knowledge `knowledge`, if it is knowledge (issue 389): the
    /// turn's from now on (`Message::turn_id`), into the branch of `thread` as what it was given
    /// (`TranscriptEntry::Given`, issue 16), for its channel to be told as given
    /// (`ActorEvent::MessageGiven`, issue 417) - the agent did not produce it -, the knowledge
    /// recorded with the turn, and the turn's context moved on past it, so that what the turn says
    /// next is produced from it. Not published: it is what the agent read, not what it said.
    /// Between turns it is recorded as heard would be, still as given.
    ///
    /// Knowledge that stands given already (`stands_given`) is not given a second time (issue
    /// 541): the model has it before it. Its knowledge is recorded with the turn all the same, and
    /// `false` answered; `true` where it is given. What is no knowledge is always given.
    ///
    /// While the calls of a step run, it is put aside and written after the step's tool message
    /// (`TurnState::after_step`, issue 389): a tool gives what it gives at any time, and nothing
    /// stands between the calls and their results, which the models refuse. Knowledge put aside
    /// twice is given once, after the step.
    pub fn given(
        &mut self,
        thread: ThreadId,
        message: Message,
        knowledge: Option<KnowledgeRef>,
    ) -> bool {
        let stands =
            knowledge.as_ref().is_some_and(|k| self.stands_given(thread, &message, &k.source));
        if let Some(turn) = self.turn_mut(thread).filter(|turn| !turn.tool_calls.is_empty()) {
            turn.after_step.push(AfterStep::Given { message, knowledge });
            return !stands;
        }
        let source =
            knowledge.as_ref().map(|knowledge| GivenSource::Knowledge(knowledge.source.clone()));
        if let (Some(turn), Some(knowledge)) = (self.turn_mut(thread), knowledge) {
            turn.work.record_knowledge(knowledge);
        }
        if !stands {
            self.give_as(thread, message, source);
        }
        !stands
    }

    /// Whether `message`, giving the knowledge `source`, stands given already: the same knowledge
    /// with the same text and classification (`alike`), on the branch of `thread` as the
    /// model is given it (`given_as_they_stand`) or put aside by the turn under way there to be
    /// given after its step.
    fn stands_given(&self, thread: ThreadId, message: &Message, source: &KnowledgeSource) -> bool {
        let on_path = given_as_they_stand(self.transcript.entries(thread))
            .into_iter()
            .any(|(node, given)| node.entry.knowledge() == Some(source) && alike(&given, message));
        let turn = self.turn(thread);
        let put_aside = turn.iter().flat_map(|turn| &turn.after_step).any(|put| {
            matches!(put, AfterStep::Given { message: given, knowledge: Some(k) }
                if k.source == *source && alike(given, message))
        });
        on_path || put_aside
    }

    /// The digest of `place` within the entity of kind `T` with `id` as the agent last knew it
    /// (issue 538): of its last knowledge of that place that holds one - what a tool read or wrote
    /// of it, what it was given to read -, on the branch of `thread` as the model sees it
    /// (`as_seen_whole`), then among the results of the step under way there and what is put aside
    /// to be given after it, as `stands_given` reads what is given. `None` if it never knew the
    /// content of the place: a listing or a search holds none.
    pub fn last_known<T: store::Stored>(
        &self,
        thread: ThreadId,
        id: T::Id,
        place: &str,
    ) -> Option<Digest> {
        let path = knowledge_as_seen_whole(self.transcript.entries(thread));
        let mut known: Vec<KnowledgeSource> = path.into_iter().flat_map(|(_, k)| k).collect();
        if let Some(turn) = self.turn(thread) {
            let results = turn.tool_calls.iter().filter_map(ToolCallState::result);
            known.extend(results.flat_map(|r| &r.knowledge).map(|k| k.source.clone()));
            let put_aside = turn.after_step.iter().filter_map(|put| match put {
                AfterStep::Given { knowledge, .. } => knowledge.as_ref(),
                AfterStep::Noted { .. } => None,
            });
            known.extend(put_aside.map(|k| k.source.clone()));
        }
        let of_place = |k: &&KnowledgeSource| {
            k.place() == Some(place) && k.about::<T>().is_some_and(|(of, _)| of == id)
        };
        known.iter().filter(of_place).filter_map(KnowledgeSource::read_whole).next_back()
    }

    /// Makes `parts` - each the id of a prompt component and the message of its part - what
    /// stands of the parts of prompt components on the branch of `thread`, as the model is
    /// given them (`parts_as_given`), at the start of the turn under way there (issue 537). A part
    /// stands where its component's message stands with the same text and classification: it
    /// stays where it is. One that changed or is new is given (`TranscriptEntry::Given`, as `given`
    /// gives). One that stands and whose component has none like it now - it changed, it holds
    /// nothing, its component failed open, or the agent's personality no longer has the
    /// component - is taken back (`TranscriptEntry::Revoked`), so that the model is never given
    /// what no longer holds, for its channel to be told as withdrawn
    /// (`ActorEvent::MessageWithdrawn`).
    pub fn give_parts(&mut self, thread: ThreadId, parts: Vec<(String, Message)>) {
        let standing: Vec<(String, Message)> = parts_as_given(self.transcript.entries(thread))
            .into_iter()
            .filter_map(|(node, message)| Some((node.entry.part_of()?.to_string(), message)))
            .collect();
        let among = |(component, message): &(String, Message), others: &[(String, Message)]| {
            others.iter().any(|(c, other)| c == component && alike(message, other))
        };
        let at = Utc::now();
        for stood in &standing {
            if !among(stood, &parts)
                && self.record_change(TranscriptEntry::Revoked { message: stood.1.id, at })
            {
                let withdrawn = ActorEvent::MessageWithdrawn { message: stood.1.id, at };
                self.to_tell.push((thread, withdrawn));
            }
        }
        for part in parts {
            if !among(&part, &standing) {
                let (component, message) = part;
                self.give_as(thread, message, Some(GivenSource::Part(component)));
            }
        }
        let head = self.transcript.digest(thread);
        if let Some(turn) = self.turn_mut(thread) {
            turn.work.context.transcript = head;
        }
    }

    /// Records `message`, given to the agent as `source`, as `given` describes: the turn's, seen
    /// by it, into the transcript, for the channel to be told.
    fn give_as(&mut self, thread: ThreadId, mut message: Message, source: Option<GivenSource>) {
        if let Some(turn) = self.turn_mut(thread) {
            message.turn_id = Some(turn.work.turn_id);
            turn.work.see_message(&message);
        }
        self.transcript.give(thread, message.clone(), source);
        self.last_message = Some(message.id);
        self.to_tell.push((thread, ActorEvent::MessageGiven { message }));
        let head = self.transcript.digest(thread);
        if let Some(turn) = self.turn_mut(thread) {
            turn.work.context.transcript = head;
        }
    }

    /// Replaces the context of the turn under way in `thread` of `actor` - the
    /// agent whose state this is - (issue 16): the branch of `thread` begins anew
    /// with `replacement` - what stands for the older messages: a summary, the
    /// skills given again, each given to read with the knowledge it is, if
    /// it is one -, the parts of prompt components that stand before the
    /// entries kept (`Transcript::carried_parts`, issue 537), and the entries
    /// of its branch from `keep_from` on - `None`: none - as they stand
    /// (`Transcript::compact`). The turn's context moves to the new head,
    /// and the history records the compaction (`HistoryEntry::Compacted`),
    /// then the replacement as messages the agent was given. Returns the
    /// heads of the transcript before and after.
    ///
    /// Nothing is removed: every reference recorded before resolves to what
    /// it did. The replacement is seen by the turn; whose it is
    /// (`Message::turn_id`) is the caller's to say (issue 284). The prompt
    /// parts the turn placed after an entry are carried over (issue 284) to
    /// where the entry stands on the new branch: one after an entry kept, or
    /// after a part kept, follows its copy, one after an entry replaced
    /// follows the replacement.
    ///
    /// How full the new branch is, it estimates as the model is given it,
    /// its notices worded by `catalogue`.
    ///
    /// `None`, and nothing changes, without a turn under way, or if the
    /// transcript is not compacted (`Transcript::compact`).
    pub fn compact(
        &mut self,
        thread: ThreadId,
        actor: ActorId,
        replacement: Vec<(Message, Option<KnowledgeRef>)>,
        keep_from: Option<Digest>,
        catalogue: &Arc<PromptCatalogue>,
    ) -> Option<(Digest, Digest)> {
        let part = self.threads.get_mut(&thread)?;
        let turn = part.turn.as_mut()?;
        // A compaction comes before a call of the model, never while the
        // calls of a step run: nothing is put aside to be given (issue 389).
        debug_assert!(turn.after_step.is_empty());
        let from = self.transcript.digest(thread)?;
        let given = replacement
            .iter()
            .map(|(message, knowledge)| {
                let source = knowledge.as_ref().map(|knowledge| knowledge.source.clone());
                (message.clone(), source.map(GivenSource::Knowledge))
            })
            .collect();
        let moved = self.transcript.compact(thread, given, keep_from)?;
        let to = self.transcript.digest(thread)?;
        let compacted = ActorEvent::ContextCompacted { turn_id: turn.work.turn_id, from, to };
        self.to_tell.push((thread, compacted));
        for (message, knowledge) in replacement {
            turn.work.see_message(&message);
            // A summary written by the agent's model is a call of it.
            if message.sender_id == actor {
                part.used = TokenUsage::counted(part.used, &message);
            }
            if let Some(knowledge) = knowledge {
                turn.work.record_knowledge(knowledge);
            }
            self.last_message = Some(message.id);
            self.to_tell.push((thread, ActorEvent::MessageGiven { message }));
        }
        turn.work.context.transcript = Some(to);
        turn.work.context.frame.prompts.retain_mut(|prompt| match prompt.at {
            PromptAt::Start | PromptAt::End => true,
            PromptAt::After(after) => {
                moved.get(&after).map(|now| prompt.at = PromptAt::after(*now)).is_some()
            }
        });
        // How full the new branch is: no call measured it yet, and the
        // summary's measured the old one. Estimated once, here, with what the
        // turn's model is given next (issue 390) - a text that cannot be had
        // leaves it unestimated, and fails the next call of the model.
        let estimated = self
            .turn(thread)
            .and_then(|turn| {
                ContextResolver::of(self, thread, catalogue)
                    .token_positions(&turn.work.context, turn.work.turn_id, actor)
                    .ok()
                    .flatten()
            })
            .and_then(|(positions, _)| positions.last().copied());
        let used = self.threads.get_mut(&thread).and_then(|part| part.used.as_mut());
        if let (Some(used), Some(estimated)) = (used, estimated) {
            used.context_tokens = estimated;
        }
        Some((from, to))
    }

    /// What answering again instead of the message `id` of the agent `own`
    /// takes back (issue 234): the message, and what follows it on the
    /// branch of `thread` - which may only be the rest of its
    /// turn (`Message::turn_id`, issue 241), besides changes to what was
    /// said. So the message is its last answer, and the model is given the
    /// transcript as it stood before it, in what stands of it now. Every id
    /// returned is what it announces as taken back: its channel's history
    /// holds every message it said (issue 417).
    ///
    /// # Errors
    /// Why it does not answer again: the message is not an answer of its
    /// own its model is still given, something else was said or recorded
    /// since, or what would be taken back holds calls of tools that ran -
    /// their effect stays, and the model would no longer know of it.
    pub fn retried(
        &self,
        thread: ThreadId,
        id: MessageId,
        own: ActorId,
    ) -> Result<Vec<MessageId>, RetryRefused> {
        let mut path = as_they_stand(self.transcript.entries(thread))
            .skip_while(|(_, m)| m.as_ref().is_none_or(|m| m.id != id));
        let Some((_, Some(message))) = path.next() else { return Err(RetryRefused::NotItsAnswer) };
        let Some(turn) = message
            .turn_id
            .filter(|_| message.sender_id == own && message.role == MessageRole::Assistant)
        else {
            return Err(RetryRefused::NotItsAnswer);
        };
        let mut taken_back = vec![message.id];
        for (node, standing) in path {
            match (&node.entry, standing) {
                (
                    TranscriptEntry::Modified { .. }
                    | TranscriptEntry::Revoked { .. }
                    | TranscriptEntry::Reclassified { .. },
                    _,
                ) => {}
                // Its own turn's end.
                (TranscriptEntry::TurnFinished { turn_id, .. }, _) if *turn_id == turn => {}
                // Taken back already.
                (TranscriptEntry::Message(_), None) => {}
                (TranscriptEntry::Message(_), Some(step)) if step.turn_id == Some(turn) => {
                    if !executed_calls(&[&step]).is_empty() {
                        return Err(RetryRefused::CallsRan);
                    }
                    taken_back.push(step.id);
                }
                // Anything else since - a compaction too: the message may be
                // in a summary already (issue 387) -, someone who joined or
                // left as well.
                (
                    TranscriptEntry::Message(_)
                    | TranscriptEntry::Given { .. }
                    | TranscriptEntry::TurnFinished { .. }
                    | TranscriptEntry::TurnGivenUp { .. }
                    | TranscriptEntry::Compacted { .. }
                    | TranscriptEntry::ParticipantJoined { .. }
                    | TranscriptEntry::ParticipantLeft { .. },
                    _,
                ) => return Err(RetryRefused::NotLast),
            }
        }
        Ok(taken_back)
    }

    /// Every classification the remote agent of an external agent was
    /// given in this channel (issue 236), with those of what it is given
    /// now: every entry of the branch of `thread` that records
    /// something of a message, recorded as given
    /// (`RemoteState::give_classified`). Every branch before was the
    /// thread's when its call was made - the branch starts anew with each
    /// (`Transcript::start_anew`) -, so what it was given before is recorded
    /// already, and only the branch is read (issue 376).
    pub fn classifications_given(&mut self, thread: ThreadId) -> &[Classification] {
        for classification in
            self.transcript.entries(thread).filter_map(|node| node.entry.classification())
        {
            self.remote.give_classified(classification);
        }
        &self.remote.given_classifications
    }

    /// Keeps the text of a prompt part the agent was given once by digest
    /// (issue 47), and returns the reference to it: given in `role` at `at`,
    /// its text classified `classification`.
    pub fn keep_prompt(
        &mut self,
        role: MessageRole,
        text: String,
        at: PromptAt,
        classification: Option<Classification>,
    ) -> PromptRef {
        let digest = digest::prompt(&text);
        self.prompts.keep(digest, || text);
        PromptRef { text: digest, role, at, classification }
    }

    /// Keeps the tool sets the agent's model is offered once by digest
    /// (issues 137, 194), and returns the digest that names the offer:
    /// what a `FrameRef` records; `None` for no sets, an offer of nothing.
    pub fn keep_offer(&mut self, tool_sets: Vec<ToolSet>) -> Option<Digest> {
        (!tool_sets.is_empty()).then(|| {
            let digest = digest::tool_sets(&tool_sets);
            self.offered_tools.keep(digest, || tool_sets);
            digest
        })
    }

    /// Keeps the frame of a context a message was produced from once by
    /// digest (issue 426), and returns the digest that names it: what the
    /// message records beside the digest of its transcript.
    pub fn keep_frame(&mut self, frame: &FrameRef) -> Digest {
        let digest = digest::frame(frame);
        self.frames.keep(digest, || frame.clone());
        digest
    }

    /// Begins a turn (issue 85) in `thread` that does `work` - begun by whoever answers
    /// (`Work::new`): the envelope it answers and why it was asked to (issue 439), whose authority
    /// it acts with, the channel's classification and its session's time zone (issue 424) as it
    /// began, and how the agent's model is given its context (`Work::given_as`, issues 49, 63).
    /// The turn adds what only it knows: the branch of `thread` the model is given first, which the
    /// turn has seen (issue 168). What its model is offered is added once it is begun
    /// (`offer_tools`), since what may be called depends on the work. All but the envelope and the
    /// authority hold for the whole turn - also one resumed after the model's configuration
    /// changed - so that what it reads, publishes and remembers is judged alike, and what its
    /// model is given too. Whatever a previous turn left behind in `thread` is gone. Returns the
    /// turn's id (issue 235).
    pub fn begin_turn(&mut self, thread: ThreadId, mut work: Work) -> TurnId {
        work.context.transcript = self.transcript.digest(thread);
        self.transcript.messages(thread).iter().for_each(|m| work.see_message(m));
        let turn_id = work.turn_id;
        let turn = TurnState { work, ..TurnState::default() };
        if self.thread_mut(thread).turn.replace(turn).is_some() {
            tracing::warn!("A turn was begun while another was under way in its thread; the other ends without an end recorded");
        }
        turn_id
    }

    /// Offers the model of the turn under way in `thread` the tools in `tool_sets` (issues 137,
    /// 194, 290): kept once by digest and named by the turn's context. Once, right after the turn
    /// was begun and before its context is built - what it offers holds for the whole turn, also
    /// one resumed after the agent's tools changed. A turn offered tools already keeps them;
    /// without a turn, nothing is offered.
    pub fn offer_tools(&mut self, thread: ThreadId, tool_sets: Vec<ToolSet>) {
        let turn = self.turn(thread);
        if turn.is_none_or(|turn| turn.work.context.frame.tools.is_some()) {
            tracing::warn!("Tools were offered with no turn under way, or to a turn offered tools already; nothing is offered");
            return;
        }
        let offered = self.keep_offer(tool_sets);
        if let Some(turn) = self.turn_mut(thread) {
            turn.work.context.frame.tools = offered;
        }
    }

    /// Begins a turn that carries out `request` (issue 269): as `begin_turn`
    /// does, offering no tools - a command turn offers its model nothing -
    /// and the command kept with the turn, so that one resumed after a
    /// restart knows what to do.
    pub fn begin_command_turn(
        &mut self,
        thread: ThreadId,
        work: Work,
        request: CommandRequest,
    ) -> TurnId {
        let turn_id = self.begin_turn(thread, work);
        if let Some(turn) = self.turn_mut(thread) {
            turn.command = Some(request);
        }
        turn_id
    }

    /// Whether the agent answers anew after its turn `turn`, given up (issue
    /// 259): the turn's end is the last thing on the branch of `thread` in the
    /// transcript, and it was given up for a reason that may be answered
    /// again (`GivenUpReason::may_be_answered_again`). Nothing is taken
    /// back: what the turn said stays.
    ///
    /// # Errors
    /// `NotLast` if something followed the turn's end; `Ended` if it was
    /// given up for another reason, or taken over; `NotItsAnswer` if the
    /// transcript holds no end of that turn.
    pub fn answers_again_after(&self, thread: ThreadId, turn: TurnId) -> Result<(), RetryRefused> {
        let ended = |entry: &TranscriptEntry| entry.ended_turn() == Some(turn);
        match self.transcript.last(thread).map(|node| &node.entry) {
            Some(TranscriptEntry::TurnGivenUp { turn_id, reason, .. }) if *turn_id == turn => {
                if reason.may_be_answered_again() {
                    Ok(())
                } else {
                    Err(RetryRefused::Ended)
                }
            }
            Some(entry) if ended(entry) => Err(RetryRefused::Ended),
            _ if self.transcript.entries(thread).any(|node| ended(&node.entry)) => {
                Err(RetryRefused::NotLast)
            }
            _ => Err(RetryRefused::NotItsAnswer),
        }
    }

    /// Takes the turn `turn_id` over (issues 06, 17, 258): its changes to
    /// the agent's `notes` and `private_notes` here, anchored at the last
    /// message it took in (`last_message`), and its end,
    /// `TurnFinished`, after what it said on the branch of `thread` - which
    /// is in the transcript already (`said`, issue 260). What the turn carried out, and how it
    /// ended, the channel's history records from what the agent sends.
    /// Returns whether there were changes to the notes.
    pub fn finish_turn(
        &mut self,
        thread: ThreadId,
        turn_id: TurnId,
        notes: Vec<NoteChange>,
        private_notes: Vec<NoteChange>,
    ) -> bool {
        // What branching (issue 08) will find the changes by: the turn's
        // last message, and the transcript as the turn left it.
        let after = self.last_message.unwrap_or_default();
        let origin = NoteOrigin { transcript: self.transcript.digest(thread) };
        let changed = !notes.is_empty() || !private_notes.is_empty();
        for change in notes {
            self.notes.push(change, origin.clone(), after);
        }
        for change in private_notes {
            self.private_notes.push(change, origin.clone(), after);
        }
        self.transcript.record(thread, TranscriptEntry::TurnFinished { turn_id, at: Utc::now() });
        changed
    }

    /// Records that the turn `turn_id` was given up for `reason` (issue
    /// 258): its end, `TurnGivenUp`, after what it said on the branch of
    /// `thread`, which stays - also
    /// for a turn that said nothing. What it staged is not applied.
    pub fn give_up_turn(&mut self, thread: ThreadId, turn_id: TurnId, reason: GivenUpReason) {
        let given_up = TranscriptEntry::TurnGivenUp { turn_id, reason, at: Utc::now() };
        self.transcript.record(thread, given_up);
    }

    /// What the agent recorded that its channel is to be told of, each in its thread (`to_tell`),
    /// taken.
    pub fn take_told(&mut self) -> Vec<(ThreadId, ActorEvent)> {
        mem::take(&mut self.to_tell)
    }

    /// The approvals its turns wait for (issue 172), in all its threads.
    pub fn pending_approvals(&self) -> Vec<ApprovalRequest> {
        self.turns().flat_map(TurnState::pending_approvals).collect()
    }

    /// The questions its turns wait for the answers to (issue 165), in all its threads.
    pub fn pending_questions(&self) -> Vec<QuestionRequest> {
        self.turns().flat_map(TurnState::pending_questions).collect()
    }
}

/// Whether `a` and `b`, given to read, give the model the same: the same text, classified alike - a
/// prompt part or knowledge that stands given where its message is alike (`give_parts`,
/// `stands_given`).
fn alike(a: &Message, b: &Message) -> bool {
    a.content == b.content && a.classification == b.classification
}

/// Why an agent does not answer again instead of one of its messages (issue
/// 234), as `ActorEvent::RetryRefused` tells it: a closed set, so that a
/// user interface can word and react to each itself. The text is for logs
/// and the TUI, after "… cannot answer again:".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum RetryRefused {
    #[error("it is not an answer of its own that it could give again")]
    NotItsAnswer,
    #[error("more was said or done since")]
    NotLast,
    #[error("calls of tools that ran would be taken back with it")]
    CallsRan,
    /// A later request, to respond or to answer again, came with it and is
    /// answered instead.
    #[error("it was asked again since")]
    AskedAgain,
    /// An external agent's remote side keeps what it was given (issue 205).
    #[error("it runs elsewhere and cannot take back what it was given")]
    Elsewhere,
    /// The answer asked for again while under way (issue 235) had ended
    /// when the request came: a cancel came first, or it failed.
    #[error("its answer had already ended")]
    Ended,
}

/// A change to an [`AgentChannelState`] since it was last stored (issue
/// 193): a variant per part of the state. A change of the agent's *state* -
/// what it heard, said, noted - as against an update of the agent itself.
/// The growing parts record what was added, the small ones are recorded
/// whole. The history and the transcript are kept apart by a store
/// (issues 379, 380).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentChannelStateChange {
    Notes(Stored<LedgerEntry<Notes>>),
    PrivateNotes(Stored<LedgerEntry<Notes>>),
    /// Its part of every thread, whole (issue 602): each changed in place at every step of its turn
    /// (`prepare_resume`, the turn's own methods), and bounded by a turn per thread.
    Threads(BTreeMap<ThreadId, AgentThreadState>),
    /// Whether the agent sleeps in its channel, whole (issue 602).
    Asleep(bool),
    /// What an external agent keeps of the remote side, whole: small, and
    /// bounded by its open tasks.
    Remote(RemoteState),
    /// The last of its channel's messages the agent took in, whole.
    LastMessage(Option<MessageId>),
    /// Where the agent stands in its channel's history, whole: small, and
    /// moved at almost every put (issue 440).
    Heard(Heard),
    /// The attachments its transcript numbered since stored, each with the
    /// message that carries it (issue 282).
    Attachments(Vec<(MessageId, ContentId)>),
}

impl Journaled for AgentChannelState {
    type Change = AgentChannelStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        let mut changes: Vec<Self::Change> = Vec::new();
        changes.extend(self.notes.changes().map(AgentChannelStateChange::Notes));
        changes.extend(self.private_notes.changes().map(AgentChannelStateChange::PrivateNotes));
        changes.push(AgentChannelStateChange::Threads(self.threads.clone()));
        changes.push(AgentChannelStateChange::Asleep(self.asleep));
        changes.push(AgentChannelStateChange::Remote(self.remote.clone()));
        changes.push(AgentChannelStateChange::LastMessage(self.last_message));
        changes.push(AgentChannelStateChange::Heard(self.heard.clone()));
        if !self.attachments.changes().is_empty() {
            changes.push(AgentChannelStateChange::Attachments(self.attachments.changes().to_vec()));
        }
        changes
    }

    /// The transcript, the prompt parts and the tool sets are marked stored
    /// by the store that wrote them.
    fn mark_stored(&mut self) {
        self.notes.mark_stored();
        self.private_notes.mark_stored();
        self.attachments.mark_stored();
    }

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            AgentChannelStateChange::Notes(change) => self.notes.apply(change)?,
            AgentChannelStateChange::PrivateNotes(change) => self.private_notes.apply(change)?,
            AgentChannelStateChange::Threads(threads) => self.threads = threads,
            AgentChannelStateChange::Asleep(asleep) => self.asleep = asleep,
            AgentChannelStateChange::Remote(remote) => self.remote = remote,
            AgentChannelStateChange::LastMessage(message) => self.last_message = message,
            AgentChannelStateChange::Heard(heard) => self.heard = heard,
            AgentChannelStateChange::Attachments(given) => self.attachments.apply(given),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{ActorState, EventContext};
    use crate::agent::AgentId;
    use crate::chain::Position;
    use crate::confidentiality::Confidentiality;
    use crate::context::{ContextResolver, KnowledgeSource};
    use crate::digest;
    use crate::message::testing::{id, make_message};
    use crate::note::NoteId;
    use crate::prompt_catalogue::testing::kernel_catalogue;
    use crate::question::{Addressee, Question, Questions};
    use crate::store::testing::{replayed, stored};
    use crate::tool::testing::{make_tool_call, make_tool_description};
    use crate::tool::{ToolCall, ToolCallState, ToolDescription};
    use crate::tool::{ToolCallResult, ToolCallStatus, ToolResultStatus};
    use crate::user::UserId;
    use crate::workspace::{Workspace, WorkspaceId};

    /// The turn under way in the thread the tests run in, `ThreadId::default()`, to change it.
    fn turn_slot(state: &mut AgentChannelState) -> &mut Option<TurnState> {
        &mut state.thread_mut(ThreadId::default()).turn
    }

    /// A state whose part of `thread` holds `turn`, standing at `step`.
    fn in_thread(thread: ThreadId, step: ActorState, turn: Option<TurnState>) -> AgentChannelState {
        let turn = turn.map(|turn| TurnState { step, ..turn });
        let part = AgentThreadState { turn, used: None };
        AgentChannelState {
            threads: BTreeMap::from([(thread, part)]),
            ..AgentChannelState::default()
        }
    }

    /// Issue 602: a change of a message reaches every branch that holds it - at once, but on the
    /// branch of a turn whose step's calls wait for their results only after the step.
    #[test]
    fn a_change_of_a_message_waits_on_a_branch_for_its_turns_step() {
        let (line, there) = (ThreadId::default(), ThreadId::new());
        let mut state = AgentChannelState::default();
        let heard = Message { content: Some("heard".to_string()), ..make_message(id(1)) };
        state.perceive(line, vec![heard.clone()]);
        state.join_thread(there, None);
        state.perceive(there, vec![heard]);
        let call = ToolCallState::approved(make_tool_call("write_note"));
        let step = ActorState::WaitingForUser;
        let turn = TurnState { step, tool_calls: vec![call], ..TurnState::default() };
        state.thread_mut(there).turn = Some(turn);

        let revoked = TranscriptEntry::Revoked { message: id(1), at: Utc::now() };
        assert!(state.record_change(revoked));
        assert!(state.transcript.messages(line).is_empty(), "taken back on the line at once");
        assert_eq!(state.transcript.messages(there).len(), 1, "not yet where the turn waits");
        let put_aside = state.turn(there).map(|turn| turn.after_step.len());
        assert_eq!(put_aside, Some(1), "but after its step");
    }

    #[test]
    fn every_thread_is_made_fit_to_run_from() {
        let executing = || {
            let call = ToolCallState::approved(make_tool_call("write_note"));
            let step = ActorState::ExecutingTools;
            let turn = TurnState { step, tool_calls: vec![call], ..TurnState::default() };
            AgentThreadState { turn: Some(turn), used: None }
        };
        let mut state = AgentChannelState::default();
        let (one, other) = (ThreadId::default(), ThreadId::new());
        state.threads.insert(one, executing());
        state.threads.insert(other, executing());
        // Each told as its own turn says, in the variant it was begun with.
        let told = |turn: &TurnState| format!("not executed in turn {}", turn.work.turn_id);
        assert_eq!(state.prepare_resume(told), 2, "a call answered in each thread");
        for thread in [one, other] {
            let turn = state.turn(thread).expect("a turn");
            let answered = turn.tool_calls[0].result().expect("answered");
            assert_eq!(answered.content, told(turn), "as its own turn says");
        }
    }

    /// Issue 602: a thread the agent is taken into begins forked as its branch of the thread it
    /// hangs at stood at its message, or empty without a branch there, and one it holds already is
    /// not forked again; a thread it is let go from goes, its part and its branch.
    #[test]
    fn it_joins_and_leaves_threads() {
        let line = ThreadId::default();
        let (side, fresh, left) = (ThreadId::new(), ThreadId::new(), ThreadId::new());
        let mut state = in_thread(left, ActorState::Idle, None);
        state.transcript.push(line, make_message(id(1)));
        state.transcript.push(left, make_message(id(2)));
        let at_the_line = Some(ThreadOrigin { thread: line, message: id(1) });
        state.join_thread(side, at_the_line);
        state.join_thread(fresh, None);
        assert_eq!(state.transcript.digest(side), state.transcript.digest(line), "forked");
        assert_eq!(state.transcript.digest(fresh), None, "begun empty");
        state.leave_thread(left);
        assert!(state.thread(left).is_none(), "its part of the thread it left gone");
        assert_eq!(state.transcript.digest(left), None, "and its branch");
        assert_eq!(state.threads_held(), BTreeSet::from([line, side]), "what it holds");

        state.transcript.push(side, make_message(id(3)));
        let before = state.transcript.digest(side);
        state.join_thread(side, at_the_line);
        assert_eq!(state.transcript.digest(side), before, "a thread it holds is not forked again");
    }

    #[test]
    fn work_under_way_is_a_turn_of_any_thread_or_a_remote_task() {
        use crate::external_agent::{OpenTask, OpenTaskState, TaskId};
        assert!(!AgentChannelState::default().has_work_under_way(), "idle");
        let mut state = in_thread(ThreadId::default(), ActorState::Idle, None);
        state.threads.insert(
            ThreadId::new(),
            AgentThreadState {
                turn: Some(TurnState { step: ActorState::Planning, ..TurnState::default() }),
                used: None,
            },
        );
        assert!(state.has_work_under_way(), "a turn in another thread goes on");

        let open = |state: OpenTaskState| {
            let mut external = AgentChannelState::default();
            external.remote.open_tasks.push(OpenTask {
                task: TaskId("t-1".to_string()),
                since: Utc::now(),
                state,
                unsaid: None,
                work: None,
            });
            external
        };
        assert!(open(OpenTaskState::Working).has_work_under_way(), "the remote agent works on it");
        assert!(!open(OpenTaskState::InputRequired).has_work_under_way(), "it waits for an answer");
    }

    #[test]
    fn a_turn_under_way_is_stored_with_the_state_and_none_between_turns() {
        let idle = AgentChannelState::default();
        let json = serde_json::to_string(&idle).unwrap();
        let decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert!(decoded.threads.is_empty(), "no turn, nor any part of a thread");

        let thread = ThreadId::default();
        let mut waiting = in_thread(thread, ActorState::WaitingForUser, Some(TurnState::default()));
        waiting.said(thread, Message::new(Classification::CHANNEL_DEFAULT));
        let json = serde_json::to_string(&waiting).unwrap();
        assert!(json.contains("\"WaitingForUser\""));
        let mut decoded: AgentChannelState = serde_json::from_str(&json).unwrap();
        assert!(
            decoded.transcript.digest(thread).is_none(),
            "the transcript is a store's to keep apart (issue 380)"
        );
        decoded.transcript = waiting.transcript.clone();
        assert_eq!(decoded.turn(thread).map(|turn| turn.step), Some(ActorState::WaitingForUser));
        let turn_id = decoded
            .turn(ThreadId::default())
            .map(|t| t.work.turn_id)
            .expect("the turn is resumed from what was stored");
        assert_eq!(
            decoded.transcript.messages(thread).len(),
            1,
            "with what it said, in the transcript (issue 260)"
        );
        assert_eq!(
            decoded.turn(ThreadId::default()).map(|t| t.work.context.transcript),
            Some(decoded.transcript.digest(thread)),
            "its context names the transcript's head"
        );
        let context =
            decoded.turn(ThreadId::default()).map(|t| t.work.context.clone()).unwrap_or_default();
        assert!(
            ContextResolver::of(&decoded, thread, kernel_catalogue())
                .resolve(&context, turn_id, ActorId::default())
                .unwrap()
                .is_some(),
            "so a resumed turn's next call resolves"
        );
        assert_eq!(
            decoded.turn(ThreadId::default()).map(|t| t.work.turn_id),
            waiting.turn(ThreadId::default()).map(|t| t.work.turn_id),
            "as the same turn"
        );
    }

    fn said(content: &str, classification: Classification) -> Message {
        Message {
            id: MessageId::new(),
            content: Some(content.to_string()),
            ..Message::new(classification)
        }
    }

    fn a_state_that_heard(content: &str, classification: Classification) -> AgentChannelState {
        let mut state = AgentChannelState::default();
        let message = said(content, classification);
        state.perceive(ThreadId::default(), vec![message]);
        state
    }

    /// Begins a turn offering `tools` as one set, or none.
    fn begin(state: &mut AgentChannelState, tools: Vec<ToolDescription>) {
        let sets = if tools.is_empty() {
            Vec::new()
        } else {
            vec![ToolSet::new("tools", "some tools", tools)]
        };
        state.begin_turn(
            ThreadId::default(),
            Work::new(
                EventContext::default(),
                None,
                Classification::CHANNEL_DEFAULT,
                Default::default(),
            ),
        );
        state.offer_tools(ThreadId::default(), sets);
    }

    /// That the agent read `source`, classified `classification` - a skill it loaded, say.
    fn knowledge(source: KnowledgeSource, classification: Option<Classification>) -> KnowledgeRef {
        KnowledgeRef {
            channel: Default::default(),
            session: Default::default(),
            source,
            classification,
            private: false,
        }
    }

    /// A workspace the agent read, unclassified: knowledge where no more of it matters.
    fn a_workspace_read() -> KnowledgeRef {
        knowledge(KnowledgeSource::entity::<Workspace>(WorkspaceId::new(), None), None)
    }

    /// Lets the turn under way of `state` call the tools `names` in one step, approved and running,
    /// so that what is given or noted meanwhile is put aside (`TurnState::after_step`): the calls,
    /// for their results.
    fn a_step_calling(state: &mut AgentChannelState, names: &[&str]) -> Vec<ToolCall> {
        let calls: Vec<ToolCall> = names
            .iter()
            .enumerate()
            .map(|(at, name)| ToolCall {
                id: Some((at + 1).to_string()),
                arguments: serde_json::json!({}),
                ..make_tool_call(name)
            })
            .collect();
        state.said(
            ThreadId::default(),
            Message {
                role: MessageRole::Assistant,
                tool_calls: calls.clone(),
                ..said("", Classification::PUBLIC)
            },
        );
        turn_slot(state).as_mut().unwrap().tool_calls =
            calls.iter().cloned().map(ToolCallState::approved).collect();
        calls
    }

    #[test]
    fn a_turn_begun_has_seen_the_transcript_and_offers_its_tools_kept_once() {
        let internal = Classification::general(Confidentiality::Internal);
        let mut state = a_state_that_heard("hello", internal.clone());
        let tool = ToolDescription {
            description: "calculates".to_string(),
            ..make_tool_description("calc")
        };
        begin(&mut state, vec![tool.clone()]);
        begin(&mut state, vec![tool]);
        let turn = state.turn(ThreadId::default()).expect("a turn was begun");
        assert_eq!(turn.work.label(), internal, "it has seen the transcript");
        assert_eq!(
            turn.work.context.transcript,
            state.transcript.digest(ThreadId::default()),
            "it continues the transcript"
        );
        assert!(
            turn.work.context.frame.tools.is_some() && state.offered_tools.len() == 1,
            "the set offered is kept once"
        );
        let offered = turn.work.context.frame.tools;
        state
            .offer_tools(ThreadId::default(), vec![ToolSet::new("more", "more tools", Vec::new())]);
        assert_eq!(
            state.turn(ThreadId::default()).and_then(|t| t.work.context.frame.tools),
            offered,
            "a turn offered tools keeps them"
        );

        begin(&mut state, Vec::new());
        assert!(
            state.turn(ThreadId::default()).is_some_and(|t| t.work.context.frame.tools.is_none()),
            "no tools, no set"
        );
    }

    #[test]
    fn a_turn_says_into_the_transcript_and_its_answer_and_ends_finished() {
        let mut state = a_state_that_heard("add 1 and 2", Classification::CHANNEL_DEFAULT);
        begin(&mut state, Vec::new());
        let turn_id = state.turn(ThreadId::default()).expect("a turn was begun").work.turn_id;
        let step = state.said(
            ThreadId::default(),
            Message { internal: true, ..said("thinking", Classification::CHANNEL_DEFAULT) },
        );
        let produced_from = state.turn(ThreadId::default()).unwrap().work.context.clone();
        let answer = said("3", Classification::CHANNEL_DEFAULT);
        assert!(answer.turn_id.is_none(), "built without it");
        let sent = state.said(ThreadId::default(), answer.clone());
        assert_eq!(
            produced_from.transcript,
            state.transcript.entries(ThreadId::default()).nth(1).map(|n| n.digest),
            "said after the step, from the transcript with it"
        );
        assert_eq!(
            state.turn(ThreadId::default()).unwrap().work.context.transcript,
            state.transcript.digest(ThreadId::default()),
            "the context moved on to the head"
        );
        assert_eq!(
            (sent.turn_id, state.context_of(&sent)),
            (Some(turn_id), Some(produced_from)),
            "sent as the turn's, with what it was produced from (issue 417)"
        );
        assert!(step.produced_from().is_some(), "a step too");
        assert_eq!(state.frames.len(), 1, "the frame they share kept once (issue 426)");
        assert!(
            state
                .transcript
                .messages(ThreadId::default())
                .iter()
                .all(|m| m.produced_from().is_none()),
            "kept without it: the transcript is what the model is given"
        );
        assert_eq!(
            state
                .turn(ThreadId::default())
                .unwrap()
                .answer
                .iter()
                .map(|m| m.content.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
            ["3"],
            "its answer, without the step"
        );
        let note = NoteChange::Add {
            id: NoteId(1),
            text: "sums".to_string(),
            classification: Classification::CHANNEL_DEFAULT,
        };

        *turn_slot(&mut state) = None;
        assert!(
            state.finish_turn(ThreadId::default(), turn_id, vec![note], Vec::new()),
            "a note changed"
        );
        assert_eq!(
            state.transcript.entries(ThreadId::default()).count(),
            4,
            "the turn's messages, followed by its end"
        );
        assert!(
            matches!(state.transcript.last(ThreadId::default()).map(|n| &n.entry), Some(TranscriptEntry::TurnFinished { turn_id: t, .. }) if *t == turn_id)
        );
        let turns: Vec<_> =
            state.transcript.messages(ThreadId::default()).iter().map(|m| m.turn_id).collect();
        assert_eq!(
            turns,
            [None, Some(turn_id), Some(turn_id)],
            "what the turn keeps is the turn's, what was heard no turn's"
        );
        assert_eq!(state.last_message, Some(answer.id), "where its changes were made");
        assert_eq!(state.notes.current().len(), 1);
    }

    /// Issue 537: the parts of prompt components stand in the transcript as given where they
    /// changed, and what no longer holds is taken back.
    #[test]
    fn a_part_is_given_where_it_changed_and_taken_back_where_it_no_longer_holds() {
        let part = |text: &str| said(text, Classification::PUBLIC);
        let standing = |state: &AgentChannelState| -> Vec<(String, String)> {
            parts_as_given(state.transcript.entries(ThreadId::default()))
                .into_iter()
                .filter_map(|(node, message)| {
                    Some((node.entry.part_of()?.to_string(), message.content.unwrap_or_default()))
                })
                .collect()
        };
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let turn_id = state.turn(ThreadId::default()).unwrap().work.turn_id;
        state.give_parts(
            ThreadId::default(),
            vec![
                ("roster".to_string(), part("[Roster]\nalice")),
                ("language".to_string(), part("[Language]\nde")),
            ],
        );
        let pair = |component: &str, text: &str| (component.to_string(), text.to_string());
        assert_eq!(
            standing(&state),
            [pair("roster", "[Roster]\nalice"), pair("language", "[Language]\nde")],
            "given after what was heard"
        );
        assert_eq!(
            state.transcript.messages(ThreadId::default()).last().map(|m| m.turn_id),
            Some(Some(turn_id)),
            "the turn's"
        );
        assert_eq!(state.take_told().len(), 2, "each told the channel as given");
        state.finish_turn(ThreadId::default(), turn_id, Vec::new(), Vec::new());

        let before = state.transcript.end();
        state.perceive(ThreadId::default(), vec![said("again", Classification::PUBLIC)]);
        begin(&mut state, Vec::new());
        state.give_parts(
            ThreadId::default(),
            vec![
                ("roster".to_string(), part("[Roster]\nalice")),
                ("language".to_string(), part("[Language]\nde")),
            ],
        );
        assert_eq!(state.transcript.end(), before.after(1), "nothing given again unchanged");

        let stood: Vec<MessageId> = parts_as_given(state.transcript.entries(ThreadId::default()))
            .into_iter()
            .map(|(_, m)| m.id)
            .collect();
        state.give_parts(
            ThreadId::default(),
            vec![("roster".to_string(), part("[Roster]\nalice, bob"))],
        );
        assert_eq!(
            standing(&state),
            [pair("roster", "[Roster]\nalice, bob")],
            "what changed given anew, what holds nothing any more taken back"
        );
        let told = state.take_told();
        assert!(
            told.iter().all(|(thread, _)| *thread == ThreadId::default()),
            "in the thread of the branch given (issue 602)"
        );
        let withdrawn: Vec<MessageId> = told
            .into_iter()
            .filter_map(|told| match told {
                (_, ActorEvent::MessageWithdrawn { message, .. }) => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(withdrawn, stood, "each taken back told the channel as withdrawn");
        let given: Vec<_> = state
            .transcript
            .messages(ThreadId::default())
            .iter()
            .filter_map(|m| m.content.clone())
            .collect();
        assert_eq!(
            given,
            ["hello", "again", "[Roster]\nalice, bob"],
            "the model is not given what no longer holds"
        );
    }

    /// Issue 537: a part given in a turn given up as `Lowered` is hidden from the model as that
    /// turn's messages are, so it does not stand: the next turn gives it again.
    #[test]
    fn a_part_of_a_turn_given_up_as_lowered_is_given_again() {
        let rules = || ("rules".to_string(), said("[Rules]\nBe brief.", Classification::PUBLIC));
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let lowered = state.turn(ThreadId::default()).unwrap().work.turn_id;
        state.give_parts(ThreadId::default(), vec![rules()]);
        state.give_up_turn(
            ThreadId::default(),
            lowered,
            GivenUpReason::Lowered { channel: Classification::PUBLIC },
        );
        *turn_slot(&mut state) = None;

        begin(&mut state, Vec::new());
        state.give_parts(ThreadId::default(), vec![rules()]);
        let given: Vec<_> = parts_as_given(state.transcript.entries(ThreadId::default()))
            .into_iter()
            .map(|(_, message)| message.turn_id)
            .collect();
        assert_eq!(
            given,
            [Some(state.turn(ThreadId::default()).unwrap().work.turn_id)],
            "given again, in the turn after"
        );
    }

    /// Issue 541: knowledge given in a turn given up as `Lowered` is hidden from the model as that
    /// turn's messages are, so it does not stand: the next turn gives it again.
    #[test]
    fn knowledge_given_in_a_turn_given_up_as_lowered_is_given_again() {
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let lowered = state.turn(ThreadId::default()).unwrap().work.turn_id;
        let skill = a_workspace_read();
        let text = || said("Collect.", Classification::PUBLIC);
        assert!(state.given(ThreadId::default(), text(), Some(skill.clone())));
        state.give_up_turn(
            ThreadId::default(),
            lowered,
            GivenUpReason::Lowered { channel: Classification::PUBLIC },
        );
        *turn_slot(&mut state) = None;

        begin(&mut state, Vec::new());
        assert!(
            state.given(ThreadId::default(), text(), Some(skill)),
            "given again, in the turn after"
        );
    }

    #[test]
    fn a_message_given_in_a_turn_is_read_not_said() {
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let turn_id = state.turn(ThreadId::default()).unwrap().work.turn_id;
        let skill = Message {
            role: MessageRole::User,
            ..said("Collect, group, write.", Classification::PUBLIC)
        };
        let source = KnowledgeSource::entity::<Workspace>(
            WorkspaceId::new(),
            Some(digest::prompt("Collect, group, write.")),
        );
        let knowledge = knowledge(source.clone(), Some(Classification::CHANNEL_DEFAULT));

        state.given(ThreadId::default(), skill.clone(), Some(knowledge.clone()));
        assert_eq!(
            state.transcript.messages(ThreadId::default()).last().map(|m| (m.id, m.turn_id)),
            Some((skill.id, Some(turn_id))),
            "into the transcript, the turn's"
        );
        assert!(
            state.transcript.last(ThreadId::default()).and_then(|n| n.entry.knowledge())
                == Some(&source),
            "as what it was given (issue 16)"
        );
        let work = &state.turn(ThreadId::default()).unwrap().work;
        assert_eq!(
            work.context.frame.knowledge,
            [knowledge],
            "the knowledge recorded with the turn"
        );
        assert_eq!(work.label(), Classification::CHANNEL_DEFAULT, "which has seen it");
        assert_eq!(
            state.turn(ThreadId::default()).unwrap().work.context.transcript,
            state.transcript.digest(ThreadId::default()),
            "what the turn says next is produced from it"
        );
        assert!(
            matches!(
                state.take_told().as_slice(),
                [(thread, ActorEvent::MessageGiven { message })]
                    if *thread == ThreadId::default()
                        && message.id == skill.id
                        && message.produced_from().is_none()
            ),
            "its channel is to be told, as what it was given (issue 417), in its thread (issue 602)"
        );
        assert!(state.take_told().is_empty(), "once");
    }

    #[test]
    fn what_is_given_while_the_calls_of_a_step_run_follows_their_results() {
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let calls = a_step_calling(&mut state, &["load_skill"]);
        let skill = said("Collect, group, write.", Classification::PUBLIC);
        let summary = said("what was given without being knowledge", Classification::PUBLIC);

        state.given(ThreadId::default(), skill.clone(), Some(a_workspace_read()));
        state.given(ThreadId::default(), summary.clone(), None);
        assert_eq!(
            state.transcript.messages(ThreadId::default()).len(),
            2,
            "nothing between the calls and their results"
        );
        assert!(state.to_tell.is_empty(), "nor told yet");

        let result = ToolCallResult::of(
            &calls[0],
            ToolResultStatus::Ok,
            "loaded".to_string(),
            Classification::PUBLIC,
        );
        turn_slot(&mut state).as_mut().unwrap().tool_calls.clear();
        let results = Message {
            role: MessageRole::Tool,
            tool_results: vec![result],
            ..said("", Classification::PUBLIC)
        };
        state.said(ThreadId::default(), results.clone());
        let order: Vec<_> =
            state.transcript.messages(ThreadId::default()).iter().map(|m| m.id).collect();
        assert_eq!(
            order[2..],
            [results.id, skill.id, summary.id],
            "after the results, in the order given"
        );
        let sources: Vec<_> = state
            .transcript
            .entries(ThreadId::default())
            .skip(3)
            .map(|n| matches!(&n.entry, TranscriptEntry::Given { source: Some(_), .. }))
            .collect();
        assert_eq!(sources, [true, false], "each as it was given, knowledge or not");
        assert!(state.turn(ThreadId::default()).unwrap().after_step.is_empty());
        assert_eq!(
            state.turn(ThreadId::default()).unwrap().work.context.transcript,
            state.transcript.digest(ThreadId::default()),
            "what the turn says next is produced from it"
        );
        let told: Vec<_> = state
            .take_told()
            .into_iter()
            .filter_map(|told| match told {
                (_, ActorEvent::MessageGiven { message }) => Some(message.id),
                _ => None,
            })
            .collect();
        assert_eq!(told, [skill.id, summary.id], "and its channel is to be told so");
    }

    /// Issue 541: knowledge that stands given - with the same text and classification - is not
    /// given again, though recorded with the turn that would give it; changed, classified
    /// otherwise or taken back, it is given. What is no knowledge is always given.
    #[test]
    fn knowledge_that_stands_given_is_not_given_again() {
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let skill = a_workspace_read();
        let public = |content| said(content, Classification::PUBLIC);
        let first = public("Collect.");
        assert!(state.given(ThreadId::default(), first.clone(), Some(skill.clone())), "given");
        let turn_id = state.turn(ThreadId::default()).unwrap().work.turn_id;
        state.finish_turn(ThreadId::default(), turn_id, Vec::new(), Vec::new());
        begin(&mut state, Vec::new());
        state.take_told();

        assert!(
            !state.given(ThreadId::default(), public("Collect."), Some(skill.clone())),
            "not given again"
        );
        assert_eq!(state.transcript.messages(ThreadId::default()).len(), 2, "it stands once");
        assert!(state.take_told().is_empty(), "nothing to tell");
        assert_eq!(
            state.turn(ThreadId::default()).unwrap().work.context.frame.knowledge,
            vec![skill.clone()],
            "recorded with the turn all the same"
        );

        let internal = said("Collect.", Classification::general(Confidentiality::Internal));
        assert!(
            state.given(ThreadId::default(), internal, Some(skill.clone())),
            "classified otherwise"
        );
        assert!(
            state.given(ThreadId::default(), public("Collect, group."), Some(skill.clone())),
            "changed"
        );
        assert!(state.record_change(TranscriptEntry::Revoked { message: first.id, at: Utc::now() }));
        assert!(state.given(ThreadId::default(), public("Collect."), Some(skill)), "taken back");
        assert!(
            state.given(ThreadId::default(), public("a summary"), None)
                && state.given(ThreadId::default(), public("a summary"), None),
            "what is no knowledge always"
        );
    }

    /// Issue 541: knowledge put aside twice while the calls of a step run is given once, after
    /// their results.
    #[test]
    fn knowledge_put_aside_twice_in_a_step_is_given_once() {
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let calls = a_step_calling(&mut state, &["load_skill", "load_skill"]);
        let skill = a_workspace_read();
        let text = said("Collect.", Classification::PUBLIC);

        assert!(
            state.given(ThreadId::default(), text.clone(), Some(skill.clone())),
            "the first given"
        );
        assert!(!state.given(ThreadId::default(), text.clone(), Some(skill)), "the second not");
        turn_slot(&mut state).as_mut().unwrap().tool_calls.clear();
        let results = calls
            .iter()
            .map(|call| {
                let loaded = "loaded".to_string();
                ToolCallResult::of(call, ToolResultStatus::Ok, loaded, Classification::PUBLIC)
            })
            .collect();
        state.said(
            ThreadId::default(),
            Message {
                role: MessageRole::Tool,
                tool_results: results,
                ..said("", Classification::PUBLIC)
            },
        );
        let given = state
            .transcript
            .messages(ThreadId::default())
            .iter()
            .filter(|m| m.content == text.content)
            .count();
        assert_eq!(given, 1, "once, after the results");
    }

    /// Issue 538: what the agent last knew of a place within an entity - by the last tool result or
    /// gift that holds its content, on the path as the model sees it, then in the step under way
    /// and what is put aside for after it -; of a place it never knew the content of, nothing.
    #[test]
    fn what_the_agent_last_knew_of_a_place_is_its_last_knowledge_of_it() {
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let workspace = WorkspaceId::new();
        let digest = |text: &str| Some(digest::content(text.as_bytes()));
        let knowing = |place: &str, of: Option<Digest>| {
            knowledge(KnowledgeSource::within::<Workspace>(workspace, place.to_string(), of), None)
        };
        let result = |call: &ToolCall, place: &str, of: Option<Digest>| ToolCallResult {
            knowledge: vec![knowing(place, of)],
            ..ToolCallResult::of(call, ToolResultStatus::Ok, String::new(), Classification::PUBLIC)
        };
        let last = |state: &AgentChannelState, place: &str| {
            state.last_known::<Workspace>(ThreadId::default(), workspace, place)
        };
        assert_eq!(last(&state, "a.txt"), None, "nothing known");

        let calls = a_step_calling(&mut state, &["read_file"]);
        turn_slot(&mut state).as_mut().unwrap().tool_calls.clear();
        let read = state.said(
            ThreadId::default(),
            Message {
                role: MessageRole::Tool,
                tool_results: vec![result(&calls[0], "a.txt", digest("one"))],
                ..said("", Classification::PUBLIC)
            },
        );
        assert_eq!(last(&state, "a.txt"), digest("one"), "as a result on the path holds it");
        assert_eq!(last(&state, "b.txt"), None, "of that place alone");
        assert_eq!(
            state.last_known::<Workspace>(ThreadId::default(), WorkspaceId::new(), "a.txt"),
            None
        );

        let calls = a_step_calling(&mut state, &["write_file", "grep_files"]);
        turn_slot(&mut state).as_mut().unwrap().tool_calls[0].status =
            ToolCallStatus::Finished(result(&calls[0], "a.txt", digest("two")));
        assert_eq!(last(&state, "a.txt"), digest("two"), "as the step under way wrote it");
        turn_slot(&mut state).as_mut().unwrap().tool_calls[1].status =
            ToolCallStatus::Finished(result(&calls[1], "a.txt", None));
        assert_eq!(last(&state, "a.txt"), digest("two"), "a search holds no content");
        state.given(
            ThreadId::default(),
            said("[File: a.txt]", Classification::PUBLIC),
            Some(knowing("a.txt", digest("given"))),
        );
        assert_eq!(
            last(&state, "a.txt"),
            digest("given"),
            "as put aside to be given after the step"
        );

        turn_slot(&mut state).as_mut().unwrap().tool_calls.clear();
        turn_slot(&mut state).as_mut().unwrap().after_step.clear();
        assert!(state.record_change(TranscriptEntry::Revoked { message: read.id, at: Utc::now() }));
        assert_eq!(last(&state, "a.txt"), None, "taken back, as the model is no longer given it");

        let lowered = state.turn(ThreadId::default()).unwrap().work.turn_id;
        let calls = a_step_calling(&mut state, &["read_file"]);
        turn_slot(&mut state).as_mut().unwrap().tool_calls.clear();
        state.said(
            ThreadId::default(),
            Message {
                role: MessageRole::Tool,
                tool_results: vec![result(&calls[0], "a.txt", digest("three"))],
                ..said("", Classification::PUBLIC)
            },
        );
        assert_eq!(last(&state, "a.txt"), digest("three"));
        state.give_up_turn(
            ThreadId::default(),
            lowered,
            GivenUpReason::Lowered { channel: Classification::PUBLIC },
        );
        *turn_slot(&mut state) = None;
        assert_eq!(last(&state, "a.txt"), None, "hidden with a turn given up as `Lowered`");
    }

    #[test]
    fn who_left_while_the_calls_of_a_step_run_is_noted_after_their_results() {
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let calls = a_step_calling(&mut state, &["ask_user"]);
        let alice = ActorId::User(UserId::new());
        let left = HistoryEntry::ParticipantLeft {
            participant: alice,
            name: "alice".to_string(),
            by: alice,
            by_name: "alice".to_string(),
            until: Position::new(1),
            at: Utc::now(),
        };

        state.note_announced(ThreadId::default(), left);
        assert!(
            !state
                .transcript
                .entries(ThreadId::default())
                .any(|n| matches!(n.entry, TranscriptEntry::ParticipantLeft { .. })),
            "nothing between the calls and their results"
        );

        let result = ToolCallResult::of(
            &calls[0],
            ToolResultStatus::NotExecuted,
            "unanswered".to_string(),
            Classification::PUBLIC,
        );
        turn_slot(&mut state).as_mut().unwrap().tool_calls.clear();
        state.said(
            ThreadId::default(),
            Message {
                role: MessageRole::Tool,
                tool_results: vec![result],
                ..said("", Classification::PUBLIC)
            },
        );
        assert!(
            matches!(state.transcript.last(ThreadId::default()).map(|n| &n.entry), Some(TranscriptEntry::ParticipantLeft { participant, .. }) if *participant == alice),
            "after the results"
        );
        assert!(state.turn(ThreadId::default()).unwrap().after_step.is_empty());
        assert_eq!(
            state.turn(ThreadId::default()).unwrap().work.context.transcript,
            state.transcript.digest(ThreadId::default()),
            "what the turn says next is produced from it"
        );
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    fn a_compaction_moves_the_turn_to_the_new_branch_and_keeps_what_was_recorded_resolvable() {
        let agent = ActorId::Agent(AgentId::new());
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let answer = Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            prompt_tokens: Some(100),
            completion_tokens: Some(10),
            ..said("hi", Classification::PUBLIC)
        };
        let sent = state.said(ThreadId::default(), answer.clone());
        let first = turn_slot(&mut state).take().unwrap().work.turn_id;
        state.finish_turn(ThreadId::default(), first, Vec::new(), Vec::new());
        let answered = state
            .transcript
            .entries(ThreadId::default())
            .find(|n| n.entry.message().is_some_and(|m| m.id == answer.id))
            .unwrap()
            .digest;
        let recorded = state.context_of(&sent).expect("sent with the answer");
        let turn = first;
        let resolve = |state: &AgentChannelState| {
            ContextResolver::of(state, ThreadId::default(), kernel_catalogue())
                .resolve(&recorded, turn, agent)
                .unwrap()
                .map(|r| r.items.len())
        };
        let before = resolve(&state);
        state.perceive(ThreadId::default(), vec![said("again", Classification::PUBLIC)]);
        let head = state.transcript.digest(ThreadId::default());
        state.take_told();

        assert_eq!(
            state.compact(
                ThreadId::default(),
                agent,
                vec![(said("summary", Classification::PUBLIC), None)],
                None,
                kernel_catalogue()
            ),
            None,
            "not between turns"
        );
        begin(&mut state, Vec::new());
        let summary = Message {
            sender_id: agent,
            role: MessageRole::User,
            prompt_tokens: Some(50),
            ..said("[Summary] they greeted", Classification::PUBLIC)
        };
        assert_eq!(
            state.compact(
                ThreadId::default(),
                agent,
                vec![(summary.clone(), None)],
                Some(digest::content(b"elsewhere")),
                kernel_catalogue()
            ),
            None,
            "nor if the transcript is not compacted"
        );
        assert_eq!(state.transcript.digest(ThreadId::default()), head, "nothing changed");
        assert!(
            state.take_told().is_empty()
                && state.turn(ThreadId::default()).unwrap().work.context.transcript == head,
            "nothing told, the turn where it was"
        );

        let source =
            KnowledgeSource::entity::<Workspace>(WorkspaceId::new(), Some(digest::prompt("steps")));
        let knowledge = knowledge(source, Some(Classification::CHANNEL_DEFAULT));
        let skill = said("[Skill: steps]", Classification::PUBLIC);
        let (from, to) = state
            .compact(
                ThreadId::default(),
                agent,
                vec![(summary.clone(), None), (skill.clone(), Some(knowledge.clone()))],
                Some(answered),
                kernel_catalogue(),
            )
            .expect("compacted");

        assert_eq!(Some(from), head, "from what stood");
        assert_eq!(state.transcript.digest(ThreadId::default()), Some(to));
        let turn_under_way = &state.turn(ThreadId::default()).unwrap().work;
        assert_eq!(
            turn_under_way.context.transcript,
            Some(to),
            "what the turn says next is produced from the new branch"
        );
        assert_eq!(
            turn_under_way.context.frame.knowledge,
            [knowledge],
            "the skill given again is knowledge of the turn"
        );
        assert!(
            state
                .transcript
                .messages(ThreadId::default())
                .iter()
                .take(2)
                .all(|m| m.turn_id.is_none()),
            "the replacement is whose its caller says (issue 284)"
        );
        assert_eq!(state.last_message, Some(skill.id), "the last it took in");

        assert_eq!(resolve(&state), before, "what was recorded before resolves to what it did");
        let told = state.take_told();
        assert!(
            matches!(
                told.first(),
                Some((_, ActorEvent::ContextCompacted { from: f, to: t, .. })) if *f == from && *t == to
            ),
            "its channel is to be told of the compaction (issue 417)"
        );
        assert!(
            told.iter().all(|(thread, _)| *thread == ThreadId::default()),
            "in the thread of the branch compacted (issue 602)"
        );
        let followed: Vec<_> = told[1..]
            .iter()
            .filter_map(|told| match told {
                (_, ActorEvent::MessageGiven { message }) => Some(message.id),
                _ => None,
            })
            .collect();
        assert_eq!(
            followed,
            [summary.id, skill.id],
            "then of what replaced the older messages, as given"
        );
        let usage =
            state.thread(ThreadId::default()).and_then(|part| part.used).expect("calls were made");
        assert_eq!((usage.calls, usage.prompt_tokens), (2, 150), "the answer kept counts once");
    }

    #[test]
    fn after_a_compaction_the_context_is_the_new_branch_as_estimated_then() {
        let agent = ActorId::Agent(AgentId::new());
        let mut state = a_state_that_heard("hello", Classification::PUBLIC);
        begin(&mut state, Vec::new());
        let first = Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            prompt_tokens: Some(100),
            completion_tokens: Some(10),
            ..said("call", Classification::PUBLIC)
        };
        state.said(ThreadId::default(), first.clone());
        let kept_from = state
            .transcript
            .entries(ThreadId::default())
            .find(|n| n.entry.message().is_some_and(|m| m.id == first.id))
            .map(|n| n.digest);
        let summary = Message {
            sender_id: agent,
            prompt_tokens: Some(500),
            completion_tokens: Some(8),
            ..said("summary", Classification::PUBLIC)
        };
        state
            .compact(
                ThreadId::default(),
                agent,
                vec![(summary, None)],
                kept_from,
                kernel_catalogue(),
            )
            .expect("compacted");
        assert_eq!(
            state
                .thread(ThreadId::default())
                .and_then(|part| part.used)
                .map(|u| (u.calls, u.context_tokens)),
            Some((2, 2 + 1)),
            "the new branch as estimated, not the summary's call over the old one"
        );

        let turn = turn_slot(&mut state).take().unwrap().work.turn_id;
        state.finish_turn(ThreadId::default(), turn, Vec::new(), Vec::new());
        assert_eq!(
            state.thread(ThreadId::default()).and_then(|part| part.used).map(|u| u.context_tokens),
            Some(3),
            "kept, between turns as well (issue 390)"
        );
    }

    #[test]
    fn a_compaction_carries_the_prompt_parts_of_the_turn_over() {
        let agent = ActorId::Agent(AgentId::new());
        let mut state = a_state_that_heard("older", Classification::PUBLIC);
        let older = state.transcript.digest(ThreadId::default());
        let kept = said("kept", Classification::PUBLIC);
        let changed = said("changed", Classification::PUBLIC);
        state.perceive(ThreadId::default(), vec![kept.clone(), changed.clone()]);
        let kept_at = state
            .transcript
            .entries(ThreadId::default())
            .find(|n| n.entry.message().is_some_and(|m| m.id == kept.id))
            .map(|n| n.digest);
        let changed_now = Message { content: Some("changed!".to_string()), ..changed.clone() };
        state.record_change(TranscriptEntry::Modified { message: changed_now });
        let change_at = state.transcript.digest(ThreadId::default());
        begin(&mut state, Vec::new());
        let parts = [
            ("first", PromptAt::Start),
            ("on the replaced", PromptAt::after(older)),
            ("on the kept", PromptAt::after(kept_at)),
            ("on a change", PromptAt::after(change_at)),
            ("at the end", PromptAt::End),
        ]
        .map(|(text, at)| state.keep_prompt(MessageRole::System, text.to_string(), at, None));
        turn_slot(&mut state).as_mut().unwrap().work.context.frame.prompts = parts.to_vec();

        state
            .compact(
                ThreadId::default(),
                agent,
                vec![(said("summary", Classification::PUBLIC), None)],
                kept_at,
                kernel_catalogue(),
            )
            .expect("compacted");

        let on_new: Vec<Option<String>> = state
            .transcript
            .entries(ThreadId::default())
            .map(|n| n.entry.message().and_then(|m| m.content.clone()))
            .collect();
        let anchor = |prompt: &PromptRef| match prompt.at {
            PromptAt::After(after) => Some(
                state
                    .transcript
                    .entries(ThreadId::default())
                    .position(|n| n.digest == after)
                    .map(|at| on_new[at].clone()),
            ),
            PromptAt::Start | PromptAt::End => None,
        };
        let anchors: Vec<_> = state
            .turn(ThreadId::default())
            .unwrap()
            .work
            .context
            .frame
            .prompts
            .iter()
            .map(anchor)
            .collect();
        assert_eq!(anchors, [
            None,
            Some(Some(Some("summary".to_string()))),
            Some(Some(Some("kept".to_string()))),
            Some(Some(Some("changed!".to_string()))),
            None,
        ], "before everything as it was, after the replacement for what it replaced, after the copy for what was kept - a change's after its message");
        assert_eq!(
            state.turn(ThreadId::default()).unwrap().work.context.frame.prompts[4].at,
            PromptAt::End,
            "at the end as it was (issue 537)"
        );
        let context = state.turn(ThreadId::default()).unwrap().work.context.clone();
        assert!(
            ContextResolver::of(&state, ThreadId::default(), kernel_catalogue())
                .resolve(&context, TurnId::new(), agent)
                .unwrap()
                .is_some(),
            "every part is placed on the new branch"
        );
    }

    #[test]
    fn a_command_turn_offers_no_tools_and_ends_finished() {
        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        let issuer = ActorId::User(UserId::new());
        let request =
            CommandRequest { command: "compact".to_string(), args: "hard".to_string(), issuer };
        let turn_id = state.begin_command_turn(
            ThreadId::default(),
            Work::new(
                EventContext::default(),
                None,
                Classification::CHANNEL_DEFAULT,
                Default::default(),
            ),
            request.clone(),
        );
        let turn = state.turn(ThreadId::default()).expect("a turn was begun");
        assert_eq!(turn.command.as_ref(), Some(&request), "kept with the turn, for one resumed");
        assert!(turn.work.context.frame.tools.is_none(), "a command turn offers its model nothing");

        *turn_slot(&mut state) = None;
        assert!(!state.finish_turn(ThreadId::default(), turn_id, Vec::new(), Vec::new()));
        assert!(
            matches!(state.transcript.last(ThreadId::default()).map(|n| &n.entry), Some(TranscriptEntry::TurnFinished { turn_id: t, .. }) if *t == turn_id)
        );
        assert_eq!(state.transcript.messages(ThreadId::default()).len(), 1, "nothing said");
    }

    #[test]
    fn a_turn_given_up_keeps_what_it_said_and_ends_given_up() {
        let mut state = a_state_that_heard("mail bob", Classification::CHANNEL_DEFAULT);
        begin(&mut state, Vec::new());
        state.said(ThreadId::default(), said("mailing", Classification::CHANNEL_DEFAULT));

        let turn_id = turn_slot(&mut state).take().expect("a turn was begun").work.turn_id;
        state.give_up_turn(ThreadId::default(), turn_id, GivenUpReason::Left);
        let contents = |messages: Vec<Message>| {
            messages.iter().map(|m| m.content.clone().unwrap_or_default()).collect::<Vec<_>>()
        };
        assert_eq!(
            contents(state.transcript.messages(ThreadId::default())),
            ["mail bob", "mailing"],
            "what it said is not unsaid"
        );
        assert_eq!(
            state.transcript.changes().map_or(0, |changes| changes.entries.len()),
            state.transcript.entries(ThreadId::default()).count(),
            "nothing branches"
        );
        assert!(
            matches!(state.transcript.last(ThreadId::default()).map(|n| &n.entry), Some(TranscriptEntry::TurnGivenUp { turn_id: t, reason: GivenUpReason::Left, .. }) if *t == turn_id)
        );

        let silent = TurnId::new();
        state.give_up_turn(ThreadId::default(), silent, GivenUpReason::Left);
        assert!(
            matches!(state.transcript.last(ThreadId::default()).map(|n| &n.entry), Some(TranscriptEntry::TurnGivenUp { turn_id: t, .. }) if *t == silent),
            "a turn that said nothing leaves its end too"
        );
    }

    #[test]
    fn an_agent_answers_anew_after_its_last_turn_given_up_for_a_failure_or_a_lowering() {
        let given_up = |reason: GivenUpReason| {
            let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
            begin(&mut state, Vec::new());
            let turn = turn_slot(&mut state).take().expect("a turn was begun").work.turn_id;
            state.give_up_turn(ThreadId::default(), turn, reason);
            (state, turn)
        };
        let (state, turn) = given_up(GivenUpReason::Failed { message: "boom".to_string() });
        assert_eq!(
            state.answers_again_after(ThreadId::default(), turn),
            Ok(()),
            "a turn that failed, though it said nothing"
        );
        assert_eq!(
            state.answers_again_after(ThreadId::default(), TurnId::new()),
            Err(RetryRefused::NotItsAnswer),
            "no turn of its own"
        );
        let lowered = given_up(GivenUpReason::Lowered { channel: Classification::PUBLIC });
        assert_eq!(lowered.0.answers_again_after(ThreadId::default(), lowered.1), Ok(()));
        for reason in [GivenUpReason::Left, GivenUpReason::Retried] {
            let (state, turn) = given_up(reason.clone());
            assert_eq!(
                state.answers_again_after(ThreadId::default(), turn),
                Err(RetryRefused::Ended),
                "{reason:?}"
            );
        }

        let (mut state, turn) = given_up(GivenUpReason::Failed { message: "boom".to_string() });
        let later = said("are you there?", Classification::CHANNEL_DEFAULT);
        state.perceive(ThreadId::default(), vec![later]);
        assert_eq!(
            state.answers_again_after(ThreadId::default(), turn),
            Err(RetryRefused::NotLast),
            "something followed it"
        );

        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        begin(&mut state, Vec::new());
        let turn = turn_slot(&mut state).take().expect("a turn was begun").work.turn_id;
        state.finish_turn(ThreadId::default(), turn, Vec::new(), Vec::new());
        assert_eq!(
            state.answers_again_after(ThreadId::default(), turn),
            Err(RetryRefused::Ended),
            "taken over, not given up"
        );
    }

    /// Issue 417: what a turn said is its answer, kept with the turn -
    /// also once a compaction in the middle of it left nothing of it in the
    /// transcript the agent holds.
    #[test]
    fn a_turns_answer_is_kept_with_it_whatever_a_compaction_left() {
        let agent = ActorId::Agent(AgentId::new());
        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        begin(&mut state, Vec::new());
        state.said(
            ThreadId::default(),
            Message {
                sender_id: agent,
                role: MessageRole::Assistant,
                ..said("first part", Classification::CHANNEL_DEFAULT)
            },
        );
        state
            .compact(
                ThreadId::default(),
                agent,
                vec![(said("[Summary]", Classification::CHANNEL_DEFAULT), None)],
                None,
                kernel_catalogue(),
            )
            .expect("compacted");
        state.transcript.mark_stored();
        state.keep_window();
        assert!(
            state
                .transcript
                .messages(ThreadId::default())
                .iter()
                .all(|m| m.content.as_deref() != Some("first part")),
            "not in the transcript held"
        );
        state.said(
            ThreadId::default(),
            Message {
                sender_id: agent,
                role: MessageRole::Assistant,
                ..said("second part", Classification::CHANNEL_DEFAULT)
            },
        );
        let answer: Vec<_> = state
            .turn(ThreadId::default())
            .unwrap()
            .answer
            .iter()
            .map(|m| m.content.clone().unwrap_or_default())
            .collect();
        assert_eq!(answer, ["first part", "second part"], "the whole answer");
        let stored: AgentChannelState =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(
            stored.turn(ThreadId::default()).map(|t| t.answer.len()),
            Some(2),
            "stored with the turn"
        );
    }

    #[test]
    fn an_agent_keeps_what_the_heads_of_its_transcript_reach() {
        use crate::message::testing::{id, make_message};
        let line = ThreadId::default();
        let mut state = AgentChannelState::default();
        state.transcript.extend(line, (1..=3).map(|n| make_message(id(n))).collect());
        let given = state.transcript.start_anew(line).expect("a branch");
        state.transcript.extend(line, vec![make_message(id(4))]);
        state.keep_window();
        assert!(state.transcript.prefix(&given).is_some(), "nothing goes before it is stored");
        state.transcript.mark_stored();
        state.keep_window();
        assert_eq!(state.transcript.messages(line).len(), 1, "the branch of its thread stays");
        assert!(state.transcript.prefix(&given).is_none(), "what no head reaches is the store's");
        state.transcript.start_anew(line);
        state.transcript.mark_stored();
        state.keep_window();
        assert_eq!(
            (state.transcript.digest(line), state.transcript.end().get()),
            (None, 4),
            "right after it began anew, nothing, and the next entry goes on after the last"
        );
    }

    #[test]
    fn an_agent_keeps_the_texts_and_tools_its_turn_under_way_names() {
        let mut state = AgentChannelState::default();
        let earlier =
            state.keep_prompt(MessageRole::System, "earlier".to_string(), PromptAt::Start, None);
        let sets = vec![ToolSet::new("calc", "calculates", Vec::new())];
        state.begin_turn(
            ThreadId::default(),
            Work::new(
                EventContext::default(),
                None,
                Classification::CHANNEL_DEFAULT,
                Default::default(),
            ),
        );
        state.offer_tools(ThreadId::default(), sets);
        let system =
            state.keep_prompt(MessageRole::System, "system".to_string(), PromptAt::Start, None);
        turn_slot(&mut state).as_mut().unwrap().work.context.frame.prompts.push(system.clone());
        let offer =
            state.turn(ThreadId::default()).unwrap().work.context.frame.tools.expect("an offer");
        let frame =
            state.keep_frame(&state.turn(ThreadId::default()).unwrap().work.context.frame.clone());
        state.keep_window();
        assert_eq!(
            (state.prompts.len(), state.offered_tools.len(), state.frames.len()),
            (2, 1, 1),
            "nothing goes before it is stored"
        );

        state.prompts.mark_stored();
        state.offered_tools.mark_stored();
        state.frames.mark_stored();
        state.keep_window();
        assert!(
            state.prompts.get(&earlier.text).is_none(),
            "what no turn under way names is the store's"
        );
        assert!(
            state.prompts.get(&system.text).is_some() && state.offered_tools.get(&offer).is_some(),
            "what it names stays"
        );
        assert!(
            state.frames.get(&frame).is_none(),
            "a frame is the store's at once: the turn holds its own (issue 426)"
        );

        *turn_slot(&mut state) = None;
        state.keep_window();
        assert!(
            state.prompts.is_empty() && state.offered_tools.is_empty(),
            "between turns, nothing"
        );
    }

    #[test]
    fn turns_in_two_threads_are_begun_and_ended_each_on_its_own() {
        let work = || {
            Work::new(
                EventContext::default(),
                None,
                Classification::CHANNEL_DEFAULT,
                Default::default(),
            )
        };
        let (one, other) = (ThreadId::default(), ThreadId::new());
        let mut state = AgentChannelState::default();
        let first = state.begin_turn(one, work());
        let second = state.begin_turn(other, work());
        let turn_in =
            |state: &AgentChannelState, thread| state.turn(thread).map(|t| t.work.turn_id);
        assert_eq!(
            (turn_in(&state, one), turn_in(&state, other)),
            (Some(first), Some(second)),
            "each thread has its own"
        );

        state.thread_mut(other).turn = None;
        state.give_up_turn(other, second, GivenUpReason::Retried);
        assert_eq!(turn_in(&state, one), Some(first), "giving up one leaves the other");
        assert!(
            state.transcript.entries(one).all(|node| node.entry.ended_turn().is_none()),
            "nor is its end recorded on the other branch"
        );
    }

    #[test]
    fn what_the_turns_of_every_thread_name_is_kept() {
        let offer_in = |state: &mut AgentChannelState, thread, name: &str| {
            let work = Work::new(
                EventContext::default(),
                None,
                Classification::CHANNEL_DEFAULT,
                Default::default(),
            );
            state.begin_turn(thread, work);
            state.offer_tools(thread, vec![ToolSet::new(name, name, Vec::new())]);
            let turn = state.turn(thread);
            turn.and_then(|t| t.work.context.frame.tools).expect("an offer")
        };
        let mut state = AgentChannelState::default();
        let one = offer_in(&mut state, ThreadId::default(), "one");
        let other = offer_in(&mut state, ThreadId::new(), "other");
        state.offered_tools.mark_stored();
        state.keep_window();
        assert!(
            state.offered_tools.get(&one).is_some() && state.offered_tools.get(&other).is_some(),
            "what either turn names stays"
        );
    }

    #[test]
    fn what_changed_in_an_agents_channel_state_replays_onto_what_was_stored() {
        use crate::content::testing::make_content_ref;
        use crate::message::testing::{id, make_message};
        let mut state = AgentChannelState::default();
        state.transcript.push(ThreadId::default(), make_message(id(1)));
        let mut copy = stored(&mut state);

        state.perceive(
            ThreadId::default(),
            vec![Message {
                attachments: vec![make_content_ref("a.png", b"a")],
                ..make_message(id(2))
            }],
        );
        state.notes.push(
            NoteChange::Add {
                id: NoteId(1),
                text: "noted".to_string(),
                classification: Classification::CHANNEL_DEFAULT,
            },
            Default::default(),
            id(2),
        );
        state.private_notes.push(
            NoteChange::Add {
                id: NoteId(1),
                text: "mine".to_string(),
                classification: Classification::CHANNEL_DEFAULT,
            },
            Default::default(),
            id(2),
        );
        state.asleep = true;
        let step = ActorState::WaitingForUser;
        *turn_slot(&mut state) = Some(TurnState { step, model_calls: 3, ..TurnState::default() });
        state.thread_mut(ThreadId::default()).used =
            TokenUsage::counted(None, &Message { prompt_tokens: Some(5), ..make_message(id(2)) });
        state.last_message = Some(id(2));
        state.heard.took(Position::new(4));
        state.heard.took_ahead(Position::new(7));

        let changes = state.changes();
        let parts: Vec<&str> = changes
            .iter()
            .map(|c| match c {
                AgentChannelStateChange::Notes(_) => "notes",
                AgentChannelStateChange::PrivateNotes(_) => "private_notes",
                AgentChannelStateChange::Threads(_) => "threads",
                AgentChannelStateChange::Asleep(_) => "asleep",
                AgentChannelStateChange::Remote(_) => "remote",
                AgentChannelStateChange::LastMessage(_) => "last_message",
                AgentChannelStateChange::Heard(_) => "heard",
                AgentChannelStateChange::Attachments(_) => "attachments",
            })
            .collect();
        assert_eq!(parts, ["notes", "private_notes", "threads", "asleep", "remote", "last_message", "heard", "attachments"], "what was added, then the small parts whole; the transcript, the prompt parts and the tool sets are the store's to write");
        replayed(&state, &mut copy);
        assert!(
            !copy.attachments.is_empty(),
            "the numbers its transcript gave attachments (issue 282)"
        );
        assert_eq!(
            copy.thread(ThreadId::default()).and_then(|part| part.used),
            state.thread(ThreadId::default()).and_then(|part| part.used),
            "the running total with them"
        );
        assert_eq!(
            copy.turn(ThreadId::default()).map(|turn| turn.step),
            Some(ActorState::WaitingForUser),
            "where its turn stands in the thread"
        );
        assert!(copy.asleep, "and whether it sleeps, its own as a whole");
        assert_eq!(
            (copy.last_message, &copy.heard),
            (Some(id(2)), &state.heard),
            "and where it stands in its channel's history (issue 440)"
        );

        state.mark_stored();
        *turn_slot(&mut state) = None;
        state.heard.took(Position::new(5));
        assert_eq!(state.changes().len(), 5, "between turns a step is the small parts alone");
        replayed(&state, &mut copy);
        assert!(copy.turn(ThreadId::default()).is_none());
    }

    #[test]
    fn what_is_perceived_is_seen_by_a_turn_under_way() {
        use crate::message::testing::{id, make_message};
        let mut state = AgentChannelState::default();
        state.perceive(ThreadId::default(), vec![make_message(id(1))]);
        assert_eq!(
            state.transcript.messages(ThreadId::default()).iter().map(|m| m.id).collect::<Vec<_>>(),
            [id(1)],
            "what was perceived"
        );

        let channel = Classification::general(Confidentiality::Confidential);
        *turn_slot(&mut state) = Some(TurnState {
            work: Work::new(Default::default(), None, channel.clone(), Default::default()),
            ..TurnState::default()
        });
        let confidential = Message { classification: channel.clone(), ..make_message(id(3)) };
        state.perceive(ThreadId::default(), vec![confidential]);
        assert_eq!(
            state.turn(ThreadId::default()).map(|t| t.work.label()),
            Some(channel),
            "the turn has seen it"
        );
        assert_eq!(
            state.turn(ThreadId::default()).map(|t| t.work.context.transcript),
            Some(state.transcript.digest(ThreadId::default())),
            "and its next message follows it (issue 260)"
        );
    }

    #[test]
    fn a_turn_in_one_thread_is_not_moved_by_what_happens_in_another() {
        use crate::message::testing::{id, make_message};
        let mut state = AgentChannelState::default();
        begin(&mut state, Vec::new());
        let before = state.turn(ThreadId::default()).map(|t| t.work.context.clone());
        let other = ThreadId::new();
        state.perceive(other, vec![make_message(id(1))]);
        state.give_parts(other, vec![("part".to_string(), make_message(id(2)))]);
        state.said(other, make_message(id(3)));
        assert_eq!(
            state.turn(ThreadId::default()).map(|t| t.work.context.clone()),
            before,
            "the turn's context stays where it was"
        );
        assert!(
            state.turn(ThreadId::default()).is_some_and(|t| t.answer.is_empty()),
            "nor is what was said there its answer"
        );
        assert_eq!(
            state.transcript.messages(other).iter().map(|m| m.id).collect::<Vec<_>>(),
            [id(1), id(2), id(3)],
            "it is all on the other thread's branch"
        );
    }

    #[test]
    fn what_its_turns_wait_for_is_found_in_every_thread() {
        let call = |name: &str| ToolCallState::pending(make_tool_call(name), false, None, None);
        let agent = ActorId::Agent(AgentId::new());
        let waiting = |name: &str| {
            let mut turn = TurnState { tool_calls: vec![call(name)], ..TurnState::default() };
            turn.work.event_context.sender_id = agent;
            turn
        };
        let mut state = in_thread(ThreadId::default(), ActorState::WaitingForUser, None);
        *turn_slot(&mut state) = Some(waiting("first"));
        state.thread_mut(ThreadId::new()).turn = Some(waiting("second"));
        assert_eq!(state.pending_approvals().len(), 2, "an approval from each thread's turn");

        let questions = Questions {
            questions: vec![Question {
                header: "Name".to_string(),
                question: "Who?".to_string(),
                choices: Vec::new(),
                multi_select: false,
            }],
            ask: vec![Addressee::Actor(ActorId::None)],
            sensitive: false,
        };
        let asking = TurnState {
            tool_calls: vec![ToolCallState::asking(make_tool_call("ask"), questions)],
            ..waiting("third")
        };
        state.thread_mut(ThreadId::new()).turn = Some(asking);
        assert_eq!(state.pending_questions().len(), 1, "and the questions of another thread's");
    }

    #[test]
    fn what_its_calls_used_is_summed_over_its_threads_with_the_fullest_context() {
        let usage = |calls, context_tokens| TokenUsage {
            calls,
            prompt_tokens: 100 * calls,
            completion_tokens: 10 * calls,
            context_size: Some(1000),
            context_tokens,
        };
        assert_eq!(AgentChannelState::default().used(), None, "no call yet");
        // `ThreadId::default()` sorts before every other thread: the fuller context comes first in
        // one state, last in the other.
        for (first, later) in [(900, 300), (300, 900)] {
            let mut state = AgentChannelState::default();
            state.thread_mut(ThreadId::default()).used = Some(usage(1, first));
            state.thread_mut(ThreadId::new()).used = Some(usage(2, later));
            assert_eq!(
                state.used(),
                Some(TokenUsage {
                    calls: 3,
                    prompt_tokens: 300,
                    completion_tokens: 30,
                    context_size: Some(1000),
                    context_tokens: 900,
                }),
                "the calls and tokens of both, the context of the fuller"
            );
        }
    }

    #[test]
    fn the_attachments_of_its_transcript_are_numbered_on_its_copy_alone() {
        use crate::content::testing::make_content_ref;
        use crate::message::testing::{id, make_message};
        let a = make_content_ref("a.png", b"a");
        let b = make_content_ref("b.png", b"b");
        let c = make_content_ref("c.png", b"c");
        let mut state = AgentChannelState::default();
        let heard = Message { attachments: vec![a.clone()], ..make_message(id(1)) };
        state.perceive(ThreadId::default(), vec![heard.clone()]);
        *turn_slot(&mut state) = Some(TurnState::default());
        let sent = state
            .said(ThreadId::default(), Message { attachments: vec![b], ..make_message(id(2)) });
        assert_eq!(
            sent.attachments[0].number, None,
            "the copy it sends is the channel's, unnumbered"
        );
        assert!(state.modify(Message { attachments: vec![c, a], ..heard }));
        let numbers = |messages: Vec<Message>| {
            messages
                .iter()
                .map(|m| m.attachments.iter().filter_map(|a| a.reference()).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            numbers(state.transcript.messages(ThreadId::default())),
            [vec!["a3", "a1"], vec!["a2"]],
            "heard, said, then one a modification added - one it kept keeps its number"
        );
        assert!(
            !state.modify(Message {
                attachments: vec![make_content_ref("d.png", b"d")],
                ..make_message(id(9))
            }),
            "of no message it holds"
        );
        state.perceive(
            ThreadId::default(),
            vec![Message {
                attachments: vec![make_content_ref("e.png", b"e")],
                ..make_message(id(4))
            }],
        );
        assert_eq!(
            numbers(state.transcript.messages(ThreadId::default()))[2],
            ["a4"],
            "what it did not take was given no number"
        );
    }

    #[test]
    fn the_classifications_given_to_a_remote_agent_are_kept_across_its_calls() {
        use crate::message::testing::{id, make_message};
        let classified = |n, level| Message {
            classification: Classification::general(level),
            ..make_message(id(n))
        };
        let mut state = AgentChannelState::default();
        state.transcript.extend(
            ThreadId::default(),
            vec![
                classified(1, Confidentiality::Confidential),
                classified(2, Confidentiality::Internal),
            ],
        );
        assert_eq!(
            state.classifications_given(ThreadId::default()).len(),
            2,
            "what it is given now"
        );
        // The call is made: the transcript starts anew.
        state.transcript.start_anew(ThreadId::default());
        state
            .transcript
            .extend(ThreadId::default(), vec![classified(3, Confidentiality::Internal)]);
        state.record_change(TranscriptEntry::Reclassified {
            message: id(3),
            classification: Classification::PUBLIC,
        });

        let given = state.classifications_given(ThreadId::default()).to_vec();
        let expected = [
            Classification::general(Confidentiality::Confidential),
            Classification::general(Confidentiality::Internal),
            Classification::PUBLIC,
        ];
        assert_eq!(given, expected, "of the call before as well, each once, and of a change");
        assert_eq!(
            state.classifications_given(ThreadId::default()).len(),
            3,
            "asked again, nothing added"
        );
    }

    #[test]
    fn what_changed_in_an_external_agents_state_replays_onto_what_was_stored() {
        use crate::external_agent::{ConversationId, OpenTask, OpenTaskState, TaskId};
        use crate::message::testing::{id, make_message};
        let mut state = AgentChannelState::default();
        state.transcript.extend(ThreadId::default(), vec![make_message(id(1))]);
        let mut copy = stored(&mut state);
        assert!(copy.remote.is_empty(), "nothing kept of the remote side yet");

        state.transcript.extend(ThreadId::default(), vec![make_message(id(2))]);
        let remote = &mut state.remote;
        remote.conversation = Some(ConversationId("c-1".to_string()));
        remote.open_tasks.push(OpenTask {
            task: TaskId("t-1".to_string()),
            since: Utc::now(),
            state: OpenTaskState::Working,
            unsaid: Some("so far".to_string()),
            work: None,
        });
        assert_eq!(state.changes().len(), 5, "the small parts; the transcript is the store's");
        replayed(&state, &mut copy);

        state.mark_stored();
        state.remote.open_tasks.clear();
        assert_eq!(state.changes().len(), 5, "the small parts alone");
        replayed(&state, &mut copy);
        assert!(copy.remote.open_tasks.is_empty());
    }

    #[test]
    fn a_turn_begun_after_a_release_is_labelled_by_the_message_as_it_stands() {
        let mut state = a_state_that_heard("the numbers", Classification::CHANNEL_DEFAULT);
        let heard = state.transcript.messages(ThreadId::default())[0].id;
        assert!(state.record_change(TranscriptEntry::Reclassified {
            message: heard,
            classification: Classification::PUBLIC
        }));

        begin(&mut state, Vec::new());
        assert_eq!(
            state.turn(ThreadId::default()).unwrap().work.label(),
            Classification::PUBLIC,
            "released, it no longer raises the label"
        );

        let agent = ActorId::Agent(AgentId::new());
        let answer = Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            turn_id: Some(TurnId::new()),
            ..said("hi", Classification::CHANNEL_DEFAULT)
        };
        state.transcript.push(ThreadId::default(), answer.clone());
        state.record_change(TranscriptEntry::Reclassified {
            message: answer.id,
            classification: Classification::PUBLIC,
        });
        assert_eq!(
            state.retried(ThreadId::default(), answer.id, agent),
            Ok(vec![answer.id]),
            "a change, not something said"
        );
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    fn only_an_agents_last_answer_is_answered_again() {
        use crate::tool::{ToolCall, ToolCallResult, ToolResultStatus};
        let agent = ActorId::Agent(AgentId::new());
        let turn = TurnId::new();
        let answer = |content: &str| Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            turn_id: Some(turn),
            ..said(content, Classification::CHANNEL_DEFAULT)
        };
        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        let question = state.transcript.messages(ThreadId::default())[0].clone();
        let first = answer("hi");
        state.transcript.push(ThreadId::default(), first.clone());

        assert_eq!(
            state.retried(ThreadId::default(), first.id, agent),
            Ok(vec![first.id]),
            "its last answer"
        );
        assert_eq!(
            state.retried(ThreadId::default(), question.id, agent),
            Err(RetryRefused::NotItsAnswer),
            "not its own"
        );
        assert_eq!(
            state.retried(ThreadId::default(), first.id, ActorId::Agent(AgentId::new())),
            Err(RetryRefused::NotItsAnswer),
            "another agent's"
        );

        // The user corrects the question: a change, not something said.
        let corrected = Message { content: Some("hello there".to_string()), ..question.clone() };
        state.record_change(TranscriptEntry::Modified { message: corrected });
        assert_eq!(
            state.retried(ThreadId::default(), first.id, agent),
            Ok(vec![first.id]),
            "still its last answer"
        );

        // Its turn ended: taken over, it is still its last answer; given up,
        // what it said stands and nothing is taken back.
        let mut finished = state.clone();
        finished.transcript.record(
            ThreadId::default(),
            TranscriptEntry::TurnFinished { turn_id: turn, at: Utc::now() },
        );
        assert_eq!(
            finished.retried(ThreadId::default(), first.id, agent),
            Ok(vec![first.id]),
            "its own turn's end"
        );
        let mut given_up = state.clone();
        given_up.transcript.record(
            ThreadId::default(),
            TranscriptEntry::TurnGivenUp {
                turn_id: turn,
                reason: GivenUpReason::Left,
                at: Utc::now(),
            },
        );
        assert_eq!(
            given_up.retried(ThreadId::default(), first.id, agent),
            Err(RetryRefused::NotLast),
            "a turn given up"
        );
        let mut compacted = state.clone();
        compacted
            .transcript
            .record(ThreadId::default(), TranscriptEntry::Compacted { at: Utc::now() });
        assert_eq!(
            compacted.retried(ThreadId::default(), first.id, agent),
            Err(RetryRefused::NotLast),
            "a compaction since, which may have summarised it (issue 387)"
        );

        // What its turn did after it is taken back with it - not a step of
        // a later turn, nor a call that ran.
        let later_turn =
            Message { internal: true, turn_id: Some(TurnId::new()), ..answer("later") };
        let mut later = state.clone();
        later.transcript.push(ThreadId::default(), later_turn);
        assert_eq!(
            later.retried(ThreadId::default(), first.id, agent),
            Err(RetryRefused::NotLast),
            "a later turn followed"
        );
        let call = ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({}),
            ..make_tool_call("f")
        };
        let mut result = ToolCallResult::of(
            &call,
            ToolResultStatus::Ok,
            "done".to_string(),
            Classification::CHANNEL_DEFAULT,
        );
        let step = Message { internal: true, ..answer("thinking") };
        state.transcript.push(ThreadId::default(), step.clone());
        assert_eq!(
            state.retried(ThreadId::default(), first.id, agent),
            Ok(vec![first.id, step.id])
        );
        result.started_at = Some(Utc::now());
        state.transcript.push(
            ThreadId::default(),
            Message {
                internal: true,
                role: MessageRole::Tool,
                tool_results: vec![result],
                ..answer("")
            },
        );
        assert_eq!(
            state.retried(ThreadId::default(), first.id, agent),
            Err(RetryRefused::CallsRan)
        );

        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        state.transcript.push(ThreadId::default(), first.clone());
        let later = said("and now?", Classification::CHANNEL_DEFAULT);
        state.perceive(ThreadId::default(), vec![later]);
        assert_eq!(
            state.retried(ThreadId::default(), first.id, agent),
            Err(RetryRefused::NotLast),
            "something was said since"
        );

        let mut state = a_state_that_heard("hello", Classification::CHANNEL_DEFAULT);
        state.transcript.push(ThreadId::default(), first.clone());
        state.record_change(TranscriptEntry::Revoked { message: first.id, at: Utc::now() });
        assert_eq!(
            state.retried(ThreadId::default(), first.id, agent),
            Err(RetryRefused::NotItsAnswer),
            "taken back already"
        );
        assert_eq!(
            serde_json::to_value(RetryRefused::NotLast).unwrap(),
            serde_json::json!("not_last"),
            "as a user interface reads it"
        );
    }
}
