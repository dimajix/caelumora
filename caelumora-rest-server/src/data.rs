use caelumora_model::agent::{Personality, PersonalityId};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy, STAGE_PERCEIVE};
use caelumora_model::meta::{Metadata, Scope};
use caelumora_model::store::EntityStore;
use caelumora_model::template::{
    ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId,
};
use caelumora_model::user::{User, UserId};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use caelumora_model::security::SecurityContext;
use caelumora_runtime::component_impl::context::{STATE_CONTEXT_APPENDER_KIND, STATE_CONTEXT_PROVIDER_KIND, STATIC_SYSTEM_PROMPT_PROVIDER_KIND};
use caelumora_runtime::component_impl::history::STATE_HISTORY_WRITER_KIND;
use caelumora_runtime::component_impl::notes::NOTES_KIND;
use caelumora_runtime::component_impl::tool::{CALCULATOR_TOOL_PROVIDER_KIND, CREATE_PLAN_TOOL_PROVIDER_KIND, STOP_TOOL_PROVIDER_KIND};

pub async fn setup_initial_data(store: &Arc<dyn EntityStore>, username: &str) -> anyhow::Result<()> {
    // The initial data belongs to the system: every user may use it, nobody
    // but the system may change it (issue 11).
    let ctx = SecurityContext::system();

    // 1. Ensure user exists
    if store.find_user_by_name(&ctx, username).await.is_err() {
        let user = User {
                id: UserId(Uuid::new_v4()),
                username: username.to_string(),
                email: format!("{}@example.com", username),
                roles: vec!["user".to_string()],
                security: SecurityContext::default(),
            };
        store.create_user(&ctx, &user).await?;
    }

    // 2. Ensure personalities exist
    let jonathan = if let Ok(p) = store.find_personality_by_name(&ctx, "jonathan-assistant", &Scope::GlobalScope).await {
        p
    } else {
        let p = create_personality(
            "jonathan-assistant",
            "A helpful and polite assistant",
            "Your name is Jonathan Heart. You are a self-made millionaire. You always wanted to be a quiz-master.",
        );
        store.put_personality(&ctx, &p).await?;
        p
    };

    let sassy = if let Ok(p) = store.find_personality_by_name(&ctx, "sassy-assistant", &Scope::GlobalScope).await {
        p
    } else {
        let p = create_personality(
            "sassy-assistant",
            "A sassy and witty assistant",
            "Your name is Sassy Sarah. You are a witty and slightly sarcastic assistant who loves to play quiz games.",
        );
        store.put_personality(&ctx, &p).await?;
        p
    };

    // 3. Ensure session templates exist
    if store.find_session_template_by_name(&ctx, "simple-session", &Scope::GlobalScope).await.is_err() {
        let template_a = SessionTemplate {
            id: SessionTemplateId(Uuid::new_v4()),
            metadata: Metadata {
                name: "simple-session".to_string(),
                scope: Scope::GlobalScope,
                description: "Simple user+assistant session".to_string(),
            },
            scope: ScopeTemplate::SessionScope,
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
                ParticipantTemplate::Agent {
                    name: "assistant".to_string(),
                    scope: ScopeTemplate::SessionScope,
                    personality_id: jonathan.id.clone(),
                }
            ],
            channels: vec![ChannelTemplate {
                name: "main".to_string(),
                moderator: "round_robin".to_string(),
                participants: vec![
                    "user".to_string(),
                    "assistant".to_string(),
                ],
            }],
            commands: Default::default(),
            security_attributes: Default::default(),
        };
        store.put_session_template(&ctx, &template_a).await?;
    }

    if store.find_session_template_by_name(&ctx, "sassy-session", &Scope::GlobalScope).await.is_err() {
        let template_b = SessionTemplate {
            id: SessionTemplateId(Uuid::new_v4()),
            metadata: Metadata {
                name: "sassy-session".to_string(),
                scope: Scope::GlobalScope,
                description: "Sassy user+assistant session".to_string(),
            },
            scope: ScopeTemplate::SessionScope,
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
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
            channels: vec![ChannelTemplate {
                name: "main".to_string(),
                moderator: "round_robin".to_string(),
                participants: vec![
                    "user".to_string(),
                    "Jonathan".to_string(),
                    "Sassy".to_string(),
                ],
            }],
            commands: Default::default(),
            security_attributes: Default::default(),
        };
        store.put_session_template(&ctx, &template_b).await?;
    }

    // Agents answer only when a user @-mentions them (issue 44).
    if store.find_session_template_by_name(&ctx, "mentions-session", &Scope::GlobalScope).await.is_err() {
        let template_c = SessionTemplate {
            id: SessionTemplateId(Uuid::new_v4()),
            metadata: Metadata {
                name: "mentions-session".to_string(),
                scope: Scope::GlobalScope,
                description: "Two assistants that answer only when @-mentioned".to_string(),
            },
            scope: ScopeTemplate::SessionScope,
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
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
            channels: vec![ChannelTemplate {
                name: "main".to_string(),
                moderator: "mentions_only".to_string(),
                participants: vec![
                    "user".to_string(),
                    "Jonathan".to_string(),
                    "Sassy".to_string(),
                ],
            }],
            commands: Default::default(),
            security_attributes: Default::default(),
        };
        store.put_session_template(&ctx, &template_c).await?;
    }

    Ok(())
}

