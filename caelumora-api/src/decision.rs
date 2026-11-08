//! What is given of a decision model (issue 551): the facet `DecisionModelClient`, through which
//! one of the decision models the platform offers is asked typed questions about a text, with
//! what it is asked (`Question`) and what it answers (`Answer`). The decision models come from the
//! plugin `caelumora-model`, which builds each of whoever asks one a client, as it builds an
//! agent's language model's; the errors of a call are those of every model (`ModelError`).

use crate::model::ModelResult;
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::confidentiality::Classification;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;

/// The most options a `Question::Choice` may have.
pub const MAX_OPTIONS: usize = 26;

/// The most questions one request may ask.
pub const MAX_QUESTIONS: usize = 64;

/// A question about the text judged, whatever serves the model: how it is written is its backend's.
#[derive(Debug, Clone, PartialEq)]
pub enum Question {
    /// Which one of the options fits: each named, with what it means -
    /// `None` where its name says it.
    Choice { instructions: String, criteria: BTreeMap<String, Option<String>> },
    /// Whether what `instructions` asks is true.
    Noul { instructions: String },
}

/// The answer to a question.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The option chosen, and how probable each option is.
    Choice { choice: String, probabilities: HashMap<String, f64> },
    /// How probable it is that the answer is yes.
    Noul { noul: f64 },
}

/// The facet through which one decision model the platform offers is asked, built for whoever asks
/// it alone.
#[async_trait]
pub trait DecisionModelClient: Send + Sync + Debug {
    /// The model's name, as the configuration gives it.
    fn name(&self) -> &str;

    /// Whether what is classified `classification` may be sent to the model, asked for
    /// `initiator`, who began the chain the work answers.
    fn may_receive(&self, initiator: ActorId, classification: &Classification) -> bool;

    /// The answers to `questions` about `state`, by the name of each, for `initiator`. What is
    /// sent holds what is classified as each of `classifications`.
    ///
    /// # Errors
    /// `ModelError::Unfit`, nothing sent, for what the model may not receive; the others for what
    /// its server failed with, an answer that leaves a question unanswered among them.
    async fn decide(
        &self,
        initiator: ActorId,
        classifications: &[Classification],
        state: &str,
        questions: &BTreeMap<String, Question>,
    ) -> ModelResult<HashMap<String, Answer>>;
}
