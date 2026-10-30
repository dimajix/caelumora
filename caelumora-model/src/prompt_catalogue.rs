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

/// The values a text is rendered with, by name: `context!{ name => "Sassy" }`.
pub use minijinja::context;

use crate::config::parse_yaml;
use crate::intern::intern;
use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use serde::de::Error as _;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
#[cfg(test)]
use std::sync::OnceLock;

/// The namespace of the core's texts: what `caelumora-model` writes for a
/// model - the notices of a context as it is resolved, the explanation of
/// how messages are marked.
pub const CORE: &str = "core";

/// The core's texts, added to the catalogue under `CORE` when the runtime
/// is built (`RuntimeBuilder::new`).
pub const CORE_PROMPTS: &str = include_str!("prompts.yaml");

/// The variant that overrides the defaults for every model (issue 57), as
/// the configuration names it among the others.
pub const DEFAULT_VARIANT: &str = "default";

/// The name of a variant of the texts (issue 57), as a model's component
/// names the one its model is given (`ModelProfile::prompt_texts`): lower
/// case letters, digits, `_` and `-`. Kept once per name for the rest of
/// the process, so that a profile that names one stays `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PromptVariant(&'static str);

impl PromptVariant {
    /// The variant `name`.
    ///
    /// # Errors
    /// `PromptError::VariantNamed` for a name that is none.
    pub fn new(name: &str) -> Result<Self, PromptError> {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(PromptError::VariantNamed(name.to_string()));
        }
        Ok(Self(intern(name)))
    }

    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for PromptVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for PromptVariant {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for PromptVariant {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::new(&name).map_err(D::Error::custom)
    }
}

/// The texts of a catalogue as a model is given them (issue 57): in the
/// variant its profile names, else as the default. What writes for a model
/// takes it when it is made - the variant of its model - and keeps it; a
/// context resolved again is worded in the variant it was recorded with.
#[derive(Debug, Clone)]
pub struct PromptTexts {
    catalogue: Arc<PromptCatalogue>,
    variant: Option<PromptVariant>,
}

impl PromptTexts {
    /// The texts of `catalogue` in `variant`, the default for `None`.
    pub fn new(catalogue: Arc<PromptCatalogue>, variant: Option<PromptVariant>) -> Self {
        Self { catalogue, variant }
    }

    /// The text `key` - `namespace.key` -, rendered with `values`.
    ///
    /// # Errors
    /// As `PromptCatalogue::render`.
    pub fn render(&self, key: &str, values: impl Serialize) -> Result<String, PromptError> {
        self.catalogue.render_in(self.variant, key, values)
    }
}

