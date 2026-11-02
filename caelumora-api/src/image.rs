//! What an agent is given of an image model (issue 525): the facet `ImageModelClient`, through
//! which it asks one of the image models the platform offers, with what it asks
//! (`ImageRequest`) and what it is given (`Image`). The image models come from the plugin
//! `caelumora-model`, which builds each agent its clients, as it builds its language model's; the
//! errors of a call are those of every model (`ModelError`).

use crate::model::ModelResult;
use async_trait::async_trait;
use bytes::Bytes;
use caelumora_domain::actor::ActorId;
use std::fmt::Debug;

/// What an image model is asked, whatever serves it: what the image shows, and what the model
/// takes besides. Where each goes, and which sizes a model takes, are its backend's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRequest {
    /// What the image shows.
    pub prompt: String,
    /// What it is not to show, where the model takes it.
    pub negative_prompt: Option<String>,
    /// Its width in pixels; the model's own where `None`.
    pub width: Option<u64>,
    /// Its height in pixels; the model's own where `None`.
    pub height: Option<u64>,
    /// The seed of its noise: the same seed with the same request repeats an image, where the
    /// model takes one.
    pub seed: u64,
}

/// An image a model generated: its file's name and its bytes. What it is, its bytes tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub name: String,
    pub content: Bytes,
}

/// The facet through which an agent asks one image model the platform offers, built for that
/// agent alone. An agent may have several.
#[async_trait]
pub trait ImageModelClient: Send + Sync + Debug {
    /// The model's name, as the configuration gives it.
    fn name(&self) -> &str;

    /// What the model is for, as a tool that offers it describes it to the agent.
    fn description(&self) -> &str;

    /// Generates the images `request` asks for, in the agent's name for `initiator`, who began the
    /// chain the work answers. Dropping the future abandons the request to the model's server.
    ///
    /// # Errors
    /// `ModelError::Unfit` for a request the model does not take, which the asker can correct;
    /// the others for what its server failed with.
    async fn generate(&self, initiator: ActorId, request: &ImageRequest)
        -> ModelResult<Vec<Image>>;
}
