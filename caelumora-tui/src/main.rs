use caelumora_model::actor::{ActorEvent, ActorId};
use caelumora_model::agent::AgentId;
use caelumora_model::approval::{ApprovalId, ApprovalResponse};
use caelumora_model::command::{parse_input, UserInput};
use caelumora_model::config::AppConfig;
use caelumora_model::mention::mention_spans;
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;
use caelumora_model::user::{User, UserId};
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::llm_impl::ollama::OllamaClient;
use caelumora_runtime::session::SessionManager;
use clap::Parser;
use caelumora_runtime::logging::{self, LogTarget};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc;
use uuid::Uuid;

fn render_content(message: &Message, content: &str, handles: &HashMap<ActorId, String>) -> String {
    let mentioned: Vec<&String> = message.mentions.iter().filter_map(|id| handles.get(id)).collect();
    let mut rendered = String::with_capacity(content.len());
    let mut pos = 0;
    for span in mention_spans(content) {
        if mentioned.contains(&&span.handle) {
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

async fn run_repl(channel: Arc<ChannelLoop>, user: UserId) -> anyhow::Result<()> {
    let user_actor = channel.user_actor(user)
        .ok_or_else(|| anyhow::anyhow!("The user is not a participant of the main channel"))?;

    let mut receiver = user_actor.subscribe();
    let mut input_rx = spawn_rustyline_input();
    // Approval id -> agent id, so `/approve <id>`/`/deny <id>` know which
    // agent to answer. Covers both plan proposals and regular tool calls -
    // both flow through the same generic ApprovalRequested event.
    let mut pending_approvals: HashMap<Uuid, AgentId> = HashMap::new();
    let handles: HashMap<ActorId, String> = channel.members()
        .into_iter()
        .filter_map(|m| Some((m.id, m.handle?)))
        .collect();

    println!("--- Chat Session Started (Type 'exit' or 'quit' to stop) ---");
    println!("Moderator: {}", channel.moderator());
    loop {
        tokio::select! {
            maybe_envelope = receiver.recv() => {
                let sender = maybe_envelope.as_ref().map(|e| e.context.sender_name.clone()).unwrap_or_default();
                match maybe_envelope.map(|envelope| envelope.event) {
                    Some(ActorEvent::CompleteMessageGenerated(message)) => {
                        if let Some(content) = &message.content {
                            println!("\r{}> {}", message.sender_name, render_content(&message, content, &handles));
                        }
                    }
                    Some(ActorEvent::FullResponseGenerated(messages)) => {
                        for message in messages {
                            if let Some(content) = &message.content {
                                println!("\r{}> {}", message.sender_name, render_content(&message, content, &handles));
                            }
                        }
                    }
                    Some(ActorEvent::IntermediateMessageGenerated(message)) => {
                        if let Some(content) = &message.content {
                            println!("\r[working] {}> {}", message.sender_name, render_content(&message, content, &handles));
                        }
                    }
                    Some(ActorEvent::TurnFailed { message }) => {
                        println!("\r[error] An agent could not answer: {}", message);
                    }
                    Some(ActorEvent::CommandExecuted { command, output }) => {
                        println!("\r{} ran /{}: {}", sender, command, output);
                    }
                    Some(ActorEvent::ApprovalRequested(request)) => {
                        pending_approvals.insert(request.id.0, request.agent_id);
                        println!("\r--- Approval requested (id: {}) ---", request.id.0);
                        println!("{}", request.description);
                        println!("Type '/approve {}' or '/deny {}' to respond.", request.id.0, request.id.0);
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
                        if let Some(rest) = input.strip_prefix("/approve ").or_else(|| input.strip_prefix("/deny ")) {
                            let approved = input.starts_with("/approve ");
                            match Uuid::parse_str(rest.trim()) {
                                Ok(id) => {
                                    match pending_approvals.remove(&id) {
                                        Some(agent_id) => {
                                            let response = ApprovalResponse { id: ApprovalId(id), agent_id };
                                            user_actor.send_approval(response, approved).await?;
                                        }
                                        None => println!("No pending approval with id {}", id),
                                    }
                                }
                                Err(_) => println!("Invalid approval id: {}", rest.trim()),
                            }
                            continue;
                        }
                        // `/approve` and `/deny` above are this terminal's own;
                        // every other command goes to the channel (issue 28).
                        match parse_input(input) {
                            UserInput::Command { name, args } => {
                                match user_actor.execute_command(&name, &args).await {
                                    // Also for an answer to the whole channel: its
                                    // announcement is not routed back to its sender.
                                    Ok(reply) => println!("{}", reply.output),
                                    Err(e) => {
                                        tracing::warn!("Command /{} failed: {:#}", name, e);
                                        let message = caelumora_runtime::error::user_message(&e)
                                            .unwrap_or_else(|| format!("/{} failed; the details are in local/tui.log", name));
                                        println!("[error] {}", message);
                                    }
                                }
                            }
                            UserInput::Message(text) => {
                                let span_id = user_actor.send_message(text).await?;
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
        let mut rl = match Editor::<(), rustyline::history::DefaultHistory>::new() {
            Ok(rl) => rl,
            Err(err) => {
                let _ = input_tx.send(Err(ReadlineError::Io(std::io::Error::other(err.to_string()))));
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let config = if let Some(config_path) = args.config {
        AppConfig::load(config_path)?
    }
    else {
        AppConfig::default()
    };

    // Into a file by default: log lines on the terminal would break the prompt.
    logging::init(&config.logging, LogTarget::File(PathBuf::from("local/tui.log")), "info")?;

    let security_manager = config.create_security_manager();
    let state_store = config.create_state_store(security_manager.clone())?;
    let entity_store = config.create_entity_store(security_manager.clone())?;
    let secret_manager = config.create_secret_manager(security_manager.clone())?;

    let runtime = Arc::new(RuntimeManager::new(
        state_store.clone(),
        entity_store.clone(),
        secret_manager,
        security_manager.clone(),
        config.commands.clone(),
        config.component_defaults.clone(),
        Arc::new(OllamaClient::default()),
    )?);
    caelumora_runtime::seed::load(entity_store.as_ref(), &config.seed).await?;

    let owner = UserId::default();
    // The session is this terminal user's own.
    let security = SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
    // Users belong to the system (issue 94), so the system records this one.
    let system = SecurityContext::system();
    if entity_store.get_user(&system, owner).await.is_err() {
        entity_store.create_user(&system, &User {
            id: owner,
            username: "user".to_string(),
            email: "user@example.com".to_string(),
            roles: vec!["user".to_string()],
        }).await?;
    }

    // By name, among those this user may use.
    let template = entity_store.list_session_templates(&security).await?.into_iter()
        .find(|t| t.metadata.name == args.template)
        .ok_or_else(|| anyhow::anyhow!("Session template '{}' not found: templates come from the `seed` of the configuration - `--config examples/config.yaml` seeds the demo ones", args.template))?;

    let session_manager = SessionManager::new(runtime.clone());
    let session_loop = session_manager.create_from_template(&security, template.id, Some("default-session".to_string())).await?;
    let session_id = session_loop.id();

    let channel = session_loop
        .channel_by_name("main")
        .ok_or_else(|| anyhow::anyhow!("Cannot find main channel in session"))?;

    // Start the interactive loop
    run_repl(channel, owner).await?;
    session_manager.stop_session(&security, session_id).await?;

    Ok(())
}
