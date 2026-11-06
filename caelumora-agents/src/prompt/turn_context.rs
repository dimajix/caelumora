//! The `turn_context` component: what the model of an agent run
//! here is given in a turn besides the conversation - the prompt parts of
//! every `PromptProvider` of the agent, placed around its transcript or
//! given into it (issues 47, 99, 537). The default `ContextProvider` of an
//! agent run here.

use crate::prompt::{prompt_for, ContextError, ContextProvider, PromptPosition, PromptProvider};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_api::component::ComponentImpl;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::actor::ActorId;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::{KnowledgeRef, Placement, PromptAt, PromptPart};
use caelumora_domain::message::MessageRole;
use std::sync::Arc;

pub const TURN_CONTEXT_KIND: &str = "turn_context";

/// The prompt parts of `prompt_providers` around the transcript: those at
/// `Start` joined into the system prompt before everything, those at
/// `BeforeTurnInput` joined into one user message before the newest message
/// of the turn, those at `End` joined into one user message after everything
/// each call is given, and those at `Given` each on its own, as a system
/// message of its component, into the transcript. The default `ContextProvider` (issue
/// 98).
#[derive(Debug)]
pub struct TurnContextProvider {
    pub prompt_providers: Vec<ComponentImpl<dyn PromptProvider>>,
}

#[async_trait]
impl ContextProvider for TurnContextProvider {
    async fn current_context(
        &self,
        situation: &Situation<'_>,
    ) -> Result<(Vec<PromptPart>, Vec<KnowledgeRef>), ContextError> {
        let mut start = Vec::new();
        let mut before_input = Vec::new();
        let mut end = Vec::new();
        let mut given = Vec::new();
        let mut knowledge = Vec::new();
        for provider in &self.prompt_providers {
            let Some(contribution) = prompt_for(provider, situation).await? else { continue };
            let text = contribution.text.trim();
            if text.is_empty() {
                continue;
            }
            knowledge.extend(contribution.knowledge);
            let part = (text.to_string(), contribution.classification);
            match provider.position() {
                PromptPosition::Start => start.push(part),
                PromptPosition::BeforeTurnInput => before_input.push(part),
                PromptPosition::End => end.push(part),
                // Framed as what is given to read (issue 539).
                PromptPosition::Given => given.push(PromptPart {
                    text: part.0,
                    placement: Placement::Given { component: provider.id.clone() },
                    classification: part.1,
                }),
            }
        }

        // The system prompt before everything, what goes before the input,
        // what is read last, then what is given into the transcript.
        let mut parts = Vec::with_capacity(3 + given.len());
        parts.extend(framed(start, MessageRole::System, PromptAt::Start));
        parts.extend(framed(before_input, MessageRole::User, before_the_input(situation)));
        parts.extend(framed(end, MessageRole::User, PromptAt::End));
        parts.extend(given);
        Ok((parts, knowledge))
    }
}

/// The prompt parts `parts` of one position joined into one, given in `role`
/// at `at` in the frame; `None` if there are none.
fn framed(
    parts: Vec<(String, Option<Classification>)>,
    role: MessageRole,
    at: PromptAt,
) -> Option<PromptPart> {
    if parts.is_empty() {
        return None;
    }
    let (text, classification) = joined(parts);
    Some(PromptPart {
        text,
        placement: Placement::Frame { at, role, component: None },
        classification,
    })
}

/// Where what goes before the turn's input stands in the frame: before the
/// newest message, which is the input - unless the agent wrote it itself,
/// in a turn that has none (e.g. after `/wakeup`): then at the end of the
/// transcript. A part given into the transcript is no input, nor the agent's
/// own words (issue 537): it is passed over in looking for the input, as a
/// turn answering anew finds one after it.
fn before_the_input(situation: &Situation<'_>) -> PromptAt {
    let transcript = &situation.agent_channel_state().transcript;
    let mut messages = transcript
        .entries()
        .rev()
        .filter_map(|n| n.entry.message().map(|m| (n.digest, n.entry.part_of(), m)))
        .skip_while(|(_, part, _)| part.is_some())
        .map(|(digest, _, message)| (digest, message));
    let own = ActorId::Agent(situation.agent_id());
    let after = match messages.next() {
        Some((_, input)) if input.sender_id != own => messages.next().map(|(digest, _)| digest),
        Some((own_message, _)) => Some(own_message),
        None => None,
    };
    PromptAt::after(after)
}

