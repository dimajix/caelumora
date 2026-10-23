//! The `decision_policy` component (issue 403): a `ResponderPolicy` that
//! asks a decision model which of the candidates should respond to what was
//! said, by what it says and by how each candidate is introduced.
//!
//! Up to 25 candidates it asks one `choice` over their handles and
//! "nobody", whose probabilities are comparable: the most probable one is
//! asked to respond - nobody, if that is most probable - and with it every
//! other above `threshold`, at most `max_responders`. Beyond that it asks
//! one `noul` per candidate, up to 64, and asks those above `threshold`.
//!
//! It is given the conversation (issue 414): what was said before, as far
//! as the moderator keeps it, and what is to be answered. Each message is
//! given as a model is given one of another participant, its sender marked
//! as its `ModelProfile` says - `prefix` by `handle` unless configured, so
//! that a sender is named as the options are. Beyond `MAX_STATE_TOKENS`
//! the oldest is left out first.
//!
//! What was said leaves the system where the endpoint lies outside it: it
//! is sent only if it may flow to the endpoint's `destination` (`public`
//! unless configured), asked in the context of the event routed. What was
//! said before is left out if it may not; what is to be answered keeps the
//! model from choosing.
//! Whatever keeps the model from choosing - that, a timeout, an error, too
//! many candidates - asks whom `on_failure` says: nobody by default (issue
//! 437), so that a model that is slow or down leaves the agents quiet
//! rather than makes every one of them answer; `all` asks everyone, so that
//! nothing said is left unanswered. It is logged, without anything that was
//! said.
//!
//! Its params say where what it is given goes, so only an operator
//! configures it (`ComponentKind::operator_only`).

use crate::client::{Answer, DecisionClient, DecisionError, Question, MAX_OPTIONS, MAX_QUESTIONS};
use async_trait::async_trait;
use caelumora_model::actor::{ActorDescription, ActorId, Envelope};
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy};
use caelumora_model::context::{ModelProfile, SenderFormat, SenderStyle};
use caelumora_model::message::{estimated_tokens, Message, MessageId};
use caelumora_model::prompt_catalogue::{context, PromptCatalogue, PromptError, PromptTexts, PromptVariant};
use crate::NAMESPACE;
use caelumora_model::remote::RemoteEndpoint;
use caelumora_model::secret::SecretManager;
use caelumora_model::value::time_zone::TimeZone;
use caelumora_runtime::component::{config_of_params, ComponentError, ComponentFacets, ComponentKind, ComponentRegistry, ComponentResult};
use caelumora_runtime::moderator::ResponderPolicy;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;
use tracing::warn;

pub const DECISION_POLICY_KIND: &str = "decision_policy";

/// The option of the `choice` that asks nobody to respond.
const NOBODY: &str = "nobody";

/// The name of the `choice` question.
const RESPONDENT: &str = "respondent";

/// How many tokens what was said may take at most, as `estimated_tokens`
/// estimates them: half of the 8,192 a decision model reads, the rest left
/// to the questions - up to 26 options, each with its introduction.
const MAX_STATE_TOKENS: u64 = 4096;

/// Whom to ask when the model does not choose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OnFailure {
    /// Every candidate.
    All,
    /// Nobody (issue 437).
    #[default]
    None,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionPolicyConfig {
    /// The server; its timeout bounds a decision, 5 s unless written.
    #[serde(default = "default_endpoint")]
    endpoint: RemoteEndpoint,
    #[serde(default = "default_model")]
    model: String,
    /// How long the model stays loaded after a decision; the server's
    /// default if left out.
    #[serde(default)]
    keep_alive: Option<String>,
    /// How probable it must be that a candidate fits to be asked besides
    /// the most probable one - or at all, beyond 25 candidates.
    #[serde(default = "default_threshold")]
    threshold: f64,
    #[serde(default = "default_max_responders")]
    max_responders: usize,
    #[serde(default)]
    on_failure: OnFailure,
    /// What the server is cleared for: where what was said goes. Left out,
    /// public.
    #[serde(default)]
    destination: Option<Classification>,
    /// How the model is told who said what (issue 135): `prefix` unless
    /// named, by `handle` unless named.
    #[serde(default = "SenderFormat::configured_default")]
    sender_format: SenderFormat,
    #[serde(default)]
    sender_style: SenderStyle,
    /// The variant of the questions the model is asked (issue 57); the
    /// default unless configured.
    #[serde(default)]
    prompt_texts: Option<PromptVariant>,
}

