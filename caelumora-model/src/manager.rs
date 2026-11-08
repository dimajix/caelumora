//! The models the platform offers, by their kind and their names, and the clients it builds of
//! them (`ModelManager`, issues 520, 525, 551).

use crate::{DecisionModelProvider, ImageModelProvider, LanguageModelProvider};
use async_trait::async_trait;
use caelumora_api::component::{ComponentError, ComponentResult};
use caelumora_api::decision::{Answer, DecisionModelClient, Question};
use caelumora_api::image::{Image, ImageModelClient, ImageRequest};
use caelumora_api::model::{LanguageModelClient, ModelError, ModelResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::message::{Message, MessageEvent};
use caelumora_domain::tool::Tool;
use caelumora_kernel::backend::NamedBackends;
use caelumora_kernel::kernel::Acting;
use caelumora_security::confidentiality::ConfidentialityPolicy;
use caelumora_security::SecurityContext;
use futures::stream::BoxStream;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tracing::debug;

/// The models the platform offers, by the names the configuration gives them
/// (`plugins.model.language`, `plugins.model.image`, `plugins.model.decision`): what builds each
/// of whoever asks one a client of them. A service the plugin offers.
#[derive(Debug)]
pub struct ModelManager {
    language: NamedBackends<dyn LanguageModelProvider>,
    image: NamedBackends<dyn ImageModelProvider>,
    decision: NamedBackends<dyn DecisionModelProvider>,
    /// Whether what is classified so may reach a decision model's server.
    confidentiality: Arc<dyn ConfidentialityPolicy>,
}

impl ModelManager {
    /// The language models `language`, the image models `image` and the decision models
    /// `decision` name, what may reach the last judged by `confidentiality`.
    pub(crate) fn new(
        language: NamedBackends<dyn LanguageModelProvider>,
        image: NamedBackends<dyn ImageModelProvider>,
        decision: NamedBackends<dyn DecisionModelProvider>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
    ) -> Self {
        Self { language, image, decision, confidentiality }
    }

    /// The client of the language model `name` - the one there is where `None` -, for whoever
    /// `acting` acts for alone, each call made in the context `acting` builds at that call.
    ///
    /// # Errors
    /// As `chosen`.
    pub(crate) fn language_client_for(
        &self,
        acting: Acting,
        name: Option<&str>,
    ) -> ComponentResult<LanguageModelClientFor> {
        let (name, model) = chosen(&self.language, name, "language model")?;
        Ok(LanguageModelClientFor { acting, name: name.to_string(), model: Arc::clone(model) })
    }

    /// The client of the image model `name` - the one there is where `None` -, for whoever
    /// `acting` acts for alone, each call made in the context `acting` builds at that call.
    ///
    /// # Errors
    /// As `chosen`.
    pub(crate) fn image_client_for(
        &self,
        acting: Acting,
        name: Option<&str>,
    ) -> ComponentResult<ImageModelClientFor> {
        let (name, model) = chosen(&self.image, name, "image model")?;
        Ok(ImageModelClientFor { acting, name: name.to_string(), model: Arc::clone(model) })
    }

    /// The client of the decision model `name` - the one there is where `None` -, for whoever
    /// `acting` acts for alone, each call made in the context `acting` builds at that call.
    /// Public, unlike the others: a channel's `decision_policy` builds its own.
    ///
    /// # Errors
    /// As `chosen`.
    pub fn decision_client_for(
        &self,
        acting: Acting,
        name: Option<&str>,
    ) -> ComponentResult<Arc<dyn DecisionModelClient>> {
        let (name, model) = chosen(&self.decision, name, "decision model")?;
        Ok(Arc::new(DecisionModelClientFor {
            acting,
            name: name.to_string(),
            model: Arc::clone(model),
            confidentiality: Arc::clone(&self.confidentiality),
        }))
    }
}

/// The model of `models` that the parameter `name` of a component names - the one there is where
/// `None` -, with its name; `what` names the kind of model in messages.
///
/// # Errors
/// `ComponentError::InvalidConfig` naming the parameter `name` where it names no model, or none
/// among several, or where no model of the kind is configured.
fn chosen<'a, T: ?Sized>(
    models: &'a NamedBackends<T>,
    name: Option<&str>,
    what: &str,
) -> ComponentResult<(&'a str, &'a Arc<T>)> {
    let chosen = models
        .chosen(name)
        .map_err(|why| ComponentError::InvalidConfig(format!("'name' {why}")))?;
    chosen.ok_or_else(|| ComponentError::InvalidConfig(format!("no {what} is configured")))
}

