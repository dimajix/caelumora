* Improve logging (REST server, internals)
* Implement Postgres template store
* Implement Postgres state store
* Implement Postgres event recorder
* Implement tool calling
* *New directory name for state_store
* Better error messages

## State Persistence
* Put "context" into message for auditability?
* How to inject StateStore into Components?
* Is it correct to put AgentState into ComponentDescriptor?
* Maybe ComponentDescriptor should be ComponentContext?