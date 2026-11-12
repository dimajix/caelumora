//! The terminal frontend: a user talks, line by line, in a channel of a session of the directory
//! the program was started in (issue 554) - one started from a template, or one of the directory
//! continued or resumed -; or it is asked what a definition's file is to its store (`id`).

mod here;
mod home;
mod sessions;

use anyhow::Context;
use anyhow::Result;
use caelumora_a2a::library::A2aLibrary;
use caelumora_agents::library::AgentsLibrary;
use caelumora_api::component::Params;
use caelumora_api::library::Library;
use caelumora_artifacts::library::ArtifactsLibrary;
use caelumora_artifacts::plugin::ArtifactsPlugin;
use caelumora_compaction::library::CompactionLibrary;
use caelumora_decision::plugin::DecisionPlugin;
use caelumora_domain::actor::{ActorEvent, ActorId};
use caelumora_domain::agent::AgentId;
use caelumora_domain::approval::{ApprovalId, ApprovalResponse};
use caelumora_domain::command::{parse_input, UserInput};
use caelumora_domain::config::AppConfig;
use caelumora_domain::mention::{is_group_handle, mention_spans};
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::project::ProjectId;
use caelumora_domain::query::Query;
use caelumora_domain::question::{Question, QuestionId, QuestionRequest, Response};
use caelumora_domain::store::StoreResultExt;
use caelumora_domain::turn::GivenUpReason;
use caelumora_domain::user::UserId;
use caelumora_domain::value::language::Language;
use caelumora_domain::work::TurnId;
use caelumora_domain::workspace::{WorkspaceChoice, WorkspaceId};
use caelumora_kernel::channel::ChannelLoop;
use caelumora_kernel::error::user_message;
use caelumora_kernel::kernel::{Kernel, KernelBuilder};
use caelumora_kernel::plugin::Plugin;
use caelumora_kernel::project::ProjectManager;
use caelumora_kernel::session::{SessionError, SessionLoop, SessionManager, SessionSpec};
use caelumora_kernel::user_provider::Credential;
use caelumora_kernel::view::Viewer;
use caelumora_logging::{init, LogTarget};
use caelumora_memory::library::MemoryLibrary;
use caelumora_memory::plugin::MemoryPlugin;
use caelumora_model::plugin::ModelPlugin;
use caelumora_model_comfyui::plugin::ComfyUiModelPlugin;
use caelumora_model_ollama::plugin::{with_local_model, OllamaModelPlugin};
use caelumora_model_systemone::plugin::SystemOneModelPlugin;
use caelumora_open_terminal::plugin::OpenTerminalPlugin;
use caelumora_security::SecurityContext;
use caelumora_skills::library::SkillsLibrary;
use caelumora_skills::plugin::SkillsPlugin;
use caelumora_store_home::plugin::HomeStorePlugin;
use caelumora_store_home::{HomeStore, HomeStoreConfig, HOME};
use caelumora_store_json::plugin::JsonStorePlugin;
use caelumora_store_postgres::plugin::PostgresStorePlugin;
use caelumora_tool_attachment::library::AttachmentToolLibrary;
use caelumora_tool_image::library::ImageToolLibrary;
use caelumora_tool_mcp::library::McpToolLibrary;
use caelumora_tool_web::library::WebToolLibrary;
use caelumora_tool_workspace::library::WorkspaceToolLibrary;
use clap::{Parser, Subcommand};
use here::{place_of, HERE};
use home::Home;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
use serde_json::Value;
use sessions::Asked;
use std::collections::HashMap;
use std::env;
use std::future;
use std::io::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use uuid::Uuid;

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

