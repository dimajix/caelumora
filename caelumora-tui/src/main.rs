//! The terminal frontend: a user talks, line by line, in the channels of a session of the
//! directory the program was started in (issue 554) - one started from a template, or one of the
//! directory continued or resumed -, in one at a time (issue 561); or it is asked what a
//! definition's file is to its store (`id`).

mod asking;
mod channels;
mod commands;
mod followed;
mod here;
mod home;
mod id_prefix;
mod printed;
mod screen;
mod sessions;
mod talk;
mod typeable;
mod under_way;

use anyhow::Context;
use anyhow::Result;
use caelumora_a2a::library::A2aLibrary;
use caelumora_agents::library::AgentsLibrary;
use caelumora_agents::program::ProgramLibrary;
use caelumora_api::component::Params;
use caelumora_api::library::Library;
use caelumora_artifacts::library::ArtifactsLibrary;
use caelumora_artifacts::plugin::ArtifactsPlugin;
use caelumora_compaction::library::CompactionLibrary;
use caelumora_decision::plugin::DecisionPlugin;
use caelumora_domain::config::AppConfig;
use caelumora_kernel::controller::UserController;
use caelumora_kernel::event::EventBus;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use caelumora_kernel::procedure::follower::AgentFollower;
use caelumora_kernel::procedure::session;
use caelumora_kernel::project::ProjectManager;
use caelumora_kernel::session::{SessionLoop, SessionManager};
use caelumora_kernel::user_provider::Credential;
use caelumora_kernel::view::{Viewer, Views};
use caelumora_logging::{init, LogTarget};
use caelumora_memory::library::MemoryLibrary;
use caelumora_memory::plugin::MemoryPlugin;
use caelumora_model::plugin::ModelPlugin;
use caelumora_model_comfyui::plugin::ComfyUiModelPlugin;
use caelumora_model_ollama::plugin::{with_local_model, OllamaModelPlugin};
use caelumora_model_systemone::plugin::SystemOneModelPlugin;
use caelumora_open_terminal::plugin::OpenTerminalPlugin;
use caelumora_skills::library::SkillsLibrary;
use caelumora_skills::plugin::SkillsPlugin;
use caelumora_store_home::plugin::HomeStorePlugin;
use caelumora_store_home::{HomeStore, HomeStoreConfig, HOME};
use caelumora_store_json::plugin::JsonStorePlugin;
use caelumora_store_postgres::plugin::PostgresStorePlugin;
use caelumora_subagents::library::SubagentsLibrary;
use caelumora_subagents::plugin::SubagentsPlugin;
use caelumora_tool_attachment::library::AttachmentToolLibrary;
use caelumora_tool_image::library::ImageToolLibrary;
use caelumora_tool_mcp::library::McpToolLibrary;
use caelumora_tool_web::library::WebToolLibrary;
use caelumora_tool_workspace::library::WorkspaceToolLibrary;
use clap::{Parser, Subcommand, ValueEnum};
use futures::future::select_all;
use here::{place_of, HERE};
use home::Home;
use printed::{ErrorKind, Printed};
use screen::{FullScreen, LineScreen, Screen, Typed};
use serde_json::Value;
use sessions::{listing, Invocation, Opening, Sessions};
use std::env;
use std::future::{self, Future};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use talk::{Handled, Talk};
use tokio::signal::unix::{signal, Signal, SignalKind};

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

    /// How the terminal is drawn: `full`, the whole window, what was said scrolled above the line
    /// typed; or `line`, line by line, for a screen reader. `line` wherever input or output is no
    /// terminal.
    #[arg(long = "screen", value_enum, default_value_t = Drawing::Full)]
    screen: Drawing,

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

/// Which way of drawing - which `Screen` - the terminal is drawn by, as `--screen` chooses it
/// (issue 569).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Drawing {
    /// The whole window: what was said scrolled above the line typed (`FullScreen`).
    Full,
    /// Line by line, each written once, for a screen reader (`LineScreen`).
    Line,
}

impl Drawing {
    /// The screen that draws the terminal so.
    ///
    /// # Errors
    /// If the terminal cannot be drawn on, or read from.
    fn screen(self) -> Result<Box<dyn Screen>> {
        Ok(match self {
            Drawing::Full => {
                Box::new(FullScreen::new().context("The terminal cannot be drawn on")?)
            }
            Drawing::Line => {
                Box::new(LineScreen::new().context("The terminal cannot be read from")?)
            }
        })
    }
}

impl Args {
    /// The way of drawing asked for - `line` wherever input or output is no terminal, which
    /// cannot be drawn on as a window.
    fn drawing(&self) -> Drawing {
        match io::stdin().is_terminal() && io::stdout().is_terminal() {
            true => self.screen,
            false => Drawing::Line,
        }
    }

