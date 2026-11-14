//! The talk of the user in the terminal (issues 28, 570, 571): the channels it follows, what a line
//! typed does in them - a command of the terminal carried out, questions put to the user answered,
//! or what is left sent to the channel talked in -, and what they show.

use crate::channels::{Channels, Happened};
use crate::commands::{terminal_help, TerminalCommand};
use crate::followed::Followed;
use crate::printed::{ErrorKind, Printed};
use crate::screen::question_line;
use crate::sessions::{listing, Opening, Sessions, SessionsError};
use crate::under_way::UnderWay;
use anyhow::Result;
use caelumora_api::command::CommandError;
use caelumora_domain::approval::ApprovalId;
use caelumora_domain::command::{parse_input, UserInput};
use caelumora_domain::question::{Question, QuestionRequest, Response};
#[cfg(test)]
use caelumora_domain::session::SessionId;
use caelumora_domain::value::command_name::CommandName;
use caelumora_kernel::actor::user::UserActor;
use caelumora_kernel::command::help::HELP_COMMAND_NAME;
use caelumora_kernel::error::user_message;
use caelumora_kernel::event::EventBus;
use caelumora_kernel::session::SessionLoop;
use caelumora_kernel::view::Views;
use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;

/// What the prompt for question `index` of `question` says.
fn prompt_of(index: usize, question: &Question) -> String {
    let how = if question.choices.is_empty() {
        "your answer"
    } else if question.multi_select {
        "the numbers of your choices, e.g. 1,3, or another answer"
    } else {
        "the number of your choice, or another answer"
    };
    format!("{} Type {how}; '-' skips:", question_line(index, question))
}

/// The response `line` gives to `question`: `-` skips it; for one with
/// choices, numbers of choices - separated by commas, several only with
/// `multi_select` - or a choice's label, else another answer; for one
/// without, the line as it is.
fn response_of(question: &Question, line: &str) -> Response {
    if line == "-" {
        return Response::Declined;
    }
    if question.choices.is_empty() {
        return Response::Text { text: line.to_string() };
    }
    let numbers: Option<Vec<usize>> = line
        .split(',')
        .map(|n| {
            n.trim().parse::<usize>().ok().filter(|n| (1..=question.choices.len()).contains(n))
        })
        .collect();
    let chosen: Option<Vec<String>> = match numbers {
        Some(numbers) => {
            Some(numbers.into_iter().map(|n| question.choices[n - 1].label.clone()).collect())
        }
        None => question.choices.iter().find(|c| c.label == line).map(|c| vec![c.label.clone()]),
    };
    match chosen {
        Some(chosen) => Response::Chosen { chosen, other: None },
        None => Response::Chosen { chosen: Vec::new(), other: Some(line.to_string()) },
    }
}

/// What a line typed came to.
pub(crate) enum Handled {
    /// What the terminal prints of it.
    Printed(Vec<Printed>),
    /// The user ends the program (`/exit`).
    Exit,
}

/// The talk of the user in the terminal (issue 571): the channels of the session it talks in that
/// it follows, the questions put to them being answered, and the sessions of the directory - what
/// a line typed and what a channel shows change.
pub(crate) struct Talk {
    /// The channels followed.
    channels: Channels,
    /// The questions being answered, with the responses given so far: while they are, every line
    /// but a command of the terminal - the approvals and `/retry` excepted - answers the next.
    answering: Option<(QuestionRequest, Vec<Response>)>,
    /// The sessions of the directory, which the commands on sessions reach.
    sessions: Sessions,
}

impl Talk {
    /// Begins the talk of the user `views` are read by in the channels of `session_loop` they take
    /// part in - learning from `bus` of those they join meanwhile -, among the `sessions` of the
    /// directory, and answers what the terminal opens with (`Channels::opening`).
    ///
    /// # Errors
    /// If the user takes part in no channel of the session.
    pub(crate) async fn begin(
        session_loop: Arc<SessionLoop>,
        views: Views,
        bus: Arc<EventBus>,
        sessions: Sessions,
    ) -> Result<(Talk, Vec<Printed>)> {
        let session_id = session_loop.id();
        let mut channels = Channels::new(session_loop, views, bus).await.ok_or_else(|| {
            anyhow::anyhow!("The user takes part in no channel of the session {session_id}")
        })?;
        let opening = channels.opening().await;
        Ok((Talk { channels, answering: None, sessions }, opening))
    }

