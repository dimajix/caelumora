//! The models of the tests of this crate: the backend `answering`, a language model that answers
//! what it is configured to, the backend `drawing`, an image model that draws what it is asked, the
//! backend `judging`, a decision model that answers every question alike, and the kernel that
//! offers the models a section configures.

use crate::plugin::{ModelPlugin, MODEL};
use crate::{DecisionModelProvider, ImageModelProvider, LanguageModelProvider};
use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;
use caelumora_api::component::Params;
use caelumora_api::decision::{Answer as Judgement, Question};
use caelumora_api::image::{Image, ImageRequest};
use caelumora_api::model::{unlabelled_message, ModelError, ModelResult};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::config::AppConfig;
use caelumora_domain::message::{Message, MessageEvent};
use caelumora_domain::tool::Tool;
use caelumora_kernel::backend::Backends;
use caelumora_kernel::kernel::{Kernel, KernelBuilder};
use caelumora_kernel::plugin::Plugin;
use caelumora_kernel::testing::TestKernel;
use caelumora_security::SecurityContext;
use futures::stream::{self, BoxStream, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// The backend of the language models of the tests.
pub(super) const ANSWERING: &str = "answering";
/// The backend of the image models of the tests.
pub(super) const DRAWING: &str = "drawing";
/// The backend of the decision models of the tests.
pub(super) const JUDGING: &str = "judging";

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

/// What a model of the backend `drawing` is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Drawn {
    description: String,
    #[serde(default)]
    max_width: Option<u64>,
}

impl Params for Drawn {}

/// An image model that draws, as `<name>.png`, the prompt and the seed it is asked for, and does
/// not take a width beyond its `max_width`.
#[derive(Debug)]
struct Drawing {
    name: String,
    drawn: Drawn,
}

#[async_trait]
impl ImageModelProvider for Drawing {
    fn description(&self) -> &str {
        &self.drawn.description
    }

    async fn generate(&self, request: &ImageRequest) -> ModelResult<Vec<Image>> {
        if let (Some(width), Some(max)) = (request.width, self.drawn.max_width) {
            if width > max {
                let reason = format!("width {width}: it takes up to {max}");
                return Err(ModelError::Unfit { model: self.name.clone(), reason });
            }
        }
        let content = Bytes::from(format!("{} {}", request.prompt, request.seed));
        Ok(vec![Image { name: format!("{}.png", self.name), content }])
    }
}

/// What a model of the backend `judging` is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Judged {
    yes: f64,
    #[serde(default = "public")]
    destination: Classification,
}

fn public() -> Classification {
    Classification::PUBLIC
}

impl Params for Judged {}

/// The name of the answer of the backend `judging` that says for whom it was asked.
pub(super) const ASKED_FOR: &str = "asked_for";

/// A decision model that answers every question yes with the probability it is configured with,
/// and says, under `ASKED_FOR`, whose context it was asked in.
#[derive(Debug)]
struct Judging(Judged);

#[async_trait]
impl DecisionModelProvider for Judging {
    fn destination(&self) -> &Classification {
        &self.0.destination
    }

    async fn decide(
        &self,
        asking: &SecurityContext,
        _: &str,
        questions: &BTreeMap<String, Question>,
    ) -> ModelResult<HashMap<String, Judgement>> {
        let answer = |name: &String| (name.clone(), Judgement::Noul { noul: self.0.yes });
        let mut answers: HashMap<_, _> = questions.keys().map(answer).collect();
        let choice = format!("{:?}", asking.principal_id);
        let asked_for = Judgement::Choice { choice, probabilities: HashMap::new() };
        answers.insert(ASKED_FOR.to_string(), asked_for);
        Ok(answers)
    }
}

/// Brings the backends `answering`, `drawing` and `judging`.
struct TestModels;

impl Plugin for TestModels {
    fn name(&self) -> &'static str {
        "test_models"
    }

    fn backends(&self, backends: &mut Backends) {
        backends.register::<dyn LanguageModelProvider, Answer, _>(ANSWERING, |_, answer, _| {
            Ok(Arc::new(Answering(answer)))
        });
        backends.register::<dyn ImageModelProvider, Drawn, _>(DRAWING, |name, drawn, _| {
            Ok(Arc::new(Drawing { name: name.to_string(), drawn }))
        });
        backends.register::<dyn DecisionModelProvider, Judged, _>(JUDGING, |_, judged, _| {
            Ok(Arc::new(Judging(judged)))
        });
    }

    fn register(&self, _: &mut KernelBuilder) -> Result<()> {
        Ok(())
    }
}

/// The configuration whose section `plugins.model` is `section`.
pub(super) fn configuring(section: Value) -> AppConfig {
    AppConfig { plugins: [(MODEL.to_string(), section)].into(), ..AppConfig::default() }
}

/// The kernel of a test, with the models' plugin and the backends `answering`, `drawing` and
/// `judging`, as `config` configures it.
///
/// # Errors
/// What keeps the kernel from being built.
pub(super) fn kernel_of(config: AppConfig) -> Result<Kernel> {
    TestKernel { config, plugins: vec![&ModelPlugin, &TestModels], ..Default::default() }
        .try_build()
}