/// The context `acting` builds now for a call of the model `name`, of the kind `what`, for
/// `initiator`, which says who asks it: what a check of the call, or a count of what it used, is to
/// go by. Logged.
fn asking(what: &str, name: &str, acting: &Acting, initiator: ActorId) -> SecurityContext {
    let asking = acting.context_for(initiator);
    debug!(
        "{what} {name} asked by {:?} for {:?}, initiated by {:?}",
        asking.caller_id, asking.principal_id, asking.initiator_id
    );
    asking
}

/// A language model as one agent asks it (`ModelManager::language_client_for`): each call made in
/// the context its `Acting` builds at that call, which says who asks which model - what a check of
/// the call, or a count of what it used, is to go by.
#[derive(Debug)]
pub(crate) struct LanguageModelClientFor {
    acting: Acting,
    /// The model's name, as the configuration gives it.
    name: String,
    model: Arc<dyn LanguageModelProvider>,
}

impl LanguageModelClientFor {
    /// Whether its model is given images (`LanguageModelProvider::sees_images`).
    pub(crate) async fn sees_images(&self) -> bool {
        self.model.sees_images().await
    }
}

#[async_trait]
impl LanguageModelClient for LanguageModelClientFor {
    async fn chat_completion(
        &self,
        initiator: ActorId,
        context: &[Message],
        tools: &[Tool],
    ) -> BoxStream<'static, ModelResult<MessageEvent>> {
        asking("model", &self.name, &self.acting, initiator);
        self.model.chat_completion(context, tools).await
    }
}

/// An image model as one agent asks it (`ModelManager::image_client_for`): each call made in the
/// context its `Acting` builds at that call, as a language model's is.
#[derive(Debug)]
pub(crate) struct ImageModelClientFor {
    acting: Acting,
    /// The model's name, as the configuration gives it.
    name: String,
    model: Arc<dyn ImageModelProvider>,
}

#[async_trait]
impl ImageModelClient for ImageModelClientFor {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        self.model.description()
    }

    async fn generate(
        &self,
        initiator: ActorId,
        request: &ImageRequest,
    ) -> ModelResult<Vec<Image>> {
        asking("image model", &self.name, &self.acting, initiator);
        self.model.generate(request).await
    }
}

/// A decision model as one asker asks it (`ModelManager::decision_client_for`): each call made in
/// the context its `Acting` builds at that call, and nothing sent that may not reach the model's
/// server.
#[derive(Debug)]
struct DecisionModelClientFor {
    acting: Acting,
    /// The model's name, as the configuration gives it.
    name: String,
    model: Arc<dyn DecisionModelProvider>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
}

impl DecisionModelClientFor {
    /// Whether what is classified `classification` may reach the model's server, asked in `asking`.
    fn cleared(&self, asking: &SecurityContext, classification: &Classification) -> bool {
        self.confidentiality.may_publish(asking, classification, self.model.destination())
    }
}

#[async_trait]
impl DecisionModelClient for DecisionModelClientFor {
    fn name(&self) -> &str {
        &self.name
    }

    fn may_receive(&self, initiator: ActorId, classification: &Classification) -> bool {
        self.cleared(&self.acting.context_for(initiator), classification)
    }

    async fn decide(
        &self,
        initiator: ActorId,
        classifications: &[Classification],
        state: &str,
        questions: &BTreeMap<String, Question>,
    ) -> ModelResult<HashMap<String, Answer>> {
        let asking = asking("decision model", &self.name, &self.acting, initiator);
        if !classifications.iter().all(|classification| self.cleared(&asking, classification)) {
            return Err(ModelError::Unfit {
                model: self.name.clone(),
                reason: "what it would be told may not reach its server".to_string(),
            });
        }
        self.model.decide(&asking, state, questions).await
    }
}
