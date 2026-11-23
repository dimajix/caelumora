//! What the program running the agents tells them of itself (issue 577), as a library
//! (`ProgramLibrary`): the kind `program_log`, which tells an agent where the program writes its
//! log and what the log's lines about its session and its channel carry.
//!
//! Where the log lies is the program's to know - its configuration's `logging.file`, or its own
//! default -, so the program builds the library with it when it lists it; a program that writes no
//! log to a file lists none. The log holds every session, user and channel of the program, so the
//! part comes only where a personality wires the kind, and says so. It never changes: in the system
//! prompt by default (parameter `position`). It names the ids of the session and the channel, and
//! is classified as the channel.

use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use crate::situation::Situation;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_api::library::{Library, LibraryBuilder};
use caelumora_domain::prompt_catalogue::{context, PromptTexts};
use std::path::PathBuf;
use std::sync::Arc;

pub const PROGRAM_LOG_KIND: &str = "program_log";

/// The name of the library, and the namespace of its texts in the prompt catalogue.
const NAMESPACE: &str = "program";

/// Its texts, added to the catalogue under `NAMESPACE`.
const PROMPTS: &str = include_str!("program_prompts.yaml");

/// The text of `program_log` in the prompt catalogue.
const LOG_TEXT: &str = "program.program_log";

/// What the program running the agents tells them of itself, as a library: the kind `program_log`.
#[derive(Debug)]
pub struct ProgramLibrary {
    log: PathBuf,
}

impl ProgramLibrary {
    /// The library of a program that writes its log to the file `log`.
    pub fn new(log: PathBuf) -> Self {
        Self { log }
    }
}

impl Library for ProgramLibrary {
    fn name(&self) -> &'static str {
        NAMESPACE
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        library.add_prompts(NAMESPACE, PROMPTS)?;
        let log = self.log.clone();
        library.components().register_async(
            writing_for_the_model(
                ComponentKind::new(PROGRAM_LOG_KIND)
                    .params::<PositionParams>()
                    .provides::<dyn PromptProvider>(),
            ),
            move |descriptor, context| {
                let log = log.clone();
                async move {
                    let prompts = texts_of(&context).await?;
                    let position =
                        PositionParams::position_of(&descriptor.params, PromptPosition::Start)?;
                    let provider = ProgramLogComponent { log, position, prompts };
                    let mut facets = ComponentFacets::empty();
                    facets.insert::<dyn PromptProvider>(Arc::new(provider));
                    Ok(facets)
                }
            },
        );
        Ok(())
    }
}

/// The part telling where the program writes its log, at `position`.
#[derive(Debug)]
struct ProgramLogComponent {
    log: PathBuf,
    position: PromptPosition,
    prompts: PromptTexts,
}

#[async_trait]
impl PromptProvider for ProgramLogComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let text = self.prompts.render(
            LOG_TEXT,
            context! {
                log => self.log.display().to_string(),
                session_id => situation.session_id().to_string(),
                channel_id => situation.channel_id().to_string(),
            },
        )?;
        // It names the channel's id: classified as the channel.
        Ok(text.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::AgentsLibrary;
    use crate::turn::Turn;
    use caelumora_domain::actor::{ActorId, EventContext};
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::channel::ThreadId;
    use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_domain::session::SessionId;
    use caelumora_domain::type_map::TypeKey;
    use caelumora_domain::work::Work;
    use caelumora_kernel::component::BuildContext;
    use caelumora_kernel::testing::TestKernel;
    use caelumora_security::SecurityContext;

    /// The part a personality wires of the kernel with the library names the file the library was
    /// built with, says that it holds more than the agent's session, and gives the ids of the
    /// session and the channel to look for, classified as the channel.
    #[tokio::test]
    async fn the_log_the_program_writes_to_is_named_with_the_ids_to_look_for() {
        let program = ProgramLibrary::new(PathBuf::from("/var/log/caelumora/tui.log"));
        let kernel =
            TestKernel { libraries: vec![&AgentsLibrary, &program], ..Default::default() }.build();
        let wired = ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("log", PROGRAM_LOG_KIND)],
        };
        let context = BuildContext::new(SecurityContext::default(), kernel.services().clone());
        let container = kernel
            .component_registry()
            .build(&wired, &[TypeKey::of::<dyn PromptProvider>()], &context)
            .await
            .unwrap();
        let [part] = container.get_all::<dyn PromptProvider>(None).unwrap().try_into().unwrap();

        let (session, channel) = (SessionId::new(), ChannelId::new());
        let mut state = AgentChannelState::default();
        state.begin_turn(
            ThreadId::default(),
            Work::new(
                EventContext::new(session, channel, ActorId::None, String::new()),
                None,
                Default::default(),
                Default::default(),
            ),
        );
        let turn = Turn::under_way(AgentId::new(), &mut state);
        let prompt = part.get_prompt(&turn.situation()).await.unwrap();
        assert!(
            prompt.text.contains("/var/log/caelumora/tui.log")
                && prompt.text.contains("every session, user and channel")
                && prompt.text.contains(&session.to_string())
                && prompt.text.contains(&channel.to_string()),
            "the log, what it holds and the ids: {}",
            prompt.text
        );
        assert_eq!(prompt.classification, None, "as the channel");
        assert_eq!(part.position(), PromptPosition::Start, "in the system prompt by default");
    }
}
