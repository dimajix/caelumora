• SessionTemplate - contains the template definition of session with all its participants.
• AgentLoop (already present) - contains an agent definition, it's mutable state, all tools etc.
• AgentManager (already present) - used for creating agents.
• SessionManager (new) - used for creating SessionLoops from SessionTemplates.
• SessionLoop (new) - Contains a collection of agent loops, the orchestration strategy. The SessionLoop is responsible for orchestrating the interaction between one or more users and one or more agents and for updating the sessions state. A SessionLoop can be created for a given Session via "new". The "new" will create all AgentLoops.
• RuntimeManager (new) - Contains global objects like AgentManager, SessionManager, ToolExecutors or more. This will be passed to the "new" method of the AgentLoop or SessionLoop.
