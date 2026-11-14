//! A channel as the terminal follows it for the user: what it prints of what the channel shows,
//! and what the user's commands there need of it (issue 561).

use crate::printed::{ErrorKind, Printed, Said, Saying, Words};
use crate::under_way::UnderWay;
use anyhow::Result;
use caelumora_domain::actor::{ActorEvent, ActorId, Envelope};
use caelumora_domain::approval::{ApprovalId, ApprovalRequest};
use caelumora_domain::channel::ChannelId;
use caelumora_domain::mention::{is_group_handle, mention_spans};
use caelumora_domain::message::{Message, MessageId};
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
use caelumora_kernel::view::Views;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;

/// A channel the user takes part in, as the terminal follows it - as the channel stream of the
/// REST server does (`docs/CHANNELS.md`, issue 386): the channel, the views it is read through in
/// their name and what their actor in it shows, the approvals pending there, which questions it
/// knows, the answers under way, the quiet calls whose results are to come, and what `/retry` asks
/// to be given again.
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
    /// The handles of the participants, as the channel was followed: what a message's mentions
    /// are highlighted by.
    handles: HashMap<ActorId, String>,
    /// The approval requests pending - those the agents' turns waited for as they were last read
    /// (`catch_up`), in the order of the participants and their calls, then those shown, in the
    /// order they were requested -, until the approver ends them - decided, timed out or
    /// withdrawn -, the user answered them, or the turns read anew no longer hold them, as the web
    /// UI keeps them: so that `/approve` and
    /// `/deny` find the channel a request waits in, and `/switch` shows it again. Which agent
    /// asked, and whether an answer counts, the approver tells, which holds the request (issue
    /// 567). Covers both plan proposals and regular tool calls - both flow through the same
    /// generic `ApprovalRequested` event.
    pending_approvals: Vec<ApprovalRequest>,
    /// The questions known - those put to the user that were open as what waits was read
    /// (`catch_up`), and those shown (issue 165) -, until they close: so that questions are shown
    /// once, also those asked anew by an agent that resumed its turn, and `/answer` and `/decline`
    /// with an id find the channel they wait in. Which are open is their actor's to tell
    /// (`UserActor::questions_to_answer`).
    known_questions: HashSet<QuestionId>,
    /// The last answer of an agent shown, which `/retry` asks it to give again (issue 234) unless
    /// one of the two below is known.
    last_answer: Option<MessageId>,
    /// The answers under way, in the order they began - those the terminal learnt of as it caught
    /// up after those it knew -: what the screen shows as such, and of which `/retry` asks for the
    /// last first (issue 235).
    under_way: Vec<UnderWay>,
    /// The calls presented `quiet` whose results are yet to come, by id, with the agent that made
    /// them and what would be printed of them: a result of one is left out where it succeeded, as
    /// its call was, and printed with the call where it did not. Dropped as the agent's turn ends,
    /// whose results will not come any more; a result whose call was missed - the watch lagged -
    /// is printed as any other.
    quiet_calls: HashMap<String, (ActorId, Printed)>,
    /// The turn given up that may be answered anew, with nothing said since (issue 259).
    given_up: Option<(ActorId, TurnId)>,
}

impl Followed {
    /// Follows `channel` for the user `views` are read by, from now on, as the views let them
    /// (`ChannelView::user_follow`), knowing its participants' handles as they are now, and what
    /// waits for the user there, as `catch_up` reads it - which whoever follows the channel tells
    /// (`waiting`). `None` if the viewer is no user the channel lists, or cannot read it.
    pub(crate) async fn new(channel: Arc<ChannelLoop>, views: Views) -> Option<Self> {
        let ActorId::User(user) = views.context().caller_id else { return None };
        let watch = views
            .channel(channel.session_id(), channel.id())
            .await
            .ok()?
            .user_follow()
            .await
            .ok()??;
        let handles =
            channel.participants().into_iter().filter_map(|m| Some((m.id, m.handle?))).collect();
        let mut followed = Self {
            channel,
            user,
            views,
            watch,
            handles,
            pending_approvals: Vec::new(),
            known_questions: HashSet::new(),
            last_answer: None,
            under_way: Vec::new(),
            quiet_calls: HashMap::new(),
            given_up: None,
        };
        followed.catch_up().await;
        Some(followed)
    }