    /// What happens next in the channels followed (`Channels::next`).
    pub(crate) async fn next(&mut self) -> Happened {
        self.channels.next().await
    }

    /// Takes in what `happened` in the channels followed, and answers what the terminal prints of
    /// it (`Channels::take_in`) - with that the questions being answered closed, where their
    /// channel tells so or is followed no more.
    pub(crate) async fn take_in(&mut self, happened: Happened) -> Vec<Printed> {
        let mut printed = self.channels.take_in(happened).await;
        let channels = &self.channels;
        let closed = |(q, _): &mut (QuestionRequest, Vec<Response>)| {
            channels.followed(q.channel_id).is_none_or(|f| !f.is_to_answer(q.id))
        };
        if let Some((request, _)) = self.answering.take_if(closed) {
            printed.push(closed_before_answered(&request));
        }
        printed
    }

    /// The answers under way in the channel talked in (`Channels::under_way`).
    pub(crate) fn under_way(&self) -> &[UnderWay] {
        self.channels.under_way()
    }

    /// Does what the user typed, `input`, and answers what it came to: a command of the terminal
    /// carried out - one that takes no arguments refused with them, one on the sessions of the
    /// directory asked of them -; while questions are being answered, any other line the answer to
    /// the next; else the line sent to the channel talked in (`send_line`).
    ///
    /// # Errors
    /// As `send_line`.
    pub(crate) async fn take_line(&mut self, input: &str) -> Result<Handled> {
        if input.is_empty() {
            return Ok(Handled::Printed(Vec::new()));
        }
        let parsed = parse_input(input);
        let terminal = match &parsed {
            UserInput::Command(line) => TerminalCommand::find(line),
            UserInput::Message(_) => None,
        };
        if let Some((terminal, args)) = terminal {
            let command = terminal.command();
            if command.usage.is_none() && !args.is_empty() {
                let message = format!("/{} takes no arguments", command.name);
                return Ok(Handled::Printed(vec![Printed::Error {
                    kind: ErrorKind::Failed,
                    message,
                }]));
            }
        }
        let printed = match terminal {
            Some((TerminalCommand::Exit, _)) => return Ok(Handled::Exit),
            Some((TerminalCommand::ChannelSwitch, name)) => self.channels.switch(name).await,
            Some((TerminalCommand::SessionNew, template)) => {
                let template = (!template.is_empty()).then(|| template.to_string());
                self.switch_session(&Opening::Start { template }).await
            }
            Some((TerminalCommand::SessionResume, id)) => {
                let opening = Opening::Resume(id.to_string());
                self.switch_session(&opening).await
            }
            Some((TerminalCommand::SessionList, _)) => self.listed().await,
            Some((TerminalCommand::SessionDelete, id)) => {
                match self.sessions.delete(id, self.channels.session_id()).await {
                    Ok(id) => vec![Printed::Notice(format!("The session {id} is deleted"))],
                    Err(e) => vec![error_printed(&e.into())],
                }
            }
            Some((TerminalCommand::QuestionDecline, id)) => self.decline_questions(id).await,
            Some((TerminalCommand::QuestionAnswer, id)) => self.begin_answering(id),
            // While questions are answered, each other line answers the next.
            _ if self.answering.is_some() => self.answer_next(input).await,
            Some((TerminalCommand::ApprovalApprove, id)) => self.answer_approval(id, true).await,
            Some((TerminalCommand::ApprovalDeny, id)) => self.answer_approval(id, false).await,
            Some((TerminalCommand::AgentRetry, _)) => self.retry().await,
            None => self.send_line(&parsed).await?,
        };
        Ok(Handled::Printed(printed))
    }

