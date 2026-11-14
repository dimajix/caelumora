//! What the terminal does of a line the user typed (issues 28, 570): a command of its own carried
//! out, questions put to the user answered, or what is left sent to the channel talked in.

use crate::channels::Channels;
use crate::followed::Followed;
use crate::printed::{ErrorKind, Printed};
use crate::screen::question_line;
use anyhow::Result;
use caelumora_domain::approval::ApprovalId;
use caelumora_domain::command::{parse_input, UserInput};
use caelumora_domain::question::{Question, QuestionRequest, Response};
use caelumora_kernel::actor::user::UserActor;
use caelumora_kernel::error::user_message;
use std::sync::Arc;
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

/// Does what the user typed, `input`, and answers what the terminal prints of it: switches to
/// another of the `channels` (`/switch`), answers questions put to the user (`answer_line`),
/// answers an approval in the channel where it is pending (`/approve`, `/deny`), or in the channel
/// talked in asks for an answer again (`/retry`) or sends a command or a message (issue 28).
///
/// # Errors
/// As [`send_line`].
pub(crate) async fn handle_line(
    input: &str,
    channels: &mut Channels,
    answering: &mut Option<(QuestionRequest, Vec<Response>)>,
) -> Result<Vec<Printed>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(name) = after(input, "/switch") {
        return Ok(channels.switch(name).await);
    }
    if let Some(printed) = answer_line(input, channels, answering).await {
        return Ok(printed);
    }
    if let Some(rest) = input.strip_prefix("/approve ").or_else(|| input.strip_prefix("/deny ")) {
        let approved = input.starts_with("/approve ");
        let Ok(id) = Uuid::parse_str(rest.trim()).map(ApprovalId) else {
            return Ok(vec![Printed::Plain(format!("Invalid approval id: {}", rest.trim()))]);
        };
        let Some(followed) = channels.awaiting(id) else {
            return Ok(vec![Printed::Plain(format!("No pending approval with id {}", id.0))]);
        };
        let actor = match acting(followed) {
            Ok(actor) => actor,
            Err(no_run) => return Ok(vec![no_run]),
        };
        let failed = actor.send_approval(id, approved).await.err();
        return Ok(failed.map(|e| error_printed(&e)).into_iter().collect());
    }
    let Some(followed) = channels.talked_in() else {
        return Ok(vec![channels.hint()]);
    };
    let actor = match acting(followed) {
        Ok(actor) => actor,
        Err(no_run) => return Ok(vec![no_run]),
    };
    if input == "/retry" {
        return Ok(match followed.retry(&actor).await {
            Ok(true) => Vec::new(),
            Ok(false) => vec![Printed::Plain("No answer of an agent to give again".to_string())],
            Err(e) => vec![error_printed(&e)],
        });
    }
    // Every other command goes to the channel.
    send_line(input, &actor).await
}

/// Answers `input` if it is about questions put to the user, with what the terminal prints of it:
/// declines them (`/decline`), begins to answer them (`/answer`) - questions named by id in
/// whichever of the `channels` knows them, else the last in the channel talked in, as the user's
/// actor there tells -, or answers the next of the questions being `answering`, in the channel
/// that asked them; `None` if it is about none. Between two runs of the user's actor in that
/// channel, only that nothing can be done there now.
async fn answer_line(
    input: &str,
    channels: &Channels,
    answering: &mut Option<(QuestionRequest, Vec<Response>)>,
) -> Option<Vec<Printed>> {
    let none_named = |rest: &str| {
        Printed::Plain(match rest.trim() {
            "" => "No open questions put to you".to_string(),
            id => format!("No open questions put to you with id {id}"),
        })
    };
    // The questions `rest` names, with the user's actor in the channel that asked them - which
    // tells which are open -; what tells why, if there are none, or no run of it.
    let named = |rest: &str| -> Result<(QuestionRequest, Arc<UserActor>), Printed> {
        let asking = channels.asking(rest).ok_or_else(|| none_named(rest))?;
        let actor = acting(asking)?;
        let request = asking.questions_put_to_user(rest).ok_or_else(|| none_named(rest))?;
        Ok((request, actor))
    };
    if let Some(rest) = after(input, "/decline") {
        let printed = match named(rest) {
            Ok((request, actor)) => {
                *answering = None;
                actor.answer_questions(request.id, None).await.err().map(|e| error_printed(&e))
            }
            Err(printed) => Some(printed),
        };
        return Some(printed.into_iter().collect());
    }
    if let Some(rest) = after(input, "/answer") {
        let printed = match named(rest) {
            Ok((request, _)) => {
                let first = request.questions.questions.first();
                let prompt = first.map(|first| Printed::Plain(prompt_of(0, first)));
                if prompt.is_some() {
                    *answering = Some((request, Vec::new()));
                }
                prompt
            }
            Err(printed) => Some(printed),
        };
        return Some(printed.into_iter().collect());
    }
    // While questions are answered, each other line answers the next.
    let (request, mut responses) = answering.take()?;
    if let Some(question) = request.questions.questions.get(responses.len()) {
        responses.push(response_of(question, input));
    }
    if let Some(next) = request.questions.questions.get(responses.len()) {
        let prompt = Printed::Plain(prompt_of(responses.len(), next));
        *answering = Some((request, responses));
        return Some(vec![prompt]);
    }
    // Closed with its channel, if it is followed no more - which `talk` tells as it drops it.
    let Some(followed) = channels.followed(request.channel_id) else {
        return Some(vec![closed_before_answered(&request)]);
    };
    let printed = match acting(followed) {
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
            *answering = Some((request, responses));
            printed
        }
    };
    Some(printed)
}

