use caelumora_model::agent::Personality;
use caelumora_model::driver::memory::MemoryStore;
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::meta::{Metadata, Scope};
use caelumora_model::session::{SessionContext, SessionId};
use caelumora_model::store::{
    StoreBackedEntityResolver, TemplateStore,
};
use caelumora_model::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId};
use caelumora_model::user::UserId;
use caelumora_model::agent::PersonalityId;
use caelumora_runtime::actor::{Actor, ActorEvent};
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionLoop;
use chrono::Utc;
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
use caelumora_runtime::actor_impl::user::UserActor;

fn init_logging() {
    let mut builder = env_logger::Builder::new();

    builder
        .filter(None, LevelFilter::Info)
        .write_style(WriteStyle::Always)
        .init();
}

fn setup_templates(store: &Arc<MemoryStore>) -> anyhow::Result<SessionTemplate> {
    let personality = Personality {
        id: PersonalityId(Uuid::new_v4()),
        metadata: Metadata {
            name: "simple-assistant".to_string(),
            scope: Scope::GlobalScope,
            description: "Simple helper personality".to_string(),
            ..Metadata::default()
        },
    };
    store.put_personality(&personality)?;

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
                        name: "assistant a".to_string(),
                        scope: ScopeTemplate::GlobalScope,
                        personality_id: personality.id,
                    },
                    ParticipantTemplate::Agent {
                        name: "assistant b".to_string(),
                        scope: ScopeTemplate::GlobalScope,
                        personality_id: personality.id,
                    },
                ],
            }
        ],
    };
    store.put_session_template(&template)?;

    Ok(template)
}

async fn run_repl(user_actor: Arc<UserActor>) -> anyhow::Result<()> {
    let mut receiver = user_actor.lock_receiver().await;
    let mut input_rx = spawn_rustyline_input();

    println!("--- Chat Session Started (Type 'exit' or 'quit' to stop) ---");
    run_repl_select_loop(user_actor.clone(), &mut receiver, &mut input_rx).await
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

async fn run_repl_select_loop(
    user_actor: Arc<UserActor>,
    receiver: &mut mpsc::Receiver<ActorEvent>,
    input_rx: &mut mpsc::UnboundedReceiver<Result<String, ReadlineError>>,
) -> anyhow::Result<()> {
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

                        let user_message = Message {
                            id: MessageId(Uuid::new_v4()),
                            prev_id: MessageId::default(),
                            sender_id: user_actor.id(),
                            sender_name: "user".to_string(),
                            role: MessageRole::User,
                            content: Some(input.to_string()),
                            reasoning: None,
                            tool_calls: vec![],
                            timestamp: Utc::now(),
                            metadata: Default::default(),
                        };

                        info!("dispatch: input msg={:?}", user_message);
                        user_actor
                            .send_message(ActorEvent::FullResponseGenerated(vec![user_message]))
                            .await?;
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

#[derive(Parser, Debug)]
#[command(author, version, about = "An accessible Chat CLI")]
struct Args {
    /// Path to a specific config file
    #[arg(short, long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _args = Args::parse();

    // Load Config
    //let config = load_config(args.config).context("Failed to load configuration")?;

    // Override config with CLI args
    //let final_name = args.name.unwrap_or(config.agent_name.clone());
    init_logging();

    // Initialize Agent (Integration Point)
    // In a real app, you would inject your OpenAI/Anthropic client here.
    let store = Arc::new(MemoryStore::new());
    let runtime = Arc::new(RuntimeManager::new(store.clone()));
    let template = setup_templates(&store)?;

    let context = SessionContext {
        id: SessionId(Uuid::new_v4()),
        owner: UserId(Uuid::nil()),
        resolver: Box::new(StoreBackedEntityResolver::new(store.clone(), store.clone())),
    };
    let session = template.instantiate("default-session".to_string(), &context)?;
    let session_loop = Arc::new(SessionLoop::new(session, runtime)?);
    let runner = tokio::spawn({
        let session_loop = session_loop.clone();
        async move {
            let result = session_loop.run().await;
            if let Err(e) = result {
                error!("Cannot start session loop: {}", e)
            }
        }
    });

    let user_actor = session_loop
        .channel_by_name("main")
        .and_then(|channel| channel.user_actor())
        .ok_or_else(|| anyhow::anyhow!("Cannot find user actor in main channel"))?;

    // Start the interactive loop
    run_repl(user_actor).await?;
    runner.abort();

    Ok(())
}
