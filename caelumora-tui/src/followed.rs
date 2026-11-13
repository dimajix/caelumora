//! A channel as the terminal follows it for the user: what it prints of what the channel shows,
//! and what the user's commands there need of it (issue 561).

use anyhow::Result;
use caelumora_domain::actor::{ActorEvent, ActorId, Envelope};
use caelumora_domain::agent::AgentId;
use caelumora_domain::approval::{ApprovalId, ApprovalRequest, ApprovalResponse};
use caelumora_domain::channel::ChannelId;
use caelumora_domain::mention::{is_group_handle, mention_spans};
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::question::{QuestionId, QuestionRequest};
use caelumora_domain::session::SessionId;
use caelumora_domain::turn::GivenUpReason;
use caelumora_domain::work::TurnId;
use caelumora_kernel::actor::user::UserActor;
use caelumora_kernel::channel::ChannelLoop;
use caelumora_kernel::view::Views;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// A channel the user takes part in, as the terminal follows it: where it lies, the views it is
/// read through in their name and their actor in it, the approvals and questions open there, and
/// what `/retry` asks to be given again.
pub(crate) struct Followed {
    /// The session of the channel.
    session_id: SessionId,
    /// The channel followed.
    channel_id: ChannelId,
    /// The views as the user reads them, whose viewer is the user it is followed for: what tells
    /// whether the channel still lists them.
    views: Views,
    /// The user's actor in the channel, through which what they say, run and answer goes.
    actor: Arc<UserActor>,
    /// The handles of the participants, as the channel was followed: what a message's mentions
    /// are highlighted by.
    handles: HashMap<ActorId, String>,
    /// Which agent waits for each approval, so that `/approve` and `/deny` know whom to answer.
    /// Covers both plan proposals and regular tool calls - both flow through the same generic
    /// `ApprovalRequested` event.
    pending_approvals: HashMap<ApprovalId, AgentId>,
    /// The questions open (issue 165), in the order they were asked.
    open_questions: Vec<QuestionRequest>,
    /// The last answer of an agent shown, which `/retry` asks it to give again (issue 234) unless
    /// one of the two below is known.
    last_answer: Option<MessageId>,
    /// The answer under way, by the agent and its turn, which `/retry` asks for first (issue 235).
    under_way: Option<(ActorId, TurnId)>,
    /// The turn given up that may be answered anew, with nothing said since (issue 259).
    given_up: Option<(ActorId, TurnId)>,
}

impl Followed {
    /// Follows `channel` for the user `views` are read by, knowing its participants' handles as
    /// they are now; `None` if the viewer is no user taking part in it.
    pub(crate) fn new(channel: &ChannelLoop, views: Views) -> Option<Self> {
        let ActorId::User(user) = views.context().caller_id else { return None };
        let actor = channel.user_actor(user)?;
        let handles =
            channel.participants().into_iter().filter_map(|m| Some((m.id, m.handle?))).collect();
        Some(Self {
            session_id: channel.session_id(),
            channel_id: channel.id(),
            views,
            actor,
            handles,
            pending_approvals: HashMap::new(),
            open_questions: Vec::new(),
            last_answer: None,
            under_way: None,
            given_up: None,
        })
    }

    /// The channel followed.
    pub(crate) fn channel_id(&self) -> ChannelId {
        self.channel_id
    }

    /// The user's actor in the channel, through which what they say, run and answer goes.
    pub(crate) fn actor(&self) -> &UserActor {
        &self.actor
    }