impl DecisionPolicyConfig {
    /// How the model is given what was said - without the times of the
    /// messages (issue 424), which who is to respond does not depend on.
    fn profile(&self) -> ModelProfile {
        ModelProfile { sender_format: self.sender_format, sender_style: self.sender_style, prompt_texts: self.prompt_texts, ..ModelProfile::default() }
    }

    /// Nothing if the threshold is a probability and at least one may be
    /// asked to respond.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` otherwise.
    fn check(&self) -> ComponentResult<()> {
        if !(0.0..=1.0).contains(&self.threshold) {
            return Err(ComponentError::InvalidConfig(format!("threshold {} is no probability", self.threshold)));
        }
        if self.max_responders == 0 {
            return Err(ComponentError::InvalidConfig("max_responders must be at least 1".to_string()));
        }
        Ok(())
    }
}

fn default_endpoint() -> RemoteEndpoint {
    RemoteEndpoint::new("http://127.0.0.1:11434")
}

fn default_model() -> String {
    "nimble".to_string()
}

fn default_threshold() -> f64 {
    0.3
}

fn default_max_responders() -> usize {
    1
}

/// Why the model did not choose.
#[derive(Debug, thiserror::Error)]
enum Undecided {
    #[error("what was said may not be sent to the decision model at {0}")]
    NotCleared(String),
    #[error("{0} candidates are more than a decision model is asked about")]
    TooMany(usize),
    #[error(transparent)]
    Model(#[from] DecisionError),
    /// What it is asked cannot be worded (issue 57).
    #[error("the questions cannot be worded: {0}")]
    Unworded(#[from] PromptError),
}

/// Asks a decision model whom to ask to respond.
#[derive(Debug)]
pub struct DecisionPolicy {
    client: DecisionClient,
    threshold: f64,
    max_responders: usize,
    on_failure: OnFailure,
    destination: Classification,
    profile: ModelProfile,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    /// What the model is asked, in the variant it names.
    prompts: PromptTexts,
}

#[async_trait]
impl ResponderPolicy for DecisionPolicy {
    async fn choose(&self, event: &Envelope, messages: &[Message], candidates: Vec<ActorDescription>) -> Vec<ActorId> {
        if candidates.is_empty() {
            return Vec::new();
        }
        match self.decide(event, messages, &candidates).await {
            Ok(chosen) => chosen,
            Err(undecided) => {
                let asked = match self.on_failure {
                    OnFailure::All => candidates.iter().map(|candidate| candidate.id).collect(),
                    OnFailure::None => Vec::new(),
                };
                warn!(candidates = candidates.len(), asked = asked.len(), "The decision model did not choose who responds: {undecided}");
                asked
            }
        }
    }
}

impl DecisionPolicy {
    /// Whom of `candidates` the model chooses to respond to what `event`
    /// said, the end of the conversation `messages`.
    ///
    /// # Errors
    /// `NotCleared` if what `event` said may not flow to the destination;
    /// `TooMany` beyond 64 candidates; `Model` if the model gave no answer,
    /// or one that names no option.
    async fn decide(&self, event: &Envelope, messages: &[Message], candidates: &[ActorDescription]) -> Result<Vec<ActorId>, Undecided> {
        let state = self.state(event, messages)?;
        let options = options(candidates);
        if candidates.len() < MAX_OPTIONS {
            let answers = self.client.ask(&event.security, &state, &choice(&self.prompts, candidates, &options)?).await?;
            self.chosen_by_choice(answers.get(RESPONDENT), candidates, &options)
        } else if candidates.len() <= MAX_QUESTIONS {
            let answers = self.client.ask(&event.security, &state, &nouls(&self.prompts, candidates, &options)?).await?;
            let probabilities = options.iter().map(|option| match answers.get(option) {
                Some(Answer::Noul { noul }) => Ok(*noul),
                _ => Err(self.invalid(format!("the question '{option}' is not answered yes or no"))),
            }).collect::<Result<Vec<_>, _>>()?;
            Ok(self.above_threshold(candidates, &probabilities, None))
        } else {
            Err(Undecided::TooMany(candidates.len()))
        }
    }

    /// What the model judges: the conversation `messages`, each message
    /// heard as a model is given one of another participant, one after
    /// another. What was said before is left out if it may not flow to the
    /// destination, asked in the context of `event`, and beyond
    /// `MAX_STATE_TOKENS` the oldest first.
    ///
    /// # Errors
    /// `NotCleared` if what `event` said may not flow to the destination.
    fn state(&self, event: &Envelope, messages: &[Message]) -> Result<String, Undecided> {
        let answered: HashSet<MessageId> = event.event.messages().iter().map(|message| message.id).collect();
        // Each text, and whether it is to be answered.
        let mut texts = VecDeque::new();
        for message in messages.iter().filter_map(Message::as_heard) {
            let to_answer = answered.contains(&message.id);
            if !self.confidentiality.may_publish(&event.security, &message.classification, &self.destination) {
                if to_answer {
                    return Err(Undecided::NotCleared(self.client.shown()));
                }
                continue;
            }
            // Who is to respond is decided by what was said, not when: the
            // profile gives no times, and the time zone is none's.
            if let Some(text) = self.profile.given(message, ActorId::None, false, TimeZone::UTC).content {
                texts.push_back((text, to_answer));
            }
        }
        let separators = 2 * texts.len().saturating_sub(1);
        let mut bytes = texts.iter().map(|(text, _)| text.len()).sum::<usize>() + separators;
        while estimated_tokens(bytes) > MAX_STATE_TOKENS && texts.front().is_some_and(|(_, to_answer)| !to_answer) {
            if let Some((text, _)) = texts.pop_front() {
                bytes = bytes.saturating_sub(text.len() + 2);
            }
        }
        Ok(texts.into_iter().map(|(text, _)| text).collect::<Vec<_>>().join("\n\n"))
    }

    /// Whom the `choice` `answer` chooses: nobody if it chose nobody, else
    /// the candidate it chose and those above the threshold besides, the
    /// most probable first, at most `max_responders`.
    fn chosen_by_choice(&self, answer: Option<&Answer>, candidates: &[ActorDescription], options: &[String]) -> Result<Vec<ActorId>, Undecided> {
        let Some(Answer::Choice { choice, probabilities }) = answer else {
            return Err(self.invalid(format!("the question '{RESPONDENT}' is not answered with a choice")));
        };
        if choice == NOBODY {
            return Ok(Vec::new());
        }
        let first = options.iter().position(|option| option == choice)
            .ok_or_else(|| self.invalid(format!("it chose '{choice}', which is no option")))?;
        let probabilities: Vec<f64> = options.iter().map(|option| probabilities.get(option).copied().unwrap_or(0.0)).collect();
        Ok(self.above_threshold(candidates, &probabilities, Some(first)))
    }

    /// The candidates whose `probabilities` reach the threshold - and
    /// `first`, if one is chosen anyway -, the most probable first, at most
    /// `max_responders`.
    fn above_threshold(&self, candidates: &[ActorDescription], probabilities: &[f64], first: Option<usize>) -> Vec<ActorId> {
        let mut ranked: Vec<usize> = (0..candidates.len())
            .filter(|&index| Some(index) != first && probabilities[index] >= self.threshold)
            .collect();
        ranked.sort_by(|a, b| probabilities[*b].total_cmp(&probabilities[*a]));
        first.into_iter().chain(ranked).take(self.max_responders).map(|index| candidates[index].id).collect()
    }

    fn invalid(&self, reason: String) -> Undecided {
        Undecided::Model(DecisionError::InvalidResponse { endpoint: self.client.shown(), reason })
    }
}

/// The name each candidate is asked about by, in their order: its handle,
/// unless it has none, or one taken before or by "nobody" - then
/// `agent-<n>`, by its place.
fn options(candidates: &[ActorDescription]) -> Vec<String> {
    let mut taken = HashSet::from([NOBODY.to_string()]);
    candidates.iter().enumerate().map(|(index, candidate)| {
        let option = candidate.handle().filter(|handle| !taken.contains(handle)).unwrap_or_else(|| format!("agent-{}", index + 1));
        taken.insert(option.clone());
        option
    }).collect()
}

/// The text `key` of `prompts` for the decision model, rendered with
/// `values`.
fn text(prompts: &PromptTexts, key: &str, values: impl Serialize) -> Result<String, PromptError> {
    prompts.render(&format!("{NAMESPACE}.{key}"), values)
}

/// Who `candidate` is, for the model: its name, and how it is introduced,
/// on one line.
fn described(prompts: &PromptTexts, candidate: &ActorDescription) -> Result<String, PromptError> {
    let introduction = candidate.introduction.as_ref().map(|introduction| introduction.as_str().split_whitespace().collect::<Vec<_>>().join(" "));
    text(prompts, "candidate", context! { name => candidate.name.as_str(), introduction })
}

/// One `choice` over the candidates, by `options`, and nobody.
fn choice(prompts: &PromptTexts, candidates: &[ActorDescription], options: &[String]) -> Result<BTreeMap<String, Question>, PromptError> {
    let mut criteria = BTreeMap::new();
    for (option, candidate) in options.iter().zip(candidates) {
        criteria.insert(option.clone(), Some(described(prompts, candidate)?));
    }
    criteria.insert(NOBODY.to_string(), Some(text(prompts, "choice.nobody", ())?));
    let instructions = text(prompts, "choice.instructions", ())?;
    Ok(BTreeMap::from([(RESPONDENT.to_string(), Question::Choice { instructions, criteria })]))
}

/// One `noul` per candidate, by its option.
fn nouls(prompts: &PromptTexts, candidates: &[ActorDescription], options: &[String]) -> Result<BTreeMap<String, Question>, PromptError> {
    options.iter().zip(candidates).map(|(option, candidate)| {
        let instructions = text(prompts, "noul", context! { candidate => described(prompts, candidate)? })?;
        Ok((option.clone(), Question::Noul { instructions }))
    }).collect()
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(DECISION_POLICY_KIND)
            .provides::<dyn ResponderPolicy>()
            .uses::<SecretManager>()
            .uses::<dyn ConfidentialityPolicy>()
            .uses::<PromptCatalogue>()
            .operator_only(),
        |descriptor, context| {
            let config: DecisionPolicyConfig = config_of_params(&descriptor.params)?;
            config.check()?;
            let catalogue = context.service::<PromptCatalogue>()?;
            catalogue.require_variant(config.prompt_texts)?;
            let prompts = PromptTexts::new(Arc::clone(catalogue), config.prompt_texts);
            let secret_manager = Arc::clone(context.service::<SecretManager>()?);
            let profile = config.profile();
            let client = DecisionClient::new(config.endpoint, config.model, config.keep_alive, secret_manager)
                .map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
            let policy = DecisionPolicy {
                client,
                threshold: config.threshold,
                max_responders: config.max_responders,
                on_failure: config.on_failure,
                destination: config.destination.unwrap_or(Classification::PUBLIC),
                profile,
                confidentiality: Arc::clone(context.service::<dyn ConfidentialityPolicy>()?),
                prompts,
            };
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ResponderPolicy>(Arc::new(policy));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PROMPTS;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};
    use caelumora_model::actor::{ActorEvent, EventContext};
    use caelumora_model::agent::AgentId;
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::store::Persistence;
    use caelumora_runtime::channel::ChannelLoop;
    use caelumora_model::confidentiality::{ConfidentialityRules, Confidentiality};
    use caelumora_model::message::{MessageId, MessageRole};
    use caelumora_model::security::{AllowAllPolicy, SecurityContext};
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::Name;
    use caelumora_model::value::text::Introduction;
    use serde_json::{json, Value};
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    /// A decision model that answers every request with `answer`, or with
    /// `status` if it is not successful, and keeps what it was asked.
    #[derive(Clone)]
    struct FakeModel {
        status: StatusCode,
        answer: Value,
        asked: Arc<Mutex<Vec<Value>>>,
    }

    impl FakeModel {
        fn answering(answer: Value) -> Self {
            Self { status: StatusCode::OK, answer, asked: Arc::default() }
        }

        fn failing(status: StatusCode) -> Self {
            Self { status, answer: Value::Null, asked: Arc::default() }
        }

        fn asked(&self) -> Vec<Value> {
            self.asked.lock().unwrap().clone()
        }
    }

    /// Serves `fake`; its URL.
    async fn serve(fake: FakeModel) -> String {
        let app = Router::new()
            .route("/v1/systemone", post(|State(fake): State<FakeModel>, Json(body): Json<Value>| async move {
                fake.asked.lock().unwrap().push(body);
                (fake.status, Json(fake.answer.clone()))
            }))
            .with_state(fake);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        url
    }

    /// A policy asking the model at `url`, as `params` configure it.
    fn policy(url: &str, params: Value) -> DecisionPolicy {
        let mut config: DecisionPolicyConfig = config_of_params(&params).unwrap();
        config.endpoint = RemoteEndpoint::new(url);
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(AllowAllPolicy::new())));
        let profile = config.profile();
        DecisionPolicy {
            client: DecisionClient::new(config.endpoint, config.model, config.keep_alive, secret_manager).unwrap(),
            threshold: config.threshold,
            max_responders: config.max_responders,
            on_failure: config.on_failure,
            destination: config.destination.unwrap_or(Classification::PUBLIC),
            profile,
            confidentiality: Arc::new(ConfidentialityRules::default()),
            prompts: prompts(),
        }
    }