    /// What tells of the sessions of the directory, the one spoken in last first, the one talked in
    /// marked as such.
    async fn listed(&self) -> Vec<Printed> {
        match self.sessions.list().await {
            Ok(listed) => {
                let talked_in = self.channels.session_id();
                vec![Printed::Plain(listing(&listed, Some(talked_in)).join("\n"))]
            }
            Err(e) => vec![error_printed(&e.into())],
        }
    }

    /// Talks in the session of the directory that `opening` asks for from now on, its channels
    /// followed in place of those of the session left (`Channels::of_session`), the questions of
    /// the session left that are being answered so no more, and the session left stopped; answers
    /// what tells so, as the terminal opens with. Where the session cannot be opened, or the user
    /// takes part in none of its channels, or it is the one talked in already, the talk stays where
    /// it is, and the user is told why.
    async fn switch_session(&mut self, opening: &Opening) -> Vec<Printed> {
        let left = self.channels.session_id();
        let (session, told) = match self.sessions.open(opening).await {
            Ok(opened) => opened,
            Err(e) => return vec![error_printed(&e.into())],
        };
        let id = session.id();
        if id == left {
            return vec![Printed::Notice(format!("You talk in the session {id} already"))];
        }
        let Some(mut followed) = self.channels.of_session(session).await else {
            if let Err(e) = self.sessions.stop(id).await {
                let e = anyhow::Error::from(e);
                warn!(
                    "The session {id} the terminal did not switch to could not be stopped: {e:#}"
                );
            }
            let message = format!("You take part in no channel of the session {id}");
            return vec![Printed::Error { kind: ErrorKind::Failed, message }];
        };
        let mut printed: Vec<Printed> = told.into_iter().collect();
        printed.push(Printed::Notice(format!("Talking in the session {id} now")));
        printed.extend(followed.opening().await);
        self.channels = followed;
        self.answering = None;
        if let Err(e) = self.sessions.stop(left).await {
            printed.push(error_printed(&e.into()));
        }
        printed
    }

    /// Answers the approval `id` names - `approved` or not - in the channel followed where it is
    /// pending, and answers what the terminal prints of it.
    async fn answer_approval(&self, id: &str, approved: bool) -> Vec<Printed> {
        let Ok(id) = Uuid::parse_str(id.trim()).map(ApprovalId) else {
            return vec![Printed::Plain(format!("Invalid approval id: {}", id.trim()))];
        };
        let Some(followed) = self.channels.awaiting(id) else {
            return vec![Printed::Plain(format!("No pending approval with id {}", id.0))];
        };
        let actor = match acting(followed) {
            Ok(actor) => actor,
            Err(no_run) => return vec![no_run],
        };
        let failed = actor.send_approval(id, approved).await.err();
        failed.map(|e| error_printed(&e)).into_iter().collect()
    }

    /// Asks, in the channel talked in, for the answer under way, the turn given up or the last
    /// answer of an agent again (`Followed::retry`), and answers what the terminal prints of it.
    async fn retry(&self) -> Vec<Printed> {
        let (followed, actor) = match self.talking() {
            Ok(talking) => talking,
            Err(printed) => return vec![printed],
        };
        match followed.retry(&actor).await {
            Ok(true) => Vec::new(),
            Ok(false) => vec![Printed::Plain("No answer of an agent to give again".to_string())],
            Err(e) => vec![error_printed(&e)],
        }
    }

    /// The questions `id` names - by id in whichever channel followed knows them, else the last in
    /// the channel talked in, as the user's actor there tells -, with that actor; what tells why,
    /// if there are none, or no run of it.
    fn questions_named(&self, id: &str) -> Result<(QuestionRequest, Arc<UserActor>), Printed> {
        let none_named = || {
            Printed::Plain(match id.trim() {
                "" => "No open questions put to you".to_string(),
                id => format!("No open questions put to you with id {id}"),
            })
        };
        let asking = self.channels.asking(id).ok_or_else(none_named)?;
        let actor = acting(asking)?;
        let request = asking.questions_put_to_user(id).ok_or_else(none_named)?;
        Ok((request, actor))
    }

