//! The terminal frontend: a user talks, line by line, in the channels of a session of the
//! directory the program was started in (issue 554) - one started from a template, or one of the
//! directory continued or resumed -, in one at a time (issue 561); or it is asked what a
//! definition's file is to its store (`id`).

mod channels;
mod followed;
mod here;
mod home;
mod printed;
mod screen;
mod sessions;
mod under_way;

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
use caelumora_kernel::actor::user::UserActor;
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
use channels::Channels;
use clap::{Parser, Subcommand};
use followed::Followed;
use here::{place_of, HERE};
use home::Home;
use printed::{ErrorKind, Printed};
use screen::{LineScreen, Screen, Typed};
use serde_json::Value;
use sessions::Asked;
use std::env;
use std::future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::signal::unix::{signal, SignalKind};
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

/// Has the user talk in the channels `channels` follows, line by line, until they end it or a
/// signal does - in the channel talked in, or in none until they switch to one -, shown and read
/// by `screen`.
async fn run_repl(mut channels: Channels, screen: &mut impl Screen) -> Result<()> {
    // The questions being answered, with the responses given so far: while they are, every line
    // but `/switch`, `/answer` and `/decline` answers the next question.
    let mut answering: Option<(QuestionRequest, Vec<Response>)> = None;

    let started = "Chat Session Started (Type 'exit' or 'quit' to stop)".to_string();
    screen.print(vec![Printed::Notice(started)]);
    // A signal ends the loop as `exit` does, so that the session is shut
    // down in order (issue 297).
    let mut signalled = std::pin::pin!(signalled());
    loop {
        tokio::select! {
            () = &mut signalled => {
                screen.print(vec![Printed::Notice("Ending".to_string())]);
                break;
            }
            happened = channels.next() => {
                let mut printed = channels.take_in(happened).await;
                // As the channel that asked them tells; one followed no more has closed them.
                let closed = |(q, _): &mut (QuestionRequest, Vec<Response>)| {
                    channels.followed(q.channel_id).is_none_or(|f| !f.is_to_answer(q.id))
                };
                if let Some((request, _)) = answering.take_if(closed) {
                    printed.push(closed_before_answered(&request));
                }
                screen.print(printed);
            }
            typed = screen.next_input() => {
                let line = match typed {
                    Typed::Line(line) => line,
                    Typed::Interrupted => {
                        screen.print(vec![Printed::Plain("CTRL-C".to_string())]);
                        break;
                    }
                    Typed::Failed(reason) => {
                        screen.print(vec![Printed::Plain(format!("Error: {reason}"))]);
                        break;
                    }
                    Typed::Ended => break,
                };
                let input = line.trim();
                if input == "exit" || input == "quit" {
                    break;
                }
                let printed = handle_line(input, &mut channels, &mut answering).await?;
                screen.print(printed);
            }
        }
    }

    Ok(())
}

/// Does what the user typed, `input`, and answers what the terminal prints of it: switches to
/// another of the `channels` (`/switch`), answers questions put to the user (`answer_line`),
/// answers an approval in the channel where it is pending (`/approve`, `/deny`), or in the channel
/// talked in asks for an answer again (`/retry`) or sends a command or a message (issue 28).
///
/// # Errors
/// As [`send_line`].
async fn handle_line(
    input: &str,
    channels: &mut Channels,
    answering: &mut Option<(QuestionRequest, Vec<Response>)>,
) -> Result<Vec<Printed>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(name) = after(input, "/switch") {
        return Ok(channels.switch(name).await);
    }
    if let Some(printed) = answer_line(input, channels, answering).await {
        return Ok(printed);
    }
    if let Some(rest) = input.strip_prefix("/approve ").or_else(|| input.strip_prefix("/deny ")) {
        let approved = input.starts_with("/approve ");
        let Ok(id) = Uuid::parse_str(rest.trim()).map(ApprovalId) else {
            return Ok(vec![Printed::Plain(format!("Invalid approval id: {}", rest.trim()))]);
        };
        let Some(followed) = channels.awaiting(id) else {
            return Ok(vec![Printed::Plain(format!("No pending approval with id {}", id.0))]);
        };
        let actor = match acting(followed) {
            Ok(actor) => actor,
            Err(no_run) => return Ok(vec![no_run]),
        };
        let failed = actor.send_approval(id, approved).await.err();
        return Ok(failed.map(|e| error_printed(&e)).into_iter().collect());
    }
    let Some(followed) = channels.talked_in() else {
        return Ok(vec![channels.hint()]);
    };
    let actor = match acting(followed) {
        Ok(actor) => actor,
        Err(no_run) => return Ok(vec![no_run]),
    };
    if input == "/retry" {
        return Ok(match followed.retry(&actor).await {
            Ok(true) => Vec::new(),
            Ok(false) => vec![Printed::Plain("No answer of an agent to give again".to_string())],
            Err(e) => vec![error_printed(&e)],
        });
    }
    // Every other command goes to the channel.
    send_line(input, &actor).await
}