    /// The questions of the decision model, as compiled in.
    fn prompts() -> PromptTexts {
        let mut catalogue = PromptCatalogue::new();
        catalogue.add(NAMESPACE, PROMPTS).expect("the questions are compiled in");
        PromptTexts::new(Arc::new(catalogue), None)
    }

    fn agent(name: &str, introduction: Option<&str>) -> ActorDescription {
        ActorDescription { id: ActorId::Agent(AgentId::new()), name: Name::new(name).unwrap(), introduction: introduction.map(|text| Introduction::new(text).unwrap()) }
    }

    /// The message `content` of `sender`, called `name`, in `role`,
    /// classified `classification`.
    fn message(sender: ActorId, name: &str, role: MessageRole, content: &str, classification: Classification) -> Message {
        Message { id: MessageId::new(), sender_id: sender, sender_name: name.to_string(), role, content: Some(content.to_string()), ..Message::new(classification) }
    }

    /// The event that says `message`, the last of `conversation`, and the
    /// conversation.
    fn saying(conversation: Vec<Message>) -> (Envelope, Vec<Message>) {
        let message = conversation.last().cloned().unwrap();
        let context = EventContext::new(Default::default(), Default::default(), message.sender_id, message.sender_name.clone());
        (Envelope::new(SecurityContext::default(), context, ActorEvent::CompleteMessageGenerated(message)), conversation)
    }

