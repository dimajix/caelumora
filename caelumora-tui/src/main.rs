use caelumora_model::agent::Personality;
use caelumora_model::driver::memory::MemoryStore;
use caelumora_model::message::{Message, MessageEvent, MessageProcessor, MessageRole};
use caelumora_model::meta::{Metadata, Scope};
use caelumora_model::session::SessionContext;
use caelumora_model::store::{
    StoreBackedEntityResolver, StoreResult, TemplateStore,
};
use caelumora_model::template::{ParticipantTemplate, ScopeTemplate, SessionTemplate};
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionLoop;
use chrono::Utc;
use clap::Parser;
use env_logger::WriteStyle;
use futures_util::StreamExt;
use log::{info, LevelFilter};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;
use caelumora_runtime::runner::LoopRunner;

fn init_logging() {
    let mut builder = env_logger::Builder::new();

    builder
        .filter(None, LevelFilter::Info)
        .write_style(WriteStyle::Always)
        .init();
}

fn setup_templates(store: &Arc<MemoryStore>) -> anyhow::Result<SessionTemplate> {
    let personality = Personality {
        id: Uuid::new_v4(),
        metadata: Metadata {
            name: "simple-assistant".to_string(),
            scope: Scope::GlobalScope,
            description: "Simple helper personality".to_string(),
            ..Metadata::default()
        },
    };
    store.put_personality(&personality)?;

    let template = SessionTemplate {
        id: Uuid::new_v4(),
        metadata: Metadata {
            name: "simple-session".to_string(),
            scope: Scope::GlobalScope,
            description: "Simple user+assistant session".to_string(),
            ..Metadata::default()
        },
        scope: ScopeTemplate::GlobalScope,
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
    };
    store.put_session_template(&template)?;

    Ok(template)
}

async fn run_repl(mut session: LoopRunner) -> anyhow::Result<()> {
    let mut rl = Editor::<(), rustyline::history::DefaultHistory>::new()?;
    //rl.set_class_name("chat-agent-class");
    //rl.append_history(vec![])?;

    println!("--- Chat Session Started (Type 'exit' or 'quit' to stop) ---");

    loop {
        let readline = rl.readline(">> ");
        match readline {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }
                if input == "exit" || input == "quit" {
                    break;
                }

                // Perform the agent response
                let user_message = Message {
                    id: Uuid::new_v4(),
                    session_id: session.id(),
                    sender_id: Uuid::nil(),
                    sender_name: "user".to_string(),
                    role: MessageRole::User,
                    content: Some(input.to_string()),
                    reasoning: None,
                    tool_calls: vec![],
                    timestamp: Utc::now(),
                    metadata: Default::default(),
                };

                info!("dispatch: input msg={:?}", user_message);

                let mut stream = session.send_messages(vec![user_message]).await;
                while let Some(event) = stream.next().await {
                    match event {
                        Ok(MessageEvent::Partial(_message)) => {
                        }
                        Ok(MessageEvent::Complete(message)) => {
                            if let Some(content) = message.content {
                                println!("{}> {}", message.sender_name, content);
                            }
                        }
                        Ok(MessageEvent::EndOfTurn(_messages)) => {}
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            break;
                        }
                    }
                }

                rl.add_history_entry(input)?;
            }
            Err(ReadlineError::Interrupted) => {
                println!("CTRL-C");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
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
        id: Uuid::new_v4(),
        owner: Uuid::nil(),
        resolver: Box::new(StoreBackedEntityResolver::new(store.clone(), store.clone())),
    };
    let session = template.instantiate("default-session".to_string(), &context)?;
    let session_loop = SessionLoop::new(session, runtime)?;
    let (session_runner, _) = LoopRunner::spawn(Box::new(session_loop));

    // Start the interactive loop
    run_repl(session_runner).await?;

    Ok(())
}
