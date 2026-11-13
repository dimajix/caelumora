//! The terminal frontend: a user talks, line by line, in a channel of a session of the directory
//! the program was started in (issue 554) - one started from a template, or one of the directory
//! continued or resumed -; or it is asked what a definition's file is to its store (`id`).

mod followed;
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
use caelumora_domain::approval::ApprovalId;
use caelumora_domain::command::{parse_input, UserInput};
use caelumora_domain::config::AppConfig;
use caelumora_domain::project::ProjectId;
use caelumora_domain::query::Query;
use caelumora_domain::question::{Question, QuestionRequest, Response};
use caelumora_domain::store::StoreResultExt;
use caelumora_domain::user::UserId;
use caelumora_domain::value::language::Language;
use caelumora_domain::workspace::{WorkspaceChoice, WorkspaceId};
use caelumora_kernel::actor::user::{ChannelWatch, Lagged};
use caelumora_kernel::actor::Shown;
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
use followed::Followed;
use here::{place_of, HERE};
use home::Home;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
use serde_json::Value;
use sessions::Asked;
use std::env;
use std::future;
use std::io::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
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

/// Has the user talk in the channel `followed`, line by line, until they end it or a signal does;
/// once they no longer take part in it, in none.
#[expect(clippy::print_stdout, reason = "the TUI writes the conversation to the terminal")]
async fn run_repl(followed: Followed) -> Result<()> {
    // The channel followed, with what the user's actor in it shows, until following it ends.
    let watch = followed.actor().watch();
    let mut followed = Some((followed, watch));
    let mut input_rx = spawn_rustyline_input();
    // The questions being answered, with the responses given so far: while they are, every line
    // answers the next question.
    let mut answering: Option<(QuestionRequest, Vec<Response>)> = None;

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
            shown = shown(&mut followed) => {
                let Some((present, _)) = &mut followed else { continue };
                match shown {
                    Ok(Shown::Envelope(envelope)) => {
                        for line in present.take_in(&envelope) {
                            println!("\r{line}");
                        }
                        if let Some((request, _)) =
                            answering.take_if(|(q, _)| !present.is_open(q.id))
                        {
                            println!(
                                "\r--- Questions {} closed before you answered ---",
                                request.id.0
                            );
                        }
                    }
                    // Where a run of the user's actor began or ended, whether they still take part
                    // is read anew (`docs/CHANNELS.md`, issue 386).
                    Ok(Shown::Started | Shown::Ended) => {
                        if let Some(line) = present.ended().await {
                            println!("\r{line}");
                            (followed, answering) = (None, None);
                        }
                    }
                    Err(Lagged(lost)) => println!("\r[{lost} events missed]"),
                }
            }
            maybe_input = input_rx.recv() => {
                let line = match maybe_input {
                    Some(Ok(line)) => line,
                    Some(Err(ReadlineError::Interrupted)) => {
                        println!("CTRL-C");
                        break;
                    }
                    Some(Err(err)) => {
                        println!("Error: {:?}", err);
                        break;
                    }
                    None => break,
                };
                let input = line.trim();
                if input == "exit" || input == "quit" {
                    break;
                }
                match &mut followed {
                    Some((present, _)) => handle_line(input, present, &mut answering).await?,
                    None if input.is_empty() => {}
                    None => println!("You take part in no channel here: type 'exit' to end"),
                }
            }
        }
    }

    Ok(())
}

/// What the channel `followed` shows next, if the terminal follows one; never otherwise.
///
/// # Errors
/// As `ChannelWatch::next`.
async fn shown(followed: &mut Option<(Followed, ChannelWatch)>) -> Result<Shown, Lagged> {
    match followed {
        Some((_, watch)) => watch.next().await,
        None => future::pending().await,
    }
}

