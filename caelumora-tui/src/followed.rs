//! A channel as the terminal follows it for the user: what it prints and shows of what the
//! channel shows, and what the user's commands there need of it (issues 561, 615).

use crate::activity::Place;
use crate::printed::{beginning, ErrorKind, Printed, Said, Saying, Words};
use crate::typeable::Typeable;
use crate::under_way::UnderWay;
use anyhow::Result;
use caelumora_api::reader::ViewResult;
use caelumora_domain::actor::{ActorEvent, ActorId, Envelope};
use caelumora_domain::agent::AgentId;
use caelumora_domain::approval::{ApprovalId, ApprovalRequest};
use caelumora_domain::channel::{ChannelId, ThreadId};
use caelumora_domain::command::{agents_commands, ParameterKind};
use caelumora_domain::history::{HistoryEntry, HistoryPage, HistoryPageItem};
use caelumora_domain::mention::MentionDirectory;
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::query::Filter;
use caelumora_domain::question::{QuestionId, QuestionRequest};
use caelumora_domain::snapshot::Detail;
use caelumora_domain::tool::ToolPresentation;
use caelumora_domain::turn::GivenUpReason;
use caelumora_domain::user::UserId;
use caelumora_domain::value::name::Name;
use caelumora_domain::work::TurnId;
use caelumora_kernel::actor::user::{ChannelWatch, Lagged, UserActor};
use caelumora_kernel::actor::Shown;
use caelumora_kernel::channel::ChannelLoop;
use caelumora_kernel::view::{ChannelView, Views};
use std::collections::{HashMap, HashSet};
use std::iter;
use std::sync::Arc;
use tracing::warn;

/// How many items of a channel's history the terminal reads as the user begins to talk in it, of
/// which it prints what was said.
const HISTORY_ITEMS: usize = 50;

/// A channel the user takes part in, as the terminal follows it - as the channel stream of the
/// REST server does (`docs/CHANNELS.md`, issue 386): the channel, the views it is read through in
/// their name and what their actor in it shows, the approvals pending there and those the user
/// answered, which questions it knows, the answers under way, the quiet calls whose results are to
/// come, the messages printed from its history, what `/retry` asks to be given again, the thread
/// of it the user follows (issue 595), and its threads open (issue 615).
pub(crate) struct Followed {
    /// The channel followed, in which the user's actor is reached where they act: a run of it may
    /// have ended and another begun since it was followed - they left and joined again. Held, not
    /// reached anew as the REST server does per request: the terminal keeps its session running,
    /// in which a channel keeps its loop - but for one deleted and restored, whose following ends
    /// with its deletion.
    channel: Arc<ChannelLoop>,
    /// The user it is followed for.
    user: UserId,
    /// The views as the user reads them, whose viewer is the user it is followed for: what tells
    /// whether the channel still lists them.
    views: Views,
    /// What the user's actor in the channel shows, across its runs.
    watch: ChannelWatch,
    /// The approval requests pending - those the agents' turns waited for as they were last read
    /// (`catch_up`), in the order of the participants and their calls, then those shown, in the
    /// order they were requested -, until the approver ends them - decided, timed out or
    /// withdrawn -, the user answered them (`answered_approvals`), or the turns read anew no longer
    /// hold them, as the web UI keeps them - but for an answer the approver refuses, after which
    /// the web UI offers the request again and the terminal does not (`answered_approvals`): so
    /// that `/approve` and `/deny` find the channel a request waits in, `/switch` shows it again,
    /// and the terminal asks what is the user's to decide. Which agent asked, and whether an
    /// answer counts, the approver tells, which holds the request (issue 567). Covers both plan
    /// proposals and regular tool calls - both flow through the same generic `ApprovalRequested`
    /// event.
    pending_approvals: Vec<ApprovalRequest>,
    /// The approvals the user answered - by hand as the answer is sent (`approve`), or by a rule
    /// of theirs as the channel shows it -, until the approver ends them (issue 580): not pending
    /// for the user any more, also where the turns read anew still hold them, since they do not
    /// tell who answered. One whose end came before the answer - known as pending from a turn not
    /// stored anew yet - stays for the run of the program, harmless, as ids are given once. An
    /// answer the approver refuses does not bring one back, unlike on the web UI: it refuses what
    /// it does not hold - decided, withdrawn, never asked - and the answer of one who may not
    /// approve, which asking again would only have refused again; where it cannot read the agent
    /// that asked, the answer might count later, and that rare case waits for the approval to
    /// end: timed out, decided by others, or withdrawn.
    answered_approvals: HashSet<ApprovalId>,
    /// The questions known - those put to the user that were open as what waits was read
    /// (`catch_up`), and those shown (issue 165) -, until they close: so that questions are shown
    /// once, also those asked anew by an agent that resumed its turn, and `/answer` and `/decline`
    /// with an id find the channel they wait in. Which are open is their actor's to tell
    /// (`UserActor::questions_to_answer`).
    known_questions: HashSet<QuestionId>,
    /// Whether the user may approve the calls of an agent, by agent, as the channel's view told it
    /// once (`may_approve`).
    approvable: HashMap<AgentId, bool>,
    /// The last answer of an agent shown, which `/retry` asks it to give again (issue 234) unless
    /// one of the two below is known.
    last_answer: Option<MessageId>,
    /// The answers under way in every thread of the channel, in the order they began - those the
    /// terminal learnt of as it caught up after those it knew -: what the screen shows as such,
    /// with what they streamed of the thread followed alone (issue 572), and of which `/retry`
    /// asks for the last of the thread followed first (issue 235).
    under_way: Vec<UnderWay>,
    /// The calls presented `quiet` whose results are yet to come, by id, with the agent that made
    /// them and what would be printed of them - shown live or printed from the history -, so that
    /// a result of one that is not left out is printed with its call. Dropped as the agent's turn
    /// ends; one of a turn ended before the history was read stays until the agent's next turn
    /// ends. A result presented `quiet` that succeeded is left out by its own presentation, its
    /// call found or not (issue 589); any other whose call was missed - the watch lagged, or the
    /// call lay before the history read - is printed alone.
    quiet_calls: HashMap<String, (ActorId, Printed)>,
    /// The messages printed from the history as the user began to talk in the channel last,
    /// replaced at each reading: one the channel shows after - said as the history was read - is
    /// printed once. Taken out as the channel shows it.
    from_history: HashSet<MessageId>,
    /// The turn given up that may be answered anew, with nothing said since (issue 259).
    given_up: Option<(ActorId, TurnId)>,
    /// The thread of the channel the user follows (issue 595): what is printed is what its view
    /// shows, and what they type goes there. The line unless they chose another.
    thread: ThreadId,
    /// The threads open in the channel, its line not among them, as they were read last
    /// (`read_threads`, issue 615).
    threads: Vec<OpenThread>,
}

/// A thread open in the channel, as the terminal keeps it (issues 595, 615).
#[derive(Debug, Clone)]
pub(crate) struct OpenThread {
    pub(crate) id: ThreadId,
    /// The agent it was delegated to - a sub-agent -, by name; `None` while it was delegated to
    /// none.
    delegated_to: Option<String>,
    /// The message it hangs at.
    hangs_at: MessageId,
}

impl OpenThread {
    /// What the footer calls it (issue 615): the agent it was delegated to, else `thread`.
    pub(crate) fn name(&self) -> &str {
        self.delegated_to.as_deref().unwrap_or("thread")
    }

    /// `Thread of` the agent it was delegated to, else `Thread`.
    pub(crate) fn title(&self) -> String {
        match &self.delegated_to {
            Some(name) => format!("Thread of {name}"),
            None => "Thread".to_string(),
        }
    }
}

impl Followed {
    /// Follows `channel` for the user `views` are read by, from now on, as the views let them
    /// (`ChannelView::user_follow`), knowing what waits for the user there, as `catch_up` reads
    /// it - which whoever follows the channel tells (`waiting`). `None` if the viewer is no user
    /// the channel lists, or cannot read it.
    pub(crate) async fn new(channel: Arc<ChannelLoop>, views: Views) -> Option<Self> {
        let ActorId::User(user) = views.context().caller_id else { return None };
        let watch = views
            .channel(channel.session_id(), channel.id())
            .await
            .ok()?
            .user_follow()
            .await
            .ok()??;
        let thread = ThreadId::line(channel.id());
        let mut followed = Self {
            channel,
            user,
            views,
            watch,
            pending_approvals: Vec::new(),
            answered_approvals: HashSet::new(),
            known_questions: HashSet::new(),
            approvable: HashMap::new(),
            last_answer: None,
            under_way: Vec::new(),
            quiet_calls: HashMap::new(),
            from_history: HashSet::new(),
            given_up: None,
            thread,
            threads: Vec::new(),
        };
        followed.catch_up().await;
        Some(followed)
    }

    /// Reads anew what waits for the user in the channel (issue 565), and answers what is printed
    /// of what it did not know: the approvals pending, as the agents' turns hold them
    /// (`ChannelView::pending_approvals`), but those the user answered, which take the place of
    /// those known - one that ended meanwhile is known no more -, and the open questions put to the
    /// user, as their actor tells, which become known. Read as the channel is followed, and where
    /// what it showed may have been lost - its watch lagged, or a run of the user's actor began -,
    /// always after the watch began, as the channel stream of the REST server reads its snapshot:
    /// nothing is lost in between, and what the channel shows of it after is shown once
    /// (`take_in`). Approvals that cannot be read stay as they were known.
    ///
    /// The turns are read as stored, which may lag behind what the channel showed. Rare cases,
    /// each in the short time between an event and the agent storing its turn, or after a lag:
    /// an approval decided may still be held by a turn - it is known as pending until it is read
    /// anew, and an answer to it the approver refuses, which the user is told (`EventRefused`);
    /// an approval requested may not be held by a turn yet, or not stored at all where storing
    /// failed - it is known no more, though the approver holds it; an approval withdrawn while
    /// the watch lagged ends without the withdrawal being told, and stays among those the user
    /// answered where they did - harmless, as ids are given once. A new run of the user's actor
    /// knows none of the questions put to the one before (issue 561), so where one began, only
    /// approvals are made up for.
    ///
    /// The answers under way are read anew as well, from the running channel
    /// (`read_under_way`): one whose end was missed is under way no more, one whose start was
    /// missed is, and those known keep what they streamed. Nothing is printed of them; the screen
    /// shows them (`Screen::show_activity`). So are the threads open (`read_threads`), which stay
    /// as they were known where they cannot be read.
    pub(crate) async fn catch_up(&mut self) -> Vec<Printed> {
        let mut printed = Vec::new();
        self.read_threads().await;
        // A thread followed whose closing was missed - the watch lagged - is left for the line.
        if self.thread != self.line() && !self.threads.iter().any(|t| t.id == self.thread) {
            printed.push(self.back_to_the_line());
        }
        if let Some(mut pending) = self.read_pending_approvals().await {
            pending.retain(|request| !self.answered_approvals.contains(&request.id));
            let unknown = pending.iter().filter(|request| !self.is_pending(request.id));
            printed.extend(unknown.map(approval_printed));
            self.pending_approvals = pending;
        }
        for questions in self.put_to_user() {
            if self.known_questions.insert(questions.id) {
                printed.push(questions_printed(&questions, self.me()));
            }
        }
        if let Some(under_way) = self.read_under_way().await {
            self.under_way = under_way;
        }
        printed
    }

    /// The channel followed.
    pub(crate) fn channel_id(&self) -> ChannelId {
        self.channel.id()
    }

    /// The name of the channel, as it is called now.
    pub(crate) fn name(&self) -> Name {
        self.channel.name()
    }

    /// The thread of the channel the user follows (issue 595): where what they type goes.
    pub(crate) fn thread(&self) -> ThreadId {
        self.thread
    }

    /// Follows `thread` of the channel from now on (issue 595) - its line, or a thread open -,
    /// called `called`, and answers what tells so: what was said in it so far and that it is
    /// followed, as talking in a channel tells. The last answer and a turn given up are read anew,
    /// of that thread; the answers under way are kept of every thread already.
    pub(crate) async fn follow(&mut self, thread: ThreadId, called: &str) -> Vec<Printed> {
        self.switch_to(thread);
        let mut printed = self.history().await;
        printed.push(Printed::Notice(format!("Following {called} of {} now", self.name())));
        printed
    }

    /// Follows `thread` from now on, what was known of the thread followed before let go: the last
    /// answer and a turn given up that `/retry` asks for, what the answers under way streamed, the
    /// messages printed from its history (issue 595).
    fn switch_to(&mut self, thread: ThreadId) {
        self.thread = thread;
        self.last_answer = None;
        self.given_up = None;
        self.under_way.iter_mut().for_each(UnderWay::complete);
        self.from_history.clear();
    }

    /// Follows the line again, as the thread followed is gone, and answers what tells so.
    fn back_to_the_line(&mut self) -> Printed {
        self.switch_to(self.line());
        Printed::Notice(format!(
            "The thread you followed is closed: following {} again",
            self.name()
        ))
    }

    /// The line of the channel.
    fn line(&self) -> ThreadId {
        ThreadId::line(self.channel_id())
    }

    /// The threads open in the channel, its line not among them, as they were read last (issue
    /// 615).
    pub(crate) fn threads(&self) -> &[OpenThread] {
        &self.threads
    }

