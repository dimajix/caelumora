//! Where the terminal program keeps what a person writes and what it writes itself (issue 554): the
//! directories of XDG, the configuration it is built with unless given another, and the files of
//! a first start.

use crate::here::HERE;
use anyhow::{Context, Result};
use caelumora_domain::command::default_commands;
use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
use caelumora_domain::config::{parse_yaml, AppConfig};
use caelumora_kernel::command::SHELL_KIND;
use caelumora_notes::command::AGENT_NOTES_KIND;
use caelumora_store_home::HOME;
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// The files of a first start by the directory of their kind - the default session's template, the
/// personality of its assistant and that of the sub-agents the assistant starts (issue 609) -,
/// those of a kind written together where its directory is not there yet.
const FIRST_FILES: [(&str, &[(&str, &str)]); 2] = [
    ("templates", &[("default.yaml", include_str!("../defaults/default.yaml"))]),
    (
        "personalities",
        &[
            ("assistant.yaml", include_str!("../defaults/assistant.yaml")),
            ("helper.yaml", include_str!("../defaults/helper.yaml")),
        ],
    ),
];

/// The commands of a session of the program unless its configuration says otherwise: those every
/// session has by default, `/workspace run` besides (issue 587), which `!` stands for, and
/// `/channel notes` of the notes (issue 607).
pub(crate) fn commands() -> ComponentConfig {
    let mut commands = default_commands();
    commands.component_nodes.push(ComponentDescriptor::new("shell", SHELL_KIND));
    commands.component_nodes.push(ComponentDescriptor::new("agent-notes", AGENT_NOTES_KIND));
    commands
}

/// The directories of the program, each `caelumora` below the one of XDG: what the person writes,
/// what the program writes, and its log.
#[derive(Debug)]
pub(crate) struct Home {
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
}

/// The directory `XDG_<name>_HOME` names, or `fallback` below the person's home.
fn xdg(name: &str, fallback: &str) -> Result<PathBuf> {
    let named = env::var_os(format!("XDG_{name}_HOME")).map(PathBuf::from);
    let dir = match named.filter(|dir| dir.is_absolute()) {
        Some(dir) => dir,
        None => env::home_dir().context("no home directory is known")?.join(fallback),
    };
    Ok(dir.join("caelumora"))
}

/// `path` as a text of a configuration.
///
/// # Errors
/// If it is no text.
fn text_of(path: &Path) -> Result<Value> {
    let text = path.to_str().with_context(|| format!("{} is no text", path.display()))?;
    Ok(Value::String(text.to_string()))
}

