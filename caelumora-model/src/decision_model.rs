//! The kind `decision_model` (issue 551): one of the decision models the platform offers, as a
//! component is given it - its client, which is built for whoever it is a component of alone.
//!
//! Parameters: `name`, the model of `plugins.model.decision` - left out, the one there is. An
//! unknown parameter is refused, so that a misspelt one does not silently fall back to its default.

use crate::manager::ModelManager;
use caelumora_api::component::{ComponentFacets, ComponentKind, Params};
use caelumora_api::decision::DecisionModelClient;
use caelumora_kernel::component::ComponentRegistry;
use caelumora_kernel::kernel::Acting;
use serde::Deserialize;

/// The kind of a decision model of an agent.
const DECISION_MODEL_KIND: &str = "decision_model";

/// The parameters of a `decision_model` component.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionModelParams {
    #[serde(default)]
    name: Option<String>,
}

impl Params for DecisionModelParams {}

/// Registers the kind `decision_model` with `registry`, which provides a `DecisionModelClient`.
pub(crate) fn register_all(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(DECISION_MODEL_KIND)
            .params::<DecisionModelParams>()
            .provides::<dyn DecisionModelClient>()
            .uses::<ModelManager>()
            .uses::<Acting>(),
        |descriptor, context| {
            let params = DecisionModelParams::read(&descriptor.params)?;
            // For whoever its components are built for, in their name at each call.
            let acting = Acting::clone(context.service::<Acting>()?);
            let models = context.service::<ModelManager>()?;
            let client = models.decision_client_for(acting, params.name.as_deref())?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn DecisionModelClient>(client);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::DECISION;
    use crate::testing::{configuring, kernel_of, ASKED_FOR, JUDGING};
    use caelumora_api::component::ComponentResult;
    use caelumora_api::decision::{Answer, Question};
    use caelumora_api::model::ModelError;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_domain::confidentiality::{Classification, Confidentiality};
    use caelumora_domain::type_map::TypeKey;
    use caelumora_kernel::component::BuildContext;
    use caelumora_kernel::kernel::Kernel;
    use caelumora_security::SecurityContext;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// The kernel of a test that offers the decision models `models`.
    fn offering(models: Value) -> Kernel {
        kernel_of(configuring(json!({ DECISION: models }))).unwrap()
    }

    /// The client of the component of the kind `decision_model` built with `params`.
    async fn client_of(
        kernel: &Kernel,
        params: Value,
    ) -> ComponentResult<Arc<dyn DecisionModelClient>> {
        client_built_as(kernel, params, SecurityContext::default()).await
    }

    /// The client of the component built with `params` in the name of `built_as`.
    async fn client_built_as(
        kernel: &Kernel,
        params: Value,
        built_as: SecurityContext,
    ) -> ComponentResult<Arc<dyn DecisionModelClient>> {
        let config = ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                params,
                ..ComponentDescriptor::new("decider", DECISION_MODEL_KIND)
            }],
        };
        let facets = [TypeKey::of::<dyn DecisionModelClient>()];
        let context = BuildContext::new(built_as, kernel.services().clone());
        let container =
            Arc::clone(kernel.component_registry()).build(&config, &facets, &context).await?;
        Ok(Arc::clone(&container.get_all::<dyn DecisionModelClient>(None)?[0].facet))
    }

    fn question() -> BTreeMap<String, Question> {
        let instructions = "Is it so?".to_string();
        BTreeMap::from([("fits".to_string(), Question::Noul { instructions })])
    }

    fn internal() -> Classification {
        Classification::general(Confidentiality::Internal)
    }

    /// Issue 551: a component's decision model is the platform's model it names, by that name,
    /// and what it answers is the model's.
    #[tokio::test]
    async fn a_decision_model_is_the_platform_s_model_its_component_names() {
        let kernel = offering(json!({
            "sure": { "type": JUDGING, "yes": 0.9 },
            "unsure": { "type": JUDGING, "yes": 0.2 },
        }));
        let client = client_of(&kernel, json!({ "name": "unsure" })).await.unwrap();

        let answers = client
            .decide(ActorId::System, &[Classification::PUBLIC], "It is so.", &question())
            .await
            .unwrap();

        assert_eq!(client.name(), "unsure");
        assert_eq!(answers["fits"], Answer::Noul { noul: 0.2 });
    }

    /// Issue 551: the server's credential is resolved for whom the client was built for - a
    /// channel's owner, for a channel's policy -, so that is the context the model is asked in.
    #[tokio::test]
    async fn a_model_is_asked_in_the_name_the_client_was_built_in() {
        let kernel = offering(json!({ "sure": { "type": JUDGING, "yes": 0.9 } }));
        let built_as = SecurityContext::system();
        let client = client_built_as(&kernel, json!({}), built_as.clone()).await.unwrap();

        let answers = client
            .decide(ActorId::None, &[Classification::PUBLIC], "It is so.", &question())
            .await
            .unwrap();

        let Answer::Choice { choice, .. } = &answers[ASKED_FOR] else { panic!("{answers:?}") };
        assert_eq!(choice, &format!("{:?}", built_as.principal_id));
    }

    #[tokio::test]
    async fn a_model_is_named_among_several_and_none_is_there_without_a_configuration() {
        let kernel = offering(json!({
            "sure": { "type": JUDGING, "yes": 0.9 },
            "unsure": { "type": JUDGING, "yes": 0.2 },
        }));
        let unnamed = client_of(&kernel, json!({})).await.unwrap_err();
        assert!(unnamed.to_string().contains("'name'"), "{unnamed}");

        let none = kernel_of(configuring(json!({}))).unwrap();
        let refused = client_of(&none, json!({})).await.unwrap_err();
        assert!(refused.to_string().contains("no decision model"), "{refused}");
        assert!(DecisionModelParams::read(&json!({ "model": "x" })).is_err(), "nothing unknown");
    }

    /// Issue 551: what the model may not receive is said to be so, and is not sent to it. Every
    /// classification counts, whichever of them it is.
    #[tokio::test]
    async fn what_may_not_reach_the_model_s_destination_is_not_sent() {
        let kernel = offering(json!({
            "outside": { "type": JUDGING, "yes": 0.9 },
            "inside": { "type": JUDGING, "yes": 0.9, "destination": { "level": "internal" } },
        }));
        let outside = client_of(&kernel, json!({ "name": "outside" })).await.unwrap();
        let inside = client_of(&kernel, json!({ "name": "inside" })).await.unwrap();

        assert!(!outside.may_receive(ActorId::System, &internal()));
        assert!(inside.may_receive(ActorId::System, &internal()));
        let held = [Classification::PUBLIC, internal()];
        let refused =
            outside.decide(ActorId::System, &held, "Internal matters.", &question()).await;
        assert!(matches!(refused, Err(ModelError::Unfit { model, .. }) if model == "outside"));
        assert!(inside
            .decide(ActorId::System, &held, "Internal matters.", &question())
            .await
            .is_ok());
    }
}