    /// The channel as a place the user can be (issue 615): where they are if it is `talked_in` -
    /// nothing marked, since the status line and its threads tell what goes on there, and it asks
    /// what waits itself -, else marked where an agent is at work in any of its threads or
    /// something waits for the user.
    pub(crate) fn place(&self, talked_in: bool) -> Place {
        Place {
            name: self.name().to_string(),
            current: talked_in,
            at_work: !talked_in && !self.under_way.is_empty(),
            waits: !talked_in && self.waits_for_user(),
        }
    }

    /// The line and the threads open as places the user can be (issue 615), each marked where an
    /// agent is at work in it but the thread followed, which the status line tells. What waits
    /// for the user there, the channel asks itself.
    pub(crate) fn thread_places(&self) -> Vec<Place> {
        let line = (self.line(), "line");
        let open = self.threads.iter().map(|thread| (thread.id, thread.name()));
        iter::once(line)
            .chain(open)
            .map(|(id, name)| {
                let current = id == self.thread;
                let answering = self.under_way.iter().any(|answer| answer.thread == id);
                Place {
                    name: name.to_string(),
                    current,
                    at_work: !current && answering,
                    waits: false,
                }
            })
            .collect()
    }

    /// Reads anew the threads open in the channel as its view lists them (issue 595), each with
    /// the sub-agent it was delegated to; where the channel cannot be read, they stay as they were
    /// known, as the approvals pending do.
    async fn read_threads(&mut self) {
        let view = match self.view().await {
            Ok(view) => view,
            Err(e) => {
                warn!("Cannot read the threads of channel {}: {}", self.channel_id(), e);
                return;
            }
        };
        let snapshot = view.snapshot(Detail::PARTICIPANTS | Detail::THREADS).await;
        let participants = &snapshot.participants;
        let name_of = |id: ActorId| participants.iter().find(|m| m.id == id)?.name.clone();
        self.threads = snapshot
            .threads
            .iter()
            .map(|thread| OpenThread {
                id: thread.id,
                delegated_to: thread.recipient.and_then(name_of),
                hangs_at: thread.opening.origin.message,
            })
            .collect();
    }

    /// Whether `envelope` changes the threads open as they were read last (issue 615): a thread
    /// opened or closed, or the first brief its delegate gives in a thread - sent by the delegate
    /// of that thread -, which tells that it was delegated, as nothing else does.
    fn changes_threads(&self, envelope: &Envelope) -> bool {
        let thread = envelope.context.thread_id;
        match envelope.event {
            ActorEvent::ThreadOpened { .. } | ActorEvent::ThreadClosed { .. } => true,
            ActorEvent::GivenToRead { .. } => {
                envelope.context.sender_id == ActorId::Delegate(thread)
                    && !self.threads.iter().any(|t| t.id == thread && t.delegated_to.is_some())
            }
            _ => false,
        }
    }

    /// What tells each thread open in `/thread list` (issue 595), in their order: its title, who
    /// takes part in it now, and the beginning of the message it hangs at; nothing more than its
    /// title where the channel cannot be read.
    pub(crate) async fn told_threads(&self) -> Vec<String> {
        let Ok(view) = self.view().await else {
            return self.threads.iter().map(OpenThread::title).collect();
        };
        let participants = view.snapshot(Detail::PARTICIPANTS).await.participants;
        let mut told = Vec::new();
        for thread in &self.threads {
            let taking_part: Vec<&str> = participants
                .iter()
                .filter(|m| m.threads.contains(&thread.id))
                .filter_map(|m| m.name.as_deref())
                .collect();
            let hangs_at = view.message(thread.hangs_at).await.ok();
            let words = hangs_at.and_then(|said| said.content).unwrap_or_default();
            let at = beginning(&words);
            told.push(format!("{} ({}), at \"{at}\"", thread.title(), taking_part.join(", ")));
        }
        told
    }

    /// What tells the user they talk in the channel now: what was said in it so far (`history`),
    /// its name, who answers there, and what waits for them there.
    pub(crate) async fn talking_in(&mut self) -> Vec<Printed> {
        let mut printed = self.history().await;
        printed.push(Printed::Notice(format!("Talking in {} now", self.name())));
        if self.thread != self.line() {
            let title = self.threads.iter().find(|t| t.id == self.thread).map(OpenThread::title);
            let title = title.unwrap_or_else(|| "a thread".to_string());
            printed.push(Printed::Notice(format!("Following {title}: /thread line for the line")));
        }
        printed.extend(self.who_answers().await);
        printed.extend(self.waiting());
        printed
    }

    /// What tells who answers in the channel, as the views tell it (issue 174); `None` if the
    /// channel cannot be read.
    async fn who_answers(&self) -> Option<Printed> {
        let view = self.view().await.ok()?;
        let unknown = || "from a bundle".to_string();
        let moderator = view.moderator().unwrap_or_else(unknown);
        Some(Printed::Plain(format!(
            "Moderator: {moderator}, responders: {}",
            view.responder_policy().unwrap_or_else(unknown)
        )))
    }

    /// The user's actor in the channel, through which what they say, run and answer goes, if a run
    /// of it is under way; `None` between two runs - the user joined and their actor has yet to
    /// start, or they left and the end of its run is yet to be read -, which a channel followed
    /// passes through as it goes.
    pub(crate) fn actor(&self) -> Option<Arc<UserActor>> {
        self.channel.user_actor(self.user)
    }

    /// Adds to `typeable` what the user can type in the channel (issue 579): its commands they
    /// may see (`UserActor::visible_commands`), the handles of its participants and of its agents,
    /// the commands its agents carry out that they may see (`command::agents_commands`), and the
    /// personalities they may read, which an agent made here is made of. What cannot be read adds
    /// nothing.
    pub(crate) async fn add_typeable(&self, typeable: &mut Typeable) {
        if let Some(actor) = self.actor() {
            typeable.add_commands(actor.visible_commands().await);
        }
        let participants = match self.view().await {
            Ok(view) => view.participants().await,
            Err(_) => Vec::new(),
        };
        let mut agents = Vec::new();
        for participant in &participants {
            let Some(handle) = participant.handle() else { continue };
            let handle = format!("@{handle}");
            typeable.add_named(ParameterKind::Participant, [handle.clone()]);
            if participant.id().as_agent_id().is_some() {
                agents.push((handle, participant.commands().await));
            }
        }
        typeable.add_named(ParameterKind::Agent, agents.iter().map(|(handle, _)| handle.clone()));
        typeable.add_agents_commands(agents_commands(agents));
        let personalities = self.views.personalities().await.unwrap_or_default();
        typeable.add_named(ParameterKind::Personality, personalities.into_iter().map(|p| p.name));
    }

    /// What the channel shows next, as `ChannelWatch::next`.
    ///
    /// # Errors
    /// As `ChannelWatch::next`.
    pub(crate) async fn next(&mut self) -> Result<Shown, Lagged> {
        self.watch.next().await
    }

    /// Whether following the channel has ended - the user no longer takes part in it, or it was
    /// deleted, or cannot be read -, as the views read it in their name now: what is asked where a
    /// run of the user's actor began or ended (`docs/CHANNELS.md`, issue 386), and where the event
    /// bus tells that the channels the user takes part in may have changed.
    pub(crate) async fn ended(&self) -> bool {
        !self.views.listed_in(self.channel.session_id(), self.channel_id()).await
    }

    /// Whether `envelope` concerns the user wherever they talk: what concerns them wherever they
    /// look (`Envelope::concerns`, issue 594) - an approval, questions put to them, a message that
    /// addresses them -, and what tells them of what they did themselves: an answer of theirs to
    /// an approval (issue 566), a call they made (issue 587). Of the end of an approval, `take_in`
    /// tells what concerns them: a withdrawal of one pending, an answer given by a rule of theirs.
    /// What the view of the thread followed leaves out concerns them nowhere (`shows`, issues 557,
    /// 595); the thread followed closing does.
    pub(crate) fn concerns_user(&self, envelope: &Envelope) -> bool {
        if matches!(envelope.event, ActorEvent::ThreadClosed { thread } if thread == self.thread) {
            return true;
        }
        if !self.shows(envelope) {
            return false;
        }
        let of_their_own = match &envelope.event {
            ActorEvent::ApprovalAnswered { .. } => envelope.context.sender_id == self.me(),
            ActorEvent::CompleteMessageGenerated(message) => {
                message.sender_id == self.me() && message.is_a_persons_call()
            }
            _ => false,
        };
        of_their_own || envelope.concerns(self.me())
    }

    /// Takes in `event`, sent in the user's name, and answers what the terminal prints of it:
    /// nothing, since what they typed is on the screen already, as typed - but for an answer of
    /// theirs to an approval, after which it is theirs to give no longer, as on the web UI, and
    /// which is told where a rule of theirs gave it (issue 13); and for a call they made
    /// (`Message::is_a_persons_call`, issue 587), which they typed as a command and whose result
    /// they see only now.
    fn take_in_own(&mut self, event: &ActorEvent) -> Vec<Printed> {
        match event {
            ActorEvent::ApprovalAnswered { id, decision, .. } => {
                self.mark_answered(*id);
                decision
                    .by_rule
                    .map(|_| Printed::Notice(format!("Approval {} answered by your rule", id.0)))
                    .into_iter()
                    .collect()
            }
            ActorEvent::CompleteMessageGenerated(message) if message.is_a_persons_call() => {
                self.shown_message(message, Saying::Complete)
            }
            _ => Vec::new(),
        }
    }

    /// Takes in `message`, complete, as `envelope` shows it, and answers what the terminal prints
    /// of it: each message as soon as it is complete, as the web UI shows it; the full response
    /// that ends an answer holds them again (issue 220). One of another thread that addresses the
    /// user (issue 557) is no answer of the thread followed that `/retry` asks for again.
    fn take_in_complete(&mut self, envelope: &Envelope, message: &Message) -> Vec<Printed> {
        if envelope.context.thread_id == self.thread {
            if matches!(message.sender_id, ActorId::Agent(_)) {
                self.last_answer = Some(message.id);
            }
            self.given_up = None;
        }
        if let Some(answer) = self.answer_of(envelope.context.sender_id) {
            answer.complete();
        }
        self.shown_message(message, Saying::Complete)
    }

    /// What shows again what waits for the user in the channel: the approvals pending and the open
    /// questions put to them that are known - one asked since, which the channel has yet to show,
    /// is shown as it shows it, and not twice.
    pub(crate) fn waiting(&self) -> Vec<Printed> {
        let approvals = self.pending_approvals.iter().map(approval_printed);
        let to_answer = self.questions_to_answer();
        let put_to_user = to_answer.iter().map(|q| questions_printed(q, self.me()));
        approvals.chain(put_to_user).collect()
    }

    /// Takes in what the channel showed: keeps what the user's commands and what is printed later
    /// need of it - the answers under way of every thread (`keep_under_way`) -, and answers what
    /// the terminal prints of it - of what was sent in the user's name, what `take_in_own` prints;
    /// of another thread, only what the view of the thread followed shows of it (`shows`, issues
    /// 557, 595). The thread followed closed, the user follows the line again. Where it changed the
    /// threads open, they are read anew (`changes_threads`, issue 615).
    pub(crate) async fn take_in(&mut self, envelope: &Envelope) -> Vec<Printed> {
        let mut printed = Vec::new();
        if let ActorEvent::ThreadClosed { thread } = envelope.event {
            if thread == self.thread {
                printed.push(self.back_to_the_line());
            }
        }
        if self.changes_threads(envelope) {
            self.read_threads().await;
        }
        self.keep_under_way(envelope);
        if !self.shows(envelope) {
            return printed;
        }
        printed.extend(self.taken_in(envelope));
        printed
    }

    /// Keeps the answers under way of every thread of the channel by `envelope` (issue 572): one
    /// begun takes the place of the agent's answer in its thread, an answer ends with its turn, and
    /// where an agent stands is that of its answer in the thread of the turn it runs, where it
    /// says it - or, said in the line, outside a turn, the agent's own, that of all its answers.
    fn keep_under_way(&mut self, envelope: &Envelope) {
        let agent = envelope.context.sender_id;
        let thread = envelope.context.thread_id;
        match &envelope.event {
            ActorEvent::ResponseStarted { turn_id } if matches!(agent, ActorId::Agent(_)) => {
                let replaced =
                    self.under_way.iter().find(|a| (a.agent, a.thread) == (agent, thread));
                if let Some(turn) = replaced.map(|answer| answer.turn) {
                    self.end_under_way(agent, turn);
                }
                let name = envelope.context.sender_name.clone();
                self.under_way.push(UnderWay::began(agent, name, thread, *turn_id, None));
            }
            // Where the agent stands in its answer (issue 243).
            ActorEvent::StateChanged(state) => {
                let in_line = thread == self.line();
                let its = |answer: &&mut UnderWay| {
                    answer.agent == agent && (in_line || answer.thread == thread)
                };
                for answer in self.under_way.iter_mut().filter(its) {
                    answer.state = Some(*state);
                }
            }
            // Its answer has ended, said or not.
            ActorEvent::TurnFinished { turn_id, .. } | ActorEvent::TurnGivenUp { turn_id, .. } => {
                self.end_under_way(agent, *turn_id);
            }
            _ => {}
        }
    }

