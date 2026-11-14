//! What the terminal does of a line the user typed (issues 28, 570): a command of its own carried
//! out, questions put to the user answered, or what is left sent to the channel talked in.

use crate::channels::Channels;
use crate::followed::Followed;
use crate::printed::{ErrorKind, Printed};
use crate::screen::question_line;
use anyhow::Result;
use caelumora_api::command::CommandError;
use caelumora_domain::approval::ApprovalId;
use caelumora_domain::command::{parse_input, Command, UserInput};
use caelumora_domain::question::{Question, QuestionRequest, Response};
use caelumora_domain::value::command_name::CommandName;
use caelumora_kernel::actor::user::UserActor;
use caelumora_kernel::command::help::HELP_COMMAND_NAME;
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

/// A command the terminal carries out itself, named by the rules of the commands of a channel
/// (`docs/COMMANDS.md`, issue 570): grouped by what it acts on, with short forms for what is typed
/// often, no name or short form the beginning of another - nor of a command of the channel, which a
/// line goes to where it begins none of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalCommand {
    /// Ends the program.
    Exit,
    /// Talks in another channel the user takes part in.
    ChannelSwitch,
    /// Asks an agent for its answer again.
    AgentRetry,
    /// Approves a call waiting for approval.
    ApprovalApprove,
    /// Denies a call waiting for approval.
    ApprovalDeny,
    /// Begins to answer questions put to the user.
    QuestionAnswer,
    /// Declines questions put to the user.
    QuestionDecline,
}

impl TerminalCommand {
    /// Every command of the terminal, in the order `/help` lists them.
    const ALL: [TerminalCommand; 7] = [
        TerminalCommand::Exit,
        TerminalCommand::ChannelSwitch,
        TerminalCommand::AgentRetry,
        TerminalCommand::ApprovalApprove,
        TerminalCommand::ApprovalDeny,
        TerminalCommand::QuestionAnswer,
        TerminalCommand::QuestionDecline,
    ];

    /// The command as the commands of a channel are told: its name, short forms, parameters and
    /// what it does.
    fn command(self) -> Command {
        match self {
            // Of no group, as `/help`: ending the program acts on no entity of a channel.
            TerminalCommand::Exit => Command {
                name: CommandName::fixed("exit"),
                aliases: vec![CommandName::fixed("quit")],
                description: "Ends the program, the session shut down in order.".to_string(),
                usage: None,
                owners_only: false,
            },
            TerminalCommand::ChannelSwitch => Command {
                name: CommandName::fixed("channel switch"),
                aliases: vec![CommandName::fixed("switch")],
                description: "Talks in another channel of the session you take part in.".to_string(),
                usage: Some("<channel>".to_string()),
                owners_only: false,
            },
            TerminalCommand::AgentRetry => Command {
                name: CommandName::fixed("agent retry"),
                aliases: vec![CommandName::fixed("retry")],
                description: "Asks for the answer under way, the turn given up, or the last answer of an agent again.".to_string(),
                usage: None,
                owners_only: false,
            },
            TerminalCommand::ApprovalApprove => Command {
                name: CommandName::fixed("approval approve"),
                aliases: vec![CommandName::fixed("approve")],
                description: "Approves the call waiting for approval of that id.".to_string(),
                usage: Some("<id>".to_string()),
                owners_only: false,
            },
            TerminalCommand::ApprovalDeny => Command {
                name: CommandName::fixed("approval deny"),
                aliases: vec![CommandName::fixed("deny")],
                description: "Denies the call waiting for approval of that id.".to_string(),
                usage: Some("<id>".to_string()),
                owners_only: false,
            },
            TerminalCommand::QuestionAnswer => Command {
                name: CommandName::fixed("question answer"),
                aliases: vec![CommandName::fixed("answer")],
                description: "Begins to answer the questions put to you of that id, or the last, one by one.".to_string(),
                usage: Some("[id]".to_string()),
                owners_only: false,
            },
            TerminalCommand::QuestionDecline => Command {
                name: CommandName::fixed("question decline"),
                aliases: vec![CommandName::fixed("decline")],
                description: "Declines the questions put to you of that id, or the last.".to_string(),
                usage: Some("[id]".to_string()),
                owners_only: false,
            },
        }
    }

