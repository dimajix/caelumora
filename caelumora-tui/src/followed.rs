//! A channel as the terminal follows it for the user: what it prints of what the channel shows,
//! and what the user's commands there need of it (issue 561).

use anyhow::Result;
use caelumora_domain::actor::{ActorEvent, ActorId, Envelope};
use caelumora_domain::approval::{ApprovalId, ApprovalRequest};
use caelumora_domain::channel::ChannelId;
use caelumora_domain::mention::{is_group_handle, mention_spans};
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::question::{QuestionId, QuestionRequest};
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
use uuid::Uuid;

/// A channel the user takes part in, as the terminal follows it - as the channel stream of the
/// REST server does (`docs/CHANNELS.md`, issue 386): the channel, the views it is read through in
/// their name and what their actor in it shows, the approvals pending there, which questions it
/// printed, and what `/retry` asks to be given again.
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
    /// The approval requests shown, in the order they were requested, until the approver ends
    /// them - decided, timed out or withdrawn - or the user answered them, as the web UI keeps
    /// them: so that `/approve` and `/deny` find the channel a request waits in, and `/switch`
    /// shows it again. Which agent asked, and whether an answer counts, the approver tells, which
    /// holds the request (issue 567). Covers both plan proposals and regular tool calls - both
    /// flow through the same generic `ApprovalRequested` event.
    pending_approvals: Vec<ApprovalRequest>,
    /// The questions printed (issue 165), until they close: so that questions asked anew by an
    /// agent that resumed its turn are printed once. Which are open is their actor's to tell
    /// (`UserActor::questions_to_answer`).
    printed_questions: HashSet<QuestionId>,
    /// The last answer of an agent shown, which `/retry` asks it to give again (issue 234) unless
    /// one of the two below is known.
    last_answer: Option<MessageId>,
    /// The answer under way, by the agent and its turn, which `/retry` asks for first (issue 235).
    under_way: Option<(ActorId, TurnId)>,
    /// The turn given up that may be answered anew, with nothing said since (issue 259).
    given_up: Option<(ActorId, TurnId)>,
}