    /// What the program is asked to do with the sessions of its directory - one thing at most, as
    /// the options exclude each other.
    fn invocation(&self) -> Invocation {
        match (&self.resume, self.continues, self.sessions) {
            (Some(prefix), _, _) => Invocation::Open(Opening::Resume(prefix.clone())),
            (None, true, _) => Invocation::Open(Opening::Continue),
            (None, false, true) => Invocation::List,
            (None, false, false) => Invocation::Open(Opening::Start { template: None }),
        }
    }
}

/// The plugins the terminal builds its kernel with (issue 323).
fn plugins() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(SkillsPlugin),
        Box::new(ArtifactsPlugin),
        Box::new(MemoryPlugin),
        Box::new(SubagentsPlugin),
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
/// essential one, the agents' (issue 370) -, the last telling the agents where `log` lies, if it is
/// a file (issue 577).
fn libraries(log: LogTarget) -> Vec<Box<dyn Library>> {
    let mut libraries: Vec<Box<dyn Library>> = vec![
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
        Box::new(SubagentsLibrary),
    ];
    if let LogTarget::File(log) = log {
        libraries.push(Box::new(ProgramLibrary::new(log)));
    }
    libraries
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
    let log = init(&config.logging, LogTarget::File(home.log()), "info")?;

    let plugins = plugins();
    let plugins: Vec<&dyn Plugin> = plugins.iter().map(Box::as_ref).collect();
    let mut builder = KernelBuilder::from_config(&config, &plugins).await?;
    for library in libraries(log) {
        builder.library(library.as_ref())?;
    }
    let kernel = builder.build()?;
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
    let controller = kernel.controller_manager().user_controller(owner).await?;
    let security = controller.security_context(Default::default(), Default::default());

    let session_manager = SessionManager::new(Arc::clone(&kernel));
    // Whether the agents of the running sessions exist, followed as long as the terminal runs.
    let _agents_followed = AgentFollower::spawn(&session_manager);
    let projects = ProjectManager::new(Arc::clone(&session_manager));
    // Where the program was started, its project and workspace, if it works there.
    let place = match config.workspaces.storage.contains_key(HERE) {
        true => Some(place_of(&kernel, &projects, &security, owner, &here).await?),
        false => None,
    };
    let views = session_manager.views(Viewer::new(security));
    let sessions =
        Sessions::new(Arc::clone(&kernel), owner, views.clone(), place, args.template.clone());
    let opening = match args.invocation() {
        Invocation::List => {
            for line in listing(&sessions.list().await?, None) {
                println!("{line}");
            }
            return Ok(());
        }
        Invocation::Open(opening) => opening,
    };
    // Made before a session is opened, so that what fails here needs no shutdown.
    let mut screen = args.drawing().screen()?;
    // Caught from before a session is opened, so that no signal ends the program without its
    // shutdown.
    let signalled = signalled();
    let (session_loop, told) = sessions.open(&opening).await?;
    screen.print(told.into_iter().collect());

    // However the talk ends - also where it cannot begin -, the session is shut down in order
    // (issue 297): its actors keep what they are in the middle of, and only what does not outlive
    // the process ends with it.
    let bus = Arc::clone(kernel.event_bus());
    let talked =
        talk(session_loop, views, sessions, controller, bus, screen.as_mut(), signalled).await;
    // The terminal given back first: the shutdown may take a while.
    drop(screen);
    session::shut_down(kernel.procedures()).await;
    talked
}

