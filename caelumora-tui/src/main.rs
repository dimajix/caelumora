use caelumora_model::actor::{ActorEvent, ActorId};
use caelumora_model::agent::AgentId;
use caelumora_model::agent::Personality;
use caelumora_model::agent::PersonalityId;
use caelumora_model::approval::{ApprovalId, ApprovalResponse};
use caelumora_model::command::{parse_input, UserInput};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy, STAGE_PERCEIVE};
use caelumora_model::config::AppConfig;
use caelumora_model::mention::mention_spans;
use caelumora_model::message::Message;
use caelumora_model::meta::{Metadata, Scope};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{SessionContext, SessionId};
use caelumora_model::store::EntityStore;
use caelumora_model::template::{ChannelTemplate, ParticipantId, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId};
use caelumora_model::user::{User, UserId};
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::component_impl::transcript_appender::TRANSCRIPT_APPENDER_KIND;
use caelumora_runtime::component_impl::state_context_provider::STATE_CONTEXT_PROVIDER_KIND;
use caelumora_runtime::component_impl::state_history_appender::STATE_HISTORY_APPENDER_KIND;
use caelumora_runtime::component_impl::static_system_prompt_provider::STATIC_SYSTEM_PROMPT_PROVIDER_KIND;
use caelumora_runtime::component_impl::create_plan_tool::CREATE_PLAN_TOOL_PROVIDER_KIND;
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::llm_impl::ollama::OllamaClient;
use caelumora_runtime::session::SessionManager;
use clap::Parser;
use caelumora_runtime::logging::{self, LogTarget};
use tracing::info;
use rustyline::error::ReadlineError;
use rustyline::Editor;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc;
use uuid::Uuid;

fn create_component_config(system_prompt: &str, enable_extended_planning: bool) -> ComponentConfig {
    // create_plan_tool contributes its own planning-nudge PromptProvider
    // (see CreatePlanToolProvider in caelumora-runtime), so the "context" node
    // needs to depend on it too - StateContextProvider only picks up
    // PromptProviders declared among its own deps - whenever it's wired in.
    let mut context_deps = vec!["system-prompt".to_string()];
    if enable_extended_planning {
        context_deps.push("create-plan-tool".to_string());
    }

    let mut component_nodes = vec![
        ComponentDescriptor {
            id: "system-prompt".to_string(),
            kind: STATIC_SYSTEM_PROMPT_PROVIDER_KIND.to_string(),
            params: serde_json::json!({
                "prompt": system_prompt
            }),
            deps: Vec::new(),
            tags: Vec::new(),
            fail_policy: ComponentFailPolicy::FailOpen,
            security_attributes: Default::default(),
        },
        ComponentDescriptor {
            id: "history-appender".to_string(),
            kind: STATE_HISTORY_APPENDER_KIND.to_string(),
            params: serde_json::Value::Null,
            deps: Vec::new(),
            tags: vec![STAGE_PERCEIVE.to_string()],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        },
        ComponentDescriptor {
            id: "transcript-appender".to_string(),
            kind: TRANSCRIPT_APPENDER_KIND.to_string(),
            params: serde_json::Value::Null,
            deps: Vec::new(),
            tags: vec![STAGE_PERCEIVE.to_string()],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        },
        ComponentDescriptor {
            id: "context".to_string(),
            kind: STATE_CONTEXT_PROVIDER_KIND.to_string(),
            params: serde_json::Value::Null,
            deps: context_deps,
            tags: Vec::new(),
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        },
    ];

    // Extended planning is opt-in per personality by wiring in the
    // create_plan_tool component - its mere presence (not a separate model
    // field) is what AgentActor uses to decide whether the create_plan tool
    // (and its planning-nudge system prompt, via "context"'s dep above) is
    // available at all.
    if enable_extended_planning {
        component_nodes.push(ComponentDescriptor {
            id: "create-plan-tool".to_string(),
            kind: CREATE_PLAN_TOOL_PROVIDER_KIND.to_string(),
            params: serde_json::Value::Null,
            deps: Vec::new(),
            tags: Vec::new(),
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        });
    }

    ComponentConfig { component_nodes }
}

