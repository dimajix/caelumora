//! Where the terminal program keeps what a person writes and what it writes itself (issue 554): the
//! directories of XDG, the configuration it is built with unless given another, and the files of
//! a first start.

use crate::here::HERE;
use anyhow::{Context, Result};
use caelumora_domain::config::{parse_yaml, AppConfig};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// The default session's template and the personality of its assistant, written on a first start,
/// each where its kind's directory is not there yet.
const FIRST_FILES: [(&str, &str, &str); 2] = [
    ("templates", "default.yaml", include_str!("../defaults/default.yaml")),
    ("personalities", "assistant.yaml", include_str!("../defaults/assistant.yaml")),
];

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
    /// definitions and the secrets in the configuration directory, the workspace `HERE`.
    ///
    /// # Errors
    /// If a directory is named by no text.
    fn defaults(&self, here: &Path) -> Result<Value> {
        let (config, data) = (text_of(&self.config)?, text_of(&self.data)?);
        let home = json!({
            "type": "home",
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
            "secrets": { "providers": [
                { "type": "file", "path": text_of(&self.config.join("secrets.json"))? },
                { "type": "env" },
            ] },
            "persistence": { "entity_store": home, "state_store": home },
            "content": { "storage": { "home": {
                "type": "home",
                "data_dir": data,
                "spaces": { "attachments": "channel", "artifacts": "project" },
            } } },
            "workspaces": { "storage": { HERE: directory } },
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

    /// Writes the default session's template and its assistant's personality, each where the
    /// directory of its kind is not there yet: so that a person who wrote a `config.yaml` first
    /// still gets them, and what the person changed or removed stays so.
    ///
    /// # Errors
    /// If a file cannot be written.
    pub(crate) fn first_start(&self) -> Result<()> {
        for (kind, file, text) in FIRST_FILES {
            let dir = self.config.join(kind);
            if dir.exists() {
                continue;
            }
            let path = dir.join(file);
            let write = || format!("cannot write {}", path.display());
            fs::create_dir_all(&dir).with_context(write)?;
            fs::write(&path, text).with_context(write)?;
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
        assert!(template.is_file(), "also beside a config.yaml written first");
        assert!(home.config.join("personalities/assistant.yaml").is_file(), "and its personality");
        fs::remove_file(&template).unwrap();
        home.first_start().unwrap();
        assert!(!template.exists(), "what the person removed stays removed");
    }
}