    /// Declines the questions `id` names (`questions_named`), and answers what the terminal prints
    /// of it; questions being answered are so no more.
    async fn decline_questions(&mut self, id: &str) -> Vec<Printed> {
        let (request, actor) = match self.questions_named(id) {
            Ok(named) => named,
            Err(printed) => return vec![printed],
        };
        self.answering = None;
        let failed = actor.answer_questions(request.id, None).await.err();
        failed.map(|e| error_printed(&e)).into_iter().collect()
    }

    /// Begins to answer the questions `id` names (`questions_named`), which are being answered from
    /// now on, and answers the prompt of the first.
    fn begin_answering(&mut self, id: &str) -> Vec<Printed> {
        let request = match self.questions_named(id) {
            Ok((request, _)) => request,
            Err(printed) => return vec![printed],
        };
        let first = request.questions.questions.first();
        let prompt = first.map(|first| Printed::Plain(prompt_of(0, first)));
        if prompt.is_some() {
            self.answering = Some((request, Vec::new()));
        }
        prompt.into_iter().collect()
    }

    /// Takes `input` as the answer to the next of the questions being answered, and answers the
    /// prompt of the one after - or, the last answered, sends the answers in the channel that asked
    /// them. Between two runs of the user's actor there, only that nothing can be done there now,
    /// and the last question asked again.
    async fn answer_next(&mut self, input: &str) -> Vec<Printed> {
        let Some((request, mut responses)) = self.answering.take() else { return Vec::new() };
        if let Some(question) = request.questions.questions.get(responses.len()) {
            responses.push(response_of(question, input));
        }
        if let Some(next) = request.questions.questions.get(responses.len()) {
            let prompt = Printed::Plain(prompt_of(responses.len(), next));
            self.answering = Some((request, responses));
            return vec![prompt];
        }
        // Closed with its channel, if it is followed no more - which `take_in` tells as it drops
        // them.
        let Some(followed) = self.channels.followed(request.channel_id) else {
            return vec![closed_before_answered(&request)];
        };
        match acting(followed) {
            Ok(actor) => {
                let failed = actor.answer_questions(request.id, Some(responses)).await.err();
                failed.map(|e| error_printed(&e)).into_iter().collect()
            }
            // The last answer is typed again once the user's actor runs.
            Err(no_run) => {
                responses.pop();
                let mut printed = vec![no_run];
                if let Some(question) = request.questions.questions.get(responses.len()) {
                    printed.push(Printed::Plain(prompt_of(responses.len(), question)));
                }
                self.answering = Some((request, responses));
                printed
            }
        }
    }

    /// The channel talked in, with the user's actor in it as it runs now; what tells why not, if it
    /// talks in none, or no run of it is under way (`acting`).
    fn talking(&self) -> Result<(&Followed, Arc<UserActor>), Printed> {
        let followed = self.channels.talked_in().ok_or_else(|| self.channels.hint())?;
        Ok((followed, acting(followed)?))
    }

    /// Sends what the user typed, `input`, to the channel talked in as a command or a message, and
    /// answers what the terminal prints of it - of `/help`, what the terminal tells of its own
    /// commands (`terminal_help`) after what the channel answers, or in its place where the channel
    /// knows nothing of what was asked or no channel is talked in.
    ///
    /// # Errors
    /// If a message cannot be sent into the channel.
    async fn send_line(&self, input: &UserInput) -> Result<Vec<Printed>> {
        let help = match input {
            UserInput::Command(line) => {
                CommandName::fixed(HELP_COMMAND_NAME).strip_from(line).and_then(terminal_help)
            }
            UserInput::Message(_) => None,
        };
        let actor = match self.talking() {
            Ok((_, actor)) => actor,
            Err(printed) => return Ok(vec![help.map_or(printed, Printed::Plain)]),
        };
        match input {
            UserInput::Command(line) => Ok(match actor.execute_command(line).await {
                // Also for an answer to the whole channel: its announcement is not routed back to
                // its sender.
                Ok(reply) => told(reply.output, help),
                Err(e) if help.is_some() && is_unknown(&e) => told(None, help),
                // The line is not logged: what follows the command may be what was said. The error
                // names the command.
                Err(e) => vec![error_printed(&e)],
            }),
            UserInput::Message(text) => {
                let span_id = actor.send_message(Some(text.clone()), None, Vec::new()).await?;
                tracing::debug!(span_id = %span_id, "Message sent");
                Ok(Vec::new())
            }
        }
    }
}