/// Does what the user typed, `input`, in the channel `followed`: answers the next of the
/// questions being `answering`, or runs one of this terminal's own commands - `/approve`,
/// `/deny`, `/answer`, `/decline` and `/retry` -, or sends a command or a message to the channel
/// (issue 28).
///
/// # Errors
/// If a message or an approval cannot be sent into the channel.
#[expect(clippy::print_stdout, reason = "the TUI writes the conversation to the terminal")]
async fn handle_line(
    input: &str,
    followed: &mut Followed,
    answering: &mut Option<(QuestionRequest, Vec<Response>)>,
) -> Result<()> {
    if input.is_empty() {
        return Ok(());
    }
    let none_named = |rest: &str| match rest.trim() {
        "" => println!("No open questions put to you"),
        id => println!("No open questions put to you with id {id}"),
    };
    if let Some(rest) =
        input.strip_prefix("/decline").filter(|r| r.is_empty() || r.starts_with(' '))
    {
        match followed.questions_put_to_user(rest) {
            Some(request) => {
                *answering = None;
                if let Err(e) = followed.actor().answer_questions(request.id, None).await {
                    println!("[error] {}", user_message(&e).unwrap_or_else(|| e.to_string()));
                }
            }
            None => none_named(rest),
        }
        return Ok(());
    }
    if let Some(rest) = input.strip_prefix("/answer").filter(|r| r.is_empty() || r.starts_with(' '))
    {
        match followed.questions_put_to_user(rest) {
            Some(request) => {
                if let Some(first) = request.questions.questions.first() {
                    println!("{}", prompt_of(0, first));
                    *answering = Some((request.clone(), Vec::new()));
                }
            }
            None => none_named(rest),
        }
        return Ok(());
    }
    // While questions are answered, each line answers the next.
    if let Some((request, mut responses)) = answering.take() {
        if let Some(question) = request.questions.questions.get(responses.len()) {
            responses.push(response_of(question, input));
        }
        match request.questions.questions.get(responses.len()) {
            Some(next) => {
                println!("{}", prompt_of(responses.len(), next));
                *answering = Some((request, responses));
            }
            None => {
                if let Err(e) = followed.actor().answer_questions(request.id, Some(responses)).await
                {
                    println!("[error] {}", user_message(&e).unwrap_or_else(|| e.to_string()));
                }
            }
        }
        return Ok(());
    }
    if let Some(rest) = input.strip_prefix("/approve ").or_else(|| input.strip_prefix("/deny ")) {
        let approved = input.starts_with("/approve ");
        match Uuid::parse_str(rest.trim()) {
            Ok(id) => {
                if !followed.approve(ApprovalId(id), approved).await? {
                    println!("No pending approval with id {}", id);
                }
            }
            Err(_) => println!("Invalid approval id: {}", rest.trim()),
        }
        return Ok(());
    }
    if input == "/retry" {
        match followed.retry().await {
            Ok(true) => {}
            Ok(false) => println!("No answer of an agent to give again"),
            Err(e) => println!("[error] {}", user_message(&e).unwrap_or_else(|| e.to_string())),
        }
        return Ok(());
    }
    // Every other command goes to the channel.
    match parse_input(input) {
        UserInput::Command(line) => {
            match followed.actor().execute_command(&line).await {
                // Also for an answer to the whole channel: its announcement is not routed back to
                // its sender.
                Ok(reply) => {
                    if let Some(output) = reply.output {
                        println!("{}", output)
                    }
                }
                Err(e) => {
                    // The line is not logged: what follows the command may be what was said. The
                    // error names the command.
                    tracing::warn!("A command failed: {:#}", e);
                    let message = user_message(&e).unwrap_or_else(|| {
                        "The command failed; the details are in the log".to_string()
                    });
                    println!("[error] {}", message);
                }
            }
        }
        UserInput::Message(text) => {
            let span_id = followed.actor().send_message(Some(text), None, Vec::new()).await?;
            tracing::debug!(span_id = %span_id, "Message sent");
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

    // The first channel of the session the user takes part in.
    let followed = session_loop
        .channels()
        .iter()
        .find_map(|channel| Followed::new(channel, views.clone()))
        .ok_or_else(|| {
            anyhow::anyhow!("The user takes part in no channel of the session {session_id}")
        })?;

    // Who answers, as the views tell it (issue 174).
    let view = views.channel(session_id, followed.channel_id()).await?;
    let unknown = || "from a bundle".to_string();
    println!(
        "Moderator: {}, responders: {}",
        view.moderator().unwrap_or_else(unknown),
        view.responder_policy().unwrap_or_else(unknown)
    );

    // Start the interactive loop; however it ends, the session is shut
    // down in order (issue 297): its actors keep what they are in the
    // middle of, and only what does not outlive the process ends with it.
    let ran = run_repl(followed).await;
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
