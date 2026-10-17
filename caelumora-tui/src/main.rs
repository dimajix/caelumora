use caelumora_model::store::query::Query;
use caelumora_model::store::StoreResultExt;
use caelumora_model::actor::{ActorEvent, ActorId};
use caelumora_model::agent::AgentId;
use caelumora_model::approval::{ApprovalId, ApprovalResponse};
use caelumora_model::command::{parse_input, UserInput};
use caelumora_model::config::AppConfig;
use caelumora_model::mention::{is_group_handle, mention_spans};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::question::{Question, QuestionId, QuestionRequest, Response};
use caelumora_model::turn::GivenUpReason;
use caelumora_model::user::UserId;
use caelumora_model::user_provider::Credential;
use caelumora_model::work::TurnId;
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::component::{ComponentError, ComponentRegistry};
use caelumora_agents::llm::LLMClient;
use caelumora_agents::plugin::AgentsPlugin;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use caelumora_runtime::session::SessionManager;
use caelumora_artifacts::plugin::ArtifactsPlugin;
use caelumora_skills::plugin::SkillsPlugin;
use caelumora_mcp::plugin::McpPlugin;
use caelumora_a2a::plugin::A2aPlugin;
use caelumora_open_terminal::plugin::OpenTerminalPlugin;
use caelumora_ollama::client::OLLAMA_CLIENT_KIND;
use caelumora_ollama::plugin::OllamaPlugin;
use caelumora_runtime::view::Viewer;
use clap::Parser;
use caelumora_logging::{LogTarget, init};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::broadcast::error::RecvError;
use std::sync::Arc;
use std::thread;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use uuid::Uuid;
use anyhow::Result;
use caelumora_runtime::error::user_message;
use rustyline::history::DefaultHistory;
use std::io::Error;

