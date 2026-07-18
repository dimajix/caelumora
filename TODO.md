* Improve logging (REST server, internals)
* Implement Postgres template store
* Implement Postgres state store
* Implement Postgres event recorder
* Better error messages

## Simple features
* Deleting sessions => Audit?
* "Retry" or changing past answers
* Creating channels

## Identity & Access Management
* User authentication
* Agent ownership
* Combining user permissions with agent permissions?

## State Persistence
* Put "context" into message for auditability?

## Tools
* Approval Workflow ("Always Approve", "Brave Mode")
* Approval privilege (who is allowed to approve? Other approvals must be ignored)
* Return errors to LLM on disallowed tools (security says "no")

## Context Management
* Implement context compression
* Implement sharing of information within a single session
* Implement knowledge sharing between sessions (possibly between different agents?)
* Memories
  * Agent channel context
  * Agent session memory
  * Agent global knowledge
  * Global knowledge

## BIG questions
* Transactional effects?
* Scaling?