/// What the tests do with the talk past its own API: tell the session it talks in and whether
/// questions are being answered, and reach the sessions of the directory.
#[cfg(test)]
impl Talk {
    /// The session talked in.
    fn session_id(&self) -> SessionId {
        self.channels.session_id()
    }

    /// Whether questions are being answered.
    fn is_answering(&self) -> bool {
        self.answering.is_some()
    }

    /// The sessions of the directory.
    fn sessions(&self) -> &Sessions {
        &self.sessions
    }
}

/// What tells that the questions of `request` closed before the user answered them.
fn closed_before_answered(request: &QuestionRequest) -> Printed {
    Printed::Notice(format!("Questions {} closed before you answered", request.id.0))
}

/// The user's actor in `followed` as it runs now (`Followed::actor`); between two runs of it, what
/// tells the user that nothing can be done there right now.
fn acting(followed: &Followed) -> Result<Arc<UserActor>, Printed> {
    followed.actor().ok_or_else(|| {
        Printed::Notice(format!(
            "No run of yours in channel {} right now: try again in a moment",
            followed.name()
        ))
    })
}

/// What the terminal prints of what a command answered, `output`, and of what `help` tells after
/// it: nothing where there is neither - a command sent to agents answers nothing.
fn told(output: Option<String>, help: Option<String>) -> Vec<Printed> {
    let told: Vec<String> = output.into_iter().chain(help).collect();
    match told.is_empty() {
        true => Vec::new(),
        false => vec![Printed::Plain(told.join("\n\n"))],
    }
}

/// Whether `error` says that the channel knows no command of the line.
fn is_unknown(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| matches!(cause.downcast_ref(), Some(CommandError::Unknown(_))))
}