    /// A message `content` of Alice, classified `classification`, and the
    /// event that says it, with nothing said before.
    fn said(content: &str, classification: Classification) -> (Envelope, Vec<Message>) {
        saying(vec![message(ActorId::User(UserId::new()), "Alice", MessageRole::User, content, classification)])
    }

    fn choosing(choice: &str, probabilities: Value) -> Value {
        json!({ "model": "nimble", "answers": { "respondent": { "type": "choice", "choice": choice, "probabilities": probabilities, "confidence": 0.5 } } })
    }

    #[tokio::test]
    async fn the_model_is_asked_to_choose_among_the_candidates_by_their_introductions() {
        let fake = FakeModel::answering(choosing("coder", json!({ "coder": 0.8, "reviewer": 0.15, "nobody": 0.05 })));
        let url = serve(fake.clone()).await;
        let (coder, reviewer) = (agent("Coder", Some("Writes\nthe code.")), agent("Reviewer", None));
        let (event, messages) = said("The test fails.", Classification::PUBLIC);

        let chosen = policy(&url, json!({ "keep_alive": "5m" })).choose(&event, &messages, vec![coder.clone(), reviewer]).await;

        assert_eq!(chosen, [coder.id]);
        assert_eq!(fake.asked(), [json!({
            "model": "nimble",
            "state": "@alice: The test fails.",
            "keep_alive": "5m",
            "questions": { "respondent": {
                "type": "choice",
                "instructions": "Which participant of the conversation should respond to the last message? Choose nobody if it needs no answer.",
                "criteria": {
                    "coder": "Coder: Writes the code.",
                    "reviewer": "Reviewer",
                    "nobody": "Nobody: the message needs no answer from any of them.",
                },
            } },
        })]);
    }