    /// Whether the view of the thread followed shows `envelope` (issue 595): the line's view
    /// (`Envelope::is_shown_in_line`), or of another thread what happens in it and in the whole
    /// channel (`Envelope::is_shown_in_thread`) - and, wherever the user follows, what waits for
    /// them, which they answer from any thread, and what addresses them, as the line's view shows
    /// it of the other threads.
    fn shows(&self, envelope: &Envelope) -> bool {
        if self.thread == self.line() {
            return envelope.is_shown_in_line(self.me());
        }
        envelope.is_shown_in_thread(self.thread)
            || envelope.event.is_waited_for()
            || envelope.addresses(self.me())
    }

    /// What the terminal prints of `envelope`, which the view of the thread followed shows.
    fn taken_in(&mut self, envelope: &Envelope) -> Vec<Printed> {
        let sender_id = envelope.context.sender_id;
        if sender_id == self.me() {
            return self.take_in_own(&envelope.event);
        }
        let is_agent = matches!(sender_id, ActorId::Agent(_));
        match &envelope.event {
            // Kept as under way (`keep_under_way`).
            ActorEvent::ResponseStarted { .. } if is_agent => {
                vec![Printed::Answering { sender: envelope.context.sender_name.clone() }]
            }
            ActorEvent::StateChanged(_) | ActorEvent::TurnFinished { .. } => Vec::new(),
            // What streams of the message being written, shown as the answer under way until the
            // message is complete.
            ActorEvent::PartialMessageGenerated(partial)
            | ActorEvent::IntermediatePartialGenerated(partial) => {
                if let Some(answer) = self.answer_of(sender_id) {
                    answer.stream(partial);
                }
                Vec::new()
            }
            ActorEvent::CompleteMessageGenerated(message) => {
                self.take_in_complete(envelope, message)
            }
            // A step of a plan under way, with the calls it made and their results.
            ActorEvent::IntermediateMessageGenerated(message) => {
                if let Some(answer) = self.answer_of(sender_id) {
                    answer.complete();
                }
                self.shown_message(message, Saying::Step)
            }
            ActorEvent::TurnGivenUp { turn_id, reason } => {
                self.given_up =
                    (is_agent && reason.may_be_answered_again()).then_some((sender_id, *turn_id));
                vec![given_up(&envelope.context.sender_name, reason)]
            }
            // Shown once: known as what waits was read (`catch_up`), or asked again by an agent
            // that resumed its turn - and not at all where the user answered it.
            ActorEvent::ApprovalRequested { request, .. }
                if self.is_pending(request.id) || self.answered_approvals.contains(&request.id) =>
            {
                Vec::new()
            }
            ActorEvent::ApprovalRequested { request, .. } => {
                self.pending_approvals.push(request.clone());
                vec![approval_printed(request)]
            }
            // Another's answer alone decides nothing; it goes to the approver alone.
            ActorEvent::ApprovalAnswered { .. } => Vec::new(),
            // The approval is over: the approver decided, or it is no longer asked for.
            ActorEvent::ApprovalGranted(response)
            | ActorEvent::ApprovalDenied(response)
            | ActorEvent::ApprovalTimedOut(response) => {
                self.end_approval(response.id);
                Vec::new()
            }
            ActorEvent::ApprovalRequestWithdrawn { id } => {
                let withdrawn = self.end_approval(*id).is_some();
                let id = id.0;
                withdrawn
                    .then(|| Printed::Notice(format!("Approval {id} withdrawn: the turn ended")))
                    .into_iter()
                    .collect()
            }
            // Shown once: known as what waits was read (`catch_up`), or asked anew by an agent
            // that resumed its turn.
            ActorEvent::QuestionRequested(request) => {
                match self.known_questions.insert(request.id) {
                    true => vec![questions_printed(request, self.me())],
                    false => Vec::new(),
                }
            }
            ActorEvent::QuestionClosed { id, .. } => {
                self.known_questions.remove(id);
                Vec::new()
            }
            // What came back from a thread, given to read where it was opened (issue 557).
            ActorEvent::GivenToRead { message, .. } => {
                let printed = self.shown_message(message, Saying::Complete);
                given_printed(printed)
            }
            // What another changed of what was said (issue 234).
            ActorEvent::MessageModified(message) => {
                said_printed(message, &self.channel.mentions(), Saying::Edited)
                    .into_iter()
                    .collect()
            }
            _ => shown(envelope),
        }
    }

    /// The answers under way in every thread of the channel, in the order they began; what they
    /// streamed of the thread followed alone (issue 572).
    pub(crate) fn under_way(&self) -> &[UnderWay] {
        &self.under_way
    }

    /// Whether an approval pending for the user or questions put to them wait here (issue 572).
    pub(crate) fn waits_for_user(&self) -> bool {
        !self.pending_approvals.is_empty() || !self.questions_to_answer().is_empty()
    }

    /// Whether approval `id` is known as pending - read as what waits (`catch_up`), or shown
    /// since -, is not over, and the user has not answered it.
    pub(crate) fn is_pending(&self, id: ApprovalId) -> bool {
        self.pending_approvals.iter().any(|request| request.id == id)
    }

    /// The approvals known as pending (`is_pending`), in the order they became known.
    pub(crate) fn pending_approvals(&self) -> &[ApprovalRequest] {
        &self.pending_approvals
    }

    /// Whether questions `id` of this channel are still the user's to answer, as their actor
    /// tells; between two runs of it, as if they were, so that what is being answered is kept until
    /// a run tells.
    pub(crate) fn is_to_answer(&self, id: QuestionId) -> bool {
        self.actor().is_none_or(|actor| actor.questions_to_answer().iter().any(|q| q.id == id))
    }

    /// Whether questions `id` of this channel are known - open for the user as what waits was read
    /// (`catch_up`), or shown since -, and not closed.
    pub(crate) fn knows_questions(&self, id: QuestionId) -> bool {
        self.known_questions.contains(&id)
    }

    /// Sends the user's answer to the pending approval `id` - `approved` or not - through `actor`,
    /// theirs in the channel; from then on it is theirs to answer no more, until it ends.
    ///
    /// # Errors
    /// As `UserActor::send_approval`.
    pub(crate) async fn approve(
        &mut self,
        actor: &UserActor,
        id: ApprovalId,
        approved: bool,
    ) -> Result<()> {
        actor.send_approval(id, approved).await?;
        self.mark_answered(id);
        Ok(())
    }

    /// Asks through `actor`, the user's in the channel, for the answer under way, the turn given
    /// up or the last answer to be given again; `false` if there is none.
    ///
    /// # Errors
    /// As `UserActor::request_retry_of_turn` and `UserActor::request_retry`.
    pub(crate) async fn retry(&self, actor: &UserActor) -> Result<bool> {
        let here = self.under_way.iter().rev().find(|answer| answer.thread == self.thread);
        let under_way = here.map(|answer| (answer.agent, answer.turn));
        match (under_way.or(self.given_up), self.last_answer) {
            (Some((agent, turn)), _) => actor.request_retry_of_turn(agent, turn).await?,
            (None, Some(answer)) => actor.request_retry(answer).await?,
            (None, None) => return Ok(false),
        };
        Ok(true)
    }

    /// The user, as an actor of the channel.
    fn me(&self) -> ActorId {
        ActorId::User(self.user)
    }

    /// The approvals pending in the channel, as the views read them in the user's name; `None`,
    /// logged, if they cannot be read.
    async fn read_pending_approvals(&self) -> Option<Vec<ApprovalRequest>> {
        let read = async { self.view().await?.pending_approvals().await };
        match read.await {
            Ok(pending) => Some(pending),
            Err(error) => {
                let channel_id = self.channel_id();
                warn!("The terminal could not read the approvals pending in channel {channel_id}: {error:#}");
                None
            }
        }
    }

    /// Takes approval `id` out of those pending; `None` if it was not.
    fn take_pending(&mut self, id: ApprovalId) -> Option<ApprovalRequest> {
        let at = self.pending_approvals.iter().position(|request| request.id == id)?;
        Some(self.pending_approvals.remove(at))
    }

    /// Takes approval `id`, which the user answered, out of those pending, and keeps it among
    /// those answered until it ends.
    fn mark_answered(&mut self, id: ApprovalId) {
        self.take_pending(id);
        self.answered_approvals.insert(id);
    }

    /// Forgets approval `id`, which the approver ended; the request if it was pending for the
    /// user, `None` if it was not, or they answered it.
    fn end_approval(&mut self, id: ApprovalId) -> Option<ApprovalRequest> {
        self.answered_approvals.remove(&id);
        self.take_pending(id)
    }

    /// Whether the user may approve the calls of agent `agent_id` here, as the web UI offers its
    /// buttons (`ParticipantView::may_approve`): asked of the channel's view once per agent, for as
    /// long as the channel is followed - an offer, which the approver asks again as it counts an
    /// answer. `false` where the channel or the agent cannot be read, asked again next time.
    pub(crate) async fn may_approve(&mut self, agent_id: AgentId) -> bool {
        if let Some(may) = self.approvable.get(&agent_id) {
            return *may;
        }
        let read = async { self.view().await?.participant(ActorId::Agent(agent_id)).await };
        match read.await {
            Ok(participant) => {
                let may = participant.may_approve().await;
                self.approvable.insert(agent_id, may);
                may
            }
            Err(error) => {
                let channel_id = self.channel_id();
                warn!("The terminal could not read agent {agent_id} in channel {channel_id}: {error:#}");
                false
            }
        }
    }

    /// The open questions put to the user that are known (`knows_questions`) - what the terminal
    /// asks and names -, in the order they were asked.
    pub(crate) fn questions_to_answer(&self) -> Vec<QuestionRequest> {
        let mut put_to_user = self.put_to_user();
        put_to_user.retain(|q| self.knows_questions(q.id));
        put_to_user
    }

    /// The open questions put to the user, in the order they were asked, as their actor tells
    /// (`UserActor::questions_to_answer`); none between two runs of it.
    fn put_to_user(&self) -> Vec<QuestionRequest> {
        self.actor().map(|actor| actor.questions_to_answer()).unwrap_or_default()
    }

    /// The answer under way of `agent` in the thread followed, if it gives one.
    fn answer_of(&mut self, agent: ActorId) -> Option<&mut UnderWay> {
        let thread = self.thread;
        self.under_way.iter_mut().find(|answer| (answer.agent, answer.thread) == (agent, thread))
    }

    /// The answers under way in every thread of the channel as its running channel tells them:
    /// those known that still are, in the order they began and with what they streamed, then those
    /// not known, in the order of the participants and of their threads, as begun now; `None` if
    /// the channel cannot be read.
    async fn read_under_way(&self) -> Option<Vec<UnderWay>> {
        let view = self.view().await.ok()?;
        let mut running = Vec::new();
        for participant in view.participants().await {
            let snapshot = participant.snapshot(Detail::THREADS).await;
            let name = snapshot.name.unwrap_or_else(|| snapshot.id.to_string());
            for (&thread, &turn) in &snapshot.turns_under_way {
                let answer =
                    UnderWay::began(snapshot.id, name.clone(), thread, turn, snapshot.state);
                running.push(answer);
            }
        }
        let is_known = |answer: &UnderWay| {
            self.under_way
                .iter()
                .any(|known| (known.agent, known.turn) == (answer.agent, answer.turn))
        };
        let still = self.under_way.iter().filter(|known| {
            running.iter().any(|answer| (known.agent, known.turn) == (answer.agent, answer.turn))
        });
        let mut under_way: Vec<UnderWay> = still.cloned().collect();
        under_way.extend(running.into_iter().filter(|answer| !is_known(answer)));
        Some(under_way)
    }

    /// Ends the answer under way of `agent` in `turn`, if it gives one, and forgets the agent's
    /// quiet calls, whose results will not come any more.
    fn end_under_way(&mut self, agent: ActorId, turn: TurnId) {
        self.under_way.retain(|answer| (answer.agent, answer.turn) != (agent, turn));
        self.quiet_calls.retain(|_, (by, _)| *by != agent);
    }

    /// What the terminal prints of `message`, shown live in a message of the kind `saying`
    /// (`message_printed`) - nothing where it was printed from the history already.
    fn shown_message(&mut self, message: &Message, saying: Saying) -> Vec<Printed> {
        if self.from_history.remove(&message.id) {
            return Vec::new();
        }
        self.message_printed(message, saying)
    }

    /// What the terminal prints of the history of the thread followed so far: what was said among
    /// the last `HISTORY_ITEMS` of it as the user sees them - of the line, its view
    /// (`history_printed`); nothing where the history cannot be read, which is logged.
    async fn history(&mut self) -> Vec<Printed> {
        let line = ThreadId::line(self.channel_id());
        let thread = self.thread;
        let read = async {
            let view = self.view().await?;
            match thread == line {
                true => view.history_page(None, HISTORY_ITEMS).await,
                false => {
                    let filter = Some(Filter::in_thread(thread));
                    view.history_page_of(None, HISTORY_ITEMS, filter).await
                }
            }
        };
        match read.await {
            Ok(page) => self.history_printed(&page),
            Err(error) => {
                let channel_id = self.channel_id();
                warn!("The terminal could not read the history of channel {channel_id}: {error:#}");
                Vec::new()
            }
        }
    }

