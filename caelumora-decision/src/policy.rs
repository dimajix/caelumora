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
//! Its model is one the platform offers (issue 551), named by `name` and asked through a client
//! built for the channel. What was said leaves the system where that model's server lies outside
//! it: it is sent only if it may reach the model (`DecisionModelClient::may_receive`). What was
//! said before is left out if it may not; what is to be answered keeps the model from choosing.
//! Whatever keeps the model from choosing - that, a timeout, an error, too many candidates - is
//! answered `Undecided`, without anything that was said: whom to ask then is the moderator's to
//! decide (issue 441).

use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_api::component::{ComponentError, ComponentResult, Params};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_api::decision::{Answer, DecisionModelClient, Question, MAX_OPTIONS, MAX_QUESTIONS};
use caelumora_api::model::ModelError;
use caelumora_domain::actor::{ActorDescription, ActorId, Envelope};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::{ModelProfile, SenderFormat, SenderStyle};
use caelumora_domain::message::{estimated_tokens, Message, MessageId};
use caelumora_domain::prompt_catalogue::{
    context, PromptCatalogue, PromptError, PromptTexts, PromptVariant,
};
use caelumora_domain::value::time_zone::TimeZone;
use caelumora_kernel::component::ComponentRegistry;
use caelumora_kernel::kernel::Acting;
use caelumora_kernel::moderator::{ResponderPolicy, Undecided};
use caelumora_model::ModelManager;
use caelumora_security::SecurityContext;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;

pub const DECISION_POLICY_KIND: &str = "decision_policy";

/// The option of the `choice` that asks nobody to respond.
const NOBODY: &str = "nobody";

/// The name of the `choice` question.
const RESPONDENT: &str = "respondent";

