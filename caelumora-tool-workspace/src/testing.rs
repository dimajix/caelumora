//! What the tests of the tools share: the texts they are given.

use crate::{NAMESPACE, PROMPTS};
use caelumora_agents::testing::catalogue_with;
use caelumora_domain::prompt_catalogue::{PromptCatalogue, PromptTexts};
use std::sync::Arc;

/// The texts of the kernel, of the agents and of these tools, as the kernel and the libraries add
/// them - for what a test makes itself.
pub(super) fn prompts() -> PromptTexts {
    PromptTexts::new(catalogue(), None)
}

/// The prompt catalogue with the texts of the kernel, of the agents and of these tools, as the
/// kernel and the libraries add them.
pub(super) fn catalogue() -> Arc<PromptCatalogue> {
    catalogue_with(NAMESPACE, PROMPTS)
}