    /// What the terminal prints of `page` of the history: what was said (`message_printed`) and
    /// given back from a thread (issue 557), told as said earlier; nothing where nothing was said.
    /// Who joined or left, threads opened or closed, commands carried out and turns given up are
    /// told live alone: they carry no id by which one the channel shows after could be told from
    /// one printed here, and tell little after the fact. The messages
    /// printed are remembered, in place of those of the page before, so that one the channel
    /// shows after - said as the history was read - is not printed twice; the quiet calls among
    /// them join those under way, so that a result that comes live finds its call. The last answer
    /// of an agent in the thread followed is what `/retry` asks for, as one shown live is - not one
    /// of another thread that addresses the user (issue 595).
    fn history_printed(&mut self, page: &HistoryPage) -> Vec<Printed> {
        let mut printed = Vec::new();
        self.from_history.clear();
        for HistoryPageItem { thread, entry, .. } in &page.items {
            if let HistoryEntry::GivenToRead { message, .. } = entry {
                self.from_history.insert(message.id);
                let given = self.message_printed(message, Saying::Complete);
                printed.extend(given_printed(given));
                continue;
            }
            let HistoryEntry::Said { message } = entry else { continue };
            self.from_history.insert(message.id);
            let answer = matches!(message.sender_id, ActorId::Agent(_)) && !message.internal;
            if answer && *thread == Some(self.thread) {
                self.last_answer = Some(message.id);
            }
            let saying = if message.internal { Saying::Step } else { Saying::Complete };
            printed.extend(self.message_printed(message, saying));
        }
        if !printed.is_empty() {
            printed.insert(0, Printed::Notice(format!("Earlier in {}", self.name())));
        }
        printed
    }

    /// What the terminal prints of `message`, said in a message of the kind `saying`: its words and
    /// what it carries (`said_printed`), the tools it calls and how calls ended - but a call
    /// presented `quiet`, kept among the quiet calls until its result comes, and a result presented
    /// `quiet` that succeeded, which are left out; a result of a quiet call that is not left out is
    /// printed with its call, as the web UI shows them.
    fn message_printed(&mut self, message: &Message, saying: Saying) -> Vec<Printed> {
        let mut printed: Vec<Printed> =
            said_printed(message, &self.channel.mentions(), saying).into_iter().collect();
        let channel_id = self.channel_id();
        for call in &message.tool_calls {
            let sender = message.sender_name.clone();
            let called = Printed::ToolCall { channel_id, sender, call: Box::new(call.clone()) };
            match (&call.id, call.presentation == ToolPresentation::QUIET) {
                (Some(id), true) => {
                    self.quiet_calls.insert(id.clone(), (message.sender_id, called));
                }
                _ => printed.push(called),
            }
        }
        for result in &message.tool_results {
            let quiet = result.id.as_ref().and_then(|id| self.quiet_calls.remove(id));
            if result.presentation == ToolPresentation::QUIET && result.status.is_ok() {
                continue;
            }
            if let Some((_, called)) = quiet {
                printed.push(called);
            }
            printed.push(Printed::ToolResult { channel_id, result: Box::new(result.clone()) });
        }
        printed
    }

    /// The channel as the views read it in the user's name.
    ///
    /// # Errors
    /// As `Views::channel`.
    async fn view(&self) -> ViewResult<ChannelView> {
        self.views.channel(self.channel.session_id(), self.channel_id()).await
    }
}

/// What the terminal prints of `envelope` that the user's commands need nothing of: what was taken
/// back of what was said, and what was done in the channel.
fn shown(envelope: &Envelope) -> Vec<Printed> {
    let sender_id = envelope.context.sender_id;
    let sender = &envelope.context.sender_name;
    let plain = |line: String| Some(Printed::Plain(line));
    let printed = match &envelope.event {
        ActorEvent::MessageRevoked { .. } => plain(format!("{sender} took a message back")),
        ActorEvent::RetryAccepted { .. } => plain(format!("{sender} answers again")),
        ActorEvent::RetryRefused { reason, .. } => {
            plain(format!("{sender} cannot answer again: {reason}"))
        }
        // What this user sent that the channel did not let in (issue 304), told to them alone.
        ActorEvent::EventRejected { rejected, reason, .. } => Some(Printed::Error {
            kind: ErrorKind::Rejected,
            message: format!("the channel did not let in your {rejected}: {reason}"),
        }),
        // What this user sent that the actor it went to did not take (issue 567), told to them
        // alone: the approver an answer it does not count.
        ActorEvent::EventRefused { refused, reason, .. } => Some(Printed::Error {
            kind: ErrorKind::Refused,
            message: format!("{sender} did not take your {refused}: {reason}"),
        }),
        // Told to its issuer alone (issue 299).
        ActorEvent::CommandStarted { command, .. } => {
            plain(format!("{sender} is carrying out /{command}."))
        }
        // Carried out by an agent in a turn of its own (issue 269), or run by a user.
        ActorEvent::CommandExecuted { command, output, turn_id, .. } => {
            let verb = if turn_id.is_some() { "carried out" } else { "ran" };
            plain(format!("{sender} {verb} /{command}: {output}"))
        }
        ActorEvent::CommandFailed { command, reason, .. } => {
            plain(format!("{sender} did not carry out /{command}: {reason}"))
        }
        // Of its own accord (issue 284).
        ActorEvent::ActivityStarted { activity, .. } => plain(format!("{sender} is {activity}.")),
        ActorEvent::ActivityFinished { output, .. } => plain(output.to_string()),
        // Who joined or left (issue 408), by whose act.
        ActorEvent::ParticipantJoined { participant, name, .. } => plain(match sender_id {
            by if by == *participant => format!("{name} joined the channel"),
            ActorId::System => format!("{name} was added to the channel"),
            _ => format!("{sender} added {name} to the channel"),
        }),
        ActorEvent::ParticipantLeft { participant, name, .. } => plain(match sender_id {
            by if by == *participant => format!("{name} left the channel"),
            ActorId::System => format!("{name} was removed from the channel"),
            _ => format!("{sender} removed {name} from the channel"),
        }),
        // A thread of the channel (issue 557): what is said in it is left out of the line.
        ActorEvent::ThreadOpened { .. } => plain(format!("{sender} opened a thread")),
        ActorEvent::ThreadClosed { .. } => plain(format!("{sender} closed a thread")),
        _ => None,
    };
    printed.into_iter().collect()
}

/// What tells that `given`, printed of a message, came back from a thread to read (issue 557):
/// nothing where nothing of it is printed.
fn given_printed(given: Vec<Printed>) -> Vec<Printed> {
    if given.is_empty() {
        return given;
    }
    let mut printed = vec![Printed::Plain("Given back to read from a thread:".to_string())];
    printed.extend(given);
    printed
}

/// What tells that `sender` gave its answer up, and why: a failure as an error.
fn given_up(sender: &str, reason: &GivenUpReason) -> Printed {
    match reason {
        GivenUpReason::Failed { message } => {
            let message = format!("{sender} could not answer: {message}");
            Printed::Error { kind: ErrorKind::Failed, message }
        }
        GivenUpReason::Lowered { channel } => Printed::Plain(format!(
            "{sender} gave its answer up: the channel was lowered to {channel}"
        )),
        GivenUpReason::CallLimit { calls } => Printed::Plain(format!(
            "{sender} gave its answer up: it asked its model {calls} times, as often as one answer \
             or one step of a plan may"
        )),
        GivenUpReason::Left | GivenUpReason::Retried => {
            Printed::Plain(format!("{sender} gave its answer up: {}", reason.cause()))
        }
    }
}

/// What shows `request` for approval.
fn approval_printed(request: &ApprovalRequest) -> Printed {
    Printed::Approval(Box::new(request.clone()))
}

/// What shows the questions of `request` (issue 165), and whether they are put to `me`.
fn questions_printed(request: &QuestionRequest, me: ActorId) -> Printed {
    let put_to_user = request.questions.is_asked(me);
    Printed::Questions { request: Box::new(request.clone()), put_to_user }
}

/// What `message` says, in a message of the kind `saying`, with where it mentions participants
/// by the handles of the channel's (`directory`); `None` if it says nothing (`said`).
fn said_printed(
    message: &Message,
    directory: &MentionDirectory,
    saying: Saying,
) -> Option<Printed> {
    let said = said(message, directory)?;
    Some(Printed::Said { sender: message.sender_name.clone(), said, saying })
}

/// What `message` says: its words, with where they mention whom it mentions by the handles of the
/// channel's participants that run now (`directory`, issue 29) - those of who joined after the
/// channel was followed too, not those of who left -, and what it carries (issue 282); `None` if
/// it says neither.
fn said(message: &Message, directory: &MentionDirectory) -> Option<Said> {
    let words = message.content.as_deref().filter(|content| !content.is_empty()).map(|content| {
        let mentions = directory.mentioned_spans(content, &message.addressees);
        Words { text: content.to_string(), mentions }
    });
    let attachments = message.attachments.clone();
    (words.is_some() || !attachments.is_empty()).then_some(Said { words, attachments })
}

#[cfg(test)]
pub(crate) mod testing {
    //! What the tests of the terminal show a channel followed: envelopes, messages, approvals and
    //! questions, as a channel would send them, and an agent whose turn waits for an approval.

    use super::Followed;
    use crate::printed::Printed;
    use caelumora_domain::actor::{ActorEvent, ActorId, Envelope, EventContext};
    use caelumora_domain::agent::testing::{make_personality, waiting_for_approval};
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::testing::make_approval_request;
    use caelumora_domain::approval::{
        ApprovalDecision, ApprovalId, ApprovalRequest, ApprovalRuleId,
    };
    use caelumora_domain::channel::{ChannelId, HistoryAccess};
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::message::{Message, MessageId};
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_kernel::actor::Shown;
    use caelumora_kernel::channel::ChannelLoop;
    use caelumora_kernel::kernel::Kernel;
    use caelumora_kernel::session::SessionLoop;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::stored_agent;

    /// Routes `envelope` to the user's actor in the channel `followed` follows, as the channel
    /// would (`UserActor::take_in_as_routed`), and takes in what the actor shows of it
    /// (`next_taken_in`): what the terminal prints.
    pub(crate) async fn routed(followed: &mut Followed, envelope: Envelope) -> Vec<Printed> {
        let actor = followed.actor().expect("the user's actor runs");
        actor.take_in_as_routed(envelope).await;
        next_taken_in(followed).await
    }

    /// Takes in the next envelope the channel `followed` follows shows, passing over where a run
    /// of the user's actor begins or ends: what the terminal prints.
    pub(crate) async fn next_taken_in(followed: &mut Followed) -> Vec<Printed> {
        loop {
            if let Ok(Shown::Envelope(shown)) = followed.next().await {
                return followed.take_in(&shown).await;
            }
        }
    }

    /// `event` as `sender` sends it in `channel`.
    pub(crate) fn from(channel: &ChannelLoop, sender: ActorId, event: ActorEvent) -> Envelope {
        let context =
            EventContext::new(channel.session_id(), channel.id(), sender, sender.to_string());
        Envelope::new(context, event)
    }

    pub(crate) fn message(sender: ActorId, content: &str) -> Message {
        Message {
            sender_id: sender,
            sender_name: "other".to_string(),
            content: Some(content.to_string()),
            ..make_message(MessageId::new())
        }
    }

    /// The answer of `by` to the approval request `id`, given by their rule `by_rule` if any.
    pub(crate) fn answered(
        by: ActorId,
        id: ApprovalId,
        by_rule: Option<ApprovalRuleId>,
    ) -> ActorEvent {
        let decision =
            ApprovalDecision { by, name: by.to_string(), by_rule, at: Default::default() };
        ActorEvent::ApprovalAnswered { id, approved: true, decision }
    }

    /// An agent of the user of `owner`, stored in `kernel`, which they take into `session_loop`
    /// and its channel `channel_id`.
    pub(crate) async fn an_agent_in(
        kernel: &Kernel,
        session_loop: &SessionLoop,
        owner: &SecurityContext,
        channel_id: ChannelId,
    ) -> AgentId {
        let stores = kernel.entity_stores();
        let personality = make_personality("helper");
        let agent = stored_agent(stores, "helper", personality, owner.caller_id).await.id;
        let into = Some((channel_id, HistoryAccess::All));
        session_loop
            .add_participant(owner, ActorId::Agent(agent), into)
            .await
            .expect("the owner takes it in");
        agent
    }

    /// Stores a turn of `agent` in `channel` that waits for the approval of one call, as the agent
    /// keeps it (`agent::testing::waiting_for_approval`), and answers the id of the request the
    /// views read from it.
    pub(crate) async fn approval_waiting_in(
        kernel: &Kernel,
        channel: &ChannelLoop,
        agent: AgentId,
    ) -> ApprovalId {
        let calls = vec![make_tool_call("test_tool")];
        let mut state = waiting_for_approval(channel.session_id(), channel.id(), agent, calls);
        kernel
            .state_store()
            .put_agent_channel_state(agent, channel.id(), &mut state)
            .await
            .expect("the state is stored");
        state.pending_approvals().first().expect("the call waits").id
    }

