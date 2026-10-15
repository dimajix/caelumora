//! The facets of an actor's message stages: components that see the
//! messages an actor perceives (`PerceiveInProcessor`) or an agent emits
//! (`EmitOutProcessor`), and may drop, change or add messages (issue 196).
//! A processor records nothing: an actor keeps all it was offered in its
//! history, and what passes its processors - what it processes - in its
//! transcript.

use caelumora_runtime::component::ComponentImpl;
use caelumora_runtime::view::ViewError;
use anyhow::{Context, Result};
use async_trait::async_trait;
use caelumora_model::component::ComponentFailPolicy;
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;
use caelumora_model::store::StoreError;
use std::fmt::Debug;

#[async_trait]
pub trait MessageProcessor: Send + Sync + Debug {
    /// What is made of `input` - filtered, changed, added to - for the actor
    /// whose context is `security`: who perceives or emits it, and where.
    ///
    /// # Errors
    /// `ProcessingError` if the messages cannot be processed.
    async fn process_messages(&self, input: Vec<Message>, security: &SecurityContext) -> Result<Vec<Message>, ProcessingError>;
}

/// Why a `MessageProcessor` cannot process what it was given (issue 336).
/// A plugin's processor translates its own errors into it, deciding there
/// what may be shown; `crate::error::user_message` judges each variant.
#[derive(Debug, thiserror::Error)]
pub enum ProcessingError {
    /// A failure in words that may be shown to users.
    #[error("{0}")]
    Failed(String),
    /// What reading through the views reported, judged as view errors are.
    #[error("{0}")]
    View(#[from] ViewError),
    /// What the store reported, judged as store errors are.
    #[error("{0}")]
    Store(#[from] StoreError),
}

pub trait PerceiveInProcessor: MessageProcessor {}
pub trait EmitOutProcessor: MessageProcessor {}

/// Passes `input` through `processors` in turn, each given what the one
/// before returned, for the actor whose context is `security`. A failing
/// processor is skipped if it fails open - the next one is given what it
/// was given - and fails the whole stage if it fails closed.
pub(crate) async fn apply_stage_processors<T>(
    processors: &[ComponentImpl<T>],
    mut input: Vec<Message>,
    security: &SecurityContext,
) -> Result<Vec<Message>>
where
    T: MessageProcessor + ?Sized + Send + Sync,
{
    for stage_processor in processors {
        // What it was given, should it be left out.
        let kept = matches!(stage_processor.fail_policy, ComponentFailPolicy::FailOpen).then(|| input.clone());
        let output = stage_processor.process_messages(input, security).await;
        let output = stage_processor.apply_fail_policy(output).with_context(|| format!("component '{}' failed", stage_processor.id))?;
        input = match output {
            Some(output) => output,
            None => kept.unwrap_or_default(),
        };
    }
    Ok(input)
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::history::HistoryEntry;
    use caelumora_model::turn::GivenUpReason;
    use caelumora_model::work::TurnId;

    /// The turns the agent of `state` gave up, as its history records them
    /// (issue 258): each with why, and the messages before its end that
    /// carry its id.
    pub(crate) fn given_up_turns(state: &AgentChannelState) -> Vec<(TurnId, GivenUpReason, Vec<Message>)> {
        let items = state.history.items();
        items.iter().enumerate()
            .filter_map(|(at, item)| {
                let HistoryEntry::TurnGivenUp { turn_id, reason, .. } = item else { return None };
                let messages = items[..at].iter()
                    .filter_map(|item| match item {
                        HistoryEntry::Said { message, .. } if message.turn_id == Some(*turn_id) => Some(message.clone()),
                        _ => None,
                    })
                    .collect();
                Some((*turn_id, reason.clone(), messages))
            })
            .collect()
    }

    /// Keeps what does not contain `word`, or fails.
    #[derive(Debug)]
    pub(crate) struct Without(pub(crate) Option<&'static str>);

    #[async_trait]
    impl MessageProcessor for Without {
        async fn process_messages(&self, input: Vec<Message>, _security: &SecurityContext) -> Result<Vec<Message>, ProcessingError> {
            let Some(word) = self.0 else { return Err(ProcessingError::Failed("broken".to_string())) };
            Ok(input.into_iter().filter(|m| !m.content.as_deref().unwrap_or_default().contains(word)).collect())
        }
    }

    impl PerceiveInProcessor for Without {}
}

#[cfg(test)]
mod tests {
    use super::testing::Without;
    use super::*;
    use caelumora_model::confidentiality::Classification;
    use std::sync::Arc;
    use caelumora_model::message::MessageRole;

    fn stage(id: &str, processor: Without, fail_policy: ComponentFailPolicy) -> ComponentImpl<dyn MessageProcessor> {
        ComponentImpl { id: id.to_string(), fail_policy, facet: Arc::new(processor) }
    }

    fn said(text: &str) -> Message {
        Message { role: MessageRole::User, content: Some(text.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }
    }

    #[tokio::test]
    async fn each_processor_filters_what_the_one_before_left_and_a_broken_one_that_fails_open_is_skipped() {
        let security = SecurityContext::default();
        let input = vec![said("hello"), said("secret plan"), said("spam offer")];
        let stages = [
            stage("no-secrets", Without(Some("secret")), ComponentFailPolicy::FailClosed),
            stage("broken", Without(None), ComponentFailPolicy::FailOpen),
            stage("no-spam", Without(Some("spam")), ComponentFailPolicy::FailClosed),
        ];
        let output = apply_stage_processors(&stages, input.clone(), &security).await.expect("the broken one fails open");
        assert_eq!(output.iter().filter_map(|m| m.content.as_deref()).collect::<Vec<_>>(), vec!["hello"], "the next is given what the broken one was given");

        let stages = [stage("broken", Without(None), ComponentFailPolicy::FailClosed)];
        assert!(apply_stage_processors(&stages, input, &security).await.is_err(), "failing closed fails the stage");
    }
}
