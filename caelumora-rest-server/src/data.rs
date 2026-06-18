use std::sync::Arc;
use uuid::Uuid;
use caelumora_model::store::{EntityStore};
use caelumora_model::agent::{Personality, PersonalityId};
use caelumora_model::meta::{Metadata, Scope};
use caelumora_model::template::{
    ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId,
};
use caelumora_model::user::{User, UserId};
use caelumora_model::component::{ComponentConfig, ComponentNodeConfig, STAGE_PERCEIVE, ComponentFailPolicy};
use caelumora_runtime::component::{
    STATE_CONTEXT_APPENDER_KIND, STATE_CONTEXT_PROVIDER_KIND, STATE_HISTORY_WRITER_KIND,
};

use caelumora_model::security::SecurityContext;

pub async fn setup_initial_data(store: &Arc<dyn EntityStore>, username: &str) -> anyhow::Result<()> {
    let ctx = SecurityContext::default();

    // 1. Ensure user exists
    if store.find_user_by_name(&ctx, username).await?.is_none() {
        let user = User {
            id: UserId(Uuid::new_v4()),
            username: username.to_string(),
            email: format!("{}@example.com", username),
            roles: vec!["user".to_string()],
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
        store.put_session_template(&ctx, &template_a).await?;
    }

    if store.find_session_template_by_name(&ctx, "sassy-session", &Scope::GlobalScope).await.is_err() {
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
        store.put_session_template(&ctx, &template_b).await?;
    }

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
