//! The models of the tests of this crate: the backend `answering`, a language model that answers
//! what it is configured to, and the kernel that offers the models a section configures.

use crate::plugin::{ModelPlugin, MODEL};
use crate::LanguageModelProvider;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_api::component::Params;
use caelumora_api::model::{unlabelled_message, ModelResult};
use caelumora_domain::config::AppConfig;
use caelumora_domain::message::{Message, MessageEvent};
use caelumora_domain::tool::Tool;
use caelumora_kernel::backend::Backends;
use caelumora_kernel::kernel::{Kernel, KernelBuilder};
use caelumora_kernel::plugin::Plugin;
use caelumora_kernel::testing::TestKernel;
use futures::stream::{self, BoxStream, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// The backend of the models of the tests.
pub(crate) const ANSWERING: &str = "answering";

/// What a model of the backend `answering` is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    answer: String,
    #[serde(default)]
    seeing: bool,
}

impl Params for Answer {}

/// A model that answers what it is configured to, whatever it is asked.
#[derive(Debug)]
struct Answering(Answer);

#[async_trait]
impl LanguageModelProvider for Answering {
    async fn sees_images(&self) -> bool {
        self.0.seeing
    }

    async fn chat_completion(
        &self,
        _: &[Message],
        _: &[Tool],
    ) -> BoxStream<'static, ModelResult<MessageEvent>> {
        let answer = Message { content: Some(self.0.answer.clone()), ..unlabelled_message() };
        stream::iter([Ok(MessageEvent::Complete(answer))]).boxed()
    }
}

/// Brings the backend `answering`.
struct Answerers;

impl Plugin for Answerers {
    fn name(&self) -> &'static str {
        "answerers"
    }

    fn backends(&self, backends: &mut Backends) {
        backends.register::<dyn LanguageModelProvider, Answer, _>(ANSWERING, |_, answer, _| {
            Ok(Arc::new(Answering(answer)))
        });
    }

    fn register(&self, _: &mut KernelBuilder) -> Result<()> {
        Ok(())
    }
}

/// The configuration whose section `plugins.model` is `section`.
pub(crate) fn configuring(section: Value) -> AppConfig {
    AppConfig { plugins: [(MODEL.to_string(), section)].into(), ..AppConfig::default() }
}

/// The kernel of a test, with the models' plugin and the backend `answering`, as `config`
/// configures it.
///
/// # Errors
/// What keeps the kernel from being built.
pub(crate) fn kernel_of(config: AppConfig) -> Result<Kernel> {
    TestKernel { config, plugins: vec![&ModelPlugin, &Answerers], ..Default::default() }.try_build()
}