    /// The command of the terminal the command line `line` - without its slash (`parse_input`) -
    /// begins with, and its arguments; `None` for a command of the channel.
    fn find(line: &str) -> Option<(TerminalCommand, &str)> {
        TerminalCommand::ALL
            .into_iter()
            .find_map(|terminal| terminal.command().strip_from(line).map(|args| (terminal, args)))
    }
}

/// What a line typed came to.
pub(crate) enum Handled {
    /// What the terminal prints of it.
    Printed(Vec<Printed>),
    /// The user ends the program (`/exit`).
    Exit,
}

/// Does what the user typed, `input`, and answers what it came to: a command of the terminal
/// carried out - one that takes no arguments refused with them -; while questions are being
/// `answering`, any other line the answer to the next; else the line sent to the channel talked in
/// of the `channels` (`send_line`).
///
/// # Errors
/// As [`send_line`].
pub(crate) async fn handle_line(
    input: &str,
    channels: &mut Channels,
    answering: &mut Option<(QuestionRequest, Vec<Response>)>,
) -> Result<Handled> {
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
            return Ok(Handled::Printed(vec![Printed::Error { kind: ErrorKind::Failed, message }]));
        }
    }
    let printed = match terminal {
        Some((TerminalCommand::Exit, _)) => return Ok(Handled::Exit),
        Some((TerminalCommand::ChannelSwitch, name)) => channels.switch(name).await,
        Some((TerminalCommand::QuestionDecline, id)) => {
            decline_questions(id, channels, answering).await
        }
        Some((TerminalCommand::QuestionAnswer, id)) => begin_answering(id, channels, answering),
        // While questions are answered, each other line answers the next.
        _ if answering.is_some() => answer_next(input, channels, answering).await,
        Some((TerminalCommand::ApprovalApprove, id)) => answer_approval(id, true, channels).await,
        Some((TerminalCommand::ApprovalDeny, id)) => answer_approval(id, false, channels).await,
        Some((TerminalCommand::AgentRetry, _)) => retry(channels).await,
        None => send_line(&parsed, channels).await?,
    };
    Ok(Handled::Printed(printed))
}

/// Answers the approval `id` names - `approved` or not - in the channel of the `channels` where it
/// is pending, and answers what the terminal prints of it.
async fn answer_approval(id: &str, approved: bool, channels: &Channels) -> Vec<Printed> {
    let Ok(id) = Uuid::parse_str(id.trim()).map(ApprovalId) else {
        return vec![Printed::Plain(format!("Invalid approval id: {}", id.trim()))];
    };
    let Some(followed) = channels.awaiting(id) else {
        return vec![Printed::Plain(format!("No pending approval with id {}", id.0))];
    };
    let actor = match acting(followed) {
        Ok(actor) => actor,
        Err(no_run) => return vec![no_run],
    };
    let failed = actor.send_approval(id, approved).await.err();
    failed.map(|e| error_printed(&e)).into_iter().collect()
}

/// Asks, in the channel talked in of the `channels`, for the answer under way, the turn given up
/// or the last answer of an agent again (`Followed::retry`), and answers what the terminal prints
/// of it.
async fn retry(channels: &Channels) -> Vec<Printed> {
    let (followed, actor) = match talking(channels) {
        Ok(talking) => talking,
        Err(printed) => return vec![printed],
    };
    match followed.retry(&actor).await {
        Ok(true) => Vec::new(),
        Ok(false) => vec![Printed::Plain("No answer of an agent to give again".to_string())],
        Err(e) => vec![error_printed(&e)],
    }
}

/// The questions `id` names - by id in whichever of the `channels` knows them, else the last in
/// the channel talked in, as the user's actor there tells -, with that actor; what tells why, if
/// there are none, or no run of it.
fn questions_named(
    id: &str,
    channels: &Channels,
) -> Result<(QuestionRequest, Arc<UserActor>), Printed> {
    let none_named = || {
        Printed::Plain(match id.trim() {
            "" => "No open questions put to you".to_string(),
            id => format!("No open questions put to you with id {id}"),
        })
    };
    let asking = channels.asking(id).ok_or_else(none_named)?;
    let actor = acting(asking)?;
    let request = asking.questions_put_to_user(id).ok_or_else(none_named)?;
    Ok((request, actor))
}

/// Declines the questions `id` names (`questions_named`), and answers what the terminal prints of
/// it; questions being `answering` are so no more.
async fn decline_questions(
    id: &str,
    channels: &Channels,
    answering: &mut Option<(QuestionRequest, Vec<Response>)>,
) -> Vec<Printed> {
    let (request, actor) = match questions_named(id, channels) {
        Ok(named) => named,
        Err(printed) => return vec![printed],
    };
    *answering = None;
    let failed = actor.answer_questions(request.id, None).await.err();
    failed.map(|e| error_printed(&e)).into_iter().collect()
}