/// Has the user `views` are read by talk in the channels of `session_loop` they take part in,
/// line by line - in the channel talked in, or in none until they switch to one -, learning from
/// `bus` of those they join meanwhile, shown and read by `screen`: from what the terminal opens
/// with (`Channels::opening`) until they end it, or `signalled` completes - also before the talk
/// began. The `sessions` of the directory are what the commands on sessions reach, the user's
/// controller `user` what keeps the rules they give with an answer.
///
/// # Errors
/// If the user takes part in no channel of the session, or a message cannot be sent into a
/// channel (`Talk::take_line`).
async fn talk(
    session_loop: Arc<SessionLoop>,
    views: Views,
    sessions: Sessions,
    user: Arc<UserController>,
    bus: Arc<EventBus>,
    screen: &mut dyn Screen,
    signalled: impl Future<Output = ()>,
) -> Result<()> {
    let mut signalled = std::pin::pin!(signalled);
    // Every channel of the session the user takes part in, talking in the first.
    let begun = Talk::begin(session_loop, views, bus, sessions, user);
    let (mut talk, opening) = tokio::select! {
        () = &mut signalled => {
            screen.print(vec![Printed::Notice("Ending".to_string())]);
            return Ok(());
        }
        begun = begun => begun?,
    };
    screen.print(opening);
    let started =
        "Chat Session Started (Type '/exit' to stop, '/help' for the commands)".to_string();
    screen.print(vec![Printed::Notice(started)]);
    // What is under way and asked as the channels were followed, before anything happens in them.
    screen.show_under_way(talk.under_way());
    screen.ask(talk.asked());
    screen.offer(talk.typeable());
    loop {
        let printed = tokio::select! {
            // A signal ends the talk as `/exit` does, so that the session is shut down in order
            // (issue 297).
            () = &mut signalled => {
                screen.print(vec![Printed::Notice("Ending".to_string())]);
                break;
            }
            occurred = talk.next() => talk.take_in(occurred).await,
            typed = screen.next_input() => match typed {
                Typed::Line(line) => match talk.take_line(line.trim()).await? {
                    Handled::Printed(printed) => printed,
                    Handled::Exit => break,
                },
                Typed::Chosen(chosen) => talk.choose(&chosen).await,
                Typed::PutAside => {
                    talk.put_aside();
                    Vec::new()
                }
                Typed::TakenUp => {
                    talk.take_up();
                    Vec::new()
                }
                // Ctrl-C ends the command run in the workspace, where one is under way (issue 587),
                // else the program.
                Typed::Interrupted => match talk.end_shell_command() {
                    Some(ended) => vec![ended],
                    None => {
                        screen.print(vec![Printed::Plain("CTRL-C".to_string())]);
                        break;
                    }
                },
                Typed::Failed(message) => {
                    screen.print(vec![Printed::Error { kind: ErrorKind::Failed, message }]);
                    break;
                }
                Typed::Ended => break,
            },
        };
        screen.print(printed);
        // Another channel may be talked in now, something else asked, and other things typed.
        screen.show_under_way(talk.under_way());
        screen.ask(talk.asked());
        screen.offer(talk.typeable());
    }
    Ok(())
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

/// What completes on SIGINT, SIGTERM or SIGHUP - the terminal closed -, caught from this call
/// on, not only once it is awaited; never, should none of them be caught.
fn signalled() -> impl Future<Output = ()> {
    let caught = |kind: SignalKind| match signal(kind) {
        Ok(caught) => Some(caught),
        Err(e) => {
            tracing::warn!(
                "A signal cannot be caught, and ends the program without a shutdown: {e}"
            );
            None
        }
    };
    let kinds = [SignalKind::interrupt(), SignalKind::terminate(), SignalKind::hangup()];
    let mut signals: Vec<Signal> = kinds.into_iter().filter_map(caught).collect();
    async move {
        if signals.is_empty() {
            return future::pending().await;
        }
        select_all(signals.iter_mut().map(|caught| Box::pin(caught.recv()))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::testing::{in_directory, InDirectory};
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::user::UserId;
    use caelumora_security::SecurityContext;
    use clap::CommandFactory;

    /// The options of the program fit together: no name taken twice, and one thing asked at most.
    #[test]
    fn the_options_fit_together() {
        Args::command().debug_assert();
        let resumed = Args::try_parse_from(["caelumora", "--resume", "0a1b"]).unwrap();
        let resumes =
            |asked| matches!(asked, Invocation::Open(Opening::Resume(prefix)) if prefix == "0a1b");
        assert!(resumes(resumed.invocation()));
        let started = Args::try_parse_from(["caelumora"]).unwrap().invocation();
        assert!(matches!(started, Invocation::Open(Opening::Start { template: None })));
        assert!(Args::try_parse_from(["caelumora", "--continue", "--sessions"]).is_err());
        assert!(Args::try_parse_from(["caelumora", "--resume", "0a1b", "--sessions"]).is_err());
        let id = Args::try_parse_from(["caelumora", "id", "personalities/a.yaml"]).unwrap();
        assert!(matches!(
            id.command,
            Some(Command::Id { file }) if file == Path::new("personalities/a.yaml")
        ));
    }

    /// Who takes part in no channel of the session cannot talk in it: the talk ends at once, with
    /// why, for its caller to shut the session down after.
    #[tokio::test]
    async fn who_takes_part_in_no_channel_cannot_talk() {
        let InDirectory { sessions, manager, kernel, owner, directory: _directory } =
            in_directory().await;
        let start = Opening::Start { template: None };
        let (session_loop, _) = sessions.open(&start).await.expect("started");
        // Someone outside the session, whom `Channels` follows no channel for, as it follows none
        // for a participant of the session in none of its channels.
        let stranger = ActorId::User(UserId::new());
        let stranger = SecurityContext::new(stranger, session_loop.id(), Default::default());
        let views = manager.views(Viewer::new(stranger));
        let mut screen = LineScreen::new().expect("a screen");
        let bus = Arc::clone(kernel.event_bus());
        let signalled = future::pending();
        // The owner's controller: whose rules matter not, the talk never beginning.
        let ActorId::User(owner) = owner.caller_id else { unreachable!("a user") };
        let user = kernel.controller_manager().user_controller(owner).await.expect("theirs");
        let talked = talk(session_loop, views, sessions, user, bus, &mut screen, signalled).await;
        let error = talked.expect_err("no channel to talk in");
        assert!(error.to_string().contains("takes part in no channel"), "{error}");
    }
}