    /// Issue 414: the model is given the conversation, each message as a
    /// model is given one of another participant - an agent's answer as it
    /// is heard -, its sender marked as configured.
    #[tokio::test]
    async fn the_conversation_is_sent_with_who_said_what() {
        let fake = FakeModel::answering(choosing("reviewer", json!({ "reviewer": 0.9 })));
        let url = serve(fake.clone()).await;
        let (alice, coder) = (ActorId::User(UserId::new()), ActorId::Agent(AgentId::new()));
        let (event, messages) = saying(vec![
            message(alice, "Alice", MessageRole::User, "The test fails.", Classification::PUBLIC),
            message(coder, "Coder", MessageRole::Assistant, "Fixed it.", Classification::PUBLIC),
            message(alice, "Alice", MessageRole::User, "Thanks, please have a look.", Classification::PUBLIC),
        ]);
        let candidates = vec![agent("Reviewer", None), agent("Coder", None)];

        policy(&url, json!({})).choose(&event, &messages, candidates.clone()).await;
        policy(&url, json!({ "sender_format": "xml", "sender_style": "name" })).choose(&event, &messages, candidates.clone()).await;
        policy(&url, json!({ "sender_format": "none" })).choose(&event, &messages, candidates).await;

        let states: Vec<Value> = fake.asked().iter().map(|asked| asked["state"].clone()).collect();
        assert_eq!(states, [
            "@alice: The test fails.\n\n@coder: Fixed it.\n\n@alice: Thanks, please have a look.",
            "<message from=\"Alice\">The test fails.</message>\n\n<message from=\"Coder\">Fixed it.</message>\n\n<message from=\"Alice\">Thanks, please have a look.</message>",
            "The test fails.\n\nFixed it.\n\nThanks, please have a look.",
        ]);
    }