/// Prints the questions of `request` (issue 165), and - if they are put to
/// `me` - how to answer them.
#[expect(clippy::print_stdout, reason = "the TUI writes the conversation to the terminal")]
fn print_questions(request: &QuestionRequest, me: ActorId) {
    println!("\r--- Questions of {} (id: {}) ---", request.tool_call.function_name, request.id.0);
    for (index, question) in request.questions.questions.iter().enumerate() {
        println!("{}. [{}] {}", index + 1, question.header, question.question);
        for (number, choice) in question.choices.iter().enumerate() {
            let recommended = if choice.recommended { " (recommended)" } else { "" };
            let description =
                choice.description.as_deref().map(|d| format!(" - {d}")).unwrap_or_default();
            println!("   {}) {}{}{}", number + 1, choice.label, recommended, description);
        }
    }
    if request.questions.is_asked(me) {
        if request.questions.sensitive {
            println!("Sensitive: the answers go to the tool alone - but what you type is shown on this terminal.");
        }
        println!("Type '/answer {}' to answer them one by one, or '/decline {}'. A message instead declines them.", request.id.0, request.id.0);
    } else {
        println!("They are put to others.");
    }
}

/// What the prompt for question `index` of `question` says.
fn prompt_of(index: usize, question: &Question) -> String {
    let how = if question.choices.is_empty() {
        "your answer"
    } else if question.multi_select {
        "the numbers of your choices, e.g. 1,3, or another answer"
    } else {
        "the number of your choice, or another answer"
    };
    format!("{}. [{}] {} - {}; '-' skips:", index + 1, question.header, question.question, how)
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

/// The open questions `arg` names by id - or, without one, the last that
/// was put to `me`.
fn questions_named(open: &[QuestionRequest], arg: &str, me: ActorId) -> Option<QuestionRequest> {
    match arg.trim() {
        "" => open.iter().rev().find(|q| q.questions.is_asked(me)).cloned(),
        id => Uuid::parse_str(id)
            .ok()
            .and_then(|id| open.iter().find(|q| q.id == QuestionId(id)).cloned()),
    }
}

#[expect(clippy::print_stdout, reason = "the TUI writes the conversation to the terminal")]
#[expect(clippy::too_many_lines, reason = "to be simplified; older than the lint (issue 452)")]
async fn run_repl(channel: Arc<ChannelLoop>, user: UserId) -> Result<()> {
    let user_actor = channel
        .user_actor(user)
        .ok_or_else(|| anyhow::anyhow!("The user is not a participant of the channel"))?;

    let mut receiver = user_actor.watch();
    let mut input_rx = spawn_rustyline_input();
    // Approval id -> agent id, so `/approve <id>`/`/deny <id>` know which
    // agent to answer. Covers both plan proposals and regular tool calls -
    // both flow through the same generic ApprovalRequested event.
    let mut pending_approvals: HashMap<Uuid, AgentId> = HashMap::new();
    // The last answer of an agent shown, which `/retry` asks it to give
    // again (issue 234) - or, while one is under way, that answer, by the
    // agent and its turn (issue 235); or, after a turn given up that may be
    // answered anew and nothing since, that turn (issue 259).
    let mut last_answer: Option<MessageId> = None;
    let mut under_way: Option<(ActorId, TurnId)> = None;
    let mut given_up: Option<(ActorId, TurnId)> = None;
    // The questions open in the channel (issue 165), in the order they were
    // asked, and those being answered, with the responses given so far:
    // while they are, every line answers the next question.
    let me = ActorId::User(user);
    let mut open_questions: Vec<QuestionRequest> = Vec::new();
    let mut answering: Option<(QuestionRequest, Vec<Response>)> = None;
    let handles: HashMap<ActorId, String> =
        channel.participants().into_iter().filter_map(|m| Some((m.id, m.handle?))).collect();

    println!("--- Chat Session Started (Type 'exit' or 'quit' to stop) ---");
    // A signal ends the loop as `exit` does, so that the session is shut
    // down in order (issue 297).
    let mut signalled = std::pin::pin!(signalled());
    loop {
        tokio::select! {
            () = &mut signalled => {
                println!("\r--- Ending ---");
                break;
            }
            received = receiver.recv() => {
                let maybe_envelope = match received {
                    Ok(envelope) => Some(envelope),
                    Err(RecvError::Lagged(lost)) => {
                        println!("\r[{} events missed]", lost);
                        continue;
                    }
                    Err(RecvError::Closed) => None,
                };
                // What the user said is on the screen already, as typed.
                if maybe_envelope.as_ref().is_some_and(|e| e.context.sender_id == ActorId::User(user)) {
                    continue;
                }
                let sender = maybe_envelope.as_ref().map(|e| e.context.sender_name.clone()).unwrap_or_default();
                let sender_id = maybe_envelope.as_ref().map(|e| e.context.sender_id);
                match maybe_envelope.map(|envelope| envelope.event) {
                    Some(ActorEvent::ResponseStarted { turn_id }) => {
                        under_way = sender_id.filter(|id| matches!(id, ActorId::Agent(_))).map(|id| (id, turn_id));
                    }
                    // Its answer has ended, said or not.
                    Some(ActorEvent::TurnFinished { .. }) if under_way.is_some_and(|(agent, _)| Some(agent) == sender_id) => {
                        under_way = None;
                    }
                    // Each message as soon as it is complete, as the web UI
                    // shows it; the full response that ends an answer holds
                    // them again (issue 220).
                    Some(ActorEvent::CompleteMessageGenerated(message)) => {
                        if matches!(message.sender_id, ActorId::Agent(_)) {
                            last_answer = Some(message.id);
                        }
                        given_up = None;
                        if let Some(said) = said(&message, &handles) {
                            println!("\r{}> {}", message.sender_name, said);
                        }
                    }
                    Some(ActorEvent::IntermediateMessageGenerated(message)) => {
                        if let Some(said) = said(&message, &handles) {
                            println!("\r[working] {}> {}", message.sender_name, said);
                        }
                    }
                    // What another changed of what was said (issue 234).
                    Some(ActorEvent::MessageModified(message)) => {
                        if let Some(said) = said(&message, &handles) {
                            println!("\r{}> [edited] {}", message.sender_name, said);
                        }
                    }
                    Some(ActorEvent::MessageRevoked { .. }) => {
                        println!("\r{} took a message back", sender);
                    }
                    Some(ActorEvent::RetryAccepted { .. }) => {
                        println!("\r{} answers again", sender);
                    }
                    Some(ActorEvent::RetryRejected { reason, .. }) => {
                        println!("\r{} cannot answer again: {}", sender, reason);
                    }
                    // What this user sent that the channel did not let in
                    // (issue 304), told to them alone.
                    Some(ActorEvent::MessageRejected { rejected, reason, .. }) => {
                        println!("\r[refused] the channel did not let in your {}: {}", rejected, reason);
                    }
                    Some(ActorEvent::TurnGivenUp { turn_id, reason }) => {
                        if under_way.is_some_and(|(agent, _)| Some(agent) == sender_id) {
                            under_way = None;
                        }
                        given_up = sender_id.filter(|id| matches!(id, ActorId::Agent(_)) && reason.may_be_answered_again()).map(|id| (id, turn_id));
                        match reason {
                            GivenUpReason::Failed { message } => println!("\r[error] {} could not answer: {}", sender, message),
                            GivenUpReason::Lowered { channel } => println!("\r{} gave its answer up: the channel was lowered to {}", sender, channel),
                            GivenUpReason::CallLimit { calls } => println!("\r{} gave its answer up: it asked its model {} times, as often as one answer or one step of a plan may", sender, calls),
                            GivenUpReason::Left | GivenUpReason::Retried => println!("\r{} gave its answer up: {}", sender, reason.cause()),
                        }
                    }
                    Some(ActorEvent::CommandStarted { command, .. }) => {
                        // Told to its issuer alone (issue 299).
                        println!("\r{} is carrying out /{}...", sender, command);
                    }
                    Some(ActorEvent::CommandExecuted { command, output, turn_id, .. }) => {
                        // Carried out by an agent in a turn of its own (issue 269), or run by a
                        // user.
                        let verb = if turn_id.is_some() { "carried out" } else { "ran" };
                        println!("\r{} {} /{}: {}", sender, verb, command, output);
                    }
                    Some(ActorEvent::CommandFailed { command, reason, .. }) => {
                        println!("\r{} did not carry out /{}: {}", sender, command, reason);
                    }
                    Some(ActorEvent::ActivityStarted { activity, .. }) => {
                        // Of its own accord (issue 284).
                        println!("\r{} is {}...", sender, activity);
                    }
                    Some(ActorEvent::ActivityFinished { output, .. }) => {
                        println!("\r{}", output);
                    }
                    // Who joined or left (issue 408), by whose act.
                    Some(ActorEvent::ParticipantJoined { participant, name }) => {
                        match sender_id {
                            Some(by) if by == participant => println!("\r{} joined the channel", name),
                            Some(ActorId::System) => println!("\r{} was added to the channel", name),
                            _ => println!("\r{} added {} to the channel", sender, name),
                        }
                    }
                    Some(ActorEvent::ParticipantLeft { participant, name, .. }) => {
                        match sender_id {
                            Some(by) if by == participant => println!("\r{} left the channel", name),
                            Some(ActorId::System) => println!("\r{} was removed from the channel", name),
                            _ => println!("\r{} removed {} from the channel", sender, name),
                        }
                    }
                    Some(ActorEvent::ApprovalRequested(request)) => {
                        pending_approvals.insert(request.id.0, request.agent_id);
                        println!("\r--- Approval requested (id: {}) ---", request.id.0);
                        println!("{}", request.description);
                        if request.explicit {
                            println!("A rule of this session or channel asks for this call to be decided by a person.");
                        }
                        println!("Type '/approve {}' or '/deny {}' to respond.", request.id.0, request.id.0);
                    }
                    // The user's own answer, given by a rule of theirs (issue 13).
                    Some(ActorEvent::ApprovalGranted(response)) | Some(ActorEvent::ApprovalDenied(response)) if !matches!(sender_id, Some(ActorId::Approver(_))) => {
                        if response.decisions.iter().any(|decision| decision.by_rule.is_some()) {
                            println!("\r--- Approval {} answered by your rule ---", response.id.0);
                        }
                    }
                    // The approval is over: decided, or no longer asked for.
                    Some(ActorEvent::ApprovalGranted(response)) | Some(ActorEvent::ApprovalDenied(response)) | Some(ActorEvent::ApprovalTimedOut(response)) => {
                        pending_approvals.remove(&response.id.0);
                    }
                    Some(ActorEvent::ApprovalRequestWithdrawn { id }) => {
                        if pending_approvals.remove(&id.0).is_some() {
                            println!("\r--- Approval {} withdrawn: the turn ended ---", id.0);
                        }
                    }
                    // Asked anew by an agent that resumed its turn: shown once.
                    Some(ActorEvent::QuestionRequested(request)) => {
                        if !open_questions.iter().any(|q| q.id == request.id) {
                            print_questions(&request, me);
                            open_questions.push(request);
                        }
                    }
                    Some(ActorEvent::QuestionClosed { id, .. }) => {
                        open_questions.retain(|q| q.id != id);
                        if answering.as_ref().is_some_and(|(q, _)| q.id == id) {
                            answering = None;
                            println!("\r--- Questions {} closed before you answered ---", id.0);
                        }
                    }
                    Some(_) => {
                    }
                    None => break,
                }
            }
            maybe_input = input_rx.recv() => {
                match maybe_input {
                    Some(Ok(line)) => {
                        let input = line.trim();
                        if input.is_empty() {
                            continue;
                        }
                        if input == "exit" || input == "quit" {
                            break;
                        }
                        if let Some(rest) = input.strip_prefix("/decline").filter(|r| r.is_empty() || r.starts_with(' ')) {
                            match questions_named(&open_questions, rest, me) {
                                Some(request) => {
                                    answering = None;
                                    if let Err(e) = user_actor.answer_questions(request.id, None).await {
                                        println!("[error] {}", user_message(&e).unwrap_or_else(|| e.to_string()));
                                    }
                                }
                                None => println!("No open questions put to you{}", if rest.trim().is_empty() { String::new() } else { format!(" with id {}", rest.trim()) }),
                            }
                            continue;
                        }
                        if let Some(rest) = input.strip_prefix("/answer").filter(|r| r.is_empty() || r.starts_with(' ')) {
                            match questions_named(&open_questions, rest, me).filter(|q| q.questions.is_asked(me)) {
                                Some(request) => {
                                    if let Some(first) = request.questions.questions.first() {
                                        println!("{}", prompt_of(0, first));
                                        answering = Some((request, Vec::new()));
                                    }
                                }
                                None => println!("No open questions put to you{}", if rest.trim().is_empty() { String::new() } else { format!(" with id {}", rest.trim()) }),
                            }
                            continue;
                        }
                        // While questions are answered, each line answers the next.
                        if let Some((request, mut responses)) = answering.take() {
                            if let Some(question) = request.questions.questions.get(responses.len()) {
                                responses.push(response_of(question, input));
                            }
                            match request.questions.questions.get(responses.len()) {
                                Some(next) => {
                                    println!("{}", prompt_of(responses.len(), next));
                                    answering = Some((request, responses));
                                }
                                None => if let Err(e) = user_actor.answer_questions(request.id, Some(responses)).await {
                                    println!("[error] {}", user_message(&e).unwrap_or_else(|| e.to_string()));
                                },
                            }
                            continue;
                        }
                        if let Some(rest) = input.strip_prefix("/approve ").or_else(|| input.strip_prefix("/deny ")) {
                            let approved = input.starts_with("/approve ");
                            match Uuid::parse_str(rest.trim()) {
                                Ok(id) => {
                                    match pending_approvals.remove(&id) {
                                        Some(agent_id) => {
                                            let response = ApprovalResponse::new(ApprovalId(id), agent_id);
                                            user_actor.send_approval(response, approved).await?;
                                        }
                                        None => println!("No pending approval with id {}", id),
                                    }
                                }
                                Err(_) => println!("Invalid approval id: {}", rest.trim()),
                            }
                            continue;
                        }
                        if input == "/retry" {
                            let asked = match (under_way.or(given_up), last_answer) {
                                (Some((agent, turn)), _) => user_actor.request_retry_of_turn(agent, turn).await,
                                (None, Some(answer)) => user_actor.request_retry(answer).await,
                                (None, None) => {
                                    println!("No answer of an agent to give again");
                                    continue;
                                }
                            };
                            if let Err(e) = asked {
                                let message = user_message(&e).unwrap_or_else(|| e.to_string());
                                println!("[error] {}", message);
                            }
                            continue;
                        }
                        // `/approve`, `/deny`, `/answer`, `/decline` and
                        // `/retry` above are this terminal's own; every
                        // other command goes to the channel (issue 28).
                        match parse_input(input) {
                            UserInput::Command(line) => {
                                match user_actor.execute_command(&line).await {
                                    // Also for an answer to the whole channel: its
                                    // announcement is not routed back to its sender.
                                    Ok(reply) => if let Some(output) = reply.output { println!("{}", output) },
                                    Err(e) => {
                                        // The line is not logged: what follows the command
                                        // may be what was said. The error names the command.
                                        tracing::warn!("A command failed: {:#}", e);
                                        let message = user_message(&e).unwrap_or_else(|| {
                                            "The command failed; the details are in the log".to_string()
                                        });
                                        println!("[error] {}", message);
                                    }
                                }
                            }
                            UserInput::Message(text) => {
                                let span_id = user_actor.send_message(Some(text), None, Vec::new()).await?;
                                tracing::debug!(span_id = %span_id, "Message sent");
                            }
                        }
                    }
                    Some(Err(ReadlineError::Interrupted)) => {
                        println!("CTRL-C");
                        break;
                    }
                    Some(Err(err)) => {
                        println!("Error: {:?}", err);
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}

fn spawn_rustyline_input() -> mpsc::UnboundedReceiver<Result<String, ReadlineError>> {
    let (input_tx, input_rx) = mpsc::unbounded_channel::<Result<String, ReadlineError>>();
    thread::spawn(move || {
        let mut rl = match Editor::<(), DefaultHistory>::new() {
            Ok(rl) => rl,
            Err(err) => {
                let _ = input_tx.send(Err(ReadlineError::Io(Error::other(err.to_string()))));
                return;
            }
        };

        loop {
            match rl.readline(">> ") {
                Ok(line) => {
                    if input_tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    let _ = input_tx.send(Err(err));
                    break;
                }
            }
        }
    });

    input_rx
}

#[derive(Parser, Debug)]
#[command(author, version, about = "An accessible Chat CLI")]
struct Args {
    /// Path to a specific config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// The session template to start a session from, by name - one of `templates` in the
    /// configuration directory, or of the configuration's `seed`.
    #[arg(short, long, default_value = "default")]
    template: String,

    /// Whom the terminal acts for, by name: the user the configured user providers know under it,
    /// with their roles - without a password, as the terminal runs in the kernel's own process
    /// (`Credential::Trusted`). The user of the system unless named.
    #[arg(short, long)]
    user: Option<String>,

    /// Continues the session of this directory spoken in last, instead of starting one; starts
    /// one if there is none.
    #[arg(long = "continue", conflicts_with_all = ["resume", "sessions"])]
    continues: bool,

    /// Resumes the session of this directory whose id begins so, as `--sessions` shows it.
    #[arg(short, long, value_name = "ID", conflicts_with = "sessions")]
    resume: Option<String>,

    /// Lists the sessions of this directory, the one spoken in last first, and ends.
    #[arg(long)]
    sessions: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

/// What the program does in place of a session.
#[derive(Debug, Subcommand)]
enum Command {
    /// Prints the id the program keeps a definition's file by - a skill, a personality, a bundle,
    /// a template -, and whether it is derived from its name or the file's own.
    Id {
        /// The file, in a directory of definitions of the configuration directory.
        file: PathBuf,
    },
}

impl Args {
    /// What the program is asked to do with the sessions of its directory - one thing at most, as
    /// the options exclude each other.
    fn asked(&self) -> Asked {
        match (&self.resume, self.continues, self.sessions) {
            (Some(prefix), _, _) => Asked::Resume(prefix.clone()),
            (None, true, _) => Asked::Continue,
            (None, false, true) => Asked::List,
            (None, false, false) => Asked::Start,
        }
    }
}

/// The plugins the terminal builds its kernel with (issue 323).
fn plugins() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(SkillsPlugin),
        Box::new(ArtifactsPlugin),
        Box::new(MemoryPlugin),
        Box::new(OpenTerminalPlugin),
        Box::new(ModelPlugin),
        Box::new(OllamaModelPlugin),
        Box::new(ComfyUiModelPlugin),
        Box::new(SystemOneModelPlugin),
        Box::new(DecisionPlugin),
        Box::new(HomeStorePlugin),
        Box::new(JsonStorePlugin),
        Box::new(PostgresStorePlugin),
    ]
}

/// The libraries the terminal builds its kernel with, of the agent side (issue 516) - first the
/// essential one, the agents' (issue 370).
fn libraries() -> Vec<Box<dyn Library>> {
    vec![
        Box::new(AgentsLibrary),
        Box::new(CompactionLibrary),
        Box::new(McpToolLibrary),
        Box::new(A2aLibrary),
        Box::new(WorkspaceToolLibrary),
        Box::new(ImageToolLibrary),
        Box::new(AttachmentToolLibrary),
        Box::new(WebToolLibrary),
        Box::new(SkillsLibrary),
        Box::new(ArtifactsLibrary),
        Box::new(MemoryLibrary),
    ]
}

#[expect(clippy::print_stdout, reason = "the TUI writes the conversation to the terminal")]
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let here = env::current_dir()
        .and_then(|dir| dir.canonicalize())
        .context("the directory the program was started in cannot be told")?;
    let home = Home::of_user()?;
    let config = with_local_model(match &args.config {
        Some(given) => AppConfig::load(given)
            .with_context(|| format!("cannot read the configuration {}", given.display()))?,
        None => {
            home.first_start()?;
            home.config(&here)?
        }
    });

    if let Some(Command::Id { file }) = &args.command {
        return print_identity(&config, &here.join(file)).await;
    }

    // Into a file by default: log lines on the terminal would break the prompt.
    init(&config.logging, LogTarget::File(home.log()), "info")?;

    let plugins = plugins();
    let plugins: Vec<&dyn Plugin> = plugins.iter().map(Box::as_ref).collect();
    let mut builder = KernelBuilder::from_config(&config, &plugins).await?;
    for library in libraries() {
        builder.library(library.as_ref())?;
    }
    let kernel = Arc::new(builder.build()?);
    kernel.seed(&config.seed).await?;

    // Whom the terminal acts for: the user the providers know by that name,
    // recorded with their roles as a login records them (issue 306).
    let users = kernel.user_manager();
    if users.is_empty() {
        anyhow::bail!("No user providers are configured (`users` section), so the terminal can act for nobody. See examples/config.yaml.");
    }
    let user =
        args.user.clone().or_else(|| env::var("USER").ok()).unwrap_or_else(|| "user".to_string());
    let owner = users
        .authenticate(&user, &Credential::Trusted)
        .await
        .map_err(|e| anyhow::anyhow!("The terminal cannot act for '{user}': {e}"))?
        .id;
    // The session is this terminal user's own; their context, with their
    // roles, is their controller's (issue 304).
    let security = kernel
        .controller_manager()
        .user_controller(owner)
        .await?
        .security_context(Default::default(), Default::default());

    let session_manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
    let projects = ProjectManager::new(Arc::clone(&session_manager));
    // Where the program was started, its project and workspace, if it works there.
    let place = match config.workspaces.storage.contains_key(HERE) {
        true => Some(place_of(&kernel, &projects, &security, owner, &here).await?),
        false => None,
    };
    let views = session_manager.views(Viewer::new(security.clone()));
    let asked = args.asked();
    let project = match (place, &asked) {
        (Some((project, _)), _) => Some(project),
        (None, Asked::Start) => None,
        (None, _) => anyhow::bail!(
            "Sessions are listed, continued and resumed by the directory they were started in, \
             which the configuration does not work in (it names no storage `{HERE}`)"
        ),
    };
    if let (Asked::List, Some(project)) = (&asked, project) {
        let listed = sessions::listed(&views, project).await?;
        if listed.is_empty() {
            println!("No session was started in this directory yet.");
        }
        for session in &listed {
            println!("{}", session.line());
        }
        return Ok(());
    }
    let chosen = match project {
        Some(project) => sessions::chosen(&views, project, &asked).await?,
        None => None,
    };
    let session_loop = match chosen {
        Some(id) => match session_manager.open(&security, id).await {
            Err(e) if matches!(e.downcast_ref(), Some(SessionError::RunsElsewhere(_))) => {
                anyhow::bail!("The session {id} is open in another terminal")
            }
            opened => opened.with_context(|| format!("The session {id} cannot be resumed"))?,
        },
        None => start(&kernel, &session_manager, &security, owner, &args.template, place).await?,
    };
    let session_id = session_loop.id();

    // The template's first channel, whatever it is called.
    let channel = session_loop
        .channels()
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("The session {session_id} has no channel"))?;

    // Who answers, as the views tell it (issue 174).
    let view = views.session(session_id).await?.channel(channel.id()).await?;
    let unknown = || "from a bundle".to_string();
    println!(
        "Moderator: {}, responders: {}",
        view.moderator().unwrap_or_else(unknown),
        view.responder_policy().unwrap_or_else(unknown)
    );

    // Start the interactive loop; however it ends, the session is shut
    // down in order (issue 297): its actors keep what they are in the
    // middle of, and only what does not outlive the process ends with it.
    let ran = run_repl(channel, owner).await;
    session_manager.shutdown().await;
    ran
}

/// Prints the identity of the definition in `file`, as the stores of `config` read it: its kind,
/// its name and its id, and whether its name derives the id - which a file renamed loses.
///
/// # Errors
/// If the configuration keeps its entities in another store than `home`, or `file` is no
/// definition of it.
#[expect(clippy::print_stdout, reason = "the TUI writes to the terminal")]
async fn print_identity(config: &AppConfig, file: &Path) -> Result<()> {
    let stores = &config.persistence.entity_store;
    if stores.kind != HOME {
        anyhow::bail!(
            "The configuration keeps its entities in the store `{}`, which keeps no definitions as \
             files; `id` asks the store `{HOME}`",
            stores.kind
        );
    }
    let params = HomeStoreConfig::read(&Value::Object(stores.params.clone()))
        .context("the parameters of the store of the entities")?;
    let identity = HomeStore::new(&params)?
        .identity_of(file)
        .await
        .with_context(|| format!("{} is no definition", file.display()))?;
    let id = identity.id;
    let whence = match identity.derived {
        true => "derived from its name",
        false => "its own",
    };
    println!("{} {}: {id}  {whence}", identity.kind, identity.name);
    if identity.derived {
        println!("Write `id: {id}` into the file to keep it when the file is renamed.");
    }
    Ok(())
}

/// A new session of `owner`, for whom `security` acts, made by `session_manager` from the template
/// named `template` - among those `owner` may use -, in `place`, the project and the workspace of
/// the directory the program was started in, if it works there.
///
/// # Errors
/// If there is no such template, or making the session fails.
async fn start(
    kernel: &Kernel,
    session_manager: &SessionManager,
    security: &SecurityContext,
    owner: UserId,
    template: &str,
    place: Option<(ProjectId, WorkspaceId)>,
) -> Result<Arc<SessionLoop>> {
    let stores = kernel.entity_stores();
    let templates = stores.session_templates.list(security, &Query::all()).await?.items;
    let template = templates.into_iter().find(|t| t.name.name == template).ok_or_else(|| {
        anyhow::anyhow!("Session template '{template}' not found: templates lie in `templates` of the configuration directory, or come from the `seed` of the configuration")
    })?;
    // Held in the language the user prefers where the template names none:
    // the frontend's to offer, not the server's (issues 331, 425).
    let preferred = stores
        .user_profiles
        .get(security, owner)
        .await
        .optional()?
        .and_then(|profile| profile.preferred_language);
    let language = preferred.filter(|_| template.language.is_none());
    let spec = SessionSpec {
        language: language.as_ref().map(Language::to_string),
        workspace: place.map(|(_, id)| WorkspaceChoice::Existing { id, share: None }),
        project: place.map(|(project, _)| project),
        ..SessionSpec::default()
    };
    session_manager.create_from_template(security, template.id, spec).await
}

/// Completes on SIGINT, SIGTERM or SIGHUP - the terminal closed -, or
/// never, should none of them be caught.
async fn signalled() {
    let caught = |kind: SignalKind| async move {
        match signal(kind) {
            Ok(mut caught) => {
                caught.recv().await;
            }
            Err(e) => {
                tracing::warn!(
                    "A signal cannot be caught, and ends the program without a shutdown: {e}"
                );
                future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        () = caught(SignalKind::interrupt()) => {}
        () = caught(SignalKind::terminate()) => {}
        () = caught(SignalKind::hangup()) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::question::Choice;
    use clap::CommandFactory;

    /// The options of the program fit together: no name taken twice, and one thing asked at most.
    #[test]
    fn the_options_fit_together() {
        Args::command().debug_assert();
        let resumed = Args::try_parse_from(["caelumora", "--resume", "0a1b"]).unwrap();
        assert!(matches!(resumed.asked(), Asked::Resume(prefix) if prefix == "0a1b"));
        assert!(matches!(Args::try_parse_from(["caelumora"]).unwrap().asked(), Asked::Start));
        assert!(Args::try_parse_from(["caelumora", "--continue", "--sessions"]).is_err());
        assert!(Args::try_parse_from(["caelumora", "--resume", "0a1b", "--sessions"]).is_err());
        let id = Args::try_parse_from(["caelumora", "id", "personalities/a.yaml"]).unwrap();
        assert!(matches!(
            id.command,
            Some(Command::Id { file }) if file == Path::new("personalities/a.yaml")
        ));
    }

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
    fn a_message_is_printed_with_what_it_carries() {
        use caelumora_domain::confidentiality::Classification;
        use caelumora_domain::content::testing::make_content_ref;
        let message = |content: Option<&str>, attachments| Message {
            content: content.map(str::to_string),
            attachments,
            ..Message::new(Classification::CHANNEL_DEFAULT)
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
}
