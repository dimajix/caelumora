//! The texts the runtime gives models (issue 57): prompt parts, and
//! whatever else a model is told in prose, kept as templates apart from the
//! code that fills them in, so that a text can be tuned without a change of
//! code.
//!
//! Every crate and plugin brings the texts it writes, a file of its own
//! compiled in (`include_str!`) and added under its namespace when it
//! registers its components: `agents.channel_mentions` is the text
//! `channel_mentions` of the agents. The catalogue is a service of the
//! runtime (`RuntimeBuilder::prompts`); whoever writes for a model gets it
//! when it is made and keeps it.
//!
//! A text is a whole part - the roster with its participants, the notes with
//! their sections -, a template of `minijinja` (Jinja2) that may branch and
//! loop over the values it is given. The code decides whether a part is
//! given at all and with which values; the template only how it reads, and
//! computes nothing the code could hand it. A value the template names and
//! is not given is an error, not an empty field.
//!
//! A block tag on a line of its own leaves no line behind (`trim_blocks`,
//! `lstrip_blocks`); one that ends a line of text takes its line break with
//! it, unless written `+%}`. A text ends in the line break its file gives it.

use crate::config::parse_yaml;
use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The values a text is rendered with, by name: `context!{ name => "Sassy" }`.
pub use minijinja::context;

/// The texts given to models, by key: those of every namespace added.
#[derive(Debug)]
pub struct PromptCatalogue {
    templates: Environment<'static>,
    /// The namespaces added, so that none is added twice.
    namespaces: BTreeSet<String>,
}

impl Default for PromptCatalogue {
    fn default() -> Self {
        Self::new()
    }
}

impl PromptCatalogue {
    /// A catalogue without texts.
    pub fn new() -> Self {
        let mut templates = Environment::new();
        // Plain text for a model, not HTML: nothing is escaped.
        templates.set_auto_escape_callback(|_| AutoEscape::None);
        templates.set_undefined_behavior(UndefinedBehavior::Strict);
        // A block tag on a line of its own leaves no line behind, and a text
        // ends in the line break its file gives it.
        templates.set_trim_blocks(true);
        templates.set_lstrip_blocks(true);
        templates.set_keep_trailing_newline(true);
        Self { templates, namespaces: BTreeSet::new() }
    }

    /// Adds the texts of `namespace` from `yaml`, a map of their keys to
    /// their templates - or to maps of further keys, which group texts:
    /// `tools: { notes: { write_note: … } }` is the text
    /// `tools.notes.write_note`.
    ///
    /// # Errors
    /// `PromptError::Unreadable` if `yaml` is not such a map;
    /// `PromptError::NamespaceTaken` if `namespace` was added before;
    /// `PromptError::Invalid` naming the first text whose template does not
    /// parse.
    pub fn add(&mut self, namespace: &str, yaml: &str) -> Result<(), PromptError> {
        let texts: BTreeMap<String, Entry> = parse_yaml(yaml)
            .map_err(|cause| PromptError::Unreadable { namespace: namespace.to_string(), cause: cause.to_string() })?;
        if !self.namespaces.insert(namespace.to_string()) {
            return Err(PromptError::NamespaceTaken(namespace.to_string()));
        }
        self.add_entries(namespace, texts)
    }

    /// Adds the texts of `entries` under `prefix`.
    fn add_entries(&mut self, prefix: &str, entries: BTreeMap<String, Entry>) -> Result<(), PromptError> {
        for (key, entry) in entries {
            let key = format!("{prefix}.{key}");
            match entry {
                Entry::Text(template) => self.templates.add_template_owned(key.clone(), template).map_err(|cause| PromptError::Invalid { key, cause })?,
                Entry::Group(entries) => self.add_entries(&key, entries)?,
            }
        }
        Ok(())
    }

    /// The text `key` - `namespace.key` -, rendered with `values`.
    ///
    /// # Errors
    /// `PromptError::Unknown` if the catalogue has no text `key`;
    /// `PromptError::Invalid` if it cannot be rendered with `values` - one
    /// it names is not among them.
    pub fn render(&self, key: &str, values: impl Serialize) -> Result<String, PromptError> {
        let template = self.templates.get_template(key).map_err(|_| PromptError::Unknown(key.to_string()))?;
        template.render(values).map_err(|cause| PromptError::Invalid { key: key.to_string(), cause })
    }
}

/// An entry of a file of texts: a text, or a group of further entries.
enum Entry {
    Text(String),
    Group(BTreeMap<String, Entry>),
}

