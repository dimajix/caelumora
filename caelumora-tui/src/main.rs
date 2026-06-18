use caelumora_model::agent::{Personality};
use caelumora_model::store::{TemplateStore};
use caelumora_model::config::{AppConfig};
use caelumora_model::meta::{Metadata, Scope};
use caelumora_model::session::{SessionContext, SessionId};
use caelumora_model::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId};
use caelumora_model::user::UserId;
use caelumora_model::agent::PersonalityId;
use caelumora_runtime::actor::ActorEvent;
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionLoop;
use clap::Parser;
use env_logger::WriteStyle;
use log::{error, info, LevelFilter};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc;
use uuid::Uuid;
use caelumora_model::component::{ComponentConfig, ComponentNodeConfig, ComponentFailPolicy, STAGE_PERCEIVE};
use caelumora_model::security::SecurityContext;
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::component::{
    STATE_CONTEXT_APPENDER_KIND, STATE_CONTEXT_PROVIDER_KIND, STATE_HISTORY_WRITER_KIND,
    STATIC_SYSTEM_PROMPT_PROVIDER_KIND,
};

fn init_logging() {
    let mut builder = env_logger::Builder::new();

    builder
        .filter(None, LevelFilter::Info)
        .write_style(WriteStyle::Always)
        .init();
}

fn create_component_config(system_prompt: &str) -> ComponentConfig {
    ComponentConfig {
        component_nodes: vec![
            ComponentNodeConfig {
                id: "system-prompt".to_string(),
                kind: STATIC_SYSTEM_PROMPT_PROVIDER_KIND.to_string(),
                params: serde_json::json!({
                    "prompt": system_prompt
                }),
                deps: Vec::new(),
                tags: Vec::new(),
                fail_policy: ComponentFailPolicy::FailOpen,
            },
            ComponentNodeConfig {
                id: STATE_HISTORY_WRITER_KIND.to_string(),
                kind: STATE_HISTORY_WRITER_KIND.to_string(),
                params: serde_json::Value::Null,
                deps: Vec::new(),
                tags: vec![STAGE_PERCEIVE.to_string()],
                fail_policy: ComponentFailPolicy::FailClosed,
            },
            ComponentNodeConfig {
                id: STATE_CONTEXT_APPENDER_KIND.to_string(),
                kind: STATE_CONTEXT_APPENDER_KIND.to_string(),
                params: serde_json::Value::Null,
                deps: Vec::new(),
                tags: vec![STAGE_PERCEIVE.to_string()],
                fail_policy: ComponentFailPolicy::FailClosed,
            },
            ComponentNodeConfig {
                id: STATE_CONTEXT_PROVIDER_KIND.to_string(),
                kind: STATE_CONTEXT_PROVIDER_KIND.to_string(),
                params: serde_json::Value::Null,
                deps: vec!["system-prompt".to_string()],
                tags: Vec::new(),
                fail_policy: ComponentFailPolicy::FailClosed,
            },
        ],
    }
}

fn create_personality(name: &str, description: &str, system_prompt: &str) -> Personality {
    Personality {
        id: PersonalityId(Uuid::new_v4()),
        metadata: Metadata {
            name: name.to_string(),
            scope: Scope::GlobalScope,
            description: description.to_string(),
            ..Metadata::default()
        },
        components: create_component_config(system_prompt),
    }
}

async fn setup_templates(store: &Arc<dyn TemplateStore>) -> anyhow::Result<SessionTemplate> {
    let ctx = SecurityContext::default();
    let personality_a = create_personality(
        "jonathan-assistant",
        "A helpful and polite assistant",
        "Your name is Jonathan Heart. You are a helpful assistant."
    );
    store.put_personality(&ctx, &personality_a).await?;

    let personality_b = create_personality(
        "sassy-assistant",
        "A sassy and witty assistant",
        "Your name is Sassy Sarah. You are a witty and slightly sarcastic assistant."
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
        channels: vec![
            ChannelTemplate {
                name: "main".to_string(),
                moderator: "round_robin".to_string(),
                participants: vec![
                    ParticipantTemplate::User,
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
            }
        ],
    };
    store.put_session_template(&ctx, &template).await?;

    Ok(template)
}

async fn run_repl(channel: Arc<ChannelLoop>) -> anyhow::Result<()> {
    let user_actor = channel.user_actor()
        .ok_or_else(|| anyhow::anyhow!("Cannot find user actor in main channel"))?;

    let mut receiver = user_actor.lock_receiver().await;
    let mut input_rx = spawn_rustyline_input();

    println!("--- Chat Session Started (Type 'exit' or 'quit' to stop) ---");
    loop {
        tokio::select! {
            maybe_event = receiver.recv() => {
                match maybe_event {
                    Some(ActorEvent::CompleteMessageGenerated(message)) => {
                        if let Some(content) = message.content {
                            println!("\r{}> {}", message.sender_name, content);
                        }
                    }
                    Some(ActorEvent::FullResponseGenerated(messages)) => {
                        for message in messages {
                            if let Some(content) = message.content {
                                println!("\r{}> {}", message.sender_name, content);
                            }
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
                        info!("dispatch: input msg={:?}", input.to_string());
                        let _ = channel.wakeup().await;
                        user_actor.send_message(input.to_string()).await?;
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

    init_logging();

    let config = if let Some(config_path) = args.config {
        AppConfig::load(config_path)?
    }
    else {
        AppConfig::default()
    };

    let security_manager = Arc::new(caelumora_model::security::DummySecurityManager::new());
    let state_store = config.create_state_store(security_manager.clone())?;
    let template_store = config.create_template_store(security_manager.clone())?;

    let runtime = Arc::new(RuntimeManager::new(state_store.clone(), template_store.clone()));
    let template = setup_templates(&template_store).await?;

    let session_id = SessionId(Uuid::new_v4());
    let owner = UserId::default();
    let security = SecurityContext::default();
    let context = SessionContext {
        id: session_id,
        owner,
        security,
        state_store: state_store.clone(),
        template_store: template_store.clone(),
    };
    let session = template.instantiate("default-session".to_string(), &context).await?;
    let session_loop = Arc::new(SessionLoop::new(session, runtime).await?);
    let runner = tokio::spawn({
        let session_loop = session_loop.clone();
        async move {
            let result = session_loop.run().await;
            if let Err(e) = result {
                error!("Cannot start session loop: {}", e)
            }
        }
    });

    let channel = session_loop
        .channel_by_name("main")
        .await
        .ok_or_else(|| anyhow::anyhow!("Cannot find main channel in session"))?;

    // Start the interactive loop
    run_repl(channel).await?;
    runner.abort();

    Ok(())
}