/// The texts given to models, by key: those of every namespace added, and
/// the variants that override some of them.
#[derive(Debug)]
pub struct PromptCatalogue {
    templates: Environment<'static>,
    /// The namespaces added, so that none is added twice.
    namespaces: BTreeSet<String>,
    /// The variants added besides the default, by name.
    variants: BTreeSet<String>,
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
        Self { templates, namespaces: BTreeSet::new(), variants: BTreeSet::new() }
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
        let texts: BTreeMap<String, Entry> = parse_yaml(yaml).map_err(|cause| {
            PromptError::Unreadable { namespace: namespace.to_string(), cause: cause.to_string() }
        })?;
        if !self.namespaces.insert(namespace.to_string()) {
            return Err(PromptError::NamespaceTaken(namespace.to_string()));
        }
        for (key, template) in flattened(namespace, texts) {
            self.templates
                .add_template_owned(key.clone(), template)
                .map_err(|cause| PromptError::Invalid { key, cause })?;
        }
        Ok(())
    }

    /// Adds the variant `variant` - `DEFAULT_VARIANT` to override the
    /// defaults for every model - from the files of the directory `dir`:
    /// one per namespace, named after it (`agents.yaml`), of the form of
    /// the namespace's file, whose entries replace the default's one by
    /// one. What a file leaves out, and a namespace without a file, stay the
    /// default's; other files than `.yaml` ones are not read. Added after
    /// every namespace, and the default before the others, which fall back
    /// to it.
    ///
    /// # Errors
    /// `PromptError::VariantNamed` for a name that is none;
    /// `PromptError::VariantUnreadable` if `dir` or a file cannot be read;
    /// `PromptError::UnknownNamespace` for a file of no namespace;
    /// `PromptError::Unreadable` for one not of the form of a namespace's;
    /// `PromptError::UnknownText` for a text the default does not have;
    /// `PromptError::UnknownValue` for a value its default is not given;
    /// `PromptError::Invalid` for a template that does not parse. Nothing
    /// is checked of what is not a text of the catalogue's - neither the
    /// name of a file nor of a value is quoted with a path.
    pub fn add_variant(&mut self, variant: &str, dir: &Path) -> Result<(), PromptError> {
        PromptVariant::new(variant)?;
        let unreadable = |e: io::Error| PromptError::VariantUnreadable {
            variant: variant.to_string(),
            cause: e.kind().to_string(),
        };
        let mut files: Vec<_> = fs::read_dir(dir)
            .map_err(unreadable)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<_, _>>()
            .map_err(unreadable)?;
        files.retain(|file| file.extension().is_some_and(|extension| extension == "yaml"));
        files.sort();
        let mut texts = Vec::new();
        for file in files {
            let namespace =
                file.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default().to_string();
            if !self.namespaces.contains(&namespace) {
                return Err(PromptError::UnknownNamespace {
                    variant: variant.to_string(),
                    namespace,
                });
            }
            let yaml = fs::read_to_string(&file).map_err(unreadable)?;
            let entries: BTreeMap<String, Entry> =
                parse_yaml(&yaml).map_err(|cause| PromptError::Unreadable {
                    namespace: format!("{variant}/{namespace}"),
                    cause: cause.to_string(),
                })?;
            texts.extend(flattened(&namespace, entries));
        }
        // Checked whole before anything is taken.
        let mut checked = Environment::new();
        for (key, template) in &texts {
            let default = self.templates.get_template(key).map_err(|_| {
                PromptError::UnknownText { variant: variant.to_string(), key: key.clone() }
            })?;
            checked
                .add_template_owned(key.clone(), template.clone())
                .map_err(|cause| PromptError::Invalid { key: variant_key(variant, key), cause })?;
            let given = default.undeclared_variables(false);
            let wanted = checked
                .get_template(key)
                .map(|template| template.undeclared_variables(false))
                .unwrap_or_default();
            if let Some(name) = wanted.difference(&given).min() {
                return Err(PromptError::UnknownValue {
                    variant: variant.to_string(),
                    key: key.clone(),
                    name: name.clone(),
                });
            }
        }
        for (key, template) in texts {
            let key = variant_key(variant, &key);
            self.templates
                .add_template_owned(key.clone(), template)
                .map_err(|cause| PromptError::Invalid { key, cause })?;
        }
        if variant != DEFAULT_VARIANT {
            self.variants.insert(variant.to_string());
        }
        Ok(())
    }

    /// Whether `variant` was added - the default always is.
    pub fn has_variant(&self, variant: PromptVariant) -> bool {
        variant.as_str() == DEFAULT_VARIANT || self.variants.contains(variant.as_str())
    }

    /// Nothing if the variant `named` by a model's component - if it names
    /// one - was added.
    ///
    /// # Errors
    /// `PromptError::NoVariant` if it was not.
    pub fn require_variant(&self, named: Option<PromptVariant>) -> Result<(), PromptError> {
        match named {
            Some(variant) if !self.has_variant(variant) => Err(PromptError::NoVariant(variant)),
            _ => Ok(()),
        }
    }

    /// The text `key` - `namespace.key` -, rendered with `values`.
    ///
    /// # Errors
    /// `PromptError::Unknown` if the catalogue has no text `key`;
    /// `PromptError::Invalid` if it cannot be rendered with `values` - one
    /// it names is not among them.
    pub fn render(&self, key: &str, values: impl Serialize) -> Result<String, PromptError> {
        self.render_in(None, key, values)
    }

    /// The text `key` as the variant `variant` has it, else as the default
    /// has it, rendered with `values`.
    ///
    /// # Errors
    /// As `render`.
    pub fn render_in(
        &self,
        variant: Option<PromptVariant>,
        key: &str,
        values: impl Serialize,
    ) -> Result<String, PromptError> {
        let of_variant = variant.and_then(|variant| {
            self.templates.get_template(&variant_key(variant.as_str(), key)).ok()
        });
        let template = match of_variant {
            Some(template) => template,
            None => self
                .templates
                .get_template(key)
                .map_err(|_| PromptError::Unknown(key.to_string()))?,
        };
        template
            .render(values)
            .map_err(|cause| PromptError::Invalid { key: key.to_string(), cause })
    }
}

/// The texts of `entries` under `prefix`, by their whole keys.
fn flattened(prefix: &str, entries: BTreeMap<String, Entry>) -> Vec<(String, String)> {
    let mut texts = Vec::new();
    for (key, entry) in entries {
        let key = format!("{prefix}.{key}");
        match entry {
            Entry::Text(template) => texts.push((key, template)),
            Entry::Group(entries) => texts.extend(flattened(&key, entries)),
        }
    }
    texts
}