/// A text or a map, nothing else: a text that reads as a number - `5` - is
/// to be quoted, rather than read back as another text - `5.0` as `5`.
impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntryVisitor;
        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = Entry;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a text, or a map of further texts")
            }
            fn visit_str<E>(self, text: &str) -> Result<Entry, E> {
                Ok(Entry::Text(text.to_string()))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entry, A::Error> {
                let mut entries = BTreeMap::new();
                while let Some((key, entry)) = map.next_entry()? {
                    entries.insert(key, entry);
                }
                Ok(Entry::Group(entries))
            }
        }
        deserializer.deserialize_any(EntryVisitor)
    }
}

/// Why a text of the catalogue cannot be had: a fault of the texts or of
/// the code that renders them, not of what a model or a person did.
#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    #[error("the prompt catalogue has no text '{0}'")]
    Unknown(String),
    #[error("the texts of '{namespace}' cannot be read: {cause}")]
    Unreadable { namespace: String, cause: String },
    #[error("the texts of '{0}' are added twice")]
    NamespaceTaken(String),
    #[error("the text '{key}' cannot be rendered")]
    Invalid { key: String, #[source] cause: minijinja::Error },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogue(yaml: &str) -> PromptCatalogue {
        let mut catalogue = PromptCatalogue::new();
        catalogue.add("agents", yaml).unwrap();
        catalogue
    }

    #[test]
    fn a_text_is_rendered_by_its_namespace_and_key_with_loops_and_conditions() {
        let catalogue = catalogue(
            "roster: |\n  [Who is here]\n  {% for other in others %}\n  - {{ other.name }}{{ \", a person\" if other.person }}\n  {% endfor %}\n",
        );
        let text = catalogue.render("agents.roster", context! { others => vec![context! { name => "Kaya", person => true }, context! { name => "Sassy", person => false }] }).unwrap();
        assert_eq!(text, "[Who is here]\n- Kaya, a person\n- Sassy\n", "block tags leave no lines behind, the text ends in its line break");
    }

    #[test]
    fn a_group_names_the_texts_it_holds() {
        let tools = catalogue("tools:\n  notes:\n    description: Notes.\n    write_note.text: What to note\n");
        assert_eq!(tools.render("agents.tools.notes.description", ()).unwrap(), "Notes.");
        assert_eq!(tools.render("agents.tools.notes.write_note.text", ()).unwrap(), "What to note", "a key may hold dots of its own");
        let choices = catalogue("choices:\n  yes: y\n  no: \"no\"\n");
        assert_eq!(choices.render("agents.choices.yes", ()).unwrap(), "y", "only true and false are no texts");
    }

    #[test]
    fn nothing_is_escaped_and_a_value_that_is_not_given_is_an_error() {
        let catalogue = catalogue("said: \"{{ name }} said <{{ text }}>\"\n");
        assert_eq!(catalogue.render("agents.said", context! { name => "Kaya", text => "a & b" }).unwrap(), "Kaya said <a & b>");
        let refused = catalogue.render("agents.said", context! { name => "Kaya" }).expect_err("no text given");
        assert!(matches!(&refused, PromptError::Invalid { key, .. } if key == "agents.said"), "{refused}");
    }

    #[test]
    fn what_cannot_be_had_is_named() {
        let catalogue = catalogue("said: x\n");
        assert_eq!(catalogue.render("agents.unsaid", ()).expect_err("no such text").to_string(), "the prompt catalogue has no text 'agents.unsaid'");
        assert_eq!(catalogue.render("skills.said", ()).expect_err("another namespace").to_string(), "the prompt catalogue has no text 'skills.said'");

        let mut catalogue = PromptCatalogue::new();
        catalogue.add("agents", "said: x\n").unwrap();
        let twice = catalogue.add("agents", "other: y\n");
        assert!(matches!(&twice, Err(PromptError::NamespaceTaken(namespace)) if namespace == "agents"), "{twice:?}");
        assert!(matches!(catalogue.add("skills", "- a list\n"), Err(PromptError::Unreadable { namespace, .. }) if namespace == "skills"));
        assert!(matches!(catalogue.add("decision", "limit: 5\n"), Err(PromptError::Unreadable { namespace, .. }) if namespace == "decision"), "a number is to be quoted");
        let refused = catalogue.add("tools", "broken: \"{% if %}\"\n").expect_err("does not parse");
        assert!(matches!(&refused, PromptError::Invalid { key, .. } if key == "tools.broken"), "{refused}");
    }
}