/// `error` as the user may be told it: of the sessions of the directory (`SessionsError`), what it
/// says - where the kernel failed, with what of its cause may be told -; else what `user_message`
/// tells. What may not be told stays in the log, where the user is told to find it, as does what
/// failed of the kernel; what the user asked amiss is not logged.
fn error_printed(error: &anyhow::Error) -> Printed {
    let of_sessions = error.chain().find_map(|cause| cause.downcast_ref::<SessionsError>());
    let told = user_message(error);
    let message = match (of_sessions, told) {
        (Some(failed @ SessionsError::Failed { .. }), told) => {
            warn!("Failed: {error:#}");
            match told {
                Some(told) => format!("{failed}: {told}"),
                None => failed.to_string(),
            }
        }
        (Some(asked_amiss), _) => asked_amiss.to_string(),
        (None, Some(told)) => told,
        (None, None) => {
            warn!("Failed: {error:#}");
            "It failed; the details are in the log".to_string()
        }
    };
    Printed::Error { kind: ErrorKind::Failed, message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::followed::testing::from;
    use crate::screen::testing::lines;
    use crate::sessions::testing::{in_directory, InDirectory};
    use caelumora_domain::actor::{ActorEvent, ActorId};
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::question::Choice;
    use caelumora_kernel::session::{SessionError, SessionLoop};
    use caelumora_kernel::view::Viewer;
    use caelumora_security::SecurityContext;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::time;

    fn question(choices: &[&str], multi_select: bool) -> Question {
        Question {
            header: "Pick".to_string(),
            question: "Which?".to_string(),
            choices: choices
                .iter()
                .map(|l| Choice { label: l.to_string(), description: None, recommended: false })
                .collect(),
            multi_select,
        }
    }

    #[test]
    fn a_line_answers_a_question_as_it_asks() {
        let chosen = |labels: &[&str]| Response::Chosen {
            chosen: labels.iter().map(|l| l.to_string()).collect(),
            other: None,
        };
        let other =
            |text: &str| Response::Chosen { chosen: Vec::new(), other: Some(text.to_string()) };
        let choices = question(&["Postgres", "SQLite", "None"], true);
        assert_eq!(response_of(&choices, "2"), chosen(&["SQLite"]));
        assert_eq!(response_of(&choices, "1, 3"), chosen(&["Postgres", "None"]));
        assert_eq!(response_of(&choices, "SQLite"), chosen(&["SQLite"]), "by its label");
        assert_eq!(response_of(&choices, "4"), other("4"), "no such choice: another answer");
        assert_eq!(response_of(&choices, "MariaDB"), other("MariaDB"));
        assert_eq!(response_of(&choices, "-"), Response::Declined);
        assert_eq!(
            response_of(&question(&[], false), "Kaya"),
            Response::Text { text: "Kaya".to_string() }
        );
        assert_eq!(response_of(&question(&[], false), "-"), Response::Declined);
    }

    /// What a talk runs in, kept as long as the test runs.
    struct Running {
        /// The owner of the directory, who talks.
        owner: SecurityContext,
        /// The directory.
        _directory: TempDir,
    }

    /// The talk of the owner of a directory in a session of theirs started from its template, the
    /// first channel of it talked in, with the session and what the talk runs in.
    async fn talking() -> (Talk, Arc<SessionLoop>, Running) {
        let InDirectory { sessions, manager, kernel, owner, directory } = in_directory().await;
        let start = Opening::Start { template: None };
        let (session_loop, _) = sessions.open(&start).await.expect("started");
        let views = manager.views(Viewer::new(owner.clone()));
        let bus = Arc::clone(kernel.event_bus());
        let begun = Talk::begin(Arc::clone(&session_loop), views, bus, sessions).await;
        let (talk, _) = begun.expect("the owner takes part");
        (talk, session_loop, Running { owner, _directory: directory })
    }

    /// Questions an agent puts to the owner of `running` in the first channel of `session_loop`,
    /// routed to their actor there as the channel would.
    async fn put_to_the_owner(session_loop: &SessionLoop, running: &Running) -> QuestionRequest {
        let me = running.owner.caller_id;
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        let ActorId::User(user) = me else { unreachable!("a user") };
        let actor = channel.user_actor(user).expect("the owner's actor runs");
        let agent = AgentId::new();
        let request = make_question_request(session_loop.id(), channel.id(), agent, me);
        let asked = ActorEvent::QuestionRequested(request.clone());
        actor.take_in_as_routed(from(&channel, ActorId::Agent(agent), asked)).await;
        request
    }

    /// What the terminal prints of `input` typed in `talk`.
    async fn typed(talk: &mut Talk, input: &str) -> Vec<String> {
        match talk.take_line(input).await.expect("handled") {
            Handled::Printed(printed) => lines(&printed),
            Handled::Exit => panic!("{input} ends the program"),
        }
    }

    /// What a command answered is printed with what the terminal tells after it, nothing where
    /// there is neither.
    #[test]
    fn what_a_command_answered_is_printed_only_where_there_is_something() {
        assert!(told(None, None).is_empty(), "a command sent to agents");
        let both = told(Some("theirs".to_string()), Some("mine".to_string()));
        assert_eq!(lines(&both), ["theirs\n\nmine"]);
        assert_eq!(lines(&told(None, Some("mine".to_string()))), ["mine"]);
    }

    /// `/exit` and `/quit` end the program, not given arguments, which they take none of; the bare
    /// words are messages.
    #[tokio::test]
    async fn the_program_ends_by_a_command_not_by_a_word() {
        let (mut talk, _, _running) = talking().await;
        for exit in ["/exit", "/quit"] {
            let handled = talk.take_line(exit).await.expect("handled");
            assert!(matches!(handled, Handled::Exit), "{exit}");
        }
        let refused = typed(&mut talk, "/exit now").await;
        assert_eq!(refused, ["Error: /exit takes no arguments"]);
        assert!(typed(&mut talk, "exit").await.is_empty(), "said");
    }

    /// `/help` tells the terminal's commands after the channel's; of one named, or of a group of
    /// the terminal alone, what the terminal tells.
    #[tokio::test]
    async fn help_tells_the_terminal_s_commands() {
        let (mut talk, _, _running) = talking().await;
        let all = typed(&mut talk, "/help").await.join("\n");
        let theirs = all.find("/participant list").expect("the channel's");
        let mine = all.find("Commands of this terminal:").expect("the terminal's");
        assert!(theirs < mine, "after the channel's: {all}");
        assert!(all.contains("- `/channel switch <channel>` (also `/switch`) — Talks"), "{all}");
        let one = typed(&mut talk, "/help switch").await.join("\n");
        assert!(one.starts_with("`/channel switch <channel>` (also `/switch`)"), "{one}");
        let group = typed(&mut talk, "/help approval").await.join("\n");
        assert!(group.contains("/approval approve") && group.contains("/approval deny"), "{group}");
        assert!(!group.contains("/exit"), "{group}");
        let unknown = typed(&mut talk, "/help nothing").await.join("\n");
        assert!(unknown.contains("Unknown command"), "{unknown}");
    }

    /// What the terminal prints of a line typed where it cannot do what the line asks: an
    /// approval named badly or not pending, nothing to give again, no questions to answer, no
    /// such channel.
    #[tokio::test]
    async fn a_line_that_cannot_be_done_is_told_why() {
        let (mut talk, _, _running) = talking().await;
        assert!(typed(&mut talk, "").await.is_empty());
        let invalid = typed(&mut talk, "/approve nope").await;
        assert_eq!(invalid, ["Invalid approval id: nope"]);
        let id = Uuid::new_v4();
        let not_pending = typed(&mut talk, &format!("/deny {id}")).await;
        assert_eq!(not_pending, [format!("No pending approval with id {id}")]);
        let retried = typed(&mut talk, "/retry").await;
        assert_eq!(retried, ["No answer of an agent to give again"]);
        let none = typed(&mut talk, "/answer").await;
        assert_eq!(none, ["No open questions put to you"]);
        let none_named = typed(&mut talk, &format!("/decline {id}")).await;
        assert_eq!(none_named, [format!("No open questions put to you with id {id}")]);
        let switched = typed(&mut talk, "/switch nowhere").await;
        let unknown = "You take part in no channel 'nowhere'";
        assert_eq!(switched.first().map(String::as_str), Some(unknown));
    }

    /// Questions put to the user are answered line by line after `/answer`, each line prompted by
    /// the next question, and are no longer being answered once the last is.
    #[tokio::test]
    async fn questions_are_answered_line_by_line() {
        let (mut talk, session_loop, running) = talking().await;
        let request = put_to_the_owner(&session_loop, &running).await;

        let prompt = typed(&mut talk, "/answer").await;
        assert_eq!(prompt, [prompt_of(0, &request.questions.questions[0])]);
        assert!(talk.is_answering(), "being answered");
        let answered = typed(&mut talk, "Kaya").await;
        assert!(answered.is_empty(), "{answered:?}");
        assert!(!talk.is_answering(), "answered");
    }

    /// The commands on the sessions of the directory: listed, the one talked in marked; deleted,
    /// but the one talked in; started and resumed, switching the talk to them.
    #[tokio::test]
    async fn the_sessions_of_the_directory_are_reached_by_commands() {
        let (mut talk, talked_in, running) = talking().await;
        let start = Opening::Start { template: None };
        let (other, _) = talk.sessions().open(&start).await.expect("started");
        // Also while questions are being answered, which stay so.
        put_to_the_owner(&talked_in, &running).await;
        typed(&mut talk, "/answer").await;
        let listed = typed(&mut talk, "/session list").await.join("\n");
        assert!(talk.is_answering(), "still being answered");
        let marked: Vec<&str> =
            listed.lines().filter(|line| line.ends_with(", talked in")).collect();
        assert_eq!(marked.len(), 1, "{listed}");
        assert!(marked[0].starts_with(&talked_in.id().to_string()[..8]), "{listed}");

        let mine = talked_in.id().to_string()[..8].to_string();
        let refused = typed(&mut talk, &format!("/session delete {mine}")).await;
        assert!(refused.join("\n").contains("switch to another"), "{refused:?}");
        let theirs = other.id().to_string()[..8].to_string();
        let deleted = typed(&mut talk, &format!("/session delete {theirs}")).await;
        assert_eq!(deleted, [format!("The session {} is deleted", other.id())]);

        // Switched to another session, the talk follows its channels, the questions being
        // answered dropped, and the one left is stopped; to the one talked in, to none that can be
        // opened, or to one the user takes part in no channel of, it stays where it is.
        let mine = talked_in.id().to_string();
        let same = typed(&mut talk, &format!("/resume {mine}")).await;
        assert!(same.join("\n").contains("already"), "{same:?}");
        let missing = typed(&mut talk, "/resume ffffffff").await;
        assert!(missing.join("\n").contains("No session"), "{missing:?}");
        let empty = typed(&mut talk, "/session new empty").await;
        assert!(empty.join("\n").contains("no channel"), "{empty:?}");
        assert_eq!(talk.session_id(), talked_in.id(), "stays where it is");
        assert!(talk.is_answering(), "still being answered");

        let told = typed(&mut talk, "/new").await.join("\n");
        assert_ne!(talk.session_id(), talked_in.id(), "the other followed");
        assert!(told.contains("Talking in the session") && told.contains("Talking in main now"));
        assert!(!told.contains("Error"), "{told}");
        assert!(!talk.is_answering(), "the questions of the session left");
        // Stopped: opened again, it is started anew, not the one that ran.
        let resumed = Opening::Resume(mine);
        let (again, _) = talk.sessions().open(&resumed).await.expect("opened again");
        assert!(!Arc::ptr_eq(&again, &talked_in), "the one left was stopped");
    }

    /// What the user asked of the sessions amiss is told as it is; what failed of the kernel with
    /// what of its cause may be told, and nothing of one that may not.
    #[test]
    fn what_failed_of_the_sessions_is_told_with_what_of_its_cause_may_be() {
        let told = |error: SessionsError| lines(&[error_printed(&error.into())]);
        let amiss = SessionsError::NoneBeginning("ff".to_string());
        assert_eq!(
            told(amiss),
            [format!("Error: {}", SessionsError::NoneBeginning("ff".to_string()))]
        );
        let id = SessionId::new();
        // As `Sessions` keeps a cause (`SessionsError::failed`): an error of the kernel, become
        // anyhow's by `?`.
        let cause = anyhow::Error::from(SessionError::RunsElsewhere(id));
        let elsewhere = SessionsError::Failed {
            what: format!("The session {id} cannot be deleted"),
            cause: cause.reallocate_into_boxed_dyn_error_without_backtrace(),
        };
        let shown = format!(
            "Error: The session {id} cannot be deleted: {}",
            SessionError::RunsElsewhere(id)
        );
        assert_eq!(told(elsewhere), [shown], "the kernel's cause, meant for users");
        let hidden = SessionsError::Failed {
            what: "The sessions of this directory cannot be read".to_string(),
            cause: "a path, say".into(),
        };
        assert_eq!(told(hidden), ["Error: The sessions of this directory cannot be read"]);
    }

    /// Questions being answered that close meanwhile are told as such as the talk takes in what
    /// the channel shows, and the next line is no answer any more.
    #[tokio::test]
    async fn questions_that_close_while_answered_are_told() {
        let (mut talk, session_loop, running) = talking().await;
        let request = put_to_the_owner(&session_loop, &running).await;
        typed(&mut talk, "/answer").await;
        assert!(talk.is_answering());
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        let ActorId::User(user) = running.owner.caller_id else { unreachable!("a user") };
        let actor = channel.user_actor(user).expect("the owner's actor runs");
        let closed = ActorEvent::QuestionClosed { id: request.id, shown_to: None };
        actor.take_in_as_routed(from(&channel, ActorId::Agent(request.agent_id), closed)).await;
        let told = async {
            loop {
                let happened = talk.next().await;
                let printed = lines(&talk.take_in(happened).await).join("\n");
                if printed.contains("closed before you answered") {
                    break;
                }
            }
        };
        time::timeout(Duration::from_secs(5), told).await.expect("told");
        assert!(!talk.is_answering(), "no longer being answered");
    }
}