    pub(crate) fn approval(channel: &ChannelLoop, agent_id: AgentId) -> ApprovalRequest {
        make_approval_request(channel.session_id(), channel.id(), agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{
        an_agent_in, answered, approval, approval_waiting_in, from, message, next_taken_in, routed,
    };
    use super::*;
    use crate::screen::testing::lines;
    use anyhow::Error;
    use caelumora_domain::actor::{ActorState, EventContext, Rejection};
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::{ApprovalRuleId, ApprovalVerdict, ApproverId};
    use caelumora_domain::chain::Position;
    use caelumora_domain::channel::{HistoryAccess, ThreadOrigin};
    use caelumora_domain::content::testing::make_content_ref;
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::message::MessageRole;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::{ToolCall, ToolCallResult, ToolResultStatus};
    use caelumora_kernel::channel::ChannelError;
    use caelumora_kernel::kernel::Kernel;
    use caelumora_kernel::session::testing::{
        owned_kernel, running_session_of_a_user, spec, test_kernel,
    };
    use caelumora_kernel::session::SessionLoop;
    use caelumora_kernel::testing::{Script, ScriptedAgents, TestKernel};
    use caelumora_kernel::view::Viewer;
    use caelumora_security::confidentiality::ConfidentialityRules;
    use caelumora_security::SecurityContext;
    use chrono::Utc;
    use std::time::Duration;
    use tokio::time;

    /// A running session of a user in `kernel`, with the context of its owner.
    async fn running(kernel: &Kernel) -> (Arc<SessionLoop>, SecurityContext) {
        let (session_loop, ctx, _) = running_session_of_a_user(kernel).await;
        (session_loop, ctx)
    }

    /// A channel followed by the owner of a running session, the first of their session.
    async fn followed() -> (Followed, Arc<ChannelLoop>, ActorId) {
        let kernel = test_kernel();
        let (session_loop, ctx) = running(&kernel).await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let views = kernel.views(Viewer::new(ctx.clone()));
        let followed =
            Followed::new(Arc::clone(&channel), views).await.expect("the owner takes part");
        (followed, channel, ctx.caller_id)
    }

    /// The ids of the questions `requests`.
    fn ids(requests: &[QuestionRequest]) -> Vec<QuestionId> {
        requests.iter().map(|request| request.id).collect()
    }

    /// Whether `followed` shows what `wanted` asks for within five seconds, passing over what it
    /// shows before.
    async fn shows(followed: &mut Followed, wanted: fn(&Shown) -> bool) -> bool {
        let shown = async { while !followed.next().await.is_ok_and(|shown| wanted(&shown)) {} };
        time::timeout(Duration::from_secs(5), shown).await.is_ok()
    }

    #[test]
    fn a_message_says_its_words_and_what_it_carries() {
        let message = |content: Option<&str>, attachments| Message {
            content: content.map(str::to_string),
            attachments,
            ..make_message(MessageId::new())
        };
        let chart = make_content_ref("chart.png", b"chart");
        let said_by = |message: Message| said(&message, &MentionDirectory::default());
        let words = |text: &str| Some(Words { text: text.to_string(), mentions: Vec::new() });
        let carried = vec![chart.clone()];
        assert_eq!(
            said_by(message(Some("hello"), Vec::new())),
            Some(Said { words: words("hello"), attachments: Vec::new() })
        );
        assert_eq!(
            said_by(message(None, vec![chart.clone()])),
            Some(Said { words: None, attachments: carried.clone() }),
            "attachments alone"
        );
        assert_eq!(
            said_by(message(Some("look"), vec![chart])),
            Some(Said { words: words("look"), attachments: carried })
        );
        assert_eq!(said_by(message(Some(""), Vec::new())), None, "nothing said");
    }

    /// A message printed, or printed again as edited, mentions by their handles who take part in
    /// the channel as it is printed: also who joined after the channel was followed, no longer who
    /// left.
    #[tokio::test]
    async fn a_message_mentions_who_takes_part_as_it_is_printed() {
        let kernel = test_kernel();
        let (session_loop, ctx) = running(&kernel).await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let views = kernel.views(Viewer::new(ctx.clone()));
        let mut followed =
            Followed::new(Arc::clone(&channel), views).await.expect("the owner takes part");
        let agent = ActorId::Agent(an_agent_in(&kernel, &session_loop, &ctx, channel.id()).await);
        let mentioning = Message { addressees: vec![agent], ..message(agent, "@helper is here") };
        let mentions = |printed: Vec<Printed>| {
            let [Printed::Said { said: Said { words: Some(words), .. }, .. }] = printed.as_slice()
            else {
                panic!("the message is printed: {printed:?}");
            };
            words.mentions.clone()
        };
        let said = from(&channel, agent, ActorEvent::CompleteMessageGenerated(mentioning.clone()));
        assert_eq!(mentions(followed.take_in(&said).await), vec![0..7], "the handle of who joined");
        channel.remove_participant(&ctx, agent).await.expect("the owner may remove it");
        let edited = from(&channel, agent, ActorEvent::MessageModified(mentioning));
        assert!(mentions(followed.take_in(&edited).await).is_empty(), "not the handle of who left");
    }

    /// What the channel did not let in of the user's is told to them as rejected, apart from what
    /// an actor refused (issue 567).
    #[tokio::test]
    async fn a_rejection_is_told_as_the_channel_s() {
        let (mut followed, channel, _) = followed().await;
        let rejected = ActorEvent::EventRejected {
            rejected: "CompleteMessageGenerated".to_string(),
            message: None,
            reason: Rejection::ClassificationRefused,
        };
        let told = lines(&followed.take_in(&from(&channel, ActorId::System, rejected)).await);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0]
                .starts_with("Rejected: the channel did not let in your CompleteMessageGenerated"),
            "{told:?}"
        );
    }