fn create_personality(name: &str, description: &str, system_prompt: &str) -> Personality {
    let mut components = ComponentConfig::default();
    components.component_nodes.push(ComponentDescriptor {
        id: "history-appender".to_string(),
        kind: STATE_HISTORY_WRITER_KIND.to_string(),
        params: serde_json::Value::Null,
        deps: vec![],
        tags: vec![STAGE_PERCEIVE.to_string()],
        fail_policy: ComponentFailPolicy::FailClosed,
        security_attributes: Default::default(),
    });
    components.component_nodes.push(ComponentDescriptor {
        id: "context".to_string(),
        kind: STATE_CONTEXT_PROVIDER_KIND.to_string(),
        params: serde_json::Value::Null,
        deps: vec!["system-prompt".to_string(), "notes".to_string()],
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
        security_attributes: Default::default(),
    });
    components.component_nodes.push(ComponentDescriptor {
        id: "context-appender".to_string(),
        kind: STATE_CONTEXT_APPENDER_KIND.to_string(),
        params: serde_json::Value::Null,
        deps: vec![],
        tags: vec![STAGE_PERCEIVE.to_string()],
        fail_policy: ComponentFailPolicy::FailClosed,
        security_attributes: Default::default(),
    });
    components.component_nodes.push(ComponentDescriptor {
        id: "system-prompt".to_string(),
        kind: STATIC_SYSTEM_PROMPT_PROVIDER_KIND.to_string(),
        params: json!({ "prompt": system_prompt }),
        deps: vec![],
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
        security_attributes: Default::default(),
    });
    components.component_nodes.push(ComponentDescriptor {
        id: "create-plan-tool".to_string(),
        kind: CREATE_PLAN_TOOL_PROVIDER_KIND.to_string(),
        params: serde_json::Value::Null,
        deps: vec![],
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
        security_attributes: Default::default(),
    });
    components.component_nodes.push(ComponentDescriptor {
        id: "calculator-tool".to_string(),
        kind: CALCULATOR_TOOL_PROVIDER_KIND.to_string(),
        params: serde_json::Value::Null,
        deps: vec![],
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
        security_attributes: Default::default(),
    });
    components.component_nodes.push(ComponentDescriptor {
        id: "notes".to_string(),
        kind: NOTES_KIND.to_string(),
        params: serde_json::Value::Null,
        deps: vec![],
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
        security_attributes: Default::default(),
    });
    components.component_nodes.push(ComponentDescriptor {
        id: "stop-tool".to_string(),
        kind: STOP_TOOL_PROVIDER_KIND.to_string(),
        params: serde_json::Value::Null,
        deps: vec![],
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
        security_attributes: Default::default(),
    });

    Personality {
        id: PersonalityId(Uuid::new_v4()),
        metadata: Metadata {
            name: name.to_string(),
            scope: Scope::GlobalScope,
            description: description.to_string(),
        },
        components,
        security_attributes: Default::default(),
    }
}