fn create_personality(name: &str, description: &str, system_prompt: &str, enable_extended_planning: bool) -> Personality {
    Personality {
        reasoning_confidentiality: caelumora_model::confidentiality::Confidentiality::Public,
        id: PersonalityId(Uuid::new_v4()),
        metadata: Metadata {
            name: name.to_string(),
            scope: Scope::GlobalScope,
            description: description.to_string(),
            ..Metadata::default()
        },
        components: create_component_config(system_prompt, enable_extended_planning),
        security_attributes: Default::default(),
    }
}

async fn setup_templates(store: &Arc<dyn EntityStore>) -> anyhow::Result<SessionTemplate> {
    // Initial data belongs to the system (issue 11).
    let ctx = SecurityContext::system();
    let personality_a = create_personality(
        "jonathan-assistant",
        "A helpful and polite assistant",
        "Your name is Jonathan Heart. You are a helpful assistant.",
        false,
    );
    store.put_personality(&ctx, &personality_a).await?;

    // Sarah has extended planning enabled so the plan/approval flow can be
    // exercised end to end from the TUI (/approve, /deny).
    let personality_b = create_personality(
        "sassy-assistant",
        "A sassy and witty assistant",
        "Your name is Sassy Sarah. You are a witty and slightly sarcastic assistant.",
        true,
    );
    store.put_personality(&ctx, &personality_b).await?;

    let template = SessionTemplate {
        id: SessionTemplateId(Uuid::new_v4()),
        metadata: Metadata {
            name: "simple-session".to_string(),
            scope: Scope::GlobalScope,
            description: "Simple user+assistant session".to_string(),
            ..Metadata::default()
        },
        scope: ScopeTemplate::GlobalScope,
        participants: vec![
            ParticipantTemplate::User { name: "user".to_string() },
            ParticipantTemplate::Agent {
                name: "Jonathan".to_string(),
                scope: ScopeTemplate::GlobalScope,
                personality_id: personality_a.id,
            },
            ParticipantTemplate::Agent {
                name: "Sarah".to_string(),
                scope: ScopeTemplate::GlobalScope,
                personality_id: personality_b.id,
            },
        ],
        channels: vec![
            ChannelTemplate {
                name: "main".to_string(),
                moderator: "round_robin".to_string(),
                confidentiality: caelumora_model::confidentiality::Confidentiality::CHANNEL_DEFAULT,
                participants: vec![
                    "user".to_string(),
                    "Jonathan".to_string(),
                    "Sarah".to_string(),
                ],
            }
        ],
        commands: Default::default(),
        security_attributes: Default::default(),
    };
    store.put_session_template(&ctx, &template).await?;

    Ok(template)
}

/// Formats a message's content for the terminal, highlighting the
/// `@`-mentions that were resolved to a participant of the channel.
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
                                channel.wake_for_delivery();
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
        Arc::new(OllamaClient::default()),
    )?);
    let template = setup_templates(&entity_store).await?;

    let session_id = SessionId(Uuid::new_v4());
    let owner = UserId::default();
    // The session is this terminal user's own.
    let security = SecurityContext::new(ActorId::User(owner), session_id, Default::default());
    if entity_store.get_user(&security, owner).await.is_err() {
        entity_store.create_user(&security, &User {
            id: owner,
            username: "user".to_string(),
            email: "user@example.com".to_string(),
            roles: vec!["user".to_string()],
            security: SecurityContext::default(),
        }).await?;
    }
    let context = SessionContext {
        id: session_id,
        owner,
        security,
        entity_store: entity_store.clone(),
    };
    let mut mapping = HashMap::new();
    mapping.insert("user".to_string(), ParticipantId::UserId(owner));
    let (session, channels) = template.instantiate("default-session".to_string(), &context, mapping).await?;

    // SessionManager owns persistence, spawning the run loop, and lifecycle events.
    let session_manager = SessionManager::new(runtime.clone());
    let session_loop = session_manager.create_session(&context.security, session, channels).await?;

    let channel = session_loop
        .channel_by_name("main")
        .await
        .ok_or_else(|| anyhow::anyhow!("Cannot find main channel in session"))?;

    // Start the interactive loop
    run_repl(channel, owner).await?;
    session_manager.stop_session(session_id).await;

    Ok(())
}