    /// Reads anew what waits for the user in the channel (issue 565), and answers what is printed
    /// of what it did not know: the approvals pending, as the agents' turns hold them
    /// (`ChannelView::pending_approvals`), which take the place of those known - one that ended
    /// meanwhile is known no more -, and the open questions put to the user, as their actor tells,
    /// which become known. Read as the channel is followed, and where what it showed may have been
    /// lost - its watch lagged, or a run of the user's actor began -, always after the watch
    /// began, as the channel stream of the REST server reads its snapshot: nothing is lost in
    /// between, and what the channel shows of it after is shown once (`take_in`). Approvals that
    /// cannot be read stay as they were known.
    ///
    /// The turns are read as stored, which may lag behind what the channel showed. Rare cases,
    /// each in the short time between an event and the agent storing its turn, or after a lag:
    /// an approval decided may still be held by a turn - it is known as pending until it is read
    /// anew, and an answer to it the approver refuses, which the user is told (`EventRefused`);
    /// an approval requested may not be held by a turn yet, or not stored at all where storing
    /// failed - it is known no more, though the approver holds it; an approval withdrawn while
    /// the watch lagged ends without the withdrawal being told; and one the user answered that is
    /// yet to be decided is shown again, since it is still pending. A new run of the user's actor
    /// knows none of the questions put to the one before (issue 561), so where one began, only
    /// approvals are made up for.
    ///
    /// The answers under way are read anew as well, from the running channel
    /// (`read_under_way`): one whose end was missed is under way no more, one whose start was
    /// missed is, and those known keep what they streamed. Nothing is printed of them; the screen
    /// shows them (`Screen::show_under_way`).
    pub(crate) async fn catch_up(&mut self) -> Vec<Printed> {
        let mut printed = Vec::new();
        if let Some(pending) = self.read_pending_approvals().await {
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

    /// What tells the user they talk in the channel now: its name, who answers there, and what
    /// waits for them there.
    pub(crate) async fn talking_in(&self) -> Vec<Printed> {
        let mut printed = vec![Printed::Notice(format!("Talking in {} now", self.name()))];
        printed.extend(self.who_answers().await);
        printed.extend(self.waiting());
        printed
    }

    /// What tells who answers in the channel, as the views tell it (issue 174); `None` if the
    /// channel cannot be read.
    async fn who_answers(&self) -> Option<Printed> {
        let view = self.views.channel(self.channel.session_id(), self.channel_id()).await.ok()?;
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

    /// Whether `envelope` concerns the user wherever they talk: it asks something of them - an
    /// approval, questions put to them, a message that mentions them -, or ends an approval for
    /// them - withdrawn, or answered by them (issue 566). Of the end of an approval, `take_in`
    /// tells what concerns them: a withdrawal of one pending, an answer given by a rule of theirs.
    pub(crate) fn concerns_user(&self, envelope: &Envelope) -> bool {
        match &envelope.event {
            ActorEvent::ApprovalRequested(_) | ActorEvent::ApprovalRequestWithdrawn { .. } => true,
            ActorEvent::ApprovalAnswered { .. } => envelope.context.sender_id == self.me(),
            ActorEvent::QuestionRequested(request) => request.questions.is_asked(self.me()),
            ActorEvent::CompleteMessageGenerated(message) => message.mentions.contains(&self.me()),
            _ => false,
        }
    }

    /// What shows again what waits for the user in the channel: the approvals pending and the open
    /// questions put to them that are known - one asked since, which the channel has yet to show,
    /// is shown as it shows it, and not twice.
    pub(crate) fn waiting(&self) -> Vec<Printed> {
        let approvals = self.pending_approvals.iter().map(approval_printed);
        let mut put_to_user = self.put_to_user();
        put_to_user.retain(|q| self.knows_questions(q.id));
        let put_to_user = put_to_user.iter().map(|q| questions_printed(q, self.me()));
        approvals.chain(put_to_user).collect()
    }

    /// Takes in what the channel showed: keeps what the user's commands and what is printed later
    /// need of it, and answers what the terminal prints of it. What was sent in the user's name is
    /// passed over and printed not at all, since what they typed is on the screen already, as
    /// typed - but for an answer of theirs to an approval, after which it is theirs to give no
    /// longer, as on the web UI, and which is told where a rule of theirs gave it (issue 13).
    pub(crate) fn take_in(&mut self, envelope: &Envelope) -> Vec<Printed> {
        let sender_id = envelope.context.sender_id;
        if sender_id == self.me() {
            let ActorEvent::ApprovalAnswered { id, decision, .. } = &envelope.event else {
                return Vec::new();
            };
            self.take_pending(*id);
            return decision
                .by_rule
                .map(|_| Printed::Notice(format!("Approval {} answered by your rule", id.0)))
                .into_iter()
                .collect();
        }
        let is_agent = matches!(sender_id, ActorId::Agent(_));
        match &envelope.event {
            ActorEvent::ResponseStarted { turn_id } if is_agent => {
                self.end_under_way(sender_id);
                let name = envelope.context.sender_name.clone();
                self.under_way.push(UnderWay::began(sender_id, name.clone(), *turn_id, None));
                vec![Printed::Answering { sender: name }]
            }
            // Where the agent stands in its answer (issue 243).
            ActorEvent::StateChanged(state) => {
                if let Some(answer) = self.answer_of(sender_id) {
                    answer.state = Some(*state);
                }
                Vec::new()
            }
            // What streams of the message being written, shown as the answer under way until the
            // message is complete.
            ActorEvent::PartialMessageGenerated(partial)
            | ActorEvent::IntermediatePartialGenerated(partial) => {
                if let Some(answer) = self.answer_of(sender_id) {
                    answer.stream(partial);
                }
                Vec::new()
            }
            // Its answer has ended, said or not.
            ActorEvent::TurnFinished { .. } => {
                self.end_under_way(sender_id);
                Vec::new()
            }
            // Each message as soon as it is complete, as the web UI shows it; the full response
            // that ends an answer holds them again (issue 220).
            ActorEvent::CompleteMessageGenerated(message) => {
                if matches!(message.sender_id, ActorId::Agent(_)) {
                    self.last_answer = Some(message.id);
                }
                self.given_up = None;
                if let Some(answer) = self.answer_of(sender_id) {
                    answer.complete();
                }
                self.message_printed(message, Saying::Complete)
            }
            // A step of a plan under way, with the calls it made and their results.
            ActorEvent::IntermediateMessageGenerated(message) => {
                if let Some(answer) = self.answer_of(sender_id) {
                    answer.complete();
                }
                self.message_printed(message, Saying::Step)
            }
            ActorEvent::TurnGivenUp { turn_id, reason } => {
                self.end_under_way(sender_id);
                self.given_up =
                    (is_agent && reason.may_be_answered_again()).then_some((sender_id, *turn_id));
                vec![given_up(&envelope.context.sender_name, reason)]
            }
            // Shown once: known as what waits was read (`catch_up`), or asked again by an agent
            // that resumed its turn.
            ActorEvent::ApprovalRequested(request) if self.is_pending(request.id) => Vec::new(),
            ActorEvent::ApprovalRequested(request) => {
                self.pending_approvals.push(request.clone());
                vec![approval_printed(request)]
            }
            // The user's own answer, given by a rule of theirs; only the approver tells when the
            // approval is over.
            // Another's answer alone decides nothing; it goes to the approver alone.
            ActorEvent::ApprovalAnswered { .. } => Vec::new(),
            // The approval is over: the approver decided, or it is no longer asked for.
            ActorEvent::ApprovalGranted(response)
            | ActorEvent::ApprovalDenied(response)
            | ActorEvent::ApprovalTimedOut(response) => {
                self.take_pending(response.id);
                Vec::new()
            }
            ActorEvent::ApprovalRequestWithdrawn { id } => {
                let withdrawn = self.take_pending(*id).is_some();
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
            _ => shown(envelope, &self.handles),
        }
    }

    /// The answers under way in the channel, in the order they began.
    pub(crate) fn under_way(&self) -> &[UnderWay] {
        &self.under_way
    }

    /// Whether approval `id` is known as pending - read as what waits (`catch_up`), or shown
    /// since -, and is not over.
    pub(crate) fn is_pending(&self, id: ApprovalId) -> bool {
        self.pending_approvals.iter().any(|request| request.id == id)
    }

    /// The open questions put to the user that `arg` names by id, or without one the last put to
    /// them, as their actor tells; none between two runs of it.
    pub(crate) fn questions_put_to_user(&self, arg: &str) -> Option<QuestionRequest> {
        let mut put_to_user = self.put_to_user().into_iter();
        match arg.trim() {
            "" => put_to_user.next_back(),
            id => {
                Uuid::parse_str(id).ok().and_then(|id| put_to_user.find(|q| q.id == QuestionId(id)))
            }
        }
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

    /// Asks through `actor`, the user's in the channel, for the answer under way, the turn given
    /// up or the last answer to be given again; `false` if there is none.
    ///
    /// # Errors
    /// As `UserActor::request_retry_of_turn` and `UserActor::request_retry`.
    pub(crate) async fn retry(&self, actor: &UserActor) -> Result<bool> {
        let under_way = self.under_way.last().map(|answer| (answer.agent, answer.turn));
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
        let (session_id, channel_id) = (self.channel.session_id(), self.channel_id());
        let read =
            async { self.views.channel(session_id, channel_id).await?.pending_approvals().await };
        match read.await {
            Ok(pending) => Some(pending),
            Err(error) => {
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

    /// The open questions put to the user, in the order they were asked, as their actor tells
    /// (`UserActor::questions_to_answer`); none between two runs of it.
    fn put_to_user(&self) -> Vec<QuestionRequest> {
        self.actor().map(|actor| actor.questions_to_answer()).unwrap_or_default()
    }

    /// The answer under way of `agent`, if it gives one.
    fn answer_of(&mut self, agent: ActorId) -> Option<&mut UnderWay> {
        self.under_way.iter_mut().find(|answer| answer.agent == agent)
    }

    /// The answers under way in the channel as its running channel tells them: those known that
    /// still are, in the order they began and with what they streamed, then those not known, in
    /// the order of the participants, as begun now; `None` if the channel cannot be read.
    async fn read_under_way(&self) -> Option<Vec<UnderWay>> {
        let view = self.views.channel(self.channel.session_id(), self.channel_id()).await.ok()?;
        let mut running = Vec::new();
        for participant in view.participants().await {
            let snapshot = participant.snapshot(Detail::empty()).await;
            if let Some(turn) = snapshot.turn_under_way {
                let name = snapshot.name.unwrap_or_else(|| snapshot.id.to_string());
                running.push(UnderWay::began(snapshot.id, name, turn, snapshot.state));
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

    /// Ends the answer under way of `agent`, if it gives one, and forgets its quiet calls, whose
    /// results will not come any more.
    fn end_under_way(&mut self, agent: ActorId) {
        self.under_way.retain(|answer| answer.agent != agent);
        self.quiet_calls.retain(|_, (by, _)| *by != agent);
    }

    /// What the terminal prints of `message`, said in a message of the kind `saying`: its words
    /// and what it carries (`said_printed`), the tools it calls and how calls ended - but a call
    /// presented `quiet`, and its result where it succeeded; where it did not, the call is printed
    /// with it, as the web UI shows them.
    fn message_printed(&mut self, message: &Message, saying: Saying) -> Vec<Printed> {
        let mut printed: Vec<Printed> =
            said_printed(message, &self.handles, saying).into_iter().collect();
        for call in &message.tool_calls {
            let sender = message.sender_name.clone();
            let called = Printed::ToolCall { sender, call: Box::new(call.clone()) };
            match (&call.id, call.presentation == ToolPresentation::QUIET) {
                (Some(id), true) => {
                    self.quiet_calls.insert(id.clone(), (message.sender_id, called));
                }
                _ => printed.push(called),
            }
        }
        for result in &message.tool_results {
            let quiet = result.id.as_ref().and_then(|id| self.quiet_calls.remove(id));
            match quiet {
                Some(_) if result.status.is_ok() => continue,
                Some((_, called)) => printed.push(called),
                None => {}
            }
            printed.push(Printed::ToolResult(Box::new(result.clone())));
        }
        printed
    }
}

/// What the terminal prints of `envelope` that the user's commands need nothing of, by the
/// `handles` of the channel's participants: what was changed of what was said, and what was done
/// in the channel.
fn shown(envelope: &Envelope, handles: &HashMap<ActorId, String>) -> Vec<Printed> {
    let sender_id = envelope.context.sender_id;
    let sender = &envelope.context.sender_name;
    let plain = |line: String| Some(Printed::Plain(line));
    let printed = match &envelope.event {
        // What another changed of what was said (issue 234).
        ActorEvent::MessageModified(message) => said_printed(message, handles, Saying::Edited),
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
            plain(format!("{sender} is carrying out /{command}..."))
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
        ActorEvent::ActivityStarted { activity, .. } => plain(format!("{sender} is {activity}...")),
        ActorEvent::ActivityFinished { output, .. } => plain(output.to_string()),
        // Who joined or left (issue 408), by whose act.
        ActorEvent::ParticipantJoined { participant, name } => plain(match sender_id {
            by if by == *participant => format!("{name} joined the channel"),
            ActorId::System => format!("{name} was added to the channel"),
            _ => format!("{sender} added {name} to the channel"),
        }),
        ActorEvent::ParticipantLeft { participant, name, .. } => plain(match sender_id {
            by if by == *participant => format!("{name} left the channel"),
            ActorId::System => format!("{name} was removed from the channel"),
            _ => format!("{sender} removed {name} from the channel"),
        }),
        _ => None,
    };
    printed.into_iter().collect()
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

/// What `message` says, in a message of the kind `saying`, by the `handles` of the channel's
/// participants; `None` if it says nothing (`said`).
fn said_printed(
    message: &Message,
    handles: &HashMap<ActorId, String>,
    saying: Saying,
) -> Option<Printed> {
    let said = said(message, handles)?;
    Some(Printed::Said { sender: message.sender_name.clone(), said, saying })
}

/// What `message` says: its words, with where they mention participants by the `handles` of the
/// channel's, and what it carries (issue 282); `None` if it says neither.
fn said(message: &Message, handles: &HashMap<ActorId, String>) -> Option<Said> {
    let words = message
        .content
        .as_deref()
        .filter(|content| !content.is_empty())
        .map(|content| words(message, content, handles));
    let attachments = message.attachments.clone();
    (words.is_some() || !attachments.is_empty()).then_some(Said { words, attachments })
}

/// The words `content` of `message`, with where they mention a participant it mentions, by the
/// `handles` of the channel's participants.
fn words(message: &Message, content: &str, handles: &HashMap<ActorId, String>) -> Words {
    let mentioned: Vec<&String> =
        message.mentions.iter().filter_map(|id| handles.get(id)).collect();
    // A group handle addresses everyone (issue 401): resolved if anyone was.
    let mentions = mention_spans(content)
        .into_iter()
        .filter(|span| {
            mentioned.contains(&&span.handle)
                || (is_group_handle(&span.handle) && !message.mentions.is_empty())
        })
        .map(|span| span.range)
        .collect();
    Words { text: content.to_string(), mentions }
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
                return followed.take_in(&shown);
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
        ApprovalRequest {
            suggested: None,
            explicit: false,
            id: ApprovalId::new(),
            session_id: channel.session_id(),
            channel_id: channel.id(),
            agent_id,
            tool_call: make_tool_call("test_tool"),
            description: "call test_tool".to_string(),
        }
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
    use caelumora_domain::actor::{ActorState, Rejection};
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::{ApprovalRuleId, ApprovalVerdict, ApproverId};
    use caelumora_domain::channel::HistoryAccess;
    use caelumora_domain::content::testing::make_content_ref;
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::{ToolCall, ToolCallResult, ToolResultStatus};
    use caelumora_kernel::channel::ChannelError;
    use caelumora_kernel::session::testing::{
        owned_kernel, running_session_of_a_user, spec, test_kernel,
    };
    use caelumora_kernel::session::{SessionLoop, SessionManager};
    use caelumora_kernel::testing::{Script, ScriptedAgents, TestKernel};
    use caelumora_kernel::view::Viewer;
    use caelumora_security::confidentiality::ConfidentialityRules;
    use caelumora_security::SecurityContext;
    use std::time::Duration;
    use tokio::time;

    /// A running session of a user, in a kernel of its own, with its manager and the context of
    /// its owner.
    async fn running() -> (Arc<SessionManager>, Arc<SessionLoop>, SecurityContext) {
        let manager = Arc::new(SessionManager::new(test_kernel()));
        let (session_loop, ctx, _) = running_session_of_a_user(&manager).await;
        (manager, session_loop, ctx)
    }

    /// A channel followed by the owner of a running session, the first of their session.
    async fn followed() -> (Followed, Arc<ChannelLoop>, ActorId) {
        let (manager, session_loop, ctx) = running().await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let views = manager.views(Viewer::new(ctx.clone()));
        let followed =
            Followed::new(Arc::clone(&channel), views).await.expect("the owner takes part");
        (followed, channel, ctx.caller_id)
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
        let said_by = |message: Message| said(&message, &HashMap::new());
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
        let told = lines(&followed.take_in(&from(&channel, ActorId::System, rejected)));
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0]
                .starts_with("[rejected] the channel did not let in your CompleteMessageGenerated"),
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
        let told = lines(&followed.take_in(&from(&channel, approver, refused)));
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0].starts_with("[refused] ")
                && told[0].contains("did not take your ApprovalAnswered"),
            "{told:?}"
        );
    }

    #[tokio::test]
    async fn what_the_user_said_is_not_printed_again() {
        let (mut followed, channel, me) = followed().await;
        let other = ActorId::User(UserId::new());
        let said = |sender| ActorEvent::CompleteMessageGenerated(message(sender, "hello"));
        assert!(followed.take_in(&from(&channel, me, said(me))).is_empty());
        assert_eq!(lines(&followed.take_in(&from(&channel, other, said(other)))), ["other> hello"]);
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
        let shown = lines(&followed.take_in(&from(
            &channel,
            agent,
            ActorEvent::ApprovalRequested(request),
        )));
        assert!(shown.first().is_some_and(|line| line.contains(&id.0.to_string())), "{shown:?}");
        let withdrawn = ActorEvent::ApprovalRequestWithdrawn { id };
        assert_eq!(
            lines(&followed.take_in(&from(&channel, agent, withdrawn.clone()))),
            [format!("--- Approval {} withdrawn: the turn ended ---", id.0)]
        );
        assert!(followed.take_in(&from(&channel, agent, withdrawn)).is_empty(), "told once");
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
            followed.take_in(&from(&channel, agent, ActorEvent::ApprovalRequested(request)));
            assert!(followed.take_in(&from(&channel, other, answered(other, id, None))).is_empty());
            assert!(followed.take_in(&from(&channel, approver, verdict(decided(id)))).is_empty());
            assert!(!followed.is_pending(id), "over");
        }
        let request = approval(&channel, agent_id);
        let id = request.id;
        followed.take_in(&from(&channel, agent, ActorEvent::ApprovalRequested(request)));
        followed.take_in(&from(&channel, other, answered(other, id, None)));
        assert!(followed.is_pending(id), "still pending after another's answer");
        assert!(followed.take_in(&from(&channel, me, answered(me, id, None))).is_empty());
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
        followed.take_in(&from(&channel, agent, ActorEvent::ApprovalRequested(request)));
        let answer = |by: ActorId, by_rule| from(&channel, by, answered(by, id, by_rule));
        let other = ActorId::User(UserId::new());
        assert!(followed.take_in(&answer(me, None)).is_empty(), "typed by the user");
        assert!(
            followed.take_in(&answer(other, Some(ApprovalRuleId::new()))).is_empty(),
            "not theirs"
        );
        assert_eq!(
            lines(&followed.take_in(&answer(me, Some(ApprovalRuleId::new())))),
            [format!("--- Approval {} answered by your rule ---", id.0)]
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
        assert_eq!(followed.questions_put_to_user("").map(|q| q.id), Some(id), "the last");
        assert_eq!(followed.questions_put_to_user(&id.0.to_string()).map(|q| q.id), Some(id));
        let closed = from(&channel, agent, ActorEvent::QuestionClosed { id, shown_to: None });
        routed(&mut followed, closed).await;
        assert!(!followed.is_to_answer(id));
        assert!(followed.questions_put_to_user("").is_none());
    }

    /// What `/answer` and `/decline` name: questions put to the user alone, by id or the last.
    #[tokio::test]
    async fn only_questions_put_to_the_user_are_named_for_answer_and_decline() {
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
        let put_to_user = |arg: &str| followed.questions_put_to_user(arg).map(|q| q.id);
        assert_eq!(put_to_user(&other_id.0.to_string()), None, "put to another");
        assert_eq!(put_to_user(""), Some(id), "the last put to the user, asked before");
        assert_eq!(put_to_user(&id.0.to_string()), Some(id));
        assert_eq!(put_to_user("no id"), None);
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
        let mentioning = Message { mentions: vec![me], ..message(other, "@owner look") };
        let asked = [
            ActorEvent::ApprovalRequested(request),
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

    /// What ends an approval for the user concerns them - withdrawn, or answered by them -, not
    /// another's answer; of it, what is told is a withdrawal of one shown, and an answer of theirs
    /// given by a rule (issue 566).
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
        assert!(followed.concerns_user(&withdrawn));
        assert!(followed.concerns_user(&by_rule(me)));
        assert!(followed.concerns_user(&typed));
        assert!(!followed.concerns_user(&by_rule(other)), "another's answer");

        assert!(followed.take_in(&withdrawn).is_empty(), "not shown: nothing told");
        followed.take_in(&from(&channel, agent, ActorEvent::ApprovalRequested(request)));
        assert!(followed.take_in(&typed).is_empty(), "typed by the user: nothing told");
        assert_eq!(followed.take_in(&by_rule(me)).len(), 1, "given by their rule: told");
    }

    /// What waits for the user as a channel is followed - the approvals the agents' turns wait
    /// for, the open questions put to them - is known, shown again, and shown once where the
    /// channel shows it after the watch began (issue 565).
    #[tokio::test]
    async fn what_waits_as_a_channel_is_followed_is_known_and_shown_once() {
        let kernel = test_kernel();
        let manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
        let (session_loop, ctx, _) = running_session_of_a_user(&manager).await;
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

        let views = manager.views(Viewer::new(ctx));
        let mut followed =
            Followed::new(Arc::clone(&channel), views).await.expect("the owner takes part");
        assert!(followed.is_pending(id), "the approval the agent's turn waits for");
        assert!(followed.knows_questions(question_id), "the questions put to the user");
        let waiting = lines(&followed.waiting()).join("\n");
        let shown_again = [id.0, question_id.0].map(|id| waiting.contains(&id.to_string()));
        assert_eq!(shown_again, [true, true], "{waiting}");

        let requested = ApprovalRequest { id, ..approval(&channel, agent_id) };
        let requested = from(&channel, agent, ActorEvent::ApprovalRequested(requested));
        assert!(followed.take_in(&requested).is_empty(), "the approval, known");
        assert!(followed.take_in(&asked).is_empty(), "the questions, known");

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
            lines(&followed.take_in(&from(&channel, agent, said))),
            ["other> let me look", "other calls terminal, its arguments not shown"]
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
            lines(&followed.take_in(&from(&channel, agent, step))),
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
        let shown =
            |followed: &mut Followed, event| followed.take_in(&from(&channel, agent, event));
        shown(&mut followed, ActorEvent::ResponseStarted { turn_id });
        shown(&mut followed, ActorEvent::StateChanged(ActorState::ExecutingTools));
        for chunk in ["Hel", "lo"] {
            let partial = Message { reasoning: Some("hm".to_string()), ..message(agent, chunk) };
            assert!(shown(&mut followed, ActorEvent::PartialMessageGenerated(partial)).is_empty());
        }
        let answer = &followed.under_way()[0];
        assert_eq!((answer.turn, answer.state), (turn_id, Some(ActorState::ExecutingTools)));
        assert_eq!((answer.content.as_str(), answer.reasoning.as_str()), ("Hello", "hmhm"));

        shown(&mut followed, ActorEvent::CompleteMessageGenerated(message(agent, "Hello")));
        let answer = &followed.under_way()[0];
        assert!(answer.content.is_empty() && answer.reasoning.is_empty(), "complete");
    }

    /// Catching up reads the answers under way anew from the running channel: one whose end was
    /// missed is under way no more (issue 569).
    #[tokio::test]
    async fn catching_up_reads_the_answers_under_way_anew() {
        let (mut followed, channel, _) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        let started = ActorEvent::ResponseStarted { turn_id: TurnId::new() };
        followed.take_in(&from(&channel, agent, started));
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
        let manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
        let (session_loop, ctx, _) = running_session_of_a_user(&manager).await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let agent = ActorId::Agent(an_agent_in(&kernel, &session_loop, &ctx, channel.id()).await);
        let views = manager.views(Viewer::new(ctx));
        let mut followed =
            Followed::new(Arc::clone(&channel), views.clone()).await.expect("the owner takes part");
        let actor = followed.actor().expect("the owner's actor runs");
        actor.send_message(Some("hello".to_string()), None, Vec::new()).await.expect("said");
        let begun = async {
            loop {
                if let Ok(Shown::Envelope(envelope)) = followed.next().await {
                    followed.take_in(&envelope);
                    if !followed.under_way().is_empty() {
                        break;
                    }
                }
            }
        };
        time::timeout(Duration::from_secs(5), begun).await.expect("the agent begins to answer");
        let partial = ActorEvent::PartialMessageGenerated(message(agent, "Hel"));
        followed.take_in(&from(&channel, agent, partial));

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
        followed.take_in(&from(&channel, agent, ActorEvent::CompleteMessageGenerated(calling)));
        let finished = ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: Vec::new() };
        followed.take_in(&from(&channel, agent, finished));

        let result = ToolCallResult::of(
            &quiet,
            ToolResultStatus::Failed,
            "no".to_string(),
            Default::default(),
        );
        let ended = Message { tool_results: vec![result], ..message(agent, "") };
        let printed = lines(&followed.take_in(&from(
            &channel,
            agent,
            ActorEvent::CompleteMessageGenerated(ended),
        )));
        assert_eq!(printed, ["attach failed: no"], "its call forgotten");
    }

    /// That an agent began to answer is printed; another user's response is no answer of an
    /// agent (issue 569).
    #[tokio::test]
    async fn an_answer_begun_is_printed() {
        let (mut followed, channel, _) = followed().await;
        let helper = ActorId::Agent(AgentId::new());
        let another = ActorId::User(UserId::new());
        let started = ActorEvent::ResponseStarted { turn_id: TurnId::new() };
        let begun = lines(&followed.take_in(&from(&channel, helper, started.clone())));
        assert_eq!(begun, [format!("{helper} is answering...")]);
        assert!(followed.take_in(&from(&channel, another, started)).is_empty(), "no agent");
    }

    #[tokio::test]
    async fn retry_asks_for_the_turn_under_way_or_given_up_before_the_last_answer() {
        let (mut followed, channel, _) = followed().await;
        let actor = followed.actor().expect("the owner's actor runs");
        let agent = ActorId::Agent(AgentId::new());
        assert!(!followed.retry(&actor).await.unwrap(), "nothing to give again");
        let refused = |e: Error| e.downcast::<ChannelError>().expect("refused by the actor");

        let said = ActorEvent::CompleteMessageGenerated(message(agent, "an answer"));
        followed.take_in(&from(&channel, agent, said));
        let error = refused(followed.retry(&actor).await.unwrap_err());
        assert!(matches!(error, ChannelError::MessageNotFound { .. }), "the last answer: {error}");

        // A turn is asked for again as it is - whether it can be the channel's to decide as it
        // admits the request (issue 567) -, a message is looked at first.
        let turn_id = TurnId::new();
        followed.take_in(&from(&channel, agent, ActorEvent::ResponseStarted { turn_id }));
        assert!(followed.retry(&actor).await.unwrap(), "the turn, asked for");

        followed.take_in(&from(
            &channel,
            agent,
            ActorEvent::TurnFinished { turn_id, messages: Vec::new() },
        ));
        let error = refused(followed.retry(&actor).await.unwrap_err());
        assert!(matches!(error, ChannelError::MessageNotFound { .. }), "the last again: {error}");

        let given_up = ActorEvent::TurnGivenUp {
            turn_id: TurnId::new(),
            reason: GivenUpReason::Failed { message: "unreachable".to_string() },
        };
        followed.take_in(&from(&channel, agent, given_up));
        assert!(followed.retry(&actor).await.unwrap(), "the turn given up, asked for");

        let said = ActorEvent::CompleteMessageGenerated(message(agent, "another answer"));
        followed.take_in(&from(&channel, agent, said));
        let error = refused(followed.retry(&actor).await.unwrap_err());
        assert!(matches!(error, ChannelError::MessageNotFound { .. }), "said since: {error}");
    }

    #[tokio::test]
    async fn a_channel_is_followed_only_by_who_takes_part() {
        let (manager, session_loop, ctx) = running().await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let followed_by = |ctx: SecurityContext| {
            Followed::new(Arc::clone(&channel), manager.views(Viewer::new(ctx)))
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
        let (_, manager) = owned_kernel(ConfidentialityRules::default());
        let (session_loop, owner, other) = running_session_of_a_user(&manager).await;
        let theirs = SecurityContext::new(other, session_loop.id(), Default::default());
        let views = manager.views(Viewer::new(theirs.clone()));
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
        let (_, manager) = owned_kernel(ConfidentialityRules::default());
        let (session_loop, owner, other) = running_session_of_a_user(&manager).await;
        let theirs = SecurityContext::new(other, session_loop.id(), Default::default());
        let views = manager.views(Viewer::new(theirs.clone()));
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
        assert_eq!(followed.questions_put_to_user("").map(|q| q.id), Some(id), "theirs to answer");

        channel.leave(&theirs, other).await.expect("they may leave");
        assert!(shows(&mut followed, |shown| matches!(shown, Shown::Ended)).await, "it stopped");
        assert!(followed.actor().is_none(), "none runs between the two runs");

        channel.add_participant(&owner, other, HistoryAccess::All).await.expect("added again");
        assert!(shows(&mut followed, |shown| matches!(shown, Shown::Started)).await, "a new run");
        assert!(!followed.ended().await, "taking part again");
        let again = followed.actor().expect("their new actor runs");
        assert!(!Arc::ptr_eq(&first, &again), "not the actor that ended");
        assert!(
            followed.questions_put_to_user("").is_none() && !followed.is_to_answer(id),
            "none the new actor would refuse"
        );
    }
}
