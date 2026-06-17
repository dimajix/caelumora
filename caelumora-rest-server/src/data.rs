use std::sync::Arc;
use uuid::Uuid;
use caelumora_model::driver::memory::MemoryStore;
use caelumora_model::agent::{Personality, PersonalityId};
use caelumora_model::meta::{Metadata, Scope};
use caelumora_model::template::{
    ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId,
};
use caelumora_model::store::TemplateStore;
use caelumora_model::user::UserId;
use caelumora_model::component::{ComponentConfig, ComponentNodeConfig, STAGE_PERCEIVE, ComponentFailPolicy};
use caelumora_runtime::component::{
    STATE_CONTEXT_APPENDER_KIND, STATE_CONTEXT_PROVIDER_KIND, STATE_HISTORY_WRITER_KIND,
};

pub fn setup_initial_data(store: &Arc<MemoryStore>) -> anyhow::Result<()> {
    let jonathan = create_personality(
        "jonathan-assistant",
        "A helpful and polite assistant",
        "Your name is Jonathan Heart. You are a self-made millionaire. You always wanted to be a quiz-master.",
    );
    store.put_personality(&jonathan)?;

    let sassy = create_personality(
        "sassy-assistant",
        "A sassy and witty assistant",
        "Your name is Sassy Sarah. You are a witty and slightly sarcastic assistant who loves to play quiz games.",
    );
    store.put_personality(&sassy)?;

    let template_a = SessionTemplate {
        id: SessionTemplateId(Uuid::new_v4()),
        metadata: Metadata {
            name: "simple-session".to_string(),
            scope: Scope::GlobalScope,
            owner: UserId::default(),
            description: "Simple user+assistant session".to_string(),
        },
        scope: ScopeTemplate::SessionScope,
        channels: vec![ChannelTemplate {
            name: "main".to_string(),
            moderator: "round_robin".to_string(),
            participants: vec![
                ParticipantTemplate::User,
                ParticipantTemplate::Agent {
                    name: "assistant".to_string(),
                    scope: ScopeTemplate::SessionScope,
                    personality_id: jonathan.id.clone(),
                }
            ],
        }],
    };
    store.put_session_template(&template_a)?;

    let template_b = SessionTemplate {
        id: SessionTemplateId(Uuid::new_v4()),
        metadata: Metadata {
            name: "sassy-session".to_string(),
            scope: Scope::GlobalScope,
            owner: UserId::default(),
            description: "Sassy user+assistant session".to_string(),
        },
        scope: ScopeTemplate::SessionScope,
        channels: vec![ChannelTemplate {
            name: "main".to_string(),
            moderator: "round_robin".to_string(),
            participants: vec![
                ParticipantTemplate::User,
                ParticipantTemplate::Agent {
                    name: "Jonathan".to_string(),
                    scope: ScopeTemplate::SessionScope,
                    personality_id: jonathan.id.clone(),
                },
                ParticipantTemplate::Agent {
                    name: "Sassy".to_string(),
                    scope: ScopeTemplate::SessionScope,
                    personality_id: sassy.id.clone(),
                },
            ],
        }],
    };
    store.put_session_template(&template_b)?;

    Ok(())
}

fn create_personality(name: &str, description: &str, _system_prompt: &str) -> Personality {
    let mut components = ComponentConfig::default();
    components.component_nodes.push(ComponentNodeConfig {
        id: "history".to_string(),
        kind: STATE_HISTORY_WRITER_KIND.to_string(),
        params: serde_json::Value::Null,
        deps: vec![],
        tags: vec![STAGE_PERCEIVE.to_string()],
        fail_policy: ComponentFailPolicy::FailClosed,
    });
    components.component_nodes.push(ComponentNodeConfig {
        id: "context".to_string(),
        kind: STATE_CONTEXT_PROVIDER_KIND.to_string(),
        params: serde_json::Value::Null,
        deps: vec![],
        tags: vec![STAGE_PERCEIVE.to_string()],
        fail_policy: ComponentFailPolicy::FailClosed,
    });
    components.component_nodes.push(ComponentNodeConfig {
        id: "appender".to_string(),
        kind: STATE_CONTEXT_APPENDER_KIND.to_string(),
        params: serde_json::Value::Null,
        deps: vec![],
        tags: vec![STAGE_PERCEIVE.to_string()],
        fail_policy: ComponentFailPolicy::FailClosed,
    });

    Personality {
        id: PersonalityId(Uuid::new_v4()),
        metadata: Metadata {
            name: name.to_string(),
            scope: Scope::GlobalScope,
            owner: UserId::default(),
            description: description.to_string(),
        },
        components,
    }
}