/// Begins to answer the questions `id` names (`questions_named`), which are `answering` from now
/// on, and answers the prompt of the first.
fn begin_answering(
    id: &str,
    channels: &Channels,
    answering: &mut Option<(QuestionRequest, Vec<Response>)>,
) -> Vec<Printed> {
    let request = match questions_named(id, channels) {
        Ok((request, _)) => request,
        Err(printed) => return vec![printed],
    };
    let first = request.questions.questions.first();
    let prompt = first.map(|first| Printed::Plain(prompt_of(0, first)));
    if prompt.is_some() {
        *answering = Some((request, Vec::new()));
    }
    prompt.into_iter().collect()
}

/// Takes `input` as the answer to the next of the questions being `answering`, and answers the
/// prompt of the one after - or, the last answered, sends the answers in the channel of the
/// `channels` that asked them. Between two runs of the user's actor there, only that nothing can
/// be done there now, and the last question asked again.
async fn answer_next(
    input: &str,
    channels: &Channels,
    answering: &mut Option<(QuestionRequest, Vec<Response>)>,
) -> Vec<Printed> {
    let Some((request, mut responses)) = answering.take() else { return Vec::new() };
    if let Some(question) = request.questions.questions.get(responses.len()) {
        responses.push(response_of(question, input));
    }
    if let Some(next) = request.questions.questions.get(responses.len()) {
        let prompt = Printed::Plain(prompt_of(responses.len(), next));
        *answering = Some((request, responses));
        return vec![prompt];
    }
    // Closed with its channel, if it is followed no more - which `talk` tells as it drops it.
    let Some(followed) = channels.followed(request.channel_id) else {
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
            *answering = Some((request, responses));
            printed
        }
    }
}