impl Followed {
    /// Follows `channel` for the user `views` are read by, from now on, as the views let them
    /// (`ChannelView::user_follow`), knowing its participants' handles as they are now; `None` if
    /// the viewer is no user the channel lists, or cannot read it.
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
        Some(Self {
            channel,
            user,
            views,
            watch,
            handles,
            pending_approvals: Vec::new(),
            printed_questions: HashSet::new(),
            last_answer: None,
            under_way: None,
            given_up: None,
        })
    }

    /// The channel followed.
    pub(crate) fn channel_id(&self) -> ChannelId {
        self.channel.id()
    }

    /// The name of the channel, as it is called now.
    pub(crate) fn name(&self) -> Name {
        self.channel.name()
    }

    /// The line that tells who answers in the channel, as the views tell it (issue 174); `None` if
    /// the channel cannot be read.
    pub(crate) async fn who_answers(&self) -> Option<String> {
        let view = self.views.channel(self.channel.session_id(), self.channel_id()).await.ok()?;
        let unknown = || "from a bundle".to_string();
        let moderator = view.moderator().unwrap_or_else(unknown);
        Some(format!(
            "Moderator: {moderator}, responders: {}",
            view.responder_policy().unwrap_or_else(unknown)
        ))
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

    /// Whether `envelope` asks something of the user: an approval, questions put to them, or a
    /// message that mentions them.
    pub(crate) fn waits_for_user(&self, envelope: &Envelope) -> bool {
        match &envelope.event {
            ActorEvent::ApprovalRequested(_) => true,
            ActorEvent::QuestionRequested(request) => request.questions.is_asked(self.me()),
            ActorEvent::CompleteMessageGenerated(message) => message.mentions.contains(&self.me()),
            _ => false,
        }
    }

    /// The lines that show again what waits for the user in the channel: the approvals pending and
    /// the open questions put to them.
    pub(crate) fn waiting(&self) -> Vec<String> {
        let approvals = self.pending_approvals.iter().flat_map(approval_shown);
        let put_to_user = self.put_to_user();
        let put_to_user = put_to_user.iter().flat_map(|q| questions_shown(q, self.me()));
        approvals.chain(put_to_user).collect()
    }

    /// Takes in what the channel showed: keeps what the user's commands need of it, and answers
    /// the lines the terminal prints of it. What was sent in the user's name is passed over and
    /// printed not at all, since what they typed is on the screen already, as typed - but for an
    /// answer of theirs to an approval, after which it is theirs to give no longer, as on the web
    /// UI, and which is told where a rule of theirs gave it (issue 13).
    pub(crate) fn take_in(&mut self, envelope: &Envelope) -> Vec<String> {
        let sender_id = envelope.context.sender_id;
        if sender_id == self.me() {
            let ActorEvent::ApprovalAnswered { id, decision, .. } = &envelope.event else {
                return Vec::new();
            };
            self.take_pending(*id);
            return decision
                .by_rule
                .map(|_| format!("--- Approval {} answered by your rule ---", id.0))
                .into_iter()
                .collect();
        }
        let is_agent = matches!(sender_id, ActorId::Agent(_));
        match &envelope.event {
            ActorEvent::ResponseStarted { turn_id } => {
                self.under_way = is_agent.then_some((sender_id, *turn_id));
                Vec::new()
            }
            // Its answer has ended, said or not.
            ActorEvent::TurnFinished { .. } if self.is_under_way(sender_id) => {
                self.under_way = None;
                Vec::new()
            }
            // Each message as soon as it is complete, as the web UI shows it; the full response
            // that ends an answer holds them again (issue 220).
            ActorEvent::CompleteMessageGenerated(message) => {
                if matches!(message.sender_id, ActorId::Agent(_)) {
                    self.last_answer = Some(message.id);
                }
                self.given_up = None;
                let said = said(message, &self.handles);
                said.map(|said| format!("{}> {said}", message.sender_name)).into_iter().collect()
            }
            ActorEvent::TurnGivenUp { turn_id, reason } => {
                if self.is_under_way(sender_id) {
                    self.under_way = None;
                }
                self.given_up =
                    (is_agent && reason.may_be_answered_again()).then_some((sender_id, *turn_id));
                vec![given_up(&envelope.context.sender_name, reason)]
            }
            ActorEvent::ApprovalRequested(request) => {
                if !self.is_pending(request.id) {
                    self.pending_approvals.push(request.clone());
                }
                approval_shown(request)
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
                    .then(|| format!("--- Approval {id} withdrawn: the turn ended ---"))
                    .into_iter()
                    .collect()
            }
            // Asked anew by an agent that resumed its turn: shown once.
            ActorEvent::QuestionRequested(request) => {
                match self.printed_questions.insert(request.id) {
                    true => questions_shown(request, self.me()),
                    false => Vec::new(),
                }
            }
            ActorEvent::QuestionClosed { id, .. } => {
                self.printed_questions.remove(id);
                Vec::new()
            }
            _ => shown(envelope, &self.handles),
        }
    }

    /// Whether approval `id` was shown and is not over.
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

    /// Whether questions `id` were printed of this channel, and are not closed.
    pub(crate) fn printed(&self, id: QuestionId) -> bool {
        self.printed_questions.contains(&id)
    }

    /// Asks through `actor`, the user's in the channel, for the answer under way, the turn given
    /// up or the last answer to be given again; `false` if there is none.
    ///
    /// # Errors
    /// As `UserActor::request_retry_of_turn` and `UserActor::request_retry`.
    pub(crate) async fn retry(&self, actor: &UserActor) -> Result<bool> {
        match (self.under_way.or(self.given_up), self.last_answer) {
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

    /// Whether `sender` gives the answer under way.
    fn is_under_way(&self, sender: ActorId) -> bool {
        self.under_way.is_some_and(|(agent, _)| agent == sender)
    }
}

/// The lines the terminal prints of `envelope` that the user's commands need nothing of, by the
/// `handles` of the channel's participants: what was said in a step or changed, and what was done
/// in the channel.
fn shown(envelope: &Envelope, handles: &HashMap<ActorId, String>) -> Vec<String> {
    let sender_id = envelope.context.sender_id;
    let sender = &envelope.context.sender_name;
    let line = match &envelope.event {
        ActorEvent::IntermediateMessageGenerated(message) => {
            said(message, handles).map(|said| format!("[working] {}> {said}", message.sender_name))
        }
        // What another changed of what was said (issue 234).
        ActorEvent::MessageModified(message) => {
            said(message, handles).map(|said| format!("{}> [edited] {said}", message.sender_name))
        }
        ActorEvent::MessageRevoked { .. } => Some(format!("{sender} took a message back")),
        ActorEvent::RetryAccepted { .. } => Some(format!("{sender} answers again")),
        ActorEvent::RetryRefused { reason, .. } => {
            Some(format!("{sender} cannot answer again: {reason}"))
        }
        // What this user sent that the channel did not let in (issue 304), told to them alone.
        ActorEvent::EventRejected { rejected, reason, .. } => {
            Some(format!("[rejected] the channel did not let in your {rejected}: {reason}"))
        }
        // What this user sent that the actor it went to did not take (issue 567), told to them
        // alone: the approver an answer it does not count.
        ActorEvent::EventRefused { refused, reason, .. } => {
            Some(format!("[refused] {sender} did not take your {refused}: {reason}"))
        }
        // Told to its issuer alone (issue 299).
        ActorEvent::CommandStarted { command, .. } => {
            Some(format!("{sender} is carrying out /{command}..."))
        }
        // Carried out by an agent in a turn of its own (issue 269), or run by a user.
        ActorEvent::CommandExecuted { command, output, turn_id, .. } => {
            let verb = if turn_id.is_some() { "carried out" } else { "ran" };
            Some(format!("{sender} {verb} /{command}: {output}"))
        }
        ActorEvent::CommandFailed { command, reason, .. } => {
            Some(format!("{sender} did not carry out /{command}: {reason}"))
        }
        // Of its own accord (issue 284).
        ActorEvent::ActivityStarted { activity, .. } => Some(format!("{sender} is {activity}...")),
        ActorEvent::ActivityFinished { output, .. } => Some(output.to_string()),
        // Who joined or left (issue 408), by whose act.
        ActorEvent::ParticipantJoined { participant, name } => Some(match sender_id {
            by if by == *participant => format!("{name} joined the channel"),
            ActorId::System => format!("{name} was added to the channel"),
            _ => format!("{sender} added {name} to the channel"),
        }),
        ActorEvent::ParticipantLeft { participant, name, .. } => Some(match sender_id {
            by if by == *participant => format!("{name} left the channel"),
            ActorId::System => format!("{name} was removed from the channel"),
            _ => format!("{sender} removed {name} from the channel"),
        }),
        _ => None,
    };
    line.into_iter().collect()
}

/// The line that tells that `sender` gave its answer up, and why.
fn given_up(sender: &str, reason: &GivenUpReason) -> String {
    match reason {
        GivenUpReason::Failed { message } => {
            format!("[error] {sender} could not answer: {message}")
        }
        GivenUpReason::Lowered { channel } => {
            format!("{sender} gave its answer up: the channel was lowered to {channel}")
        }
        GivenUpReason::CallLimit { calls } => format!(
            "{sender} gave its answer up: it asked its model {calls} times, as often as one answer \
             or one step of a plan may"
        ),
        GivenUpReason::Left | GivenUpReason::Retried => {
            format!("{sender} gave its answer up: {}", reason.cause())
        }
    }
}

/// The lines that show `request` for approval, and how to answer it.
fn approval_shown(request: &ApprovalRequest) -> Vec<String> {
    let id = request.id.0;
    let mut lines =
        vec![format!("--- Approval requested (id: {id}) ---"), request.description.clone()];
    if request.explicit {
        lines.push(
            "A rule of this session or channel asks for this call to be decided by a person."
                .to_string(),
        );
    }
    lines.push(format!("Type '/approve {id}' or '/deny {id}' to respond."));
    lines
}

/// What `message` says as the terminal prints it: its words, the participants it mentions
/// highlighted, and what it carries by name, type and size (issue 282); `None` if it says neither.
fn said(message: &Message, handles: &HashMap<ActorId, String>) -> Option<String> {
    let words = message
        .content
        .as_deref()
        .filter(|content| !content.is_empty())
        .map(|content| render_content(message, content, handles));
    let attachments = message.attachments.iter().map(|attachment| {
        format!(
            "[{}, {}, {}]",
            attachment.name.as_str(),
            attachment.media_type,
            attachment.size_shown()
        )
    });
    let parts: Vec<String> = words.into_iter().chain(attachments).collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn render_content(message: &Message, content: &str, handles: &HashMap<ActorId, String>) -> String {
    let mentioned: Vec<&String> =
        message.mentions.iter().filter_map(|id| handles.get(id)).collect();
    let mut rendered = String::with_capacity(content.len());
    let mut pos = 0;
    for span in mention_spans(content) {
        // A group handle addresses everyone (issue 401): resolved if anyone was.
        if mentioned.contains(&&span.handle)
            || (is_group_handle(&span.handle) && !message.mentions.is_empty())
        {
            rendered.push_str(&content[pos..span.range.start]);
            rendered.push_str("\x1b[1;36m");
            rendered.push_str(&content[span.range.clone()]);
            rendered.push_str("\x1b[0m");
            pos = span.range.end;
        }
    }
    rendered.push_str(&content[pos..]);
    rendered
}

/// The lines that show the questions of `request` (issue 165), and - if they are put to `me` -
/// how to answer them.
fn questions_shown(request: &QuestionRequest, me: ActorId) -> Vec<String> {
    let id = request.id.0;
    let mut lines =
        vec![format!("--- Questions of {} (id: {id}) ---", request.tool_call.function_name)];
    for (index, question) in request.questions.questions.iter().enumerate() {
        lines.push(format!("{}. [{}] {}", index + 1, question.header, question.question));
        for (number, choice) in question.choices.iter().enumerate() {
            let recommended = if choice.recommended { " (recommended)" } else { "" };
            let description =
                choice.description.as_deref().map(|d| format!(" - {d}")).unwrap_or_default();
            lines.push(format!(
                "   {}) {}{}{}",
                number + 1,
                choice.label,
                recommended,
                description
            ));
        }
    }
    if request.questions.is_asked(me) {
        if request.questions.sensitive {
            lines.push(
                "Sensitive: the answers go to the tool alone - but what you type is shown on this \
                 terminal."
                    .to_string(),
            );
        }
        lines.push(format!(
            "Type '/answer {id}' to answer them one by one, or '/decline {id}'. A message instead \
             declines them."
        ));
    } else {
        lines.push("They are put to others.".to_string());
    }
    lines
}

#[cfg(test)]
pub(crate) mod testing {
    //! What the tests of the terminal show a channel followed: envelopes, messages, approvals and
    //! questions, as a channel would send them.

    use super::Followed;
    use caelumora_domain::actor::{ActorEvent, ActorId, Envelope, EventContext};
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::{
        ApprovalDecision, ApprovalId, ApprovalRequest, ApprovalRuleId,
    };
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::message::{Message, MessageId};
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_kernel::actor::Shown;
    use caelumora_kernel::channel::ChannelLoop;

    /// Routes `envelope` to the user's actor in the channel `followed` follows, as the channel
    /// would (`UserActor::take_in_as_routed`), and takes in what the actor shows of it, passing
    /// over where a run of it begins or ends: the lines the terminal prints.
    pub(crate) async fn routed(followed: &mut Followed, envelope: Envelope) -> Vec<String> {
        let actor = followed.actor().expect("the user's actor runs");
        actor.take_in_as_routed(envelope).await;
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

    pub(crate) fn approval(channel: &ChannelLoop, agent_id: AgentId) -> ApprovalRequest {
        ApprovalRequest {
            suggested: None,
            explicit: false,
            id: ApprovalId::new(),
            session_id: channel.session_id(),
            channel_id: channel.id(),
            agent_id,
            tool_call: make_tool_call("test_tool"),
            description: "test".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{answered, approval, from, message, routed};
    use super::*;
    use anyhow::Error;
    use caelumora_domain::actor::Rejection;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::{ApprovalRuleId, ApprovalVerdict, ApproverId};
    use caelumora_domain::channel::HistoryAccess;
    use caelumora_domain::content::testing::make_content_ref;
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_kernel::channel::ChannelError;
    use caelumora_kernel::session::testing::{
        owned_kernel, running_session_of_a_user, spec, test_kernel,
    };
    use caelumora_kernel::session::{SessionLoop, SessionManager};
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
    fn a_message_is_printed_with_what_it_carries() {
        let message = |content: Option<&str>, attachments| Message {
            content: content.map(str::to_string),
            attachments,
            ..make_message(MessageId::new())
        };
        let chart = make_content_ref("chart.png", b"chart");
        let printed = |message: Message| said(&message, &HashMap::new());
        assert_eq!(printed(message(Some("hello"), Vec::new())).as_deref(), Some("hello"));
        assert_eq!(
            printed(message(None, vec![chart.clone()])).as_deref(),
            Some("[chart.png, image/png, 5 B]"),
            "attachments alone"
        );
        assert_eq!(
            printed(message(Some("look"), vec![chart])).as_deref(),
            Some("look [chart.png, image/png, 5 B]")
        );
        assert_eq!(printed(message(Some(""), Vec::new())), None, "nothing said");
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
        let told = followed.take_in(&from(&channel, ActorId::System, rejected));
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
        let told = followed.take_in(&from(&channel, approver, refused));
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
        assert_eq!(followed.take_in(&from(&channel, other, said(other))), ["other> hello"]);
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
        let shown =
            followed.take_in(&from(&channel, agent, ActorEvent::ApprovalRequested(request)));
        assert!(shown.first().is_some_and(|line| line.contains(&id.0.to_string())), "{shown:?}");
        let withdrawn = ActorEvent::ApprovalRequestWithdrawn { id };
        assert_eq!(
            followed.take_in(&from(&channel, agent, withdrawn.clone())),
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
            followed.take_in(&answer(me, Some(ApprovalRuleId::new()))),
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
            assert!(followed.waits_for_user(&envelope), "{:?}", envelope.event);
            routed(&mut followed, envelope).await;
        }
        for event in not_asked {
            let envelope = from(&channel, agent, event);
            assert!(!followed.waits_for_user(&envelope), "{:?}", envelope.event);
            routed(&mut followed, envelope).await;
        }
        let waiting = followed.waiting().join("\n");
        assert!(waiting.contains(&ids[0]) && waiting.contains(&ids[1]), "{waiting}");
        assert!(!waiting.contains(&ids[2]), "put to another: {waiting}");
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