    /// What an actor did not take of the user's is told to them, by whom (issue 567).
    #[tokio::test]
    async fn a_refusal_is_told_by_whom() {
        let (mut followed, channel, me) = followed().await;
        let approver = ActorId::Approver(ApproverId::new());
        let refused = ActorEvent::EventRefused {
            to: me,
            refused: "ApprovalAnswered".to_string(),
            message: None,
            reason: Rejection::Forbidden,
        };
        let told = lines(&followed.take_in(&from(&channel, approver, refused)).await);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0].starts_with("Refused: ")
                && told[0].contains("did not take your ApprovalAnswered"),
            "{told:?}"
        );
    }

    #[tokio::test]
    async fn what_the_user_said_is_not_printed_again() {
        let (mut followed, channel, me) = followed().await;
        let other = ActorId::User(UserId::new());
        let said = |sender| ActorEvent::CompleteMessageGenerated(message(sender, "hello"));
        assert!(followed.take_in(&from(&channel, me, said(me))).await.is_empty());
        assert_eq!(
            lines(&followed.take_in(&from(&channel, other, said(other))).await),
            ["other: hello"]
        );
    }

    /// Issue 587: a call the user made, which they typed as a command, is printed with its result
    /// as the channel tells it - wherever they talk.
    #[tokio::test]
    async fn a_call_the_user_made_is_printed_with_its_result() {
        let (mut followed, channel, me) = followed().await;
        let call = make_tool_call("run_command");
        let result =
            ToolCallResult::of(&call, ToolResultStatus::Ok, "hi".to_string(), Default::default());
        let called = Message {
            role: MessageRole::User,
            content: None,
            tool_calls: vec![call],
            tool_results: vec![result],
            ..message(me, "")
        };
        let event = from(&channel, me, ActorEvent::CompleteMessageGenerated(called));
        assert!(followed.concerns_user(&event), "wherever they talk");
        let told = lines(&followed.take_in(&event).await);
        assert_eq!(told.len(), 2, "{told:?}");
        assert!(told[0].contains("calls run_command") && told[1].contains("answered"), "{told:?}");
    }

    /// Issue 557: of another thread the line's view prints its approvals and what addresses the
    /// user alone; what came back from it is printed as given back, and a thread opened as such.
    #[tokio::test]
    async fn of_another_thread_only_what_waits_or_mentions_the_user_is_printed() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let thread_id = ThreadId::new();
        let in_other = |event| {
            let envelope = from(&channel, agent, event);
            Envelope { context: EventContext { thread_id, ..envelope.context }, ..envelope }
        };
        let said = ActorEvent::CompleteMessageGenerated(message(agent, "aside"));
        assert!(!followed.concerns_user(&in_other(said.clone())));
        assert!(followed.take_in(&in_other(said)).await.is_empty(), "said in another thread");
        let mentioning = Message { addressees: vec![me], ..message(agent, "@owner, look") };
        let mentioning = ActorEvent::CompleteMessageGenerated(mentioning);
        assert!(followed.concerns_user(&in_other(mentioning.clone())), "it addresses them");
        assert_eq!(lines(&followed.take_in(&in_other(mentioning)).await), ["other: @owner, look"]);
        assert_eq!(followed.last_answer, None, "no answer of the line to ask for again");
        let request =
            ActorEvent::ApprovalRequested { request: approval(&channel, agent_id), to: None };
        assert!(followed.concerns_user(&in_other(request.clone())));
        assert_eq!(followed.take_in(&in_other(request)).await.len(), 1, "its call waits");
        // An answer begun in another thread ends with its turn there.
        let turn_id = TurnId::new();
        assert!(followed
            .take_in(&in_other(ActorEvent::ResponseStarted { turn_id }))
            .await
            .is_empty());
        assert_eq!(followed.under_way().len(), 1, "kept, though nothing is printed of it");
        let finished = ActorEvent::TurnFinished { turn_id, messages: Vec::new(), deferred: false };
        assert!(followed.take_in(&in_other(finished)).await.is_empty());
        assert!(followed.under_way().is_empty(), "ended, in another thread");

        let origin =
            ThreadOrigin { thread: ThreadId::line(channel.id()), message: MessageId::new() };
        let opened = ActorEvent::ThreadOpened { thread: thread_id, origin };
        let printed = lines(&followed.take_in(&from(&channel, agent, opened)).await);
        assert_eq!(printed, [format!("{agent} opened a thread")]);
        let recipient = ActorId::Agent(AgentId::new());
        let given = ActorEvent::GivenToRead { recipient, message: message(agent, "found it") };
        let delegate = ActorId::Delegate(thread_id);
        assert_eq!(
            lines(&followed.take_in(&from(&channel, delegate, given)).await),
            ["Given back to read from a thread:", "other: found it"]
        );
    }

    /// Issue 595: following a thread, the terminal prints what its view shows - what happens in it
    /// and in the whole channel, not what is said in the line -, and what waits for the user or
    /// addresses them wherever it was asked; the last answer `/retry` asks for is the thread's; as
    /// the thread closes, which concerns the user wherever they talk, they follow the line again,
    /// nothing of the thread left to ask for again.
    #[tokio::test]
    async fn a_thread_followed_shows_what_happens_in_it_and_the_line_again_once_it_closes() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let thread_id = ThreadId::new();
        let in_thread = |event| {
            let envelope = from(&channel, agent, event);
            Envelope { context: EventContext { thread_id, ..envelope.context }, ..envelope }
        };
        let printed = lines(&followed.follow(thread_id, "a thread").await);
        assert_eq!(printed.last().map(String::as_str), Some("Following a thread of main now"));
        assert_eq!(followed.thread(), thread_id);

        let said = |text| ActorEvent::CompleteMessageGenerated(message(agent, text));
        assert!(followed.take_in(&from(&channel, agent, said("in the line"))).await.is_empty());
        let in_it = message(agent, "in it");
        let answered = ActorEvent::CompleteMessageGenerated(in_it.clone());
        assert_eq!(lines(&followed.take_in(&in_thread(answered)).await), ["other: in it"]);
        let actor = followed.actor().expect("the owner's actor runs");
        let error = followed.retry(&actor).await.expect_err("no such message in the channel");
        let error = error.downcast::<ChannelError>().expect("refused by the actor");
        assert!(
            matches!(error, ChannelError::MessageNotFound { message, .. } if message == in_it.id),
            "the thread's answer to ask for again: {error}"
        );
        let request =
            ActorEvent::ApprovalRequested { request: approval(&channel, agent_id), to: None };
        assert_eq!(followed.take_in(&from(&channel, agent, request)).await.len(), 1, "it waits");
        let mentioning = Message { addressees: vec![me], ..message(agent, "@owner, here") };
        let mentioning = ActorEvent::CompleteMessageGenerated(mentioning);
        let shown = lines(&followed.take_in(&from(&channel, agent, mentioning)).await);
        assert_eq!(shown, ["other: @owner, here"], "mentioning them in the line");

        let closed = from(&channel, agent, ActorEvent::ThreadClosed { thread: thread_id });
        assert!(followed.concerns_user(&closed), "told wherever they talk");
        let printed = lines(&followed.take_in(&closed).await);
        assert_eq!(printed[0], "The thread you followed is closed: following main again");
        assert_eq!(followed.thread(), ThreadId::line(channel.id()), "the line followed again");
        assert!(!followed.retry(&actor).await.unwrap(), "nothing of the thread to give again");
    }

    /// An approval shown waits until the approver ends it - decided, timed out or withdrawn -, or
    /// the user answers it; another's answer ends nothing (issue 567).
    #[tokio::test]
    async fn an_approval_waits_until_the_approver_ends_it() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let request = approval(&channel, agent_id);
        let id = request.id;
        let shown = lines(
            &followed
                .take_in(&from(
                    &channel,
                    agent,
                    ActorEvent::ApprovalRequested { request, to: None },
                ))
                .await,
        );
        assert!(shown.first().is_some_and(|line| line.contains(&id.0.to_string())), "{shown:?}");
        let withdrawn = ActorEvent::ApprovalRequestWithdrawn { id };
        assert_eq!(
            lines(&followed.take_in(&from(&channel, agent, withdrawn.clone())).await),
            [format!("Approval {} withdrawn: the turn ended", id.0)]
        );
        assert!(followed.take_in(&from(&channel, agent, withdrawn)).await.is_empty(), "told once");
        assert!(!followed.is_pending(id), "no longer pending");

        // Over once the approver tells the verdict, or that it timed out; another user's answer
        // alone decides nothing.
        let approver = ActorId::Approver(ApproverId::new());
        let other = ActorId::User(UserId::new());
        let decided = |id| ApprovalVerdict::new(id, agent_id);
        let over: [fn(ApprovalVerdict) -> ActorEvent; 3] =
            [ActorEvent::ApprovalGranted, ActorEvent::ApprovalDenied, ActorEvent::ApprovalTimedOut];
        for verdict in over {
            let request = approval(&channel, agent_id);
            let id = request.id;
            followed
                .take_in(&from(
                    &channel,
                    agent,
                    ActorEvent::ApprovalRequested { request, to: None },
                ))
                .await;
            assert!(followed
                .take_in(&from(&channel, other, answered(other, id, None)))
                .await
                .is_empty());
            assert!(followed
                .take_in(&from(&channel, approver, verdict(decided(id))))
                .await
                .is_empty());
            assert!(!followed.is_pending(id), "over");
        }
        let request = approval(&channel, agent_id);
        let id = request.id;
        followed
            .take_in(&from(&channel, agent, ActorEvent::ApprovalRequested { request, to: None }))
            .await;
        followed.take_in(&from(&channel, other, answered(other, id, None))).await;
        assert!(followed.is_pending(id), "still pending after another's answer");
        assert!(followed.take_in(&from(&channel, me, answered(me, id, None))).await.is_empty());
        assert!(!followed.is_pending(id), "the user's own answer: theirs to give no longer");
        assert!(followed.waiting().is_empty(), "nor shown again as waiting for them");
    }

    #[tokio::test]
    async fn an_approval_answered_by_a_rule_of_the_user_is_told_to_them_alone() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let request = approval(&channel, agent_id);
        let id = request.id;
        let agent = ActorId::Agent(agent_id);
        followed
            .take_in(&from(&channel, agent, ActorEvent::ApprovalRequested { request, to: None }))
            .await;
        let answer = |by: ActorId, by_rule| from(&channel, by, answered(by, id, by_rule));
        let other = ActorId::User(UserId::new());
        assert!(followed.take_in(&answer(me, None)).await.is_empty(), "typed by the user");
        assert!(
            followed.take_in(&answer(other, Some(ApprovalRuleId::new()))).await.is_empty(),
            "not theirs"
        );
        assert_eq!(
            lines(&followed.take_in(&answer(me, Some(ApprovalRuleId::new()))).await),
            [format!("Approval {} answered by your rule", id.0)]
        );
    }

    #[tokio::test]
    async fn questions_asked_anew_are_shown_once_and_open_until_closed() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let request = make_question_request(channel.session_id(), channel.id(), agent_id, me);
        let id = request.id;
        let asked = from(&channel, agent, ActorEvent::QuestionRequested(request));
        assert!(!routed(&mut followed, asked.clone()).await.is_empty());
        assert!(routed(&mut followed, asked).await.is_empty(), "asked anew");
        assert!(followed.is_to_answer(id));
        assert_eq!(ids(&followed.questions_to_answer()), [id]);
        let closed = from(&channel, agent, ActorEvent::QuestionClosed { id, shown_to: None });
        routed(&mut followed, closed).await;
        assert!(!followed.is_to_answer(id));
        assert!(followed.questions_to_answer().is_empty());
    }

    /// What the terminal asks the user, and `/answer` and `/decline` name: questions put to them
    /// alone.
    #[tokio::test]
    async fn only_questions_put_to_the_user_are_theirs_to_answer() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let mine = make_question_request(channel.session_id(), channel.id(), agent_id, me);
        let elsewhere = make_question_request(
            channel.session_id(),
            channel.id(),
            agent_id,
            ActorId::User(UserId::new()),
        );
        let (id, other_id) = (mine.id, elsewhere.id);
        for request in [mine, elsewhere] {
            routed(&mut followed, from(&channel, agent, ActorEvent::QuestionRequested(request)))
                .await;
        }
        assert!(!followed.is_to_answer(other_id), "not the user's to answer");
        assert_eq!(ids(&followed.questions_to_answer()), [id], "not those put to another");
    }

    /// What waits for the user - an approval, questions put to them, a message that mentions
    /// them - is told as such, and shown again with what is still open.
    #[tokio::test]
    async fn what_waits_for_the_user_is_told_and_shown_again() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let other = ActorId::User(UserId::new());
        let request = approval(&channel, agent_id);
        let mine = make_question_request(channel.session_id(), channel.id(), agent_id, me);
        let theirs = make_question_request(channel.session_id(), channel.id(), agent_id, other);
        let ids = [request.id.0, mine.id.0, theirs.id.0].map(|id| id.to_string());
        let mentioning = Message { addressees: vec![me], ..message(other, "@owner look") };
        let asked = [
            ActorEvent::ApprovalRequested { request, to: None },
            ActorEvent::QuestionRequested(mine),
            ActorEvent::CompleteMessageGenerated(mentioning),
        ];
        let not_asked = [
            ActorEvent::QuestionRequested(theirs),
            ActorEvent::CompleteMessageGenerated(message(other, "hello")),
        ];
        for event in asked {
            let envelope = from(&channel, agent, event);
            assert!(followed.concerns_user(&envelope), "{:?}", envelope.event);
            routed(&mut followed, envelope).await;
        }
        for event in not_asked {
            let envelope = from(&channel, agent, event);
            assert!(!followed.concerns_user(&envelope), "{:?}", envelope.event);
            routed(&mut followed, envelope).await;
        }
        let waiting = lines(&followed.waiting()).join("\n");
        assert!(waiting.contains(&ids[0]) && waiting.contains(&ids[1]), "{waiting}");
        assert!(!waiting.contains(&ids[2]), "put to another: {waiting}");
    }

    /// What ends an approval concerns the user - withdrawn, decided by the approver (issue 594),
    /// or answered by them -, not another's answer; of it, what is told is a withdrawal of one
    /// shown, and an answer of theirs given by a rule (issue 566).
    #[tokio::test]
    async fn the_end_of_an_approval_concerns_the_user() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let other = ActorId::User(UserId::new());
        let request = approval(&channel, agent_id);
        let id = request.id;
        let withdrawn = from(&channel, agent, ActorEvent::ApprovalRequestWithdrawn { id });
        let by_rule = |by| from(&channel, by, answered(by, id, Some(ApprovalRuleId::new())));
        let typed = from(&channel, me, answered(me, id, None));
        let approver = ActorId::Approver(ApproverId::new());
        let granted = ActorEvent::ApprovalGranted(ApprovalVerdict::new(id, agent_id));
        let granted = from(&channel, approver, granted);
        assert!(followed.concerns_user(&withdrawn));
        assert!(followed.concerns_user(&granted), "decided");
        assert!(followed.concerns_user(&by_rule(me)));
        assert!(followed.concerns_user(&typed));
        assert!(!followed.concerns_user(&by_rule(other)), "another's answer");

        assert!(followed.take_in(&withdrawn).await.is_empty(), "not shown: nothing told");
        followed
            .take_in(&from(&channel, agent, ActorEvent::ApprovalRequested { request, to: None }))
            .await;
        assert!(followed.take_in(&typed).await.is_empty(), "typed by the user: nothing told");
        assert_eq!(followed.take_in(&by_rule(me)).await.len(), 1, "given by their rule: told");
        assert!(followed.take_in(&granted).await.is_empty(), "decided: nothing told");
    }

    /// What waits for the user as a channel is followed - the approvals the agents' turns wait
    /// for, the open questions put to them - is known, shown again, and shown once where the
    /// channel shows it after the watch began (issue 565).
    #[tokio::test]
    async fn what_waits_as_a_channel_is_followed_is_known_and_shown_once() {
        let kernel = test_kernel();
        let (session_loop, ctx) = running(&kernel).await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let agent_id = an_agent_in(&kernel, &session_loop, &ctx, channel.id()).await;
        let me = ctx.caller_id;
        let agent = ActorId::Agent(agent_id);
        let id = approval_waiting_in(&kernel, &channel, agent_id).await;
        let questions = make_question_request(channel.session_id(), channel.id(), agent_id, me);
        let question_id = questions.id;
        let asked = from(&channel, agent, ActorEvent::QuestionRequested(questions));
        let ActorId::User(user) = me else { unreachable!("a user") };
        let actor = channel.user_actor(user).expect("the owner's actor runs");
        actor.take_in_as_routed(asked.clone()).await;

        let views = kernel.views(Viewer::new(ctx));
        let mut followed =
            Followed::new(Arc::clone(&channel), views).await.expect("the owner takes part");
        assert!(followed.is_pending(id), "the approval the agent's turn waits for");
        assert!(followed.knows_questions(question_id), "the questions put to the user");
        let waiting = lines(&followed.waiting()).join("\n");
        let shown_again = [id.0, question_id.0].map(|id| waiting.contains(&id.to_string()));
        assert_eq!(shown_again, [true, true], "{waiting}");

        let requested = ApprovalRequest { id, ..approval(&channel, agent_id) };
        let requested =
            from(&channel, agent, ActorEvent::ApprovalRequested { request: requested, to: None });
        assert!(followed.take_in(&requested).await.is_empty(), "the approval, known");
        assert!(followed.take_in(&asked).await.is_empty(), "the questions, known");

        // Asked after the channel was followed, not shown yet: shown as the channel shows them,
        // and not shown again as waiting before.
        let later = make_question_request(channel.session_id(), channel.id(), agent_id, me);
        let later_id = later.id.0.to_string();
        actor.take_in_as_routed(from(&channel, agent, ActorEvent::QuestionRequested(later))).await;
        assert!(!lines(&followed.waiting()).join("\n").contains(&later_id), "not known yet");
        let shown = next_taken_in(&mut followed).await;
        assert!(lines(&shown).join("\n").contains(&later_id), "{shown:?}");
        assert!(lines(&followed.waiting()).join("\n").contains(&later_id), "known once shown");
    }

    /// An approval the user answered is pending for them no more as the answer is sent - also
    /// where the turns read anew, which do not tell who answered, still hold it, or the agent asks
    /// for it again -, until the approver ends it (issue 580).
    #[tokio::test]
    async fn an_approval_answered_is_pending_for_the_user_no_more_until_it_ends() {
        let kernel = test_kernel();
        let (session_loop, ctx, _) = running_session_of_a_user(&kernel).await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let agent_id = an_agent_in(&kernel, &session_loop, &ctx, channel.id()).await;
        let agent = ActorId::Agent(agent_id);
        let id = approval_waiting_in(&kernel, &channel, agent_id).await;
        let views = kernel.views(Viewer::new(ctx));
        let mut followed =
            Followed::new(Arc::clone(&channel), views).await.expect("the owner takes part");
        let actor = followed.actor().expect("the owner's actor runs");

        followed.approve(&actor, id, true).await.expect("the answer is sent");
        assert!(!followed.is_pending(id), "answered");
        assert!(followed.catch_up().await.is_empty(), "not told again as the turns are read anew");
        assert!(!followed.is_pending(id), "though the turn still holds it");
        let requested = ApprovalRequest { id, ..approval(&channel, agent_id) };
        let requested =
            from(&channel, agent, ActorEvent::ApprovalRequested { request: requested, to: None });
        assert!(
            followed.take_in(&requested).await.is_empty(),
            "nor as the agent asks for it again"
        );
        assert!(!followed.is_pending(id) && followed.waiting().is_empty(), "nor waiting");

        let approver = ActorId::Approver(ApproverId::new());
        let granted = ActorEvent::ApprovalGranted(ApprovalVerdict::new(id, agent_id));
        assert!(followed.take_in(&from(&channel, approver, granted)).await.is_empty());
        followed.catch_up().await;
        assert!(followed.is_pending(id), "once it ended, known as the turns hold it");
    }

    /// An approval answered stays answered where the approver refuses the answer, and where a
    /// rule of the user's answered it, as the channel shows it (issue 580).
    #[tokio::test]
    async fn an_approval_answered_stays_so_where_the_answer_is_refused_or_given_by_a_rule() {
        let kernel = test_kernel();
        let (session_loop, ctx, _) = running_session_of_a_user(&kernel).await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let agent_id = an_agent_in(&kernel, &session_loop, &ctx, channel.id()).await;
        let me = ctx.caller_id;
        let id = approval_waiting_in(&kernel, &channel, agent_id).await;
        let views = kernel.views(Viewer::new(ctx));
        let mut followed =
            Followed::new(Arc::clone(&channel), views).await.expect("the owner takes part");
        let actor = followed.actor().expect("the owner's actor runs");
        let approver = ActorId::Approver(ApproverId::new());

        followed.approve(&actor, id, true).await.expect("the answer is sent");
        let refused = ActorEvent::EventRefused {
            to: me,
            refused: "ApprovalAnswered".to_string(),
            message: None,
            reason: Rejection::Invalid,
        };
        followed.take_in(&from(&channel, approver, refused)).await;
        followed.catch_up().await;
        assert!(!followed.is_pending(id) && followed.waiting().is_empty(), "refused, still so");

        let granted = ActorEvent::ApprovalGranted(ApprovalVerdict::new(id, agent_id));
        followed.take_in(&from(&channel, approver, granted)).await;
        followed.catch_up().await;
        assert!(followed.is_pending(id), "ended, known anew as the turns hold it");
        followed.take_in(&from(&channel, me, answered(me, id, Some(ApprovalRuleId::new())))).await;
        followed.catch_up().await;
        assert!(!followed.is_pending(id), "answered by a rule of the user's");
    }

    /// Catching up tells the questions put to the user that it did not know, once, and the
    /// channel showing them after shows them no more (issue 565).
    #[tokio::test]
    async fn catching_up_tells_the_questions_not_known_once() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        assert!(followed.catch_up().await.is_empty(), "nothing waits");
        let questions = make_question_request(channel.session_id(), channel.id(), agent_id, me);
        let id = questions.id.0.to_string();
        let asked = from(&channel, agent, ActorEvent::QuestionRequested(questions));
        let actor = followed.actor().expect("the owner's actor runs");
        actor.take_in_as_routed(asked).await;

        assert!(lines(&followed.catch_up().await).join("\n").contains(&id), "not known before");
        assert!(followed.catch_up().await.is_empty(), "known since");
        let shown = next_taken_in(&mut followed).await;
        assert!(shown.is_empty(), "{shown:?}");
    }

    /// The tools a message calls and how calls ended are printed after what it says - but a call
    /// presented `quiet`, and its result where it succeeded (issue 569).
    #[tokio::test]
    async fn tool_calls_and_their_results_are_printed_but_quiet_ones() {
        let (mut followed, channel, _) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        let shown = make_tool_call("terminal");
        let quiet = |id: &str| ToolCall {
            id: Some(id.to_string()),
            presentation: ToolPresentation::QUIET,
            ..make_tool_call("attach")
        };
        let (attached, failing) = (quiet("call_2"), quiet("call_3"));
        let calls = vec![shown.clone(), attached.clone(), failing.clone()];
        let calling = Message { tool_calls: calls, ..message(agent, "let me look") };
        let said = ActorEvent::CompleteMessageGenerated(calling);
        assert_eq!(
            lines(&followed.take_in(&from(&channel, agent, said)).await),
            ["other: let me look", "other calls terminal, its arguments not shown"]
        );

        let result = |call: &ToolCall, status| {
            ToolCallResult::of(call, status, "done".to_string(), Default::default())
        };
        let results = vec![
            result(&shown, ToolResultStatus::Ok),
            result(&attached, ToolResultStatus::Ok),
            result(&failing, ToolResultStatus::Failed),
        ];
        let ended = Message { content: None, tool_results: results, ..message(agent, "") };
        let step = ActorEvent::IntermediateMessageGenerated(ended);
        assert_eq!(
            lines(&followed.take_in(&from(&channel, agent, step)).await),
            [
                "terminal answered: done",
                "other calls attach, its arguments not shown",
                "attach failed: done"
            ]
        );
    }

    /// An answer under way keeps where its agent stands and what it streamed of the message it
    /// writes, until the message is complete (issue 569).
    #[tokio::test]
    async fn an_answer_under_way_keeps_its_state_and_what_it_streamed() {
        let (mut followed, channel, _) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        let turn_id = TurnId::new();
        let shown = |event| from(&channel, agent, event);
        followed.take_in(&shown(ActorEvent::ResponseStarted { turn_id })).await;
        followed.take_in(&shown(ActorEvent::StateChanged(ActorState::ExecutingTools))).await;
        for chunk in ["Hel", "lo"] {
            let partial = Message { reasoning: Some("hm".to_string()), ..message(agent, chunk) };
            let partial = shown(ActorEvent::PartialMessageGenerated(partial));
            assert!(followed.take_in(&partial).await.is_empty());
        }
        let answer = &followed.under_way()[0];
        assert_eq!((answer.turn, answer.state), (turn_id, Some(ActorState::ExecutingTools)));
        assert_eq!((answer.content.as_str(), answer.reasoning.as_str()), ("Hello", "hmhm"));

        followed
            .take_in(&shown(ActorEvent::CompleteMessageGenerated(message(agent, "Hello"))))
            .await;
        let answer = &followed.under_way()[0];
        assert!(answer.content.is_empty() && answer.reasoning.is_empty(), "complete");
    }

    /// Issue 572: the answers under way of every thread are kept, each with its thread and what it
    /// streamed of the thread followed alone; one begun takes the place of the agent's answer in
    /// its thread, and an answer ends with its turn, not with another of its agent; where an agent
    /// stands is that of its answer in the thread it says it in, or, said in the line, of all.
    #[tokio::test]
    async fn the_answers_under_way_of_every_thread_are_kept_each_ending_with_its_turn() {
        let (mut followed, channel, _) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        let line = ThreadId::line(channel.id());
        let aside = ThreadId::new();
        let in_thread = |thread_id, event| {
            let envelope = from(&channel, agent, event);
            Envelope { context: EventContext { thread_id, ..envelope.context }, ..envelope }
        };
        let here = TurnId::new();
        let there = TurnId::new();
        followed.take_in(&in_thread(line, ActorEvent::ResponseStarted { turn_id: here })).await;
        let started = in_thread(aside, ActorEvent::ResponseStarted { turn_id: there });
        assert!(followed.take_in(&started).await.is_empty(), "nothing printed of another thread");
        let kept = |followed: &Followed| {
            let answers = followed.under_way().iter();
            answers.map(|a| (a.thread, a.turn, a.content.clone())).collect::<Vec<_>>()
        };
        for (thread, text) in [(line, "here"), (aside, "there")] {
            let partial = ActorEvent::PartialMessageGenerated(message(agent, text));
            followed.take_in(&in_thread(thread, partial)).await;
        }
        assert_eq!(
            kept(&followed),
            [(line, here, "here".to_string()), (aside, there, String::new())],
            "what streams of the thread followed alone"
        );

        followed.follow(aside, "a thread").await;
        assert_eq!(kept(&followed)[0].2, "", "what streamed of the thread left forgotten");
        let state = |followed: &Followed| {
            followed.under_way().iter().map(|answer| answer.state).collect::<Vec<_>>()
        };
        let running = ActorEvent::StateChanged(ActorState::ExecutingTools);
        followed.take_in(&in_thread(aside, running)).await;
        assert_eq!(state(&followed), [None, Some(ActorState::ExecutingTools)], "its turn's");
        let waiting = ActorEvent::StateChanged(ActorState::WaitingForUser);
        followed.take_in(&in_thread(line, waiting)).await;
        assert_eq!(
            state(&followed),
            [Some(ActorState::WaitingForUser); 2],
            "said in the line, which the view of the thread followed does not show: the agent's own"
        );

        let anew = TurnId::new();
        followed.take_in(&in_thread(aside, ActorEvent::ResponseStarted { turn_id: anew })).await;
        assert_eq!(
            kept(&followed),
            [(line, here, String::new()), (aside, anew, String::new())],
            "begun anew, in place of the one before in its thread"
        );
        let reason = GivenUpReason::Failed { message: "unreachable".to_string() };
        let given_up = ActorEvent::TurnGivenUp { turn_id: here, reason };
        followed.take_in(&in_thread(line, given_up)).await;
        assert_eq!(kept(&followed), [(aside, anew, String::new())], "the other turn runs on");
    }

    /// Catching up reads the answers under way anew from the running channel: one whose end was
    /// missed is under way no more (issue 569).
    #[tokio::test]
    async fn catching_up_reads_the_answers_under_way_anew() {
        let (mut followed, channel, _) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        let started = ActorEvent::ResponseStarted { turn_id: TurnId::new() };
        followed.take_in(&from(&channel, agent, started)).await;
        assert_eq!(followed.under_way().len(), 1);
        followed.catch_up().await;
        assert!(followed.under_way().is_empty(), "no turn of it runs in the channel");
    }

    /// Catching up keeps an answer known with what it streamed, and learns of one whose start was
    /// missed - also as a channel is followed, with the agent's name (issue 569).
    #[tokio::test]
    async fn catching_up_keeps_the_answers_known_and_learns_of_the_others() {
        let kernel =
            TestKernel { libraries: vec![&ScriptedAgents(Script::Hangs)], ..Default::default() }
                .build();
        let (session_loop, ctx) = running(&kernel).await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let agent = ActorId::Agent(an_agent_in(&kernel, &session_loop, &ctx, channel.id()).await);
        let views = kernel.views(Viewer::new(ctx));
        let mut followed =
            Followed::new(Arc::clone(&channel), views.clone()).await.expect("the owner takes part");
        let actor = followed.actor().expect("the owner's actor runs");
        actor.send_message(None, Some("hello".to_string()), None, Vec::new()).await.expect("said");
        let begun = async {
            loop {
                if let Ok(Shown::Envelope(envelope)) = followed.next().await {
                    followed.take_in(&envelope).await;
                    if !followed.under_way().is_empty() {
                        break;
                    }
                }
            }
        };
        time::timeout(Duration::from_secs(5), begun).await.expect("the agent begins to answer");
        let partial = ActorEvent::PartialMessageGenerated(message(agent, "Hel"));
        followed.take_in(&from(&channel, agent, partial)).await;

        followed.catch_up().await;
        let kept =
            followed.under_way().iter().map(|a| (a.agent, a.content.clone())).collect::<Vec<_>>();
        assert_eq!(kept, [(agent, "Hel".to_string())], "known, kept with what it streamed");

        let anew = Followed::new(Arc::clone(&channel), views).await.expect("the owner takes part");
        let learnt = anew.under_way().iter().map(|a| (a.agent, a.name.clone())).collect::<Vec<_>>();
        assert_eq!(learnt, [(agent, "helper".to_string())], "learnt of as it was followed");
    }

    /// The quiet calls of an agent are forgotten as its turn ends: their results will not come.
    #[tokio::test]
    async fn quiet_calls_are_forgotten_as_the_turn_ends() {
        let (mut followed, channel, _) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        let quiet = ToolCall { presentation: ToolPresentation::QUIET, ..make_tool_call("attach") };
        let calling = Message { tool_calls: vec![quiet.clone()], ..message(agent, "") };
        followed
            .take_in(&from(&channel, agent, ActorEvent::CompleteMessageGenerated(calling)))
            .await;
        let finished = ActorEvent::TurnFinished {
            turn_id: TurnId::new(),
            messages: Vec::new(),
            deferred: false,
        };
        followed.take_in(&from(&channel, agent, finished)).await;

        let result = ToolCallResult::of(
            &quiet,
            ToolResultStatus::Failed,
            "no".to_string(),
            Default::default(),
        );
        let ended = Message { tool_results: vec![result], ..message(agent, "") };
        let printed = lines(
            &followed
                .take_in(&from(&channel, agent, ActorEvent::CompleteMessageGenerated(ended)))
                .await,
        );
        assert_eq!(printed, ["attach failed: no"], "its call forgotten");
    }

    /// Issue 589: a result presented `quiet` that succeeded is left out by its own presentation,
    /// though its call was not seen - it lay before the history read, or its turn ended -; one
    /// that did not succeed is printed alone.
    #[tokio::test]
    async fn a_quiet_result_is_left_out_without_its_call() {
        let (mut followed, channel, _) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        let quiet = ToolCall { presentation: ToolPresentation::QUIET, ..make_tool_call("attach") };
        let ended = |status, content: &str| {
            let result =
                ToolCallResult::of(&quiet, status, content.to_string(), Default::default());
            let ended = Message { tool_results: vec![result], ..message(agent, "") };
            from(&channel, agent, ActorEvent::CompleteMessageGenerated(ended))
        };
        let succeeded = followed.take_in(&ended(ToolResultStatus::Ok, "attached")).await;
        assert!(lines(&succeeded).is_empty(), "left out: {:?}", lines(&succeeded));
        let failed = followed.take_in(&ended(ToolResultStatus::Failed, "no")).await;
        assert_eq!(lines(&failed), ["attach failed: no"]);
    }

    /// Issue 589: a quiet call whose result is presented otherwise is printed with it, by the
    /// result's presentation.
    #[tokio::test]
    async fn a_quiet_call_is_printed_with_a_result_presented_otherwise() {
        let (mut followed, channel, _) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        let quiet = ToolCall { presentation: ToolPresentation::QUIET, ..make_tool_call("attach") };
        let calling = Message { tool_calls: vec![quiet.clone()], ..message(agent, "") };
        let called = followed
            .take_in(&from(&channel, agent, ActorEvent::CompleteMessageGenerated(calling)))
            .await;
        assert!(lines(&called).is_empty(), "the call kept back");
        let result = ToolCallResult {
            presentation: ToolPresentation::DEFAULT,
            ..ToolCallResult::of(
                &quiet,
                ToolResultStatus::Ok,
                "attached".to_string(),
                Default::default(),
            )
        };
        let ended = Message { tool_results: vec![result], ..message(agent, "") };
        let printed = lines(
            &followed
                .take_in(&from(&channel, agent, ActorEvent::CompleteMessageGenerated(ended)))
                .await,
        );
        assert_eq!(printed.len(), 2, "the call and its result: {printed:?}");
        assert_eq!(printed[1], "attach answered: attached");
    }

    /// That an agent began to answer is printed; another user's response is no answer of an
    /// agent (issue 569).
    #[tokio::test]
    async fn an_answer_begun_is_printed() {
        let (mut followed, channel, _) = followed().await;
        let helper = ActorId::Agent(AgentId::new());
        let another = ActorId::User(UserId::new());
        let started = ActorEvent::ResponseStarted { turn_id: TurnId::new() };
        let begun = lines(&followed.take_in(&from(&channel, helper, started.clone())).await);
        assert_eq!(begun, [format!("{helper} is answering.")]);
        assert!(followed.take_in(&from(&channel, another, started)).await.is_empty(), "no agent");
    }

    #[tokio::test]
    async fn retry_asks_for_the_turn_under_way_or_given_up_before_the_last_answer() {
        let (mut followed, channel, _) = followed().await;
        let actor = followed.actor().expect("the owner's actor runs");
        let agent = ActorId::Agent(AgentId::new());
        assert!(!followed.retry(&actor).await.unwrap(), "nothing to give again");
        let refused = |e: Error| e.downcast::<ChannelError>().expect("refused by the actor");

        let said = ActorEvent::CompleteMessageGenerated(message(agent, "an answer"));
        followed.take_in(&from(&channel, agent, said)).await;
        let error = refused(followed.retry(&actor).await.unwrap_err());
        assert!(matches!(error, ChannelError::MessageNotFound { .. }), "the last answer: {error}");
        // A turn under way in another thread is not the thread followed's to ask for (issue 572).
        let aside = from(&channel, agent, ActorEvent::ResponseStarted { turn_id: TurnId::new() });
        let thread_id = ThreadId::new();
        followed
            .take_in(&Envelope { context: EventContext { thread_id, ..aside.context }, ..aside })
            .await;
        let error = refused(followed.retry(&actor).await.unwrap_err());
        assert!(matches!(error, ChannelError::MessageNotFound { .. }), "passed over: {error}");

        // A turn is asked for again as it is - whether it can be the channel's to decide as it
        // admits the request (issue 567) -, a message is looked at first.
        let turn_id = TurnId::new();
        followed.take_in(&from(&channel, agent, ActorEvent::ResponseStarted { turn_id })).await;
        assert!(followed.retry(&actor).await.unwrap(), "the turn, asked for");

        followed
            .take_in(&from(
                &channel,
                agent,
                ActorEvent::TurnFinished { turn_id, messages: Vec::new(), deferred: false },
            ))
            .await;
        let error = refused(followed.retry(&actor).await.unwrap_err());
        assert!(matches!(error, ChannelError::MessageNotFound { .. }), "the last again: {error}");

        let given_up = ActorEvent::TurnGivenUp {
            turn_id: TurnId::new(),
            reason: GivenUpReason::Failed { message: "unreachable".to_string() },
        };
        followed.take_in(&from(&channel, agent, given_up)).await;
        assert!(followed.retry(&actor).await.unwrap(), "the turn given up, asked for");

        let said = ActorEvent::CompleteMessageGenerated(message(agent, "another answer"));
        followed.take_in(&from(&channel, agent, said)).await;
        let error = refused(followed.retry(&actor).await.unwrap_err());
        assert!(matches!(error, ChannelError::MessageNotFound { .. }), "said since: {error}");
    }

    /// What was said in a channel so far is printed as the user begins to talk in it, and a
    /// message printed so is not printed again as the channel shows it (issue 569).
    #[tokio::test]
    async fn what_was_said_so_far_is_printed_once_as_the_user_begins_to_talk() {
        let kernel = test_kernel();
        let (session_loop, owner, other) = running_session_of_a_user(&kernel).await;
        let side = spec("side", vec![owner.caller_id, other]);
        let channel = session_loop.create_channel(&owner, side).await.expect("the owner makes it");
        let views = kernel.views(Viewer::new(owner));
        let mut followed =
            Followed::new(Arc::clone(&channel), views).await.expect("the owner takes part");
        let talking_in = lines(&followed.talking_in().await);
        assert!(!talking_in.join("\n").contains("Earlier"), "nothing said yet: {talking_in:?}");

        let ActorId::User(user) = other else { unreachable!("a user") };
        let theirs = channel.user_actor(user).expect("they take part");
        theirs.send_message(None, Some("hello".to_string()), None, Vec::new()).await.expect("said");
        let recorded = async {
            loop {
                let talking_in = lines(&followed.talking_in().await);
                if talking_in.iter().any(|line| line == "other: hello") {
                    break talking_in;
                }
                time::sleep(Duration::from_millis(20)).await;
            }
        };
        let talking_in = time::timeout(Duration::from_secs(5), recorded).await.expect("recorded");
        assert_eq!(talking_in[..3], ["Earlier in side", "other: hello", "Talking in side now"]);

        let shown = async {
            loop {
                if let Ok(Shown::Envelope(envelope)) = followed.next().await {
                    if matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_)) {
                        break followed.take_in(&envelope).await;
                    }
                }
            }
        };
        let printed = time::timeout(Duration::from_secs(5), shown).await.expect("shown");
        assert!(printed.is_empty(), "printed from the history already: {:?}", lines(&printed));
    }

    /// Of a page of the history, what was said is printed - the user's own words, a step of a
    /// plan, tool calls and how they ended -, a quiet call left out and its result paired with it
    /// as it comes live; the last answer of an agent in the line is what `/retry` asks for (issue
    /// 569), not one of another thread that addresses the user after it (issue 595).
    #[tokio::test]
    async fn what_a_page_of_the_history_says_is_printed() {
        let (mut followed, channel, me) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        let shown_call = make_tool_call("terminal");
        let quiet = ToolCall {
            id: Some("call_2".to_string()),
            presentation: ToolPresentation::QUIET,
            ..make_tool_call("attach")
        };
        let mine = Message { sender_name: "owner".to_string(), ..message(me, "look") };
        let calling =
            Message { tool_calls: vec![shown_call.clone(), quiet.clone()], ..message(agent, "") };
        let result = |call: &ToolCall, status| {
            ToolCallResult::of(call, status, "done".to_string(), Default::default())
        };
        let ended = Message {
            tool_results: vec![result(&shown_call, ToolResultStatus::Ok)],
            ..message(agent, "")
        };
        let step = Message { internal: true, ..message(agent, "thinking it over") };
        let answer = message(agent, "here");
        let elsewhere = message(agent, "@owner there");
        let line = Some(ThreadId::line(channel.id()));
        let said = [mine, calling, ended, step, answer.clone()].map(|message| (line, message));
        let of_thread = (Some(ThreadId::new()), elsewhere);
        let items = said.into_iter().chain([of_thread]).map(|(thread, message)| HistoryPageItem {
            seq: Position::START,
            thread,
            entry: HistoryEntry::Said { message },
        });
        let page = HistoryPage { items: items.collect(), earlier: None, digest: None };
        assert_eq!(
            lines(&followed.history_printed(&page)),
            [
                "Earlier in main",
                "owner: look",
                "other calls terminal, its arguments not shown",
                "terminal answered: done",
                "other, working: thinking it over",
                "other: here",
                "other: @owner there",
            ]
        );

        let actor = followed.actor().expect("the owner's actor runs");
        let error = followed.retry(&actor).await.expect_err("no such message in the channel");
        let error = error.downcast::<ChannelError>().expect("refused by the actor");
        assert!(
            matches!(error, ChannelError::MessageNotFound { message, .. } if message == answer.id),
            "the last answer of the line: {error}"
        );

        let late = Message {
            tool_results: vec![result(&quiet, ToolResultStatus::Ok)],
            ..message(agent, "")
        };
        let shown = followed
            .take_in(&from(&channel, agent, ActorEvent::CompleteMessageGenerated(late)))
            .await;
        assert!(shown.is_empty(), "its call printed from the history, quiet: {:?}", lines(&shown));
    }

    /// Issue 557: what came back from a thread is printed from the history as given back, and not
    /// again as it comes live.
    #[tokio::test]
    async fn what_came_back_from_a_thread_is_printed_from_the_history_once() {
        let (mut followed, channel, _) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        let thread = ThreadId::new();
        let (recipient, by) = (agent, ActorId::Delegate(thread));
        let found = message(ActorId::Agent(AgentId::new()), "found it");
        let given =
            HistoryEntry::GivenToRead { recipient, message: found.clone(), by, at: Utc::now() };
        let line = Some(ThreadId::line(channel.id()));
        let item = HistoryPageItem { seq: Position::START, thread: line, entry: given };
        let page = HistoryPage { items: vec![item], earlier: None, digest: None };
        assert_eq!(
            lines(&followed.history_printed(&page)),
            ["Earlier in main", "Given back to read from a thread:", "other: found it"]
        );
        let live = ActorEvent::GivenToRead { recipient, message: found };
        assert!(followed.take_in(&from(&channel, by, live)).await.is_empty(), "printed already");
    }

    #[tokio::test]
    async fn a_channel_is_followed_only_by_who_takes_part() {
        let kernel = test_kernel();
        let (session_loop, ctx) = running(&kernel).await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let followed_by = |ctx: SecurityContext| {
            Followed::new(Arc::clone(&channel), kernel.views(Viewer::new(ctx)))
        };
        let stranger = SecurityContext::new(
            ActorId::User(UserId::new()),
            session_loop.id(),
            Default::default(),
        );
        assert!(followed_by(stranger).await.is_none(), "a stranger");
        assert!(followed_by(SecurityContext::system()).await.is_none(), "no user");
        let followed = followed_by(ctx).await.expect("the owner takes part");
        assert_eq!(followed.channel_id(), channel.id());
    }

    /// Following a channel ends where the actor of a user who no longer takes part in it stops:
    /// they left, another removed them, or the channel was deleted. Asked of a user who does not
    /// own the channel, under the ownership policy.
    #[tokio::test]
    async fn following_ends_where_the_user_no_longer_takes_part() {
        let kernel = owned_kernel(ConfidentialityRules::default());
        let (session_loop, owner, other) = running_session_of_a_user(&kernel).await;
        let theirs = SecurityContext::new(other, session_loop.id(), Default::default());
        let views = kernel.views(Viewer::new(theirs.clone()));
        let side = |name| spec(name, vec![owner.caller_id, other]);
        let is_end = |shown: &Shown| matches!(shown, Shown::Ended);

        let channel = session_loop.create_channel(&owner, side("left")).await.unwrap();
        let mut followed =
            Followed::new(Arc::clone(&channel), views.clone()).await.expect("they take part");
        assert!(!followed.ended().await, "taking part");
        channel.leave(&theirs, other).await.expect("they may leave");
        assert!(shows(&mut followed, is_end).await, "their actor stopped");
        assert!(followed.ended().await, "left");

        let channel = session_loop.create_channel(&owner, side("removed")).await.unwrap();
        let mut followed =
            Followed::new(Arc::clone(&channel), views.clone()).await.expect("they take part");
        channel.remove_participant(&owner, other).await.expect("the owner may remove them");
        assert!(shows(&mut followed, is_end).await, "their actor stopped");
        assert!(followed.ended().await, "removed");

        let channel = session_loop.create_channel(&owner, side("deleted")).await.unwrap();
        let mut followed =
            Followed::new(Arc::clone(&channel), views.clone()).await.expect("they take part");
        assert!(session_loop.delete_channel(&owner, channel.id()).await.unwrap());
        assert!(shows(&mut followed, is_end).await, "their actor stopped");
        assert!(followed.ended().await, "deleted");
    }

    /// Following a channel outlives a run of the user's actor: who left and was added again is
    /// shown the new run, and acts through their new actor, not the one that ended - which knows
    /// none of the questions put to the one before.
    #[tokio::test]
    async fn following_a_channel_outlives_a_run_of_the_users_actor() {
        let kernel = owned_kernel(ConfidentialityRules::default());
        let (session_loop, owner, other) = running_session_of_a_user(&kernel).await;
        let theirs = SecurityContext::new(other, session_loop.id(), Default::default());
        let views = kernel.views(Viewer::new(theirs.clone()));
        let spec = spec("again", vec![owner.caller_id, other]);
        let channel = session_loop.create_channel(&owner, spec).await.unwrap();
        let mut followed =
            Followed::new(Arc::clone(&channel), views).await.expect("they take part");
        let first = followed.actor().expect("their actor runs");
        let request =
            make_question_request(channel.session_id(), channel.id(), AgentId::new(), other);
        let id = request.id;
        let asked = from(
            &channel,
            ActorId::Agent(request.agent_id),
            ActorEvent::QuestionRequested(request),
        );
        routed(&mut followed, asked).await;
        assert_eq!(ids(&followed.questions_to_answer()), [id], "theirs to answer");

        channel.leave(&theirs, other).await.expect("they may leave");
        assert!(shows(&mut followed, |shown| matches!(shown, Shown::Ended)).await, "it stopped");
        assert!(followed.actor().is_none(), "none runs between the two runs");

        let into = Some((channel.id(), HistoryAccess::All));
        session_loop.add_participant(&owner, other, into).await.expect("added again");
        assert!(shows(&mut followed, |shown| matches!(shown, Shown::Started)).await, "a new run");
        assert!(!followed.ended().await, "taking part again");
        let again = followed.actor().expect("their new actor runs");
        assert!(!Arc::ptr_eq(&first, &again), "not the actor that ended");
        assert!(
            followed.questions_to_answer().is_empty() && !followed.is_to_answer(id),
            "none the new actor would refuse"
        );
    }
}