/// Where the text `key` of `variant` is kept: in place of the default's for
/// the default variant, beside it for another.
fn variant_key(variant: &str, key: &str) -> String {
    match variant {
        DEFAULT_VARIANT => key.to_string(),
        variant => format!("{variant}/{key}"),
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
    Invalid {
        key: String,
        #[source]
        cause: minijinja::Error,
    },
    #[error("'{0}' is no name of a variant of the texts: lower case letters, digits, '_' and '-'")]
    VariantNamed(String),
    #[error("the texts have no variant '{0}': it is not configured (`prompts`)")]
    NoVariant(PromptVariant),
    #[error("the texts of the variant '{variant}' cannot be read: {cause}")]
    VariantUnreadable { variant: String, cause: String },
    #[error("the variant '{variant}' has a file of '{namespace}', which is no namespace of texts")]
    UnknownNamespace { variant: String, namespace: String },
    #[error("the variant '{variant}' has a text '{key}', which the defaults do not have")]
    UnknownText { variant: String, key: String },
    #[error("the text '{key}' of the variant '{variant}' names '{name}', which its default is not given")]
    UnknownValue { variant: String, key: String, name: String },
}

/// A catalogue with the core's texts alone, for the tests of what renders
/// them.
#[cfg(test)]
pub(crate) fn core() -> &'static Arc<PromptCatalogue> {
    static CATALOGUE: OnceLock<Arc<PromptCatalogue>> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        let mut catalogue = PromptCatalogue::new();
        catalogue
            .add(CORE, CORE_PROMPTS)
            .unwrap_or_else(|e| panic!("the core's texts cannot be read: {e}"));
        Arc::new(catalogue)
    })
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
        assert_eq!(
            text, "[Who is here]\n- Kaya, a person\n- Sassy\n",
            "block tags leave no lines behind, the text ends in its line break"
        );
    }

    #[test]
    fn a_group_names_the_texts_it_holds() {
        let tools = catalogue(
            "tools:\n  notes:\n    description: Notes.\n    write_note.text: What to note\n",
        );
        assert_eq!(tools.render("agents.tools.notes.description", ()).unwrap(), "Notes.");
        assert_eq!(
            tools.render("agents.tools.notes.write_note.text", ()).unwrap(),
            "What to note",
            "a key may hold dots of its own"
        );
        let choices = catalogue("choices:\n  yes: y\n  no: \"no\"\n");
        assert_eq!(
            choices.render("agents.choices.yes", ()).unwrap(),
            "y",
            "only true and false are no texts"
        );
    }

    #[test]
    fn nothing_is_escaped_and_a_value_that_is_not_given_is_an_error() {
        let catalogue = catalogue("said: \"{{ name }} said <{{ text }}>\"\n");
        assert_eq!(
            catalogue.render("agents.said", context! { name => "Kaya", text => "a & b" }).unwrap(),
            "Kaya said <a & b>"
        );
        let refused = catalogue
            .render("agents.said", context! { name => "Kaya" })
            .expect_err("no text given");
        assert!(
            matches!(&refused, PromptError::Invalid { key, .. } if key == "agents.said"),
            "{refused}"
        );
    }

    #[test]
    fn what_cannot_be_had_is_named() {
        let catalogue = catalogue("said: x\n");
        assert_eq!(
            catalogue.render("agents.unsaid", ()).expect_err("no such text").to_string(),
            "the prompt catalogue has no text 'agents.unsaid'"
        );
        assert_eq!(
            catalogue.render("skills.said", ()).expect_err("another namespace").to_string(),
            "the prompt catalogue has no text 'skills.said'"
        );

        let mut catalogue = PromptCatalogue::new();
        catalogue.add("agents", "said: x\n").unwrap();
        let twice = catalogue.add("agents", "other: y\n");
        assert!(
            matches!(&twice, Err(PromptError::NamespaceTaken(namespace)) if namespace == "agents"),
            "{twice:?}"
        );
        assert!(
            matches!(catalogue.add("skills", "- a list\n"), Err(PromptError::Unreadable { namespace, .. }) if namespace == "skills")
        );
        assert!(
            matches!(catalogue.add("decision", "limit: 5\n"), Err(PromptError::Unreadable { namespace, .. }) if namespace == "decision"),
            "a number is to be quoted"
        );
        let refused = catalogue.add("tools", "broken: \"{% if %}\"\n").expect_err("does not parse");
        assert!(
            matches!(&refused, PromptError::Invalid { key, .. } if key == "tools.broken"),
            "{refused}"
        );
    }

    /// A directory of a variant holding `files`, each a name and its text.
    fn variant_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            fs::write(dir.path().join(name), text).unwrap();
        }
        dir
    }

    fn texts() -> PromptCatalogue {
        catalogue("greeting: \"Hello, {{ name }}.\"\nfarewell: Bye.\ntools:\n  notes:\n    description: Notes.\n")
    }

    #[test]
    fn a_variant_replaces_texts_of_the_default_one_by_one_and_the_default_variant_them_all() {
        let mut catalogue = texts();
        let gemma = variant_dir(&[
            (
                "agents.yaml",
                "greeting: \"Hi {{ name }}!\"\ntools:\n  notes:\n    description: Your notes.\n",
            ),
            ("README.md", "not read"),
        ]);
        catalogue
            .add_variant(
                DEFAULT_VARIANT,
                variant_dir(&[("agents.yaml", "farewell: Goodbye.\n")]).path(),
            )
            .unwrap();
        catalogue.add_variant("gemma4", gemma.path()).unwrap();
        let variant = |name: &str| PromptVariant::new(name).unwrap();
        let catalogue = Arc::new(catalogue);
        let greeting = |name: Option<&str>| {
            PromptTexts::new(Arc::clone(&catalogue), name.map(variant))
                .render("agents.greeting", context! { name => "Kaya" })
                .unwrap()
        };
        assert_eq!(greeting(Some("gemma4")), "Hi Kaya!", "the variant's");
        assert_eq!(greeting(None), "Hello, Kaya.", "the default's where none is asked for");
        assert_eq!(
            greeting(Some("llama")),
            "Hello, Kaya.",
            "and for a variant that has no such text"
        );
        let gemma = PromptTexts::new(Arc::clone(&catalogue), Some(variant("gemma4")));
        assert_eq!(
            gemma.render("agents.tools.notes.description", ()).unwrap(),
            "Your notes.",
            "in a group, too"
        );
        assert_eq!(
            gemma.render("agents.farewell", ()).unwrap(),
            "Goodbye.",
            "what a variant leaves out, as the default variant has it"
        );
        assert_eq!(catalogue.render("agents.farewell", ()).unwrap(), "Goodbye.");
        assert!(
            catalogue.has_variant(variant("gemma4"))
                && catalogue.has_variant(variant(DEFAULT_VARIANT))
                && !catalogue.has_variant(variant("llama"))
        );
        assert!(
            catalogue.require_variant(None).is_ok()
                && catalogue.require_variant(Some(variant("gemma4"))).is_ok()
        );
        assert_eq!(
            catalogue
                .require_variant(Some(variant("llama")))
                .expect_err("not configured")
                .to_string(),
            "the texts have no variant 'llama': it is not configured (`prompts`)"
        );
    }

    #[test]
    fn a_variant_that_does_not_fit_the_defaults_is_refused_whole() {
        for (files, refused) in [
            (&[("skills.yaml", "x: y\n")][..], "a file of 'skills', which is no namespace"),
            (
                &[("agents.yaml", "greting: Hi.\n")][..],
                "a text 'agents.greting', which the defaults do not have",
            ),
            (
                &[("agents.yaml", "farewell: \"Bye, {{ name }}.\"\n")][..],
                "names 'name', which its default is not given",
            ),
            (
                &[("agents.yaml", "farewell: \"{% if %}\"\n")][..],
                "'gemma4/agents.farewell' cannot be rendered",
            ),
            (&[("agents.yaml", "- a list\n")][..], "the texts of 'gemma4/agents' cannot be read"),
        ] {
            let mut catalogue = texts();
            let dir = variant_dir(files);
            let error = catalogue.add_variant("gemma4", dir.path()).expect_err(refused).to_string();
            assert!(error.contains(refused), "{error}");
            assert!(!error.contains(&dir.path().display().to_string()), "no path: {error}");
            assert_eq!(
                catalogue
                    .render_in(Some(PromptVariant::new("gemma4").unwrap()), "agents.farewell", ())
                    .unwrap(),
                "Bye.",
                "nothing taken"
            );
        }
        let mut catalogue = texts();
        assert!(matches!(
            catalogue.add_variant("Gemma 4", variant_dir(&[]).path()),
            Err(PromptError::VariantNamed(_))
        ));
        let missing = catalogue
            .add_variant("gemma4", &variant_dir(&[]).path().join("missing"))
            .expect_err("no directory")
            .to_string();
        assert!(
            missing.contains("the texts of the variant 'gemma4' cannot be read")
                && !missing.contains("missing"),
            "{missing}"
        );
    }
}