    /// Whether following the channel has ended - the user no longer takes part in it, or it was
    /// deleted, or cannot be read -, as the views read it in their name now, with the line that
    /// tells so: what is asked where a run of the user's actor began or ended
    /// (`docs/CHANNELS.md`, issue 386).
    pub(crate) async fn ended(&self) -> Option<&'static str> {
        let taken_part_in = self.views.listed_in(self.session_id, self.channel_id).await;
        (!taken_part_in).then_some(
            "--- You no longer take part in this channel, or it was deleted: type 'exit' to end ---",
        )
    }

    /// Takes in what the channel showed: keeps what the user's commands need of it, and answers
    /// the lines the terminal prints of it. What was sent in the user's name is passed over, kept
    /// and printed not at all, since what they typed is on the screen already, as typed - but for
    /// an approval a rule of theirs answered (issue 13).
    pub(crate) fn take_in(&mut self, envelope: &Envelope) -> Vec<String> {
        let sender_id = envelope.context.sender_id;
        if sender_id == self.me() && !answered_by_rule(&envelope.event) {
            return Vec::new();
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
                self.pending_approvals.insert(request.id, request.agent_id);
                approval_shown(request)
            }
            // The user's own answer, given by a rule of theirs: no longer theirs to give, as after
            // `/approve`, though only the approver tells when the approval is over.
            ActorEvent::ApprovalGranted(response) | ActorEvent::ApprovalDenied(response)
                if sender_id == self.me() =>
            {
                self.pending_approvals.remove(&response.id);
                vec![format!("--- Approval {} answered by your rule ---", response.id.0)]
            }
            // Another user's answer alone does not decide it.
            ActorEvent::ApprovalGranted(_) | ActorEvent::ApprovalDenied(_)
                if !matches!(sender_id, ActorId::Approver(_)) =>
            {
                Vec::new()
            }
            // The approval is over: decided, or no longer asked for.
            ActorEvent::ApprovalGranted(response)
            | ActorEvent::ApprovalDenied(response)
            | ActorEvent::ApprovalTimedOut(response) => {
                self.pending_approvals.remove(&response.id);
                Vec::new()
            }
            ActorEvent::ApprovalRequestWithdrawn { id } => {
                let withdrawn = self.pending_approvals.remove(id).is_some();
                let id = id.0;
                withdrawn
                    .then(|| format!("--- Approval {id} withdrawn: the turn ended ---"))
                    .into_iter()
                    .collect()
            }
            // Asked anew by an agent that resumed its turn: shown once.
            ActorEvent::QuestionRequested(request) => {
                if self.is_open(request.id) {
                    return Vec::new();
                }
                let lines = questions_shown(request, self.me());
                self.open_questions.push(request.clone());
                lines
            }
            ActorEvent::QuestionClosed { id, .. } => {
                self.open_questions.retain(|q| q.id != *id);
                Vec::new()
            }
            _ => shown(envelope, &self.handles),
        }
    }

    /// Answers approval `id` for the agent waiting for it; `false` if none is pending.
    ///
    /// # Errors
    /// If the answer cannot be sent into the channel.
    pub(crate) async fn approve(&mut self, id: ApprovalId, approved: bool) -> Result<bool> {
        let Some(agent_id) = self.pending_approvals.remove(&id) else {
            return Ok(false);
        };
        self.actor.send_approval(ApprovalResponse::new(id, agent_id), approved).await?;
        Ok(true)
    }

    /// The open questions put to the user that `arg` names by id, or without one the last put to
    /// them.
    pub(crate) fn questions_put_to_user(&self, arg: &str) -> Option<&QuestionRequest> {
        let mut put_to_user =
            self.open_questions.iter().filter(|q| q.questions.is_asked(self.me()));
        match arg.trim() {
            "" => put_to_user.next_back(),
            id => {
                Uuid::parse_str(id).ok().and_then(|id| put_to_user.find(|q| q.id == QuestionId(id)))
            }
        }
    }

    /// Whether questions `id` are still open.
    pub(crate) fn is_open(&self, id: QuestionId) -> bool {
        self.open_questions.iter().any(|q| q.id == id)
    }

    /// Asks for the answer under way, the turn given up or the last answer to be given again;
    /// `false` if there is none.
    ///
    /// # Errors
    /// As `UserActor::request_retry_of_turn` and `UserActor::request_retry`.
    pub(crate) async fn retry(&self) -> Result<bool> {
        match (self.under_way.or(self.given_up), self.last_answer) {
            (Some((agent, turn)), _) => self.actor.request_retry_of_turn(agent, turn).await?,
            (None, Some(answer)) => self.actor.request_retry(answer).await?,
            (None, None) => return Ok(false),
        };
        Ok(true)
    }

    /// The user, as an actor of the channel.
    fn me(&self) -> ActorId {
        self.views.context().caller_id
    }

    /// Whether `sender` gives the answer under way.
    fn is_under_way(&self, sender: ActorId) -> bool {
        self.under_way.is_some_and(|(agent, _)| agent == sender)
    }
}