    /// Issue 414: what was said before is left out where it may not go, and
    /// beyond the budget, the oldest first - what is to be answered never.
    #[tokio::test]
    async fn what_was_said_before_is_left_out_where_it_may_not_go_and_beyond_the_budget() {
        let fake = FakeModel::answering(choosing("a", json!({ "a": 1.0 })));
        let url = serve(fake.clone()).await;
        let alice = ActorId::User(UserId::new());
        let long = "x".repeat(4 * MAX_STATE_TOKENS as usize);
        let (event, messages) = saying(vec![
            message(alice, "Alice", MessageRole::User, &long, Classification::PUBLIC),
            message(alice, "Alice", MessageRole::User, "Internal matters.", Classification::general(Confidentiality::Internal)),
            message(alice, "Alice", MessageRole::User, "Before.", Classification::PUBLIC),
            message(alice, "Alice", MessageRole::User, "Now.", Classification::PUBLIC),
        ]);

        policy(&url, json!({})).choose(&event, &messages, vec![agent("A", None)]).await;

        assert_eq!(fake.asked()[0]["state"], "@alice: Before.\n\n@alice: Now.");
    }

    #[tokio::test]
    async fn the_most_probable_is_asked_and_others_above_the_threshold_up_to_the_limit() {
        let (a, b, c) = (agent("A", None), agent("B", None), agent("C", None));
        let candidates = vec![a.clone(), b.clone(), c.clone()];
        let (event, messages) = said("Who?", Classification::PUBLIC);
        let probabilities = json!({ "a": 0.2, "b": 0.45, "c": 0.35, "nobody": 0.0 });
        let url = serve(FakeModel::answering(choosing("b", probabilities))).await;

        assert_eq!(policy(&url, json!({})).choose(&event, &messages, candidates.clone()).await, [b.id], "one by default");
        assert_eq!(policy(&url, json!({ "max_responders": 3 })).choose(&event, &messages, candidates.clone()).await, [b.id, c.id], "those above the threshold");
        assert_eq!(policy(&url, json!({ "max_responders": 3, "threshold": 0.5 })).choose(&event, &messages, candidates.clone()).await, [b.id], "the chosen one even below it");

        let url = serve(FakeModel::answering(choosing("nobody", json!({ "a": 0.3, "b": 0.3, "c": 0.0, "nobody": 0.4 })))).await;
        assert!(policy(&url, json!({ "max_responders": 3 })).choose(&event, &messages, candidates).await.is_empty(), "nobody, if that is most probable");
    }

