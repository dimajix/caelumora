//! What the tests of the tool share: the texts it is given.

use crate::{NAMESPACE, PROMPTS};
use caelumora_agents::testing::prompts_with;
use caelumora_domain::prompt_catalogue::PromptTexts;

/// The texts of the kernel, of the agents and of this tool, as the kernel and the libraries add
/// them - for what a test makes itself.
pub(super) fn prompts() -> PromptTexts {
    prompts_with(NAMESPACE, PROMPTS)
}