/// Merges `over` into `base`, what a person wrote over the defaults: a mapping key by key, at
/// every depth, but a backend of another `type` whole, since the default's parameters are not
/// its; a list and anything else whole; nothing said (`null`) changes nothing.
fn merge(base: &mut Value, over: Value) {
    match (base, over) {
        (_, Value::Null) => {}
        (Value::Object(base), Value::Object(over))
            if over.get("type").is_none_or(|kind| base.get("type") == Some(kind)) =>
        {
            for (key, value) in over {
                match base.get_mut(&key) {
                    Some(inner) => merge(inner, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, over) => *base = over,
    }
}

impl Home {
    /// The person's directories, as XDG says.
    ///
    /// # Errors
    /// If no home directory is known.
    pub(crate) fn of_user() -> Result<Self> {
        Ok(Self {
            config: xdg("CONFIG", ".config")?,
            data: xdg("DATA", ".local/share")?,
            state: xdg("STATE", ".local/state")?,
        })
    }

    /// The file the program logs into.
    pub(crate) fn log(&self) -> PathBuf {
        self.state.join("tui.log")
    }

    /// What the program is built with unless told otherwise, for the directory `here` it works in:
    /// everything allowed, the person's own name accepted, its data below the data directory, the
    /// definitions and the secrets in the configuration directory, the workspace `HERE`, and its
    /// `commands`.
    ///
    /// # Errors
    /// If a directory is named by no text.
    fn defaults(&self, here: &Path) -> Result<Value> {
        let (config, data) = (text_of(&self.config)?, text_of(&self.data)?);
        let home = json!({
            "type": HOME,
            "config_dir": config,
            "data_dir": data,
            "definitions": {
                "skill": { "dir": "skills", "format": { "markdown": { "body": "body" } } },
                "personality": { "dir": "personalities", "format": "yaml" },
                "component_bundle": { "dir": "bundles", "format": "yaml" },
                "session_template": { "dir": "templates", "format": "yaml" },
            },
            "within": { "memory": "agent", "user_profile": "user" },
        });
        let directory = json!({
            "type": "directory",
            // Not `workspaces`, where the documents of the workspaces lie.
            "root": text_of(&self.data.join("links"))?,
            "dir": text_of(here)?,
            "shell": true,
            "destination": { "level": "strictly_confidential" },
            "environment": "inherit",
        });
        Ok(json!({
            "security": { "type": "allow_all" },
            "users": { "providers": [{ "type": "accept_any" }] },
            "secrets": { "vaults": {
                "local": {
                    "type": "file",
                    "path": text_of(&self.config.join("secrets.json"))?,
                    "fallback": "env",
                },
                "env": { "type": "env" },
            } },
            "persistence": { "entity_store": home, "state_store": home },
            "content": { "storage": { "home": {
                "type": HOME,
                "data_dir": data,
                "spaces": { "attachments": "channel", "artifacts": "project" },
            } } },
            "workspaces": { "storage": { HERE: directory } },
            "commands": commands(),
        }))
    }

    /// The configuration the program is built with unless given another: the defaults, and over
    /// them what `config.yaml` of the configuration directory says (`merge`).
    ///
    /// # Errors
    /// If `config.yaml` cannot be read, or what it makes is no configuration.
    pub(crate) fn config(&self, here: &Path) -> Result<AppConfig> {
        let mut config = self.defaults(here)?;
        let own = self.config.join("config.yaml");
        let read = || format!("cannot read {}", own.display());
        if own.is_file() {
            let text = fs::read_to_string(&own).with_context(read)?;
            merge(&mut config, parse_yaml(&text).with_context(read)?);
        }
        AppConfig::from_yaml(&config.to_string())
            .with_context(|| format!("{} over the defaults is no configuration", own.display()))
    }

    /// Writes the files of a first start (`FIRST_FILES`), those of a kind together where the
    /// directory of the kind is not there yet: so that a person who wrote a `config.yaml` first
    /// still gets them, and what the person changed or removed stays so.
    ///
    /// # Errors
    /// If a directory or a file cannot be written.
    pub(crate) fn first_start(&self) -> Result<()> {
        for (kind, files) in FIRST_FILES {
            let dir = self.config.join(kind);
            if dir.exists() {
                continue;
            }
            fs::create_dir_all(&dir).with_context(|| format!("cannot write {}", dir.display()))?;
            for (file, text) in files {
                let path = dir.join(file);
                fs::write(&path, text)
                    .with_context(|| format!("cannot write {}", path.display()))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(dir: &Path) -> Home {
        Home { config: dir.join("config"), data: dir.join("data"), state: dir.join("state") }
    }

    /// The configuration of `home` with `written` as its `config.yaml`.
    fn configured(home: &Home, written: &str) -> AppConfig {
        fs::create_dir_all(&home.config).unwrap();
        fs::write(home.config.join("config.yaml"), written).unwrap();
        home.config(Path::new("/")).unwrap()
    }

    #[test]
    fn what_the_person_writes_lies_over_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());

        let config = configured(&home, "workspaces:\n  storage:\n    here:\n      shell: false\n");

        let here = &config.workspaces.storage[HERE];
        assert_eq!(here.kind, "directory", "the rest of the default stays");
        assert_eq!(here.params["shell"], json!(false), "what is written wins");
        assert_eq!(config.persistence.entity_store.kind, "home", "and the other sections");
    }

    /// Issue 587: a session has the commands every session has by default, `/workspace run`, which
    /// `!` stands for, and `/channel notes` (issue 607).
    #[test]
    fn a_session_runs_commands_in_the_directory_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = home(dir.path()).config(Path::new("/")).unwrap();

        assert_eq!(config.commands, commands());
        let kinds: Vec<&str> =
            config.commands.component_nodes.iter().map(|c| c.kind.as_str()).collect();
        let defaults = default_commands();
        let by_default = defaults.component_nodes.iter().map(|c| c.kind.as_str());
        for kind in by_default.chain([SHELL_KIND, AGENT_NOTES_KIND]) {
            assert!(kinds.contains(&kind), "{kind} in {kinds:?}");
        }
    }

    #[test]
    fn a_backend_of_another_type_takes_nothing_of_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());

        let config =
            configured(&home, "persistence:\n  entity_store:\n    type: json\n    path: x\n");

        let store = &config.persistence.entity_store;
        assert_eq!(store.kind, "json");
        assert_eq!(store.params.keys().collect::<Vec<_>>(), ["path"], "nothing of `home`'s");
    }

    #[test]
    fn what_says_nothing_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        for written in ["", "# nothing yet\n", "content:\n"] {
            let config = configured(&home, written);
            assert_eq!(config.persistence.entity_store.kind, "home", "{written:?}");
            assert!(config.content.storage.contains_key("home"), "{written:?}");
        }
    }

    #[test]
    fn the_first_start_writes_what_a_session_needs_and_no_later_one() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        fs::create_dir_all(&home.config).unwrap();
        fs::write(home.config.join("config.yaml"), "").unwrap();

        home.first_start().unwrap();

        let template = home.config.join("templates/default.yaml");
        let helper = home.config.join("personalities/helper.yaml");
        assert!(template.is_file(), "also beside a config.yaml written first");
        assert!(home.config.join("personalities/assistant.yaml").is_file(), "and its personality");
        assert!(helper.is_file(), "and that of its sub-agents, beside it");
        fs::remove_file(&template).unwrap();
        fs::remove_file(&helper).unwrap();
        home.first_start().unwrap();
        assert!(!template.exists(), "what the person removed stays removed");
        assert!(!helper.exists(), "also beside what the person kept of its kind");
    }

    /// The files a first start writes have nothing wrong with them; a field a person mistypes in
    /// one of their own is told with its file.
    #[tokio::test]
    async fn what_is_wrong_with_the_definitions_a_person_wrote_is_told() {
        use crate::printed::Printed;
        use crate::problems_of_definitions;
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        home.first_start().unwrap();
        let config = home.config(dir.path()).unwrap();

        let problems = problems_of_definitions(&config).await.unwrap();

        assert!(problems.is_empty(), "nothing wrong with the first start's: {problems:?}");
        let mistyped = home.config.join("personalities/mistyped.yaml");
        fs::write(&mistyped, "descripton: A typo\n").unwrap();
        let problems = problems_of_definitions(&config).await.unwrap();
        let told = format!("{}: fields not read: descripton", mistyped.display());
        assert!(
            matches!(&problems[..], [Printed::Notice(notice)] if *notice == told),
            "{problems:?}"
        );
    }

    /// Issue 609: the default session started from the files of a first start, in a kernel built
    /// as the program builds it, has an assistant offered to start sub-agents of the helper
    /// written beside it, and the helper can be built.
    #[tokio::test]
    async fn the_assistant_of_a_first_start_is_offered_sub_agents_of_its_helper() {
        use crate::sessions::Opening;
        use crate::{kernel_of, sessions_for};
        use caelumora_domain::scope::Scope;
        use caelumora_domain::value::name::Name;
        use caelumora_kernel::component::BuildContext;
        use caelumora_kernel::procedure::session;
        use caelumora_logging::LogTarget;
        use caelumora_model_ollama::plugin::with_local_model;
        use caelumora_security::SecurityContext;
        use caelumora_subagents::component::START_SUBAGENT;
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        let here = dir.path().join("here");
        fs::create_dir_all(&here).unwrap();
        home.first_start().unwrap();
        let config = with_local_model(home.config(&here).unwrap());
        let kernel = kernel_of(&config, LogTarget::File(home.log())).await.unwrap();
        let template = "default".to_string();
        let (_, sessions) = sessions_for(&kernel, &config, &here, "user", template).await.unwrap();

        let started = sessions.open(&Opening::Start { template: None }).await;
        let (session_loop, _) = started.expect("started from the template written");
        let main = session_loop.channel_named("main").expect("its channel");
        let views = sessions.views();
        let channel = views.channel(session_loop.id(), main.id()).await.unwrap();
        let participants = channel.participants().await;
        let assistant = participants.iter().find(|p| p.handle().as_deref() == Some("assistant"));
        let tool_sets = assistant.expect("the assistant").tool_sets().await.unwrap();
        let subagents = tool_sets.iter().find(|set| set.name == "subagents").expect("its set");
        assert!(subagents.unavailable.is_none(), "available: {:?}", subagents.unavailable);
        let start = subagents.tools.iter().find(|tool| tool.name == START_SUBAGENT);
        let start = start.expect("starting one");
        assert!(start.description.contains("helper"), "of the helper: {}", start.description);
        // The helper found by its name, as a sub-agent's personality is, and built as its actor
        // asks (`every_personality_of_the_example_seed_can_be_built` of `caelumora-app`).
        let place = Scope::SessionScope { session_id: session_loop.id() };
        let helper = Name::new("helper").unwrap();
        let helper = views.personality_named(&helper, &place).await.expect("the helper");
        let asked = kernel.agent_kinds().facets(helper.kind).expect("a kind the program runs");
        let context = BuildContext::new(SecurityContext::system(), kernel.services().clone());
        let built = kernel.component_registry().build(&helper.components, asked, &context).await;
        built.expect("the helper's components built");
        session::shut_down(kernel.procedures()).await;
    }
}