/// The prompt parts of one position joined into one: their texts, and the
/// join of their classifications - `None` if one of them is not known, or
/// they have no audience in common (issue 168).
fn joined(parts: Vec<(String, Option<Classification>)>) -> (String, Option<Classification>) {
    let (texts, classifications): (Vec<String>, Vec<Option<Classification>>) =
        parts.into_iter().unzip();
    let classification = classifications
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .and_then(|c| Classification::join_all(&c));
    (texts.join("\n\n"), classification)
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        ComponentKind::new(TURN_CONTEXT_KIND)
            .provides::<dyn ContextProvider>()
            .needs::<dyn PromptProvider>(),
        |_descriptor, context| async move {
            // Every prompt part of the agent, in the order of its
            // configuration: the parts of one position are joined in this
            // order, and a part that changes its place changes the prompt -
            // and its cache - for nothing.
            let prompt_providers = context.all::<dyn PromptProvider>().await?;

            let mut facets = ComponentFacets::empty();
            facets
                .insert::<dyn ContextProvider>(Arc::new(TurnContextProvider { prompt_providers }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::system_prompt::SystemPromptComponent;
    use crate::prompt::Prompt;
    use crate::turn::Turn;
    use anyhow::Result;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::component::ComponentFailPolicy;
    use caelumora_domain::confidentiality::Classification;
    use caelumora_domain::message::{Message, MessageId};
    use futures::executor;

    fn message(content: &str) -> Message {
        Message {
            id: MessageId::new(),
            role: MessageRole::User,
            content: Some(content.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        }
    }

    /// `provider` as a component with `fail_policy`.
    fn part(
        provider: Arc<dyn PromptProvider>,
        fail_policy: ComponentFailPolicy,
    ) -> ComponentImpl<dyn PromptProvider> {
        ComponentImpl { id: "part".to_string(), fail_policy, facet: provider }
    }

    /// What `provider` answers for `turn`, set in it as the agent's actor
    /// sets it.
    async fn assemble(provider: &TurnContextProvider, turn: &mut Turn<'_>) -> Result<()> {
        let (parts, knowledge) = provider.current_context(&turn.situation()).await?;
        turn.set_context(parts, knowledge);
        Ok(())
    }

    /// A prompt part that cannot be had.
    #[derive(Debug)]
    struct Unavailable;

    #[async_trait::async_trait]
    impl PromptProvider for Unavailable {
        async fn get_prompt(&self, _situation: &Situation<'_>) -> Result<Prompt, ContextError> {
            Err(ContextError::Failed("unavailable".to_string()))
        }
    }

    #[tokio::test]
    async fn a_failing_prompt_part_is_left_out_or_fails_the_turn_as_its_component_says() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::new(), &mut agent_channel_state);
        let brief = || {
            part(
                Arc::new(SystemPromptComponent {
                    prompt: "Be brief.".to_string(),
                    position: PromptPosition::Start,
                    classification: Classification::PUBLIC,
                }),
                ComponentFailPolicy::FailClosed,
            )
        };

        let open = TurnContextProvider {
            prompt_providers: vec![
                part(Arc::new(Unavailable), ComponentFailPolicy::FailOpen),
                brief(),
            ],
        };
        assemble(&open, &mut turn).await.expect("a part that fails open is left out");
        assert_eq!(
            turn.full_context().await.unwrap().0[0].content.as_deref(),
            Some("Be brief."),
            "the others are given"
        );

        let closed = TurnContextProvider {
            prompt_providers: vec![
                part(Arc::new(Unavailable), ComponentFailPolicy::FailClosed),
                brief(),
            ],
        };
        let error = assemble(&closed, &mut turn)
            .await
            .expect_err("a part that fails closed fails the turn");
        assert!(format!("{error:#}").contains("component 'part'"), "{error:#}");
    }

    #[tokio::test]
    async fn prompt_parts_go_where_their_position_says_and_only_the_conversation_is_stored() {
        let agent_id = AgentId::new();
        let mut agent_channel_state = AgentChannelState::default();
        let provider = TurnContextProvider {
            prompt_providers: vec![
                part(
                    Arc::new(SystemPromptComponent {
                        prompt: "Be brief.".to_string(),
                        position: PromptPosition::Start,
                        classification: Classification::PUBLIC,
                    }),
                    ComponentFailPolicy::FailClosed,
                ),
                part(
                    Arc::new(SystemPromptComponent {
                        prompt: "Notes: none.".to_string(),
                        position: PromptPosition::BeforeTurnInput,
                        classification: Classification::PUBLIC,
                    }),
                    ComponentFailPolicy::FailClosed,
                ),
            ],
        };
        let contents = |turn: &mut Turn| {
            executor::block_on(turn.full_context())
                .unwrap()
                .0
                .iter()
                .map(|m| format!("{:?}:{}", m.role, m.content.clone().unwrap_or_default()))
                .collect::<Vec<_>>()
        };

        let mut earlier = message("earlier");
        earlier.role = MessageRole::User;
        let mut question = message("question");
        question.role = MessageRole::User;
        agent_channel_state.perceive(vec![earlier, question]);
        let mut turn = Turn::begun(agent_id, &mut agent_channel_state);
        assemble(&provider, &mut turn).await.unwrap();
        assert_eq!(
            contents(&mut turn),
            ["System:Be brief.", "User:earlier", "User:Notes: none.", "User:question"],
            "the system prompt first, the notes before the turn's input"
        );
        assert_eq!(
            turn.situation().agent_channel_state().transcript.messages().len(),
            2,
            "prompt parts are never stored in the context"
        );

        // A turn without new input - the newest message is the agent's own.
        let mut own = message("my answer");
        own.sender_id = ActorId::Agent(agent_id);
        agent_channel_state.perceive(vec![own]);
        let mut turn = Turn::begun(agent_id, &mut agent_channel_state);
        assemble(&provider, &mut turn).await.unwrap();
        assert_eq!(
            contents(&mut turn).last().map(String::as_str),
            Some("User:Notes: none."),
            "then at the end"
        );
    }

    #[tokio::test]
    async fn a_part_given_into_the_transcript_goes_on_its_own_as_its_components() {
        let given = |id: &str, prompt: &str| ComponentImpl {
            id: id.to_string(),
            fail_policy: ComponentFailPolicy::FailClosed,
            facet: Arc::new(SystemPromptComponent {
                prompt: prompt.to_string(),
                position: PromptPosition::Given,
                classification: Classification::PUBLIC,
            }) as Arc<dyn PromptProvider>,
        };
        let provider = TurnContextProvider {
            prompt_providers: vec![given("rules", "Be brief."), given("empty", "  ")],
        };
        let mut agent_channel_state = AgentChannelState::default();
        let turn = Turn::begun(AgentId::new(), &mut agent_channel_state);
        let (parts, _) = provider.current_context(&turn.situation()).await.unwrap();
        assert_eq!(
            parts,
            [PromptPart {
                text: "Be brief.".to_string(),
                placement: Placement::Given { component: "rules".to_string() },
                classification: Some(Classification::PUBLIC),
            }],
            "not joined, but named by its component - and one that says nothing is none"
        );
    }

    /// Issue 537: the parts at the end are joined into one, given as a user message after
    /// everything each call is given.
    #[tokio::test]
    async fn the_parts_at_the_end_are_joined_and_placed_after_everything() {
        let at_end = |prompt: &str| ComponentImpl {
            id: "end".to_string(),
            fail_policy: ComponentFailPolicy::FailClosed,
            facet: Arc::new(SystemPromptComponent {
                prompt: prompt.to_string(),
                position: PromptPosition::End,
                classification: Classification::PUBLIC,
            }) as Arc<dyn PromptProvider>,
        };
        let provider = TurnContextProvider {
            prompt_providers: vec![at_end("[Time]\nnow"), at_end("[Why]\nasked")],
        };
        let mut agent_channel_state = AgentChannelState::default();
        agent_channel_state.transcript.push(message("hello"));
        let turn = Turn::begun(AgentId::new(), &mut agent_channel_state);
        let (parts, _) = provider.current_context(&turn.situation()).await.unwrap();
        let placed: Vec<_> = parts.iter().map(|p| p.placement.clone()).collect();
        assert_eq!(
            placed,
            [Placement::Frame { at: PromptAt::End, role: MessageRole::User, component: None }],
            "one part, at the end, whatever the transcript holds"
        );
        assert!(parts[0].text.contains("[Time]") && parts[0].text.contains("[Why]"), "joined");
    }

    /// Issue 537: a part given into the transcript after the input is no input, nor the agent's
    /// own - a turn that answers anew finds the input before it.
    #[tokio::test]
    async fn the_turns_input_is_found_before_the_parts_given_after_it() {
        let agent_id = AgentId::new();
        let mut agent_channel_state = AgentChannelState::default();
        let mut question = message("question");
        question.role = MessageRole::User;
        agent_channel_state.perceive(vec![question]);
        agent_channel_state.begin_turn(Default::default());
        let rules = Message {
            sender_id: ActorId::Agent(agent_id),
            role: MessageRole::System,
            classification: Classification::PUBLIC,
            ..message("[Rules]\nBe brief.")
        };
        agent_channel_state.give_parts(vec![("rules".to_string(), rules)]);
        agent_channel_state.turn = None;

        let provider = TurnContextProvider {
            prompt_providers: vec![
                part(
                    Arc::new(SystemPromptComponent {
                        prompt: "Notes: none.".to_string(),
                        position: PromptPosition::BeforeTurnInput,
                        classification: Classification::PUBLIC,
                    }),
                    ComponentFailPolicy::FailClosed,
                ),
                ComponentImpl {
                    id: "rules".to_string(),
                    fail_policy: ComponentFailPolicy::FailClosed,
                    facet: Arc::new(SystemPromptComponent {
                        prompt: "[Rules]\nBe brief.".to_string(),
                        position: PromptPosition::Given,
                        classification: Classification::PUBLIC,
                    }),
                },
            ],
        };
        let before = agent_channel_state.transcript.end();
        let mut turn = Turn::begun(agent_id, &mut agent_channel_state);
        assemble(&provider, &mut turn).await.unwrap();
        let contents: Vec<_> = turn
            .full_context()
            .await
            .unwrap()
            .0
            .iter()
            .map(|m| m.content.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            contents,
            ["Notes: none.", "question", "[Rules]\nBe brief."],
            "the notes before the input, not after the part"
        );
        drop(turn);
        assert_eq!(agent_channel_state.transcript.end(), before, "the part stands as it was");
    }
}