/// How many tokens what was said may take at most, as `estimated_tokens`
/// estimates them: half of the 8,192 a decision model reads, the rest left
/// to the questions - up to 26 options, each with its introduction.
const MAX_STATE_TOKENS: u64 = 4096;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionPolicyParams {
    /// The model of `plugins.model.decision`: the one there is if left out.
    #[serde(default)]
    name: Option<String>,
    /// How probable it must be that a candidate fits to be asked besides
    /// the most probable one - or at all, beyond 25 candidates.
    #[serde(default = "default_threshold")]
    threshold: f64,
    #[serde(default = "default_max_responders")]
    max_responders: usize,
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

impl DecisionPolicyParams {
    /// How the model is given what was said - without the times of the
    /// messages (issue 424), which who is to respond does not depend on.
    fn profile(&self) -> ModelProfile {
        ModelProfile {
            sender_format: self.sender_format,
            sender_style: self.sender_style,
            prompt_texts: self.prompt_texts,
            ..ModelProfile::default()
        }
    }
}

impl Params for DecisionPolicyParams {
    /// # Errors
    /// `ComponentError::InvalidConfig` if the threshold is no probability or nobody may be asked
    /// to respond.
    fn check(&self) -> ComponentResult<()> {
        if !(0.0..=1.0).contains(&self.threshold) {
            return Err(ComponentError::InvalidConfig(format!(
                "threshold {} is no probability",
                self.threshold
            )));
        }
        if self.max_responders == 0 {
            return Err(ComponentError::InvalidConfig(
                "max_responders must be at least 1".to_string(),
            ));
        }
        Ok(())
    }
}

fn default_threshold() -> f64 {
    0.3
}

fn default_max_responders() -> usize {
    1
}

/// Why the model did not choose.
#[derive(Debug, thiserror::Error)]
enum NoChoice {
    #[error("what was said may not be sent to the decision model {0}")]
    NotCleared(String),
    #[error("{0} candidates are more than a decision model is asked about")]
    TooMany(usize),
    #[error(transparent)]
    Model(#[from] ModelError),
    /// An answer that is none to what it was asked: the model's name, and why.
    #[error("the decision model {0} answered what cannot be read: {1}")]
    Unreadable(String, String),
    /// What it is asked cannot be worded (issue 57).
    #[error("the questions cannot be worded: {0}")]
    Unworded(#[from] PromptError),
}

/// Asks a decision model whom to ask to respond.
#[derive(Debug)]
pub struct DecisionPolicy {
    client: Arc<dyn DecisionModelClient>,
    threshold: f64,
    max_responders: usize,
    profile: ModelProfile,
    /// What the model is asked, in the variant it names.
    prompts: PromptTexts,
}

#[async_trait]
impl ResponderPolicy for DecisionPolicy {
    async fn choose(
        &self,
        event: &Envelope,
        context: &SecurityContext,
        messages: &[Message],
        candidates: Vec<ActorDescription>,
    ) -> Result<Vec<ActorId>, Undecided> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        self.decide(event, context.initiator_id, messages, &candidates)
            .await
            .map_err(|why| Undecided(why.to_string()))
    }
}

impl DecisionPolicy {
    /// Whom of `candidates` the model chooses to respond to what `event`
    /// said, the end of the conversation `messages`, asked for `initiator`.
    ///
    /// # Errors
    /// `NotCleared` if what `event` said may not reach the model; `TooMany` beyond 64 candidates;
    /// `Unworded` if it cannot be asked; `Model` if the model gave no answer; `Unreadable` if it
    /// gave one that names no option.
    async fn decide(
        &self,
        event: &Envelope,
        initiator: ActorId,
        messages: &[Message],
        candidates: &[ActorDescription],
    ) -> Result<Vec<ActorId>, NoChoice> {
        let (state, held) = self.state(event, initiator, messages)?;
        let options = options(candidates);
        if candidates.len() < MAX_OPTIONS {
            let answers = self
                .client
                .decide(initiator, &held, &state, &choice(&self.prompts, candidates, &options)?)
                .await?;
            self.chosen_by_choice(answers.get(RESPONDENT), candidates, &options)
        } else if candidates.len() <= MAX_QUESTIONS {
            let answers = self
                .client
                .decide(initiator, &held, &state, &nouls(&self.prompts, candidates, &options)?)
                .await?;
            let probabilities =
                options
                    .iter()
                    .map(|option| match answers.get(option) {
                        Some(Answer::Noul { noul }) => Ok(*noul),
                        _ => Err(self
                            .invalid(format!("the question '{option}' is not answered yes or no"))),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
            Ok(self.above_threshold(candidates, &probabilities, None))
        } else {
            Err(NoChoice::TooMany(candidates.len()))
        }
    }

    /// What the model judges, and how what it holds is classified: the conversation `messages`,
    /// each message heard as a model is given one of another participant, one after another.
    /// What was said before is left out if it may not reach the model, asked for `initiator`, and
    /// beyond `MAX_STATE_TOKENS` the oldest first.
    ///
    /// # Errors
    /// `NotCleared` if what `event` said may not reach the model.
    fn state(
        &self,
        event: &Envelope,
        initiator: ActorId,
        messages: &[Message],
    ) -> Result<(String, Vec<Classification>), NoChoice> {
        let answered: HashSet<MessageId> =
            event.event.messages().iter().map(|message| message.id).collect();
        // Each text, whether it is to be answered, and how it is classified.
        let mut texts = VecDeque::new();
        for message in messages.iter().filter_map(Message::as_heard) {
            let to_answer = answered.contains(&message.id);
            if !self.client.may_receive(initiator, &message.classification) {
                if to_answer {
                    return Err(NoChoice::NotCleared(self.client.name().to_string()));
                }
                continue;
            }
            let classification = message.classification.clone();
            // Who is to respond is decided by what was said, not when: the
            // profile gives no times, and the time zone is none's.
            if let Some(text) =
                self.profile.as_said(message, ActorId::None, false, TimeZone::UTC).content
            {
                texts.push_back((text, to_answer, classification));
            }
        }
        let separators = 2 * texts.len().saturating_sub(1);
        let mut bytes = texts.iter().map(|(text, ..)| text.len()).sum::<usize>() + separators;
        while estimated_tokens(bytes) > MAX_STATE_TOKENS
            && texts.front().is_some_and(|(_, to_answer, _)| !to_answer)
        {
            if let Some((text, ..)) = texts.pop_front() {
                bytes = bytes.saturating_sub(text.len() + 2);
            }
        }
        let (texts, held): (Vec<_>, Vec<_>) =
            texts.into_iter().map(|(text, _, classification)| (text, classification)).unzip();
        Ok((texts.join("\n\n"), held))
    }

    /// Whom the `choice` `answer` chooses: nobody if it chose nobody, else
    /// the candidate it chose and those above the threshold besides, the
    /// most probable first, at most `max_responders`.
    fn chosen_by_choice(
        &self,
        answer: Option<&Answer>,
        candidates: &[ActorDescription],
        options: &[String],
    ) -> Result<Vec<ActorId>, NoChoice> {
        let Some(Answer::Choice { choice, probabilities }) = answer else {
            return Err(
                self.invalid(format!("the question '{RESPONDENT}' is not answered with a choice"))
            );
        };
        if choice == NOBODY {
            return Ok(Vec::new());
        }
        let first = options
            .iter()
            .position(|option| option == choice)
            .ok_or_else(|| self.invalid(format!("it chose '{choice}', which is no option")))?;
        let probabilities: Vec<f64> = options
            .iter()
            .map(|option| probabilities.get(option).copied().unwrap_or(0.0))
            .collect();
        Ok(self.above_threshold(candidates, &probabilities, Some(first)))
    }

    /// The candidates whose `probabilities` reach the threshold - and
    /// `first`, if one is chosen anyway -, the most probable first, at most
    /// `max_responders`.
    fn above_threshold(
        &self,
        candidates: &[ActorDescription],
        probabilities: &[f64],
        first: Option<usize>,
    ) -> Vec<ActorId> {
        let mut ranked: Vec<usize> = (0..candidates.len())
            .filter(|&index| Some(index) != first && probabilities[index] >= self.threshold)
            .collect();
        ranked.sort_by(|a, b| probabilities[*b].total_cmp(&probabilities[*a]));
        first
            .into_iter()
            .chain(ranked)
            .take(self.max_responders)
            .map(|index| candidates[index].id)
            .collect()
    }

    fn invalid(&self, reason: String) -> NoChoice {
        NoChoice::Unreadable(self.client.name().to_string(), reason)
    }
}

/// The name each candidate is asked about by, in their order: its handle,
/// unless it has none, or one taken before or by "nobody" - then
/// `agent-<n>`, by its place.
fn options(candidates: &[ActorDescription]) -> Vec<String> {
    let mut taken = HashSet::from([NOBODY.to_string()]);
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let option = candidate
                .handle()
                .filter(|handle| !taken.contains(handle))
                .unwrap_or_else(|| format!("agent-{}", index + 1));
            taken.insert(option.clone());
            option
        })
        .collect()
}

/// The text `key` of `prompts` for the decision model, rendered with
/// `values`.
fn text(prompts: &PromptTexts, key: &str, values: impl Serialize) -> Result<String, PromptError> {
    prompts.render(&format!("{NAMESPACE}.{key}"), values)
}

/// Who `candidate` is, for the model: its name, and how it is introduced,
/// on one line.
fn described(prompts: &PromptTexts, candidate: &ActorDescription) -> Result<String, PromptError> {
    let introduction = candidate
        .introduction
        .as_ref()
        .map(|introduction| introduction.as_str().split_whitespace().collect::<Vec<_>>().join(" "));
    text(prompts, "candidate", context! { name => candidate.name.as_str(), introduction })
}

/// One `choice` over the candidates, by `options`, and nobody.
fn choice(
    prompts: &PromptTexts,
    candidates: &[ActorDescription],
    options: &[String],
) -> Result<BTreeMap<String, Question>, PromptError> {
    let mut criteria = BTreeMap::new();
    for (option, candidate) in options.iter().zip(candidates) {
        criteria.insert(option.clone(), Some(described(prompts, candidate)?));
    }
    criteria.insert(NOBODY.to_string(), Some(text(prompts, "choice.nobody", ())?));
    let instructions = text(prompts, "choice.instructions", ())?;
    Ok(BTreeMap::from([(RESPONDENT.to_string(), Question::Choice { instructions, criteria })]))
}

/// One `noul` per candidate, by its option.
fn nouls(
    prompts: &PromptTexts,
    candidates: &[ActorDescription],
    options: &[String],
) -> Result<BTreeMap<String, Question>, PromptError> {
    options
        .iter()
        .zip(candidates)
        .map(|(option, candidate)| {
            let instructions =
                text(prompts, "noul", context! { candidate => described(prompts, candidate)? })?;
            Ok((option.clone(), Question::Noul { instructions }))
        })
        .collect()
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(DECISION_POLICY_KIND)
            .params::<DecisionPolicyParams>()
            .provides::<dyn ResponderPolicy>()
            .uses::<ModelManager>()
            .uses::<Acting>()
            .uses::<PromptCatalogue>(),
        |descriptor, context| {
            let params = DecisionPolicyParams::read(&descriptor.params)?;
            let catalogue = context.service::<PromptCatalogue>()?;
            catalogue.require_variant(params.prompt_texts)?;
            let prompts = PromptTexts::new(Arc::clone(catalogue), params.prompt_texts);
            let profile = params.profile();
            // For the channel it is built for, in its owner's name.
            let acting = Acting::clone(context.service::<Acting>()?);
            let models = context.service::<ModelManager>()?;
            let client = models.decision_client_for(acting, params.name.as_deref())?;
            let policy = DecisionPolicy {
                client,
                threshold: params.threshold,
                max_responders: params.max_responders,
                profile,
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
    use crate::plugin::DecisionPlugin;
    use crate::PROMPTS;
    use caelumora_api::model::ModelResult;
    use caelumora_domain::actor::{ActorEvent, EventContext};
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_domain::confidentiality::Confidentiality;
    use caelumora_domain::message::{MessageId, MessageRole};
    use caelumora_domain::user::UserId;
    use caelumora_domain::value::name::Name;
    use caelumora_domain::value::text::Introduction;
    use caelumora_kernel::channel::ChannelLoop;
    use caelumora_kernel::component::testing::{built, descriptor};
    use caelumora_kernel::testing::TestKernel;
    use caelumora_model::plugin::ModelPlugin;
    use caelumora_security::confidentiality::{ConfidentialityPolicy, ConfidentialityRules};
    use caelumora_security::Policies;
    use caelumora_store::{EntityStores, Persistence};
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A decision model that answers every request with `answer`, or fails if it is `None`, that
    /// may be told what flows to `destination`, and keeps what it was asked.
    #[derive(Debug, Clone)]
    struct FakeModel {
        answer: Option<Value>,
        destination: Classification,
        asked: Arc<Mutex<Vec<Value>>>,
    }

    impl FakeModel {
        fn answering(answer: Value) -> Self {
            Self {
                answer: Some(answer),
                destination: Classification::PUBLIC,
                asked: Arc::default(),
            }
        }

        fn failing() -> Self {
            Self { answer: None, destination: Classification::PUBLIC, asked: Arc::default() }
        }

        /// The model, cleared for `destination`, and keeping what it was asked with this one.
        fn cleared_for(&self, destination: Classification) -> Self {
            Self { destination, ..self.clone() }
        }

        fn asked(&self) -> Vec<Value> {
            self.asked.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DecisionModelClient for FakeModel {
        fn name(&self) -> &str {
            "fake"
        }

        fn may_receive(&self, _: ActorId, classification: &Classification) -> bool {
            ConfidentialityRules::default().may_publish(
                &SecurityContext::default(),
                classification,
                &self.destination,
            )
        }

        async fn decide(
            &self,
            initiator: ActorId,
            classifications: &[Classification],
            state: &str,
            questions: &BTreeMap<String, Question>,
        ) -> ModelResult<HashMap<String, Answer>> {
            assert!(classifications.iter().all(|c| self.may_receive(initiator, c)));
            let questions: serde_json::Map<String, Value> =
                questions.iter().map(|(name, q)| (name.clone(), written(q))).collect();
            self.asked.lock().unwrap().push(json!({ "state": state, "questions": questions }));
            let failed = ModelError::Backend {
                endpoint: "fake".to_string(),
                model: "fake".to_string(),
                message: "it fails".to_string(),
            };
            let answer = self.answer.clone().ok_or(failed)?;
            let answers = answer["answers"].as_object().unwrap();
            Ok(answers.iter().map(|(name, a)| (name.clone(), read(a))).collect())
        }
    }

    /// `question` as a test compares it.
    fn written(question: &Question) -> Value {
        match question {
            Question::Choice { instructions, criteria } => {
                json!({ "type": "choice", "instructions": instructions, "criteria": criteria })
            }
            Question::Noul { instructions } => {
                json!({ "type": "noul", "instructions": instructions })
            }
        }
    }

    /// The answer `answer` of a test says.
    fn read(answer: &Value) -> Answer {
        match answer["type"].as_str().unwrap() {
            "choice" => Answer::Choice {
                choice: answer["choice"].as_str().unwrap().to_string(),
                probabilities: serde_json::from_value(answer["probabilities"].clone()).unwrap(),
            },
            _ => Answer::Noul { noul: answer["noul"].as_f64().unwrap() },
        }
    }

    /// A policy asking `model`, as `params` configure it.
    fn policy(model: &FakeModel, params: Value) -> DecisionPolicy {
        let params = DecisionPolicyParams::read(&params).unwrap();
        let profile = params.profile();
        DecisionPolicy {
            client: Arc::new(model.clone()),
            threshold: params.threshold,
            max_responders: params.max_responders,
            profile,
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
        ActorDescription {
            id: ActorId::Agent(AgentId::new()),
            name: Name::new(name).unwrap(),
            introduction: introduction.map(|text| Introduction::new(text).unwrap()),
        }
    }

    /// The message `content` of `sender`, called `name`, in `role`,
    /// classified `classification`.
    fn message(
        sender: ActorId,
        name: &str,
        role: MessageRole,
        content: &str,
        classification: Classification,
    ) -> Message {
        Message {
            id: MessageId::new(),
            sender_id: sender,
            sender_name: name.to_string(),
            role,
            content: Some(content.to_string()),
            ..Message::new(classification)
        }
    }

    /// The event that says `message`, the last of `conversation`, and the
    /// conversation.
    fn saying(conversation: Vec<Message>) -> (Envelope, Vec<Message>) {
        let message = conversation.last().cloned().unwrap();
        let context = EventContext::new(
            Default::default(),
            Default::default(),
            message.sender_id,
            message.sender_name.clone(),
        );
        (Envelope::new(context, ActorEvent::CompleteMessageGenerated(message)), conversation)
    }

    /// A message `content` of Alice, classified `classification`, and the
    /// event that says it, with nothing said before.
    fn said(content: &str, classification: Classification) -> (Envelope, Vec<Message>) {
        saying(vec![message(
            ActorId::User(UserId::new()),
            "Alice",
            MessageRole::User,
            content,
            classification,
        )])
    }

    fn choosing(choice: &str, probabilities: Value) -> Value {
        json!({ "model": "nimble", "answers": { "respondent": { "type": "choice", "choice": choice, "probabilities": probabilities, "confidence": 0.5 } } })
    }

    #[tokio::test]
    async fn the_model_is_asked_to_choose_among_the_candidates_by_their_introductions() {
        let fake = FakeModel::answering(choosing(
            "coder",
            json!({ "coder": 0.8, "reviewer": 0.15, "nobody": 0.05 }),
        ));
        let coder = agent("Coder", Some("Writes\nthe code."));
        let reviewer = agent("Reviewer", None);
        let (event, messages) = said("The test fails.", Classification::PUBLIC);

        let chosen = policy(&fake, json!({}))
            .choose(&event, &SecurityContext::default(), &messages, vec![coder.clone(), reviewer])
            .await
            .unwrap();

        assert_eq!(chosen, [coder.id]);
        assert_eq!(
            fake.asked(),
            [json!({
                "state": "@alice: The test fails.",
                "questions": { "respondent": {
                    "type": "choice",
                    "instructions": "Which participant of the conversation should respond to the last message? Choose nobody if it needs no answer.",
                    "criteria": {
                        "coder": "Coder: Writes the code.",
                        "reviewer": "Reviewer",
                        "nobody": "Nobody: the message needs no answer from any of them.",
                    },
                } },
            })]
        );
    }

    /// Issue 414: the model is given the conversation, each message as a
    /// model is given one of another participant - an agent's answer as it
    /// is heard -, its sender marked as configured.
    #[tokio::test]
    async fn the_conversation_is_sent_with_who_said_what() {
        let fake = FakeModel::answering(choosing("reviewer", json!({ "reviewer": 0.9 })));
        let (alice, coder) = (ActorId::User(UserId::new()), ActorId::Agent(AgentId::new()));
        let (event, messages) = saying(vec![
            message(alice, "Alice", MessageRole::User, "The test fails.", Classification::PUBLIC),
            message(coder, "Coder", MessageRole::Assistant, "Fixed it.", Classification::PUBLIC),
            message(
                alice,
                "Alice",
                MessageRole::User,
                "Thanks, please have a look.",
                Classification::PUBLIC,
            ),
        ]);
        let candidates = vec![agent("Reviewer", None), agent("Coder", None)];

        policy(&fake, json!({}))
            .choose(&event, &SecurityContext::default(), &messages, candidates.clone())
            .await
            .unwrap();
        policy(&fake, json!({ "sender_format": "xml", "sender_style": "name" }))
            .choose(&event, &SecurityContext::default(), &messages, candidates.clone())
            .await
            .unwrap();
        policy(&fake, json!({ "sender_format": "none" }))
            .choose(&event, &SecurityContext::default(), &messages, candidates)
            .await
            .unwrap();

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
        let alice = ActorId::User(UserId::new());
        let long = "x".repeat(4 * MAX_STATE_TOKENS as usize);
        let (event, messages) = saying(vec![
            message(alice, "Alice", MessageRole::User, &long, Classification::PUBLIC),
            message(
                alice,
                "Alice",
                MessageRole::User,
                "Internal matters.",
                Classification::general(Confidentiality::Internal),
            ),
            message(alice, "Alice", MessageRole::User, "Before.", Classification::PUBLIC),
            message(alice, "Alice", MessageRole::User, "Now.", Classification::PUBLIC),
        ]);

        policy(&fake, json!({}))
            .choose(&event, &SecurityContext::default(), &messages, vec![agent("A", None)])
            .await
            .unwrap();

        assert_eq!(fake.asked()[0]["state"], "@alice: Before.\n\n@alice: Now.");
    }

    #[tokio::test]
    async fn the_most_probable_is_asked_and_others_above_the_threshold_up_to_the_limit() {
        let (a, b, c) = (agent("A", None), agent("B", None), agent("C", None));
        let candidates = vec![a.clone(), b.clone(), c.clone()];
        let (event, messages) = said("Who?", Classification::PUBLIC);
        let probabilities = json!({ "a": 0.2, "b": 0.45, "c": 0.35, "nobody": 0.0 });
        let fake = FakeModel::answering(choosing("b", probabilities));

        assert_eq!(
            policy(&fake, json!({}))
                .choose(&event, &SecurityContext::default(), &messages, candidates.clone())
                .await
                .unwrap(),
            [b.id],
            "one by default"
        );
        assert_eq!(
            policy(&fake, json!({ "max_responders": 3 }))
                .choose(&event, &SecurityContext::default(), &messages, candidates.clone())
                .await
                .unwrap(),
            [b.id, c.id],
            "those above the threshold"
        );
        assert_eq!(
            policy(&fake, json!({ "max_responders": 3, "threshold": 0.5 }))
                .choose(&event, &SecurityContext::default(), &messages, candidates.clone())
                .await
                .unwrap(),
            [b.id],
            "the chosen one even below it"
        );

        let fake = FakeModel::answering(choosing(
            "nobody",
            json!({ "a": 0.3, "b": 0.3, "c": 0.0, "nobody": 0.4 }),
        ));
        assert!(
            policy(&fake, json!({ "max_responders": 3 }))
                .choose(&event, &SecurityContext::default(), &messages, candidates)
                .await
                .unwrap()
                .is_empty(),
            "nobody, if that is most probable"
        );
    }

    #[tokio::test]
    async fn beyond_twenty_five_candidates_each_is_asked_about_on_its_own() {
        let candidates: Vec<ActorDescription> =
            (0..30).map(|n| agent(&format!("Agent {n}"), None)).collect();
        let answers: serde_json::Map<String, Value> = (0..30)
            .map(|n| {
                (
                    format!("agent-{n}"),
                    json!({ "type": "noul", "noul": if n == 7 { 0.9 } else { 0.1 } }),
                )
            })
            .collect();
        let fake = FakeModel::answering(json!({ "answers": answers }));
        let (event, messages) = said("Who?", Classification::PUBLIC);

        assert_eq!(
            policy(&fake, json!({}))
                .choose(&event, &SecurityContext::default(), &messages, candidates.clone())
                .await
                .unwrap(),
            [candidates[7].id]
        );
        let questions = fake.asked()[0]["questions"].as_object().unwrap().clone();
        assert_eq!(questions.len(), 30);
        assert_eq!(
            questions["agent-7"],
            json!({ "type": "noul", "instructions": "Should this participant respond to the last message? Agent 7" })
        );
    }

    /// Issue 441: a policy whose model does not choose says so - whom to ask then is the
    /// moderator's to decide.
    #[tokio::test]
    async fn a_model_that_fails_leaves_it_undecided() {
        let fake = FakeModel::failing();
        let (a, b) = (agent("A", None), agent("B", None));
        let (event, messages) = said("Who?", Classification::PUBLIC);

        assert!(
            policy(&fake, json!({}))
                .choose(&event, &SecurityContext::default(), &messages, vec![a.clone(), b.clone()])
                .await
                .is_err(),
            "a model that fails"
        );

        let fake = FakeModel::answering(choosing("someone", json!({})));
        assert!(
            policy(&fake, json!({}))
                .choose(&event, &SecurityContext::default(), &messages, vec![a, b])
                .await
                .is_err(),
            "an answer that names no option"
        );
    }

    #[tokio::test]
    async fn what_may_not_reach_the_model_is_not_sent() {
        let fake = FakeModel::answering(choosing("a", json!({ "a": 1.0 })));
        let a = agent("A", None);
        let (event, messages) =
            said("Internal matters.", Classification::general(Confidentiality::Internal));

        assert!(
            policy(&fake, json!({}))
                .choose(&event, &SecurityContext::default(), &messages, vec![a.clone()])
                .await
                .is_err(),
            "undecided"
        );
        assert!(fake.asked().is_empty(), "nothing sent to a model cleared for what is public");
        let cleared = fake.cleared_for(Classification::general(Confidentiality::Internal));
        assert_eq!(
            policy(&cleared, json!({}))
                .choose(&event, &SecurityContext::default(), &messages, vec![a.clone()])
                .await
                .unwrap(),
            [a.id]
        );
        assert_eq!(fake.asked().len(), 1, "sent where it is cleared");
    }

    #[test]
    fn a_candidate_is_asked_about_by_its_handle_unless_another_has_it() {
        let candidates =
            [agent("Coder", None), agent("coder", None), agent("Nobody", None), agent("!", None)];
        assert_eq!(options(&candidates), ["coder", "agent-2", "agent-3", "agent-4"]);
    }

    #[test]
    fn its_parameters_are_checked() {
        let checked = |params: Value| DecisionPolicyParams::read(&params).map(drop);
        assert!(checked(json!({})).is_ok(), "every one has a default");
        assert!(checked(json!({ "threshold": 1.5 })).is_err());
        assert!(checked(json!({ "max_responders": 0 })).is_err());
        assert!(checked(json!({ "sender_format": "xml", "sender_style": "name" })).is_ok());
        assert!(checked(json!({ "sender_format": "header" })).is_err());
        assert!(checked(json!({ "name": "nimble" })).is_ok());
        assert!(checked(json!({ "endpoint": { "url": "http://decide.example" } })).is_err());
        assert!(checked(json!({ "destination": { "level": "internal" } })).is_err());
        assert!(checked(json!({ "temperature": 0.1 })).is_err(), "nothing unknown");
    }

    /// Issue 551: a policy needs a decision model the platform offers, and is not built without.
    #[tokio::test]
    async fn without_a_decision_model_it_is_not_built() {
        let kernel =
            TestKernel { plugins: vec![&ModelPlugin, &DecisionPlugin], ..Default::default() }
                .build();
        let responders = descriptor("responders", DECISION_POLICY_KIND, json!({}));
        let refused = built::<dyn ResponderPolicy>(&kernel, responders).await.unwrap_err();
        assert!(refused.to_string().contains("no decision model"), "{refused}");
    }

    /// Issue 551: where what was said goes is the model's configuration, none of its params, so
    /// that a request writes it as any other policy.
    #[tokio::test]
    async fn a_request_writes_it_as_any_other_policy() {
        let mut registry = ComponentRegistry::built_in(
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .component_bundles,
        );
        register(&mut registry);
        let config = ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("responders", DECISION_POLICY_KIND)],
        };
        registry
            .require_acceptable(&config, ChannelLoop::COMPONENT_FACETS, &SecurityContext::default())
            .await
            .expect("nothing in it is the operator's");
    }
}