/// What tells that the questions of `request` closed before the user answered them.
pub(crate) fn closed_before_answered(request: &QuestionRequest) -> Printed {
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

/// The channel talked in of the `channels`, with the user's actor in it as it runs now; what
/// tells why not, if it talks in none, or no run of it is under way (`acting`).
fn talking(channels: &Channels) -> Result<(&Followed, Arc<UserActor>), Printed> {
    let followed = channels.talked_in().ok_or_else(|| channels.hint())?;
    Ok((followed, acting(followed)?))
}

/// Sends what the user typed, `input`, to the channel talked in of the `channels` as a command or a
/// message, and answers what the terminal prints of it - of `/help`, what the terminal tells of its
/// own commands (`terminal_help`) after what the channel answers, or in its place where the channel
/// knows nothing of what was asked or no channel is talked in.
///
/// # Errors
/// If a message cannot be sent into the channel.
async fn send_line(input: &UserInput, channels: &Channels) -> Result<Vec<Printed>> {
    let help = match input {
        UserInput::Command(line) => {
            CommandName::fixed(HELP_COMMAND_NAME).strip_from(line).and_then(terminal_help)
        }
        UserInput::Message(_) => None,
    };
    let actor = match talking(channels) {
        Ok((_, actor)) => actor,
        Err(printed) => return Ok(vec![help.map_or(printed, Printed::Plain)]),
    };
    match input {
        UserInput::Command(line) => Ok(match actor.execute_command(line).await {
            // Also for an answer to the whole channel: its announcement is not routed back to its
            // sender.
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

/// What `/help` with `args` tells of the commands of the terminal: all of them without one, the one
/// named by its name or a short form, or those of the group named; `None` if it names none of them.
fn terminal_help(args: &str) -> Option<String> {
    let wanted = args.trim().trim_start_matches('/');
    let commands = TerminalCommand::ALL.map(TerminalCommand::command);
    if let Some(command) = commands.iter().find(|command| command.strip_from(wanted) == Some("")) {
        return Some(command.help_line());
    }
    let group = match wanted {
        "" => None,
        _ => Some(CommandName::new(wanted.split_whitespace().collect::<Vec<_>>().join(" ")).ok()?),
    };
    let in_group = |command: &&Command| group.as_ref().is_none_or(|g| g.begins(&command.name));
    let listed: Vec<String> = commands
        .iter()
        .filter(in_group)
        .map(|command| format!("- {}", command.help_line()))
        .collect();
    (!listed.is_empty()).then(|| format!("Commands of this terminal:\n\n{}", listed.join("\n")))
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
        match handle_line(input, channels, answering).await.expect("handled") {
            Handled::Printed(printed) => lines(&printed),
            Handled::Exit => panic!("{input} ends the program"),
        }
    }

    /// The commands of the terminal are found in a command line by their names and short forms,
    /// as a command of the channel is; a command of the channel begins none of them.
    #[test]
    fn a_command_of_the_terminal_is_found_by_its_names() {
        let switch = Some((TerminalCommand::ChannelSwitch, "side"));
        assert_eq!(TerminalCommand::find("channel   switch side"), switch);
        assert_eq!(TerminalCommand::find("switch side"), switch, "its short form");
        assert_eq!(TerminalCommand::find("quit"), Some((TerminalCommand::Exit, "")));
        assert_eq!(TerminalCommand::find("retry"), Some((TerminalCommand::AgentRetry, "")));
        assert_eq!(TerminalCommand::find("approvals"), None, "a word of its own");
        assert_eq!(TerminalCommand::find("channel list"), None, "the channel's");
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

    /// No name or short form of the terminal's commands begins another, nor one of the commands
    /// of the channel the user may see.
    #[tokio::test]
    async fn the_terminal_s_commands_collide_with_none() {
        let commands = TerminalCommand::ALL.map(TerminalCommand::command);
        for (index, command) in commands.iter().enumerate() {
            let names: Vec<_> = command.names().collect();
            for name in &names {
                assert!(CommandName::new(name.as_str()).is_ok(), "/{name} is a command name");
            }
            for (at, name) in names.iter().enumerate() {
                assert!(
                    names[at + 1..].iter().all(|other| !name.begins(other) && !other.begins(name)),
                    "/{name} beside the other names of /{}",
                    command.name
                );
            }
            for other in &commands[index + 1..] {
                assert!(!command.collides_with(other), "/{} and /{}", command.name, other.name);
            }
        }
        let (channels, _, _) = followed_alone().await;
        let actor = channels.talked_in().and_then(Followed::actor).expect("the owner's actor runs");
        let of_channel = actor.visible_commands().await;
        assert!(!of_channel.is_empty(), "the channel's commands");
        for command in &commands {
            let colliding = of_channel.iter().find(|theirs| theirs.collides_with(command));
            assert!(colliding.is_none(), "/{} and the channel's {colliding:?}", command.name);
        }
    }

    /// `/exit` and `/quit` end the program, not given arguments, which they take none of; the bare
    /// words are messages.
    #[tokio::test]
    async fn the_program_ends_by_a_command_not_by_a_word() {
        let (mut channels, _, _) = followed_alone().await;
        let mut answering = None;
        for exit in ["/exit", "/quit"] {
            let handled = handle_line(exit, &mut channels, &mut answering).await.expect("handled");
            assert!(matches!(handled, Handled::Exit), "{exit}");
        }
        let refused = typed(&mut channels, &mut answering, "/exit now").await;
        assert_eq!(refused, ["Error: /exit takes no arguments"]);
        assert!(typed(&mut channels, &mut answering, "exit").await.is_empty(), "said");
    }

    /// `/help` tells the terminal's commands after the channel's; of one named, or of a group of
    /// the terminal alone, what the terminal tells.
    #[tokio::test]
    async fn help_tells_the_terminal_s_commands() {
        let (mut channels, _, _) = followed_alone().await;
        let answering = &mut None;
        let all = typed(&mut channels, answering, "/help").await.join("\n");
        let theirs = all.find("/participant list").expect("the channel's");
        let mine = all.find("Commands of this terminal:").expect("the terminal's");
        assert!(theirs < mine, "after the channel's: {all}");
        assert!(all.contains("- `/channel switch <channel>` (also `/switch`) — Talks"), "{all}");
        let one = typed(&mut channels, answering, "/help switch").await.join("\n");
        assert!(one.starts_with("`/channel switch <channel>` (also `/switch`)"), "{one}");
        let group = typed(&mut channels, answering, "/help approval").await.join("\n");
        assert!(group.contains("/approval approve") && group.contains("/approval deny"), "{group}");
        assert!(!group.contains("/exit"), "{group}");
        let unknown = typed(&mut channels, answering, "/help nothing").await.join("\n");
        assert!(unknown.contains("Unknown command"), "{unknown}");
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