fn render_content(message: &Message, content: &str, handles: &HashMap<ActorId, String>) -> String {
    let mentioned: Vec<&String> = message.mentions.iter().filter_map(|id| handles.get(id)).collect();
    let mut rendered = String::with_capacity(content.len());
    let mut pos = 0;
    for span in mention_spans(content) {
        // A group handle addresses everyone (issue 401): resolved if anyone was.
        if mentioned.contains(&&span.handle) || (is_group_handle(&span.handle) && !message.mentions.is_empty()) {
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
fn print_questions(request: &QuestionRequest, me: ActorId) {
    println!("\r--- Questions of {} (id: {}) ---", request.tool_call.function_name, request.id.0);
    for (index, question) in request.questions.questions.iter().enumerate() {
        println!("{}. [{}] {}", index + 1, question.header, question.question);
        for (number, choice) in question.choices.iter().enumerate() {
            let recommended = if choice.recommended { " (recommended)" } else { "" };
            let description = choice.description.as_deref().map(|d| format!(" - {d}")).unwrap_or_default();
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
    let numbers: Option<Vec<usize>> = line.split(',').map(|n| n.trim().parse::<usize>().ok().filter(|n| (1..=question.choices.len()).contains(n))).collect();
    let chosen: Option<Vec<String>> = match numbers {
        Some(numbers) => Some(numbers.into_iter().map(|n| question.choices[n - 1].label.clone()).collect()),
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
        id => Uuid::parse_str(id).ok().and_then(|id| open.iter().find(|q| q.id == QuestionId(id)).cloned()),
    }
}

async fn run_repl(channel: Arc<ChannelLoop>, user: UserId) -> Result<()> {
    let user_actor = channel.user_actor(user)
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
    let handles: HashMap<ActorId, String> = channel.members()
        .into_iter()
        .filter_map(|m| Some((m.id, m.handle?)))
        .collect();

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
                        if let Some(content) = &message.content {
                            println!("\r{}> {}", message.sender_name, render_content(&message, content, &handles));
                        }
                    }
                    Some(ActorEvent::IntermediateMessageGenerated(message)) => {
                        if let Some(content) = &message.content {
                            println!("\r[working] {}> {}", message.sender_name, render_content(&message, content, &handles));
                        }
                    }
                    // What another changed of what was said (issue 234).
                    Some(ActorEvent::MessageModified(message)) => {
                        if let Some(content) = &message.content {
                            println!("\r{}> [edited] {}", message.sender_name, render_content(&message, content, &handles));
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
                            GivenUpReason::Left | GivenUpReason::Retried => println!("\r{} gave its answer up: {}", sender, reason.cause()),
                        }
                    }
                    Some(ActorEvent::CommandStarted { command, .. }) => {
                        // Told to its issuer alone (issue 299).
                        println!("\r{} is carrying out /{}...", sender, command);
                    }
                    Some(ActorEvent::CommandExecuted { command, output, turn_id, .. }) => {
                        // Carried out by an agent in a turn of its own (issue 269), or run by a user.
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
                    Some(ActorEvent::ParticipantLeft { participant, name }) => {
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
                        if response.by_rule.is_some() {
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
                            UserInput::Command { name, args } => {
                                match user_actor.execute_command(&name, &args).await {
                                    // Also for an answer to the whole channel: its
                                    // announcement is not routed back to its sender.
                                    Ok(reply) => if let Some(output) = reply.output { println!("{}", output) },
                                    Err(e) => {
                                        tracing::warn!("Command /{} failed: {:#}", name, e);
                                        let message = user_message(&e)
                                            .unwrap_or_else(|| format!("/{} failed; the details are in local/tui.log", name));
                                        println!("[error] {}", message);
                                    }
                                }
                            }
                            UserInput::Message(text) => {
                                let span_id = user_actor.send_message(text, None).await?;
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

    /// The session template to start a session from, by name - one the
    /// configuration's `seed` provides, e.g. `examples/config.yaml`.
    #[arg(short, long, default_value = "sassy-session")]
    template: String,

    /// Whom the terminal acts for, by name: the user the configured user
    /// providers know under it, with their roles - without a password, as
    /// the terminal runs in the runtime's own process (`Credential::Trusted`).
    #[arg(short, long, default_value = "user")]
    user: String,
}

/// The plugins the terminal builds its runtime with (issue 323) - first the
/// essential one, the agents' (issue 370).
fn plugins() -> Vec<Box<dyn Plugin>> {
    vec![Box::new(AgentsPlugin), Box::new(SkillsPlugin), Box::new(ArtifactsPlugin), Box::new(OllamaPlugin), Box::new(McpPlugin), Box::new(A2aPlugin), Box::new(OpenTerminalPlugin)]
}

/// Names the defaults of the facets of which an agent has one that the core
/// leaves to the program (issue 329) among the kinds of `components`: the
/// model is served by Ollama. `component_defaults` of the configuration
/// still replaces them.
///
/// # Errors
/// If a plugin that brings a default kind is not among `plugins`.
fn default_kinds(components: &mut ComponentRegistry) -> Result<(), ComponentError> {
    components.set_default_kind::<dyn LLMClient>(OLLAMA_CLIENT_KIND)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let config = if let Some(config_path) = args.config {
        AppConfig::load(config_path)?
    }
    else {
        AppConfig::default()
    };

    // Into a file by default: log lines on the terminal would break the prompt.
    init(&config.logging, LogTarget::File(PathBuf::from("local/tui.log")), "info")?;

    let plugins = plugins();
    let plugins: Vec<&dyn Plugin> = plugins.iter().map(Box::as_ref).collect();
    let mut builder = RuntimeBuilder::from_config(&config, &plugins)?;
    default_kinds(builder.components())?;
    let runtime = Arc::new(builder.build()?);
    runtime.seed(&config.seed).await?;
    let entity_stores = runtime.entity_stores();

    // Whom the terminal acts for: the user the providers know by that name,
    // recorded with their roles as a login records them (issue 306).
    let users = runtime.user_manager();
    if users.is_empty() {
        anyhow::bail!("No user providers are configured (`users` section), so the terminal can act for nobody. See examples/config.yaml.");
    }
    let owner = users.authenticate(&args.user, &Credential::Trusted).await
        .map_err(|e| anyhow::anyhow!("The terminal cannot act for '{}': {e}", args.user))?.id;
    // The session is this terminal user's own; their context, with their
    // roles, is their controller's (issue 304).
    let security = runtime.controller_manager().user_controller(owner).await?.security_context(Default::default(), Default::default());

    // By name, among those this user may use.
    let template = entity_stores.session_templates.list(&security, &Query::all()).await.map(|page| page.items)?.into_iter()
        .find(|t| t.name.name == args.template.as_str())
        .ok_or_else(|| anyhow::anyhow!("Session template '{}' not found: templates come from the `seed` of the configuration - `--config examples/config.yaml` seeds the demo ones", args.template))?;

    let session_manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
    let session_loop = session_manager.create_from_template(&security, template.id, Some("default-session"), None).await?;
    let session_id = session_loop.id();

    // Its channels held in the language the user prefers where the template
    // names none: the frontend's to set, not the server's (issue 331).
    let preferred = entity_stores.user_profiles.get(&security, owner).await.optional()?.and_then(|profile| profile.preferred_language);
    if let Some(language) = preferred {
        for channel in session_loop.channels().into_iter().filter(|channel| channel.channel().language.is_none()) {
            // The session exists either way; reported, not undone.
            if let Err(e) = channel.update_language(&security, Some(language.as_str())).await {
                tracing::warn!(channel = %channel.id(), "The language of the channel could not be set: {e:#}");
            }
        }
    }

    // The template's first channel, whatever it is called.
    let channel = session_loop.channels().into_iter().next()
        .ok_or_else(|| anyhow::anyhow!("Session template '{}' has no channel", args.template))?;

    // Who answers, as the views tell it (issue 174).
    let view = session_manager.views(Viewer::new(security.clone())).session(session_id).await?.channel(channel.id()).await?;
    let unknown = || "preconfigured".to_string();
    println!("Moderator: {}, responders: {}", view.moderator().unwrap_or_else(unknown), view.responder_policy().unwrap_or_else(unknown));

    // Start the interactive loop; however it ends, the session is shut
    // down in order (issue 297): its actors keep what they are in the
    // middle of, and only what does not outlive the process ends with it.
    let ran = run_repl(channel, owner).await;
    session_manager.shutdown().await;
    ran
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
                tracing::warn!("A signal cannot be caught, and ends the program without a shutdown: {e}");
                std::future::pending::<()>().await;
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
    use caelumora_model::question::Choice;

    fn question(choices: &[&str], multi_select: bool) -> Question {
        Question {
            header: "Pick".to_string(),
            question: "Which?".to_string(),
            choices: choices.iter().map(|l| Choice { label: l.to_string(), description: None, recommended: false }).collect(),
            multi_select,
        }
    }

    #[test]
    fn a_line_answers_a_question_as_it_asks() {
        let chosen = |labels: &[&str]| Response::Chosen { chosen: labels.iter().map(|l| l.to_string()).collect(), other: None };
        let other = |text: &str| Response::Chosen { chosen: Vec::new(), other: Some(text.to_string()) };
        let choices = question(&["Postgres", "SQLite", "None"], true);
        assert_eq!(response_of(&choices, "2"), chosen(&["SQLite"]));
        assert_eq!(response_of(&choices, "1, 3"), chosen(&["Postgres", "None"]));
        assert_eq!(response_of(&choices, "SQLite"), chosen(&["SQLite"]), "by its label");
        assert_eq!(response_of(&choices, "4"), other("4"), "no such choice: another answer");
        assert_eq!(response_of(&choices, "MariaDB"), other("MariaDB"));
        assert_eq!(response_of(&choices, "-"), Response::Declined);
        assert_eq!(response_of(&question(&[], false), "Kaya"), Response::Text { text: "Kaya".to_string() });
        assert_eq!(response_of(&question(&[], false), "-"), Response::Declined);
    }
}