    #[tokio::test]
    async fn beyond_twenty_five_candidates_each_is_asked_about_on_its_own() {
        let candidates: Vec<ActorDescription> = (0..30).map(|n| agent(&format!("Agent {n}"), None)).collect();
        let answers: serde_json::Map<String, Value> = (0..30).map(|n| (format!("agent-{n}"), json!({ "type": "noul", "noul": if n == 7 { 0.9 } else { 0.1 } }))).collect();
        let fake = FakeModel::answering(json!({ "answers": answers }));
        let url = serve(fake.clone()).await;
        let (event, messages) = said("Who?", Classification::PUBLIC);

        assert_eq!(policy(&url, json!({})).choose(&event, &messages, candidates.clone()).await, [candidates[7].id]);
        let questions = fake.asked()[0]["questions"].as_object().unwrap().clone();
        assert_eq!(questions.len(), 30);
        assert_eq!(questions["agent-7"], json!({ "type": "noul", "instructions": "Should this participant respond to the last message? Agent 7" }));
    }

    #[tokio::test]
    async fn a_model_that_fails_asks_whom_the_configuration_says() {
        let url = serve(FakeModel::failing(StatusCode::INTERNAL_SERVER_ERROR)).await;
        let (a, b) = (agent("A", None), agent("B", None));
        let (event, messages) = said("Who?", Classification::PUBLIC);

        assert!(policy(&url, json!({})).choose(&event, &messages, vec![a.clone(), b.clone()]).await.is_empty(), "nobody by default");
        assert_eq!(policy(&url, json!({ "on_failure": "all" })).choose(&event, &messages, vec![a, b]).await.len(), 2, "everyone, as configured");

        let url = serve(FakeModel::answering(choosing("someone", json!({})))).await;
        let (a, b) = (agent("A", None), agent("B", None));
        assert_eq!(policy(&url, json!({ "on_failure": "all" })).choose(&event, &messages, vec![a.clone(), b.clone()]).await, [a.id, b.id], "an answer that names no option");
    }

    #[tokio::test]
    async fn what_may_not_flow_to_the_endpoint_is_not_sent() {
        let fake = FakeModel::answering(choosing("a", json!({ "a": 1.0 })));
        let url = serve(fake.clone()).await;
        let a = agent("A", None);
        let (event, messages) = said("Internal matters.", Classification::general(Confidentiality::Internal));

        assert!(policy(&url, json!({})).choose(&event, &messages, vec![a.clone()]).await.is_empty(), "asked as on failure: nobody");
        assert!(fake.asked().is_empty(), "nothing sent to a public endpoint");
        assert_eq!(policy(&url, json!({ "destination": { "level": "internal" } })).choose(&event, &messages, vec![a.clone()]).await, [a.id]);
        assert_eq!(fake.asked().len(), 1, "sent where it is cleared");
    }

    #[test]
    fn a_candidate_is_asked_about_by_its_handle_unless_another_has_it() {
        let candidates = [agent("Coder", None), agent("coder", None), agent("Nobody", None), agent("!", None)];
        assert_eq!(options(&candidates), ["coder", "agent-2", "agent-3", "agent-4"]);
    }

    #[test]
    fn its_parameters_are_checked() {
        let checked = |params: Value| config_of_params::<DecisionPolicyConfig>(&params).and_then(|config| config.check());
        assert!(checked(json!({})).is_ok(), "every one has a default");
        assert!(checked(json!({ "threshold": 1.5 })).is_err());
        assert!(checked(json!({ "max_responders": 0 })).is_err());
        assert!(checked(json!({ "on_failure": "some" })).is_err());
        assert!(checked(json!({ "sender_format": "xml", "sender_style": "name" })).is_ok());
        assert!(checked(json!({ "sender_format": "header" })).is_err());
        assert!(checked(json!({ "endpoint": { "url": "http://decide.example", "auth": { "type": "bearer", "secret": "DECIDE" } } })).is_ok());
        assert!(checked(json!({ "temperature": 0.1 })).is_err(), "nothing unknown");
    }

    /// Where it sends what was said is the operator's to say: a request
    /// names it only through a `ref` (`ComponentKind::operator_only`).
    #[tokio::test]
    async fn a_request_does_not_write_it_itself() {
        let mut registry = ComponentRegistry::built_in(Persistence::in_memory().entity_stores().unwrap().component_bundles);
        register(&mut registry);
        let config = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("responders", DECISION_POLICY_KIND)] };
        let refused = registry.require_acceptable(&config, ChannelLoop::COMPONENT_FACETS, &SecurityContext::default()).await;
        assert!(refused.is_err_and(|e| e.to_string().contains("configured by an operator")));
    }
}
