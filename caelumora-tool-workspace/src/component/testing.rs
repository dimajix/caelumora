//! What the tests of the component and of its facets share: the component, built as `register`
//! builds it.

use super::{WorkspaceComponent, WorkspaceParams};
use crate::prompts;
use caelumora_api::component::Params;
use serde_json::Value;

/// The component with `params`, read as `register` reads them.
pub(super) fn component(params: Value) -> WorkspaceComponent {
    WorkspaceComponent::new(WorkspaceParams::read(&params).unwrap(), prompts()).unwrap()
}