/// Answers `input` if it is about questions put to the user, with what the terminal prints of it:
/// declines them (`/decline`), begins to answer them (`/answer`) - questions named by id in
/// whichever of the `channels` knows them, else the last in the channel talked in, as the user's
/// actor there tells -, or answers the next of the questions being `answering`, in the channel
/// that asked them; `None` if it is about none. Between two runs of the user's actor in that
/// channel, only that nothing can be done there now.
async fn answer_line(
    input: &str,
    channels: &Channels,
    answering: &mut Option<(QuestionRequest, Vec<Response>)>,
) -> Option<Vec<Printed>> {
    let none_named = |rest: &str| {
        Printed::Plain(match rest.trim() {
            "" => "No open questions put to you".to_string(),
            id => format!("No open questions put to you with id {id}"),
        })
    };
    // The questions `rest` names, with the user's actor in the channel that asked them - which
    // tells which are open -; what tells why, if there are none, or no run of it.
    let named = |rest: &str| -> Result<(QuestionRequest, Arc<UserActor>), Printed> {
        let asking = channels.asking(rest).ok_or_else(|| none_named(rest))?;
        let actor = acting(asking)?;
        let request = asking.questions_put_to_user(rest).ok_or_else(|| none_named(rest))?;
        Ok((request, actor))
    };
    if let Some(rest) = after(input, "/decline") {
        let printed = match named(rest) {
            Ok((request, actor)) => {
                *answering = None;
                actor.answer_questions(request.id, None).await.err().map(|e| error_printed(&e))
            }
            Err(printed) => Some(printed),
        };
        return Some(printed.into_iter().collect());
    }
    if let Some(rest) = after(input, "/answer") {
        let printed = match named(rest) {
            Ok((request, _)) => {
                let first = request.questions.questions.first();
                let prompt = first.map(|first| Printed::Plain(prompt_of(0, first)));
                if prompt.is_some() {
                    *answering = Some((request, Vec::new()));
                }
                prompt
            }
            Err(printed) => Some(printed),
        };
        return Some(printed.into_iter().collect());
    }
    // While questions are answered, each other line answers the next.
    let (request, mut responses) = answering.take()?;
    if let Some(question) = request.questions.questions.get(responses.len()) {
        responses.push(response_of(question, input));
    }
    if let Some(next) = request.questions.questions.get(responses.len()) {
        let prompt = Printed::Plain(prompt_of(responses.len(), next));
        *answering = Some((request, responses));
        return Some(vec![prompt]);
    }
    // Closed with its channel, if it is followed no more - which `run_repl` tells as it drops it.
    let Some(followed) = channels.followed(request.channel_id) else {
        return Some(vec![closed_before_answered(&request)]);
    };
    let printed = match acting(followed) {
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
    };
    Some(printed)
}

/// What tells that the questions of `request` closed before the user answered them.
fn closed_before_answered(request: &QuestionRequest) -> Printed {
    Printed::Notice(format!("Questions {} closed before you answered", request.id.0))
}

/// What follows `command` at the start of `input`, if `input` begins with it as a word of its own.
fn after<'a>(input: &'a str, command: &str) -> Option<&'a str> {
    input.strip_prefix(command).filter(|rest| rest.is_empty() || rest.starts_with(' '))
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

/// Sends what the user typed, `input`, to the channel as a command or a message, through
/// `actor`, the user's in it, and answers what the terminal prints of it.
///
/// # Errors
/// If a message cannot be sent into the channel.
async fn send_line(input: &str, actor: &UserActor) -> Result<Vec<Printed>> {
    match parse_input(input) {
        UserInput::Command(line) => Ok(match actor.execute_command(&line).await {
            // Also for an answer to the whole channel: its announcement is not routed back to its
            // sender.
            Ok(reply) => reply.output.map(Printed::Plain).into_iter().collect(),
            // The line is not logged: what follows the command may be what was said. The error
            // names the command.
            Err(e) => vec![error_printed(&e)],
        }),
        UserInput::Message(text) => {
            let span_id = actor.send_message(Some(text), None, Vec::new()).await?;
            tracing::debug!(span_id = %span_id, "Message sent");
            Ok(Vec::new())
        }
    }
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
    // Made before a session is opened, so that what fails here needs no shutdown.
    let mut screen = LineScreen::new().context("The terminal cannot be read from")?;
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

    // Every channel of the session the user takes part in, talking in the first.
    let channels =
        Channels::new(Arc::clone(&session_loop), views, kernel.event_bus()).await.ok_or_else(
            || anyhow::anyhow!("The user takes part in no channel of the session {session_id}"),
        )?;
    screen.print(channels.opening().await);

    // Start the interactive loop; however it ends, the session is shut
    // down in order (issue 297): its actors keep what they are in the
    // middle of, and only what does not outlive the process ends with it.
    let ran = run_repl(channels, &mut screen).await;
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
    use crate::followed::testing::from;
    use crate::screen::testing::lines;
    use caelumora_domain::actor::{ActorEvent, ActorId};
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::question::Choice;
    use caelumora_kernel::session::testing::{running_session_of_a_user, test_kernel};
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
        lines(&handle_line(input, channels, answering).await.expect("handled"))
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
        let unknown = "--- You take part in no channel 'nowhere' ---";
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