/// What tells that the questions of `request` closed before the user answered them.
pub(crate) fn closed_before_answered(request: &QuestionRequest) -> Printed {
    Printed::Notice(format!("Questions {} closed before you answered", request.id.0))
}

/// What follows `command` at the start of `input`, if `input` begins with it as a word of its own.
fn after<'a>(input: &'a str, command: &str) -> Option<&'a str> {
    input.strip_prefix(command).filter(|rest| rest.is_empty() || rest.starts_with(' '))
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

/// Sends what the user typed, `input`, to the channel as a command or a message, through
/// `actor`, the user's in it, and answers what the terminal prints of it.
///
/// # Errors
/// If a message cannot be sent into the channel.
async fn send_line(input: &str, actor: &UserActor) -> Result<Vec<Printed>> {
    match parse_input(input) {
        UserInput::Command(line) => Ok(match actor.execute_command(&line).await {
            // Also for an answer to the whole channel: its announcement is not routed back to its
            // sender.
            Ok(reply) => reply.output.map(Printed::Plain).into_iter().collect(),
            // The line is not logged: what follows the command may be what was said. The error
            // names the command.
            Err(e) => vec![error_printed(&e)],
        }),
        UserInput::Message(text) => {
            let span_id = actor.send_message(Some(text), None, Vec::new()).await?;
            tracing::debug!(span_id = %span_id, "Message sent");
            Ok(Vec::new())
        }
    }
}

/// `error` as the user may be told it (`user_message`); what may not be told stays in the log,
/// and the user is told where to find it.
fn error_printed(error: &anyhow::Error) -> Printed {
    let message = user_message(error).unwrap_or_else(|| {
        tracing::warn!("Failed: {:#}", error);
        "It failed; the details are in the log".to_string()
    });
    Printed::Error { kind: ErrorKind::Failed, message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::followed::testing::from;
    use crate::screen::testing::lines;
    use caelumora_domain::actor::{ActorEvent, ActorId};
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::question::Choice;
    use caelumora_kernel::session::testing::{running_session_of_a_user, test_kernel};
    use caelumora_kernel::session::{SessionLoop, SessionManager};
    use caelumora_kernel::view::Viewer;

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

    /// What is being answered: nothing.
    type Answering = Option<(QuestionRequest, Vec<Response>)>;

    /// The channels of a running session as its owner follows them, who takes part in its first
    /// channel alone, with the session and the owner.
    async fn followed_alone() -> (Channels, Arc<SessionLoop>, ActorId) {
        let kernel = test_kernel();
        let manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
        let (session_loop, owner, _) = running_session_of_a_user(&manager).await;
        let me = owner.caller_id;
        let views = manager.views(Viewer::new(owner));
        let channels = Channels::new(Arc::clone(&session_loop), views, kernel.event_bus())
            .await
            .expect("the owner takes part");
        (channels, session_loop, me)
    }

    /// What the terminal prints of `input` typed in `channels`, while `answering`.
    async fn typed(channels: &mut Channels, answering: &mut Answering, input: &str) -> Vec<String> {
        lines(&handle_line(input, channels, answering).await.expect("handled"))
    }

    /// What the terminal prints of a line typed where it cannot do what the line asks: an
    /// approval named badly or not pending, nothing to give again, no questions to answer, no
    /// such channel.
    #[tokio::test]
    async fn a_line_that_cannot_be_done_is_told_why() {
        let (mut channels, _, _) = followed_alone().await;
        let (channels, answering) = (&mut channels, &mut None);
        assert!(typed(channels, answering, "").await.is_empty());
        let invalid = typed(channels, answering, "/approve nope").await;
        assert_eq!(invalid, ["Invalid approval id: nope"]);
        let id = Uuid::new_v4();
        let not_pending = typed(channels, answering, &format!("/deny {id}")).await;
        assert_eq!(not_pending, [format!("No pending approval with id {id}")]);
        let retried = typed(channels, answering, "/retry").await;
        assert_eq!(retried, ["No answer of an agent to give again"]);
        let none = typed(channels, answering, "/answer").await;
        assert_eq!(none, ["No open questions put to you"]);
        let none_named = typed(channels, answering, &format!("/decline {id}")).await;
        assert_eq!(none_named, [format!("No open questions put to you with id {id}")]);
        let switched = typed(channels, answering, "/switch nowhere").await;
        let unknown = "You take part in no channel 'nowhere'";
        assert_eq!(switched.first().map(String::as_str), Some(unknown));
    }

    /// Questions put to the user are answered line by line after `/answer`, each line prompted by
    /// the next question, and are no longer being answered once the last is.
    #[tokio::test]
    async fn questions_are_answered_line_by_line() {
        let (mut channels, session_loop, me) = followed_alone().await;
        let channel_id = channels.talked_in().expect("talked in").channel_id();
        let channel = session_loop.channel(channel_id).expect("the channel runs");
        let ActorId::User(user) = me else { unreachable!("a user") };
        let actor = channel.user_actor(user).expect("the owner's actor runs");
        let agent = AgentId::new();
        let request = make_question_request(session_loop.id(), channel_id, agent, me);
        let asked = ActorEvent::QuestionRequested(request.clone());
        actor.take_in_as_routed(from(&channel, ActorId::Agent(agent), asked)).await;

        let (channels, answering) = (&mut channels, &mut None);
        let prompt = typed(channels, answering, "/answer").await;
        assert_eq!(prompt, [prompt_of(0, &request.questions.questions[0])]);
        assert!(answering.is_some(), "being answered");
        let answered = typed(channels, answering, "Kaya").await;
        assert!(answered.is_empty(), "{answered:?}");
        assert!(answering.is_none(), "answered");
    }
}