/// Whether `event` is an answer to an approval that a rule gave (`ApprovalDecision::by_rule`).
fn answered_by_rule(event: &ActorEvent) -> bool {
    match event {
        ActorEvent::ApprovalGranted(response) | ActorEvent::ApprovalDenied(response) => {
            response.decisions.iter().any(|decision| decision.by_rule.is_some())
        }
        _ => false,
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
        ActorEvent::RetryRejected { reason, .. } => {
            Some(format!("{sender} cannot answer again: {reason}"))
        }
        // What this user sent that the channel did not let in (issue 304), told to them alone.
        ActorEvent::MessageRejected { rejected, reason, .. } => {
            Some(format!("[refused] the channel did not let in your {rejected}: {reason}"))
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
mod tests {
    use super::*;
    use anyhow::Error;
    use caelumora_domain::actor::EventContext;
    use caelumora_domain::approval::{ApprovalDecision, ApprovalRuleId, ApproverId};
    use caelumora_domain::content::testing::make_content_ref;
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::question::{Addressee, Question, Questions};
    use caelumora_domain::tool::ToolCall;
    use caelumora_domain::user::UserId;
    use caelumora_kernel::channel::ChannelError;
    use caelumora_kernel::session::testing::{
        ended, owned_kernel, running_session_of_a_user, spec, test_kernel,
    };
    use caelumora_kernel::session::{SessionLoop, SessionManager};
    use caelumora_kernel::view::Viewer;
    use caelumora_security::confidentiality::ConfidentialityRules;
    use caelumora_security::SecurityContext;
    use serde_json::json;
    use std::time::Duration;

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
        let followed = Followed::new(&channel, views).expect("the owner takes part");
        (followed, channel, ctx.caller_id)
    }

    /// `event` as `sender` sends it in `channel`.
    fn from(channel: &ChannelLoop, sender: ActorId, event: ActorEvent) -> Envelope {
        let context =
            EventContext::new(channel.session_id(), channel.id(), sender, sender.to_string());
        Envelope::new(context, event)
    }

    fn message(sender: ActorId, content: &str) -> Message {
        Message {
            sender_id: sender,
            sender_name: "other".to_string(),
            content: Some(content.to_string()),
            ..make_message(MessageId::new())
        }
    }

    fn tool_call() -> ToolCall {
        ToolCall {
            id: Some("call_1".to_string()),
            function_name: "test_tool".to_string(),
            arguments: json!({}),
            visibility: Default::default(),
            presentation: Default::default(),
        }
    }

    fn approval(channel: &ChannelLoop, agent_id: AgentId) -> ApprovalRequest {
        ApprovalRequest {
            suggested: None,
            explicit: false,
            id: ApprovalId::new(),
            session_id: channel.session_id(),
            channel_id: channel.id(),
            agent_id,
            tool_call: tool_call(),
            description: "test".to_string(),
        }
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

    #[tokio::test]
    async fn what_the_user_said_is_not_printed_again() {
        let (mut followed, channel, me) = followed().await;
        let other = ActorId::User(UserId::new());
        let said = |sender| ActorEvent::CompleteMessageGenerated(message(sender, "hello"));
        assert!(followed.take_in(&from(&channel, me, said(me))).is_empty());
        assert_eq!(followed.take_in(&from(&channel, other, said(other))), ["other> hello"]);
    }

    #[tokio::test]
    async fn an_approval_is_pending_until_answered_or_withdrawn() {
        let (mut followed, channel, _) = followed().await;
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
        assert!(!followed.approve(id, true).await.unwrap(), "no longer pending");

        // Over once the approver tells the verdict, or that it timed out; another user's answer
        // alone decides nothing.
        let approver = ActorId::Approver(ApproverId::new());
        let other = ActorId::User(UserId::new());
        let decided = |id| ApprovalResponse::new(id, agent_id);
        let over: [fn(ApprovalResponse) -> ActorEvent; 3] =
            [ActorEvent::ApprovalGranted, ActorEvent::ApprovalDenied, ActorEvent::ApprovalTimedOut];
        for verdict in over {
            let request = approval(&channel, agent_id);
            let id = request.id;
            followed.take_in(&from(&channel, agent, ActorEvent::ApprovalRequested(request)));
            let by_other = ActorEvent::ApprovalGranted(decided(id));
            assert!(followed.take_in(&from(&channel, other, by_other)).is_empty());
            assert!(followed.take_in(&from(&channel, approver, verdict(decided(id)))).is_empty());
            assert!(!followed.approve(id, true).await.unwrap(), "over");
        }
        let request = approval(&channel, agent_id);
        let id = request.id;
        followed.take_in(&from(&channel, agent, ActorEvent::ApprovalRequested(request)));
        let by_other = ActorEvent::ApprovalGranted(decided(id));
        followed.take_in(&from(&channel, other, by_other));
        assert!(followed.approve(id, true).await.unwrap(), "still pending after another's answer");

        let request = approval(&channel, agent_id);
        let id = request.id;
        followed.take_in(&from(&channel, agent, ActorEvent::ApprovalRequested(request)));
        assert!(followed.approve(id, false).await.unwrap(), "answered");
        assert!(!followed.approve(id, true).await.unwrap(), "answered once");
    }

    #[tokio::test]
    async fn an_approval_answered_by_a_rule_of_the_user_is_told_to_them_alone() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let request = approval(&channel, agent_id);
        let id = request.id;
        let agent = ActorId::Agent(agent_id);
        followed.take_in(&from(&channel, agent, ActorEvent::ApprovalRequested(request)));
        let answer = |by: ActorId, by_rule: Option<ApprovalRuleId>| {
            let name = by.to_string();
            let decision = ApprovalDecision { by, name, by_rule, at: Default::default() };
            let response = ApprovalResponse {
                decisions: vec![decision],
                ..ApprovalResponse::new(id, agent_id)
            };
            from(&channel, by, ActorEvent::ApprovalGranted(response))
        };
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
        assert!(!followed.approve(id, true).await.unwrap(), "answered once, as by the user");
    }

    /// A question of `agent_id` in `channel`, put to `addressee`.
    fn asked_of(channel: &ChannelLoop, agent_id: AgentId, addressee: ActorId) -> QuestionRequest {
        QuestionRequest {
            id: QuestionId::new(),
            session_id: channel.session_id(),
            channel_id: channel.id(),
            agent_id,
            tool_call: tool_call(),
            questions: Questions {
                questions: vec![Question {
                    header: "Name".to_string(),
                    question: "Who?".to_string(),
                    choices: Vec::new(),
                    multi_select: false,
                }],
                ask: vec![Addressee::Actor(addressee)],
                sensitive: false,
            },
        }
    }

    #[tokio::test]
    async fn questions_asked_anew_are_shown_once_and_open_until_closed() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let request = asked_of(&channel, agent_id, me);
        let id = request.id;
        let asked = from(&channel, agent, ActorEvent::QuestionRequested(request));
        assert!(!followed.take_in(&asked).is_empty());
        assert!(followed.take_in(&asked).is_empty(), "asked anew");
        assert!(followed.is_open(id));
        assert_eq!(followed.questions_put_to_user("").map(|q| q.id), Some(id), "the last");
        assert_eq!(followed.questions_put_to_user(&id.0.to_string()).map(|q| q.id), Some(id));
        followed.take_in(&from(&channel, agent, ActorEvent::QuestionClosed { id, shown_to: None }));
        assert!(!followed.is_open(id));
        assert!(followed.questions_put_to_user("").is_none());
    }

    /// What `/answer` and `/decline` name: questions put to the user alone, by id or the last.
    #[tokio::test]
    async fn only_questions_put_to_the_user_are_named_for_answer_and_decline() {
        let (mut followed, channel, me) = followed().await;
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let mine = asked_of(&channel, agent_id, me);
        let elsewhere = asked_of(&channel, agent_id, ActorId::User(UserId::new()));
        let (id, other_id) = (mine.id, elsewhere.id);
        for request in [mine, elsewhere] {
            followed.take_in(&from(&channel, agent, ActorEvent::QuestionRequested(request)));
        }
        assert!(followed.is_open(other_id), "open, though put to another");
        let put_to_user = |arg: &str| followed.questions_put_to_user(arg).map(|q| q.id);
        assert_eq!(put_to_user(&other_id.0.to_string()), None, "put to another");
        assert_eq!(put_to_user(""), Some(id), "the last put to the user, asked before");
        assert_eq!(put_to_user(&id.0.to_string()), Some(id));
        assert_eq!(put_to_user("no id"), None);
    }

    #[tokio::test]
    async fn retry_asks_for_the_turn_under_way_or_given_up_before_the_last_answer() {
        let (mut followed, channel, _) = followed().await;
        let agent = ActorId::Agent(AgentId::new());
        assert!(!followed.retry().await.unwrap(), "nothing to give again");
        let refused = |e: Error| e.downcast::<ChannelError>().expect("refused by the channel");

        let said = ActorEvent::CompleteMessageGenerated(message(agent, "an answer"));
        followed.take_in(&from(&channel, agent, said));
        let error = refused(followed.retry().await.unwrap_err());
        assert!(matches!(error, ChannelError::MessageNotFound { .. }), "the last answer: {error}");

        let turn_id = TurnId::new();
        followed.take_in(&from(&channel, agent, ActorEvent::ResponseStarted { turn_id }));
        let error = refused(followed.retry().await.unwrap_err());
        assert!(matches!(error, ChannelError::NotAParticipant { .. }), "the turn: {error}");

        followed.take_in(&from(
            &channel,
            agent,
            ActorEvent::TurnFinished { turn_id, messages: Vec::new() },
        ));
        let error = refused(followed.retry().await.unwrap_err());
        assert!(matches!(error, ChannelError::MessageNotFound { .. }), "the last again: {error}");

        let given_up = ActorEvent::TurnGivenUp {
            turn_id: TurnId::new(),
            reason: GivenUpReason::Failed { message: "unreachable".to_string() },
        };
        followed.take_in(&from(&channel, agent, given_up));
        let error = refused(followed.retry().await.unwrap_err());
        assert!(matches!(error, ChannelError::NotAParticipant { .. }), "given up: {error}");

        let said = ActorEvent::CompleteMessageGenerated(message(agent, "another answer"));
        followed.take_in(&from(&channel, agent, said));
        let error = refused(followed.retry().await.unwrap_err());
        assert!(matches!(error, ChannelError::MessageNotFound { .. }), "said since: {error}");
    }

    #[tokio::test]
    async fn a_channel_is_followed_only_by_who_takes_part() {
        let (manager, session_loop, ctx) = running().await;
        let channel = session_loop.channels().into_iter().next().expect("the main channel");
        let views_of = |ctx: SecurityContext| manager.views(Viewer::new(ctx));
        let stranger = SecurityContext::new(
            ActorId::User(UserId::new()),
            session_loop.id(),
            Default::default(),
        );
        assert!(Followed::new(&channel, views_of(stranger)).is_none(), "a stranger");
        assert!(Followed::new(&channel, views_of(SecurityContext::system())).is_none(), "no user");
        let followed = Followed::new(&channel, views_of(ctx)).expect("the owner takes part");
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

        let channel = session_loop.create_channel(&owner, side("left")).await.unwrap();
        let followed = Followed::new(&channel, views.clone()).expect("they take part");
        let mut watch = followed.actor().watch();
        assert_eq!(followed.ended().await, None, "taking part");
        channel.leave(&theirs, other).await.expect("they may leave");
        assert!(ended(&mut watch, Duration::from_secs(5)).await, "their actor stopped");
        assert!(followed.ended().await.is_some(), "left");

        let channel = session_loop.create_channel(&owner, side("removed")).await.unwrap();
        let followed = Followed::new(&channel, views.clone()).expect("they take part");
        let mut watch = followed.actor().watch();
        channel.remove_participant(&owner, other).await.expect("the owner may remove them");
        assert!(ended(&mut watch, Duration::from_secs(5)).await, "their actor stopped");
        assert!(followed.ended().await.is_some(), "removed");

        let channel = session_loop.create_channel(&owner, side("deleted")).await.unwrap();
        let followed = Followed::new(&channel, views.clone()).expect("they take part");
        let mut watch = followed.actor().watch();
        assert!(session_loop.delete_channel(&owner, channel.id()).await.unwrap());
        assert!(ended(&mut watch, Duration::from_secs(5)).await, "their actor stopped");
        assert!(followed.ended().await.is_some(), "deleted");
    }
}
