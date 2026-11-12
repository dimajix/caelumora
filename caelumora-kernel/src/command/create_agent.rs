//! The built-in `/create-agent` command (issue 555).

use crate::agent::{AgentManager, AgentSpec};
use crate::command::{command_error, ChannelCommandProvider, CommandContext};
use crate::component::ComponentRegistry;
use crate::view::Viewer;
use anyhow::Error;
use async_trait::async_trait;
use caelumora_api::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_api::reader::ViewError;
use caelumora_domain::command::{Command, CommandOutput, CREATE_AGENT_KIND};
use caelumora_domain::scope::Scope;
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;
use std::sync::Arc;

pub const CREATE_AGENT_COMMAND_NAME: &str = "create-agent";

/// `/create-agent <personality> [as <name>]`: makes an agent of the personality named, as the
/// issuer finds it from the channel (`Views::personality_named`), in the session's scope - or in
/// the channel's, for a personality of the channel's scope, which makes agents only there
/// (`Scope::admits`) -, named after the personality unless `as` names it; the last `as` separates
/// the two, so that a personality's name may hold one (`AgentManager::create`). It does not bring
/// the agent in: `/add` does. Only the issuer is answered: nothing changed in the channel.
#[derive(Debug)]
pub struct CreateAgentComponent {
    agents: Arc<AgentManager>,
}

impl ProvidesCommands for CreateAgentComponent {
    fn commands(&self) -> Vec<Command> {
        vec![Command {
            name: CommandName::fixed(CREATE_AGENT_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Makes an agent of the personality named, in this session, named after it unless named.".to_string(),
            usage: Some("<personality> [as <name>]".to_string()),
            owners_only: false,
        }]
    }
}

#[async_trait]
impl ChannelCommandProvider for CreateAgentComponent {
    async fn execute(
        &self,
        _name: &CommandName,
        args: &str,
        context: &CommandContext<'_>,
    ) -> CommandResult<CommandOutput> {
        let invalid = |reason: String| CommandError::InvalidArguments {
            command: CREATE_AGENT_COMMAND_NAME.to_string(),
            reason,
        };
        let (personality, name) = match args.rsplit_once(" as ") {
            Some((personality, name)) => (personality.trim(), name.trim()),
            None => (args.trim(), args.trim()),
        };
        if personality.is_empty() || name.is_empty() {
            return Err(invalid("name the personality, and the agent after 'as'".to_string()));
        }
        let unusable = |what: &str| invalid(format!("'{what}' is no name"));
        let personality = Name::new(personality).map_err(|_| unusable(personality))?;
        let name = Name::new(name).map_err(|_| unusable(name))?;
        let failed = |e: Error| command_error(CREATE_AGENT_COMMAND_NAME, &e);
        let session = context
            .channel
            .session_loop()
            .ok_or_else(|| failed(Error::msg("the channel belongs to no session")))?;
        let issuer = context.issuer.security_context();
        let place =
            Scope::ChannelScope { session_id: session.id(), channel_id: context.channel.id() };
        let personality = match session
            .views(Viewer::new(issuer.clone()))
            .personality_named(&personality, &place)
            .await
        {
            Ok(personality) => personality,
            Err(e @ ViewError::NoPersonalityNamed(_)) => return Err(invalid(e.to_string())),
            Err(e) => return Err(failed(e.into())),
        };
        let scope = match &personality.name.scope {
            in_channel @ Scope::ChannelScope { .. } => in_channel.clone(),
            _ => Scope::SessionScope { session_id: session.id() },
        };
        let spec = AgentSpec::new(name.as_str(), scope, personality.id);
        let agent = self.agents.create(&issuer, spec).await.map_err(|e| failed(e.into()))?;
        Ok(CommandOutput::private(format!(
            "Made the agent {} of {}; /add brings it into a channel.",
            agent.name.name, personality.name.name
        )))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(CREATE_AGENT_KIND)
            .provides::<dyn ChannelCommandProvider>()
            .uses::<AgentManager>(),
        |_descriptor, context| {
            let agents = Arc::clone(context.service::<AgentManager>()?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ChannelCommandProvider>(Arc::new(CreateAgentComponent { agents }));
            Ok(facets)
        },
    );
}
