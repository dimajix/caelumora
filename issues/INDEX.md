# Issues — Index

Every issue, one line each, under its area and by number. Generated from the field lines of the
issue files by `issues/generate.sh` - never edit it by hand: change the issue file, and the
pre-commit hook (`.githooks/pre-commit`) or a run of the script brings this file up to date. How
issues are kept is described in [README.md](README.md); those open or in progress are listed in
[OPEN.md](OPEN.md).

## Observability and operations

| No. | Status | Issue |
|---|---|---|
| [01](01-improve-logging.md) | done | Two logging frontends (`log` and `tracing`) are used side by side, and the REST handlers, session loop and event bus are nearly silent. |
| [02](02-better-error-messages.md) | done | Failures lose the context that would make them actionable — which component, which endpoint, which parameter. |
| [21](21-scaling.md) | done | The runtime is single-process by design; which of the three scaling axes actually matters has never been decided. |
| [122](122-several-instances.md) | open | The server cannot run in several instances: everything live is in one process, and nothing keeps agents and users in step between processes. |
| [130](130-llm-configuration.md) | done | The language model is configured through environment variables, not in the configuration file like everything else. |
| [209](209-token-usage.md) | done | The tokens of each model call are recorded per message but summed up nowhere, and the model's context size is unknown, so nobody sees how full an agent's context is. |
| [297](297-orderly-shutdown.md) | done | Caelumora ends without an orderly shutdown: a signal ends turns mid-step, breaks off streams and leaves the `local` shell's commands running. |
| [307](307-load-and-performance-tests.md) | open | Nothing measures how Caelumora behaves under load, so its bottlenecks are guessed at, and a real model would hide them behind its own latency. |
| [336](336-errors-of-a-plugin-shown-as-a-failure.md) | done | The core turns an error into what a person is told by the error types it knows, so what a plugin's error says is shown as a failure wherever it travels inside an `anyhow`. |

## Persistence

| No. | Status | Issue |
|---|---|---|
| [03](03-postgres-entity-store.md) | done | `EntityStore` has only in-memory and JSON-file implementations; a Postgres one is the prerequisite for everything multi-process. |
| [04](04-postgres-state-store.md) | done | `StateStore` likewise — and it holds the state whose loss is most expensive, including agents suspended on an approval. |
| [20](20-transactional-effects.md) | rejected | A cognitive turn persists state, runs tools and emits events with no boundary around them; a crash mid-tool re-executes it on restart. |
| [62](62-json-store-concurrent-writes.md) | done | Concurrent writes of the same JSON store file share one temporary file, so one fails and the newest state can be lost. |
| [77](77-template-swallows-store-errors.md) | done | `ParticipantTemplate::instantiate` treats every store error as "not found" and creates a second agent. |
| [93](93-entity-store-dialects.md) | done | The `EntityStore` offers a different set of methods for every entity kind, and templates are stored by name but deleted by id. |
| [108](108-serde-defaults-for-legacy.md) | done | Stored state carries `#[serde(default)]` only so that documents from before a field existed still load, although no such documents need to. |
| [178](178-scoped-lookup-by-name.md) | done | Definitions cannot be looked up by name within their scope, and the one lookup that uses a scope - a template's agents - finds too much and ignores the personality. |
| [182](182-entity-reads-not-cached.md) | open | Personalities, agents and users are read from the store again for every use - for every member of a channel, on every turn - though they change rarely. |
| [193](193-external-state-rewritten-whole.md) | done | Every state the `StateStore` keeps is rewritten whole for everything that happens in it - an agent's for every message heard and every step of a turn - so the writes grow with the square of a channel's length. |
| [215](215-json-store-concurrent-puts-of-one-state.md) | done | Concurrent puts of one state in the JSON store can leave a journal beside the new snapshot and lose a change appended in between, and the test that puts one state concurrently fails now and then. |
| [250](250-unique-names-per-name-space.md) | done | No kind keeps its names unique in its name space as a whole: channels check it by hand when created, not when renamed; agents, personalities, templates and users not at all. |
| [292](292-store-lists-read-everything.md) | done | A list of what a user may read is made by reading every entity of the kind and asking the policy of each - for artifacts, of which there may be hundreds of thousands, per version too. |
| [293](293-store-queries.md) | done | A store list takes no condition and no limit: every narrowing is done by its caller after everything was read, and no database could take it over. |
| [295](295-access-as-a-query-condition.md) | rejected | A list asks the policy entity by entity, so a database store cannot push access into its query and a full page may be followed by an empty one. |
| [318](318-restore-returns-nothing.md) | done | A store's restore returns nothing, so every caller reads or rebuilds what it restored - under a permission the restore did not need - and the managers answer a restore differently. |
| [376](376-histories-held-whole-in-memory.md) | done | A channel's history and an agent's history and transcript are held in memory and loaded whole, though they grow with the channel and almost only their recent end is used. |
| [379](379-history-stored-as-it-stands.md) | done | A page of a channel's or an agent's history costs folding the whole chain, however far back and however small the page is, though the history is read almost only in pages. |
| [380](380-transcript-window.md) | done | An agent's transcript holds every branch it ever had in memory, though only the active branch is given to the model and an old branch is read for the context of an old message alone. |
| [383](383-prompt-texts-and-tool-sets-kept-per-turn.md) | done | Every prompt text and tool set an agent was ever given in a channel is held in memory for the record, though only the current turn's and an old context's are read. |
| [387](387-agent-history-window-without-compaction.md) | done | An agent that never compacts its context - every external agent - holds its whole history in a channel in memory, since the window of its history begins at its last compaction. |
| [416](416-messages-stored-once-per-holder.md) | rejected | Every history and transcript stores each message it holds in full, so one message said in a channel lies in the store once for the channel and twice for every agent in it, and every change of it as often again. |
| [421](421-history-filter-as-data.md) | done | `HistoryStore::page` takes `Keep`, a Rust closure, for what a viewer may read and what a search looks for; a database cannot turn it into a `WHERE`, so it has to become data. |
| [426](426-context-ref-kept-once.md) | done | Every message a turn says is stored in its channel's history with the whole `ContextRef` it was produced from - a digest per prompt part, the knowledge read, the model profile - repeated nearly unchanged from message to message, often larger than the message itself. |
| [440](440-agent-position-in-history-not-journaled.md) | done | The journal of an agent's channel state records neither what of the channel's history the agent took in (`heard`) nor its last message (`last_message`), so a state loaded again stands where its last snapshot left it, and its channel delivers everything since once more - old requests to respond among it, which every agent then answers again. |
| [446](446-stores-check-access-each-and-are-enumerated.md) | done | The memory and the JSON store each implement every operation on entities with its access checks, and the model opens them by a closed enum, so a third store - Postgres - would repeat the checks and could not live in a crate of its own. |
| [447](447-store-seam-not-fit-for-a-database.md) | done | A storage of documents is listed by reading every id and then each document on its own, compares names as values only it can hold, and a backend cannot open asynchronously or reach a secret, so a database behind the seam of issue 446 would read a kind row by row, could keep no unique index of names and could not connect. |
| [448](448-persistence-inherits-the-state-stores.md) | done | The trait `Persistence` inherits `StateStore`, `HistoryStore` and `TranscriptStore` to add two methods, so a backend configured for the entities has to be a state store as well, the runtime holds two `StoreBackend`s of which half is unused each - and the checked layer the runtime uses is called the backend, the backend `Persistence`. |
| [450](450-store-overwrites-a-change-made-meanwhile.md) | open | `CheckedStore` reads a document, asks the policy and writes in steps of their own, so a second process that changes the same entity in between is overwritten without notice, and the permission is judged on what was stored before it. |
| [451](451-history-reader-judged-after-reading.md) | done | Whether a reader is given an item of a history is judged in Rust after the store read it, so a page of a reader who sees little of a long history reads it far back, also in Postgres. |
| [456](456-stored-and-deletable-apart-without-reason.md) | done | Whether a kind is marked when it is deleted is a trait of its own beside `Stored`, with a default that says "never deleted", though once the component bundles are a kind like every other no stored kind is removed for good any more. |
| [457](457-store-answers-misread-by-seed-and-ref.md) | done | The seed asks whether a document is there with `get`, which does not find one marked deleted, and then fails to create it again, so deleting a seeded personality makes every later start fail; and a store failure while a `ref` is resolved is reported as an invalid configuration that names no bundle. |
| [459](459-deleted-entities-a-flag-not-a-concept.md) | open | Whether an operation of a store is about the entities there or those deleted is a `bool` passed down, and the access checks come in pairs that differ only in it, so that what makes reaching a deleted entity special - kept for the record, asked by a verb of its own - is nowhere one concept; and the documents call it a trash. |
| [460](460-kinds-kept-in-one-form-in-postgres.md) | open | The Postgres store keeps every kind alike - a table of id, name, mark and body, searched by a GIN index of the body -, so a kind that is reached in a way of its own, such as the groups by their members, cannot have a table or column of its own that answers that faster. |

## Sessions and channels

| No. | Status | Issue |
|---|---|---|
| [08](08-retry-and-edit-past-answers.md) | rejected | A conversation is append-only — no retry, no correcting a past message, so a derailed turn keeps influencing every later one. |
| [09](09-creating-channels.md) | done | Channels exist only as instantiated from a session template; there is no create, no close, and no way to add a participant afterwards. |
| [30](30-user-actor-for-authenticated-user.md) | done | Messages and approvals from the REST API go through the channel's first `UserActor`, so they are recorded in the name of someone other than the authenticated sender. |
| [34](34-snapshot-on-stream-connect.md) | done | Event streams deliver only what happens after they connect, so a client cannot learn the current state and loses whatever happens while it connects. |
| [38](38-global-stream-loses-events-silently.md) | done | A client of the global event stream that falls behind loses events without being told, and its session list stays wrong until a reload. |
| [39](39-global-events-not-filtered.md) | done | The global event stream sends every user the events of every session, including the ids and new names of sessions they may not see. |
| [42](42-export-import-chats.md) | open | Chats cannot be exported or imported, so a conversation cannot leave the server it happened on or be brought into another. |
| [46](46-channel-participants.md) | done | The participants of a channel are fixed when it is built; nobody can be added to or removed from a running channel. |
| [67](67-change-channel-level.md) | done | A channel's confidentiality level can only be set when the channel is created, not changed afterwards. |
| [70](70-channels-as-entities.md) | done | Channels are stored inside the session document, so every change to a channel has to go through the session and renaming a channel is not stored at all. |
| [73](73-watch-leaving-duplicates-events.md) | done | `ChannelLoop::watch_leaving` is a second channel for the fact that someone left, which the `EventBus` already announces as `UserChannelLeft`/`AgentChannelLeft`. |
| [75](75-agent-resumes-half-turn.md) | done | An agent stopped on purpose mid-turn resumes that turn like after a crash, and a resume in `ExecutingTools` re-runs its tools. |
| [76](76-channel-cancel-flag.md) | done | The channel's own cancel flag suppresses the final response of a cancelled turn, so the other agents' transcripts diverge from the history. |
| [79](79-user-actor-exits-on-send-error.md) | done | A `UserActor` ends itself when one forward fails, and the three actor run loops differ only in how they handle errors. |
| [90](90-message-path-wake-and-queue.md) | done | Frontends must wake the channel before sending, the `UserActor` queues every message once more than needed, and perceiving a message begins a turn. |
| [120](120-participant-copies.md) | done | Sessions and channels store full copies of their participants' user records, which go stale and disclose email and roles to every participant. |
| [121](121-agent-controller-per-process.md) | done | An agent that takes part in several running sessions has one controller in each, so its document has more than one writer. |
| [124](124-observer-participants.md) | open | Nobody can join a channel just to watch it: joining makes a participant who can write, and without joining it cannot be followed live. |
| [134](134-session-privacy.md) | done | Once classification no longer keeps sessions apart, nothing expresses that a session's participants have not agreed to be read from another. |
| [141](141-turn-during-reclassification.md) | done | A turn running while its channel's classification is lowered classifies its answer above the channel, and the channel drops it. |
| [143](143-joining-a-channel-with-foreign-knowledge.md) | open | A participant added to a channel later sees what its agents read from other sessions, whose participants never agreed to be read by them. |
| [171](171-readded-while-still-stopping.md) | done | A participant removed and added again within seconds gets a new actor while the old one still stops, and both write its state. |
| [173](173-message-inspection.md) | open | Nothing examines what a message says before the channel routes it, so content that should be classified stricter, redacted or refused reaches every participant. |
| [174](174-moderator-as-component.md) | done | The moderator is a name mapped to four built-in policies, not a component of the channel, so it can be neither configured nor extended. |
| [175](175-user-driven-moderator.md) | done | There is no moderator under which every agent answers a person's message but no agent's message makes another agent answer. |
| [187](187-deleted-channel-keeps-remote-tasks.md) | done | Deleting a channel aborts it without a grace period, which may leave an external agent's remote tasks running. |
| [199](199-external-agent-speak-wakes-all.md) | done | An external agent's report on a finished task wakes every sleeping agent and ends a quiet channel, though it addresses neither. |
| [221](221-adding-session-participants.md) | done | A session's participants are fixed when it is created: nobody - neither a user nor an agent, existing or created from a personality - can be added to it later. |
| [222](222-queued-requests-answered-one-by-one.md) | done | An agent - run here or external - asked while it answers works through every queued request afterwards, one answer each, to a conversation that has moved on. |
| [223](223-full-inbox-drops-events.md) | done | An actor whose inbox is full - an agent in a long turn in a busy channel - loses what was said, requests and approvals without anyone learning of it. |
| [234](234-modify-revoke-and-retry-messages.md) | done | A message once said can be neither changed nor taken back by its author, and an agent cannot be asked to answer again, so a mistake keeps influencing every later turn. |
| [235](235-retry-of-an-answer-under-way.md) | done | An answer still being generated cannot be asked for again; only stopping every agent of the channel gets there. |
| [238](238-retry-of-an-older-answer.md) | open | Only an agent's last answer can be asked for again, and the alternatives of an answer are not kept. |
| [239](239-reclassified-message-not-in-agents-state.md) | done | A reclassified message keeps its old classification in every agent's state, so a released message still raises the label of later answers. |
| [245](245-session-and-channel-descriptions.md) | done | Sessions and channels have no description, so neither the users nor the agents in them learn what they are for. |
| [258](258-what-was-said-is-lost.md) | done | What a turn said - the steps of a plan with their tool calls and results, and every message of a turn given up - is shown live but never reaches the channel's history, so what an agent did cannot be followed. |
| [259](259-answer-again-after-a-turn-given-up.md) | done | An agent cannot be asked to answer again after a turn it gave up: the turn's end makes its last message not the last, and a turn that ended is no turn under way. |
| [261](261-turn-finished-repeats-the-answer.md) | rejected | `TurnFinished` carries the answer the channel already has from the messages said, only for the moderator to decide whom to ask. |
| [262](262-stopped-turn-not-recorded-while-reflecting.md) | done | An agent removed while its turn ends leaves the turn's messages in the channel's history without an end. |
| [342](342-invitations.md) | open | Nobody can be invited to a session or channel: who is added is taken in without being asked, told nothing beforehand and unable to decline. |
| [362](362-retry-of-a-message-of-an-agent-that-left.md) | done | The channel admits a request to answer a message again for an agent that left, and drops it unseen, while the same request for a turn is refused. |
| [377](377-stopping-a-session-aborts-its-actors.md) | done | Stopping or deleting a session aborts its actors at once, so a turn ends mid-step, a tool is cut off and a deleted session's external agents keep their remote tasks. |
| [381](381-inactive-channels-held-in-memory.md) | open | A channel nobody has spoken in for days keeps its loop, its actors and their states in memory until its session is stopped, and nothing notices that it is idle. |
| [382](382-agents-held-without-a-session.md) | open | An agent stays loaded with its memory while nothing uses it, and what still holds it once its sessions are unloaded has not been checked. |
| [385](385-inbox-lost-on-restart.md) | done | An actor's inbox is a queue in memory, so a restart loses what was routed to it and not yet taken in - a change to a message, a request, a command - for good. |
| [386](386-actor-state-after-a-stop.md) | done | Whoever follows a channel keeps an agent's last state and its answer under way after the session stops, and the follower's reconnect starts the session again. |
| [388](388-resume-interrupted-work-after-a-restart.md) | done | Work interrupted by a restart - a turn stopped mid-step at the shutdown - waits until someone acts in its session, since nothing starts the session again. |
| [403](403-decision-model-policy.md) | done | Every responder policy chooses who answers without looking at the message - by turn, by chance or by whether a user wrote - so in a channel of agents with different roles the one that fits a question is asked only if someone mentions it. |
| [406](406-personality-frozen-in-running-agents.md) | done | A running agent's actor takes its personality once, when it is built, while other parts of the runtime read it anew from the store, so a change of a personality reaches some of its uses and not others - its limiting roles among them. |
| [408](408-membership-changes-in-history.md) | done | When someone is added to a channel or leaves it, neither its history nor the agents' transcripts record it, so the chat does not show who added whom, and an agent learns of the change only from the roster of its next system prompt, without knowing that something changed. |
| [414](414-decision-policy-conversation.md) | done | The `decision_policy` is given only the message just said - or the messages of the turn just ended - without anything said before it and without who said what, so a reply like "Yes, do that" cannot be told apart from any other. |
| [417](417-user-history-from-its-actor.md) | done | Live, a user sees what the moderator routed to their actor; on reload they see the channel's history, filtered by what they may read, so the two can differ and whatever went to the user alone is gone. |
| [418](418-moderator-decides-delivery-only.md) | done | `Moderator::route` returns the envelopes to deliver, so a moderator could alter a message for one recipient or speak in any name, and the channel cannot record what was decided apart from what was sent. |
| [420](420-channel-built-empty.md) | done | A channel's founders are written into its document as it is built and never join, so no `ParticipantJoined` records them; build it empty and let its builder add them as anyone is added. |
| [422](422-route-depth-in-moderator.md) | done | `ChannelLoop::decide` refuses to ask the moderator about an event once its `span_depth` reaches a hard-coded `max_route_decisions`, so the channel keeps a routing rule of its own beside the moderator, which decides everything else about routing. |
| [423](423-newcomer-misses-what-is-said-as-it-joins.md) | done | An actor added to a running channel catches up from the store before `run` takes it in, so what the channel records in between is in the newcomer's range but neither caught up nor delivered to it. |
| [431](431-hearing-during-a-turn.md) | done | An agent takes in what others say only between its turns, so a turn of many steps - tool calls, a plan - works on to its end without what was said meanwhile: a correction, an addition, an instruction to stop doing something reaches it only after it has done it. |
| [434](434-hearing-during-a-turn-from-the-history-untested.md) | done | That an agent hears between the steps of a turn (issue 431) is tested only with an inbox whose queue holds everything, so nothing shows that it works the same when the inbox is behind and reads from the channel's history, or after a restart in the middle of a turn that heard something. |
| [435](435-another-initiators-words-wait-though-asked-again.md) | done | An agent whose personality hears during a turn with `others: wait` hears nothing more in that turn once someone other than its initiator has spoken, even when its initiator asks it again afterwards - by mentioning it or otherwise - though between turns that request would let it hear the other's words. |
| [436](436-work-resumed-after-a-restart-is-not-bounded.md) | rejected | A server that went down with many turns under way starts every session with work under way at its start, and all their turns then run at once, however many there are. |
| [437](437-mentions-of-people-and-failed-decisions-ask-agents.md) | done | A message that `@`-mentions only a person is answered by the agents the responder policy chooses, and when the decision model of `decision_policy` does not choose in time, every agent of the channel is asked - so in a channel of several agents, agents answer what was not meant for them. |
| [439](439-an-agent-is-not-told-why-it-was-asked.md) | done | A request to respond carries no reason, so an agent cannot tell whether it was mentioned or only chosen by a responder policy that guessed it might fit, takes every request as an order to answer, and answers with small talk or that it has nothing to say instead of staying silent. |
| [441](441-nobody-is-told-when-no-responder-was-chosen.md) | open | When the decision model of `decision_policy` does not choose - a timeout, an error, an answer naming no option - and nobody is asked, a person's message that mentions nobody stays unanswered without anyone telling them why. |
| [458](458-deleted-while-it-runs.md) | open | Deleting a session or a channel marks it, deletes what lies in its scope and the groups of its participants while it still runs, and stops it only afterwards, so what its agents write into its scope meanwhile is left behind, and what it announces as it stops reaches its owner alone. |

## Identity and access management

| No. | Status | Issue |
|---|---|---|
| [10](10-user-authentication.md) | done | There is no authentication at all; the REST server fabricates its own `SecurityContext` and `DummySecurityManager` approves everything. |
| [11](11-agent-ownership.md) | done | `SecurityAttributes` carries an `owner_id` that nothing enforces — every agent, session and template is effectively global. |
| [12](12-user-and-agent-permissions.md) | done | When an agent acts for a user, two sets of rights are involved and there is no rule for combining them. |
| [23](23-acls-in-security-attributes.md) | done | Who may reach a workspace's files or an artifact follows from the channel one stands in, not from the thing: `SecurityAttributes` can name no reader but the owner and a hand-written list. |
| [24](24-opa-security-manager.md) | rejected | `DummySecurityManager` is the only implementation; deployment-specific policy has no home outside the compiled binary. |
| [25](25-file-user-backend.md) | done | There is no source of identities to authenticate against; a hand-edited user file is the smallest one, and defines the provider seam. |
| [26](26-database-user-backend.md) | open | A file cannot serve several processes or an existing corporate user table; no database-backed user provider exists. |
| [27](27-ldap-user-backend.md) | open | Where identities are managed in a directory, the system has no way to authenticate against it or read its groups. |
| [31](31-session-renewal-from-ui.md) | done | The session token is only renewed by requests, so a web UI that is open but only receiving events is logged out after 15 minutes and loses the typed input. |
| [33](33-sse-outlives-its-token.md) | done | An open server-sent event stream is authenticated once, when it connects, and keeps delivering after its token has expired or its user was removed. |
| [64](64-confidentiality-compartments.md) | done | Confidentiality has ordered levels only; information that may be shared between the channels of one topic or group but not others cannot be expressed. |
| [69](69-scoped-actor-runtime.md) | done | Every actor's `ActorRuntime` hands out the process-wide services unrestricted, so it cannot be given to components without letting them read around the confidentiality rules. |
| [87](87-security-context-in-entities.md) | done | Stored agents and users carry a `SecurityContext` that the actors overwrite at start. |
| [94](94-users-unchecked-in-store.md) | done | The store checks access for every entity kind except users, where a placeholder comment stands in for the check. |
| [95](95-require-for-security-manager.md) | done | The runtime asks the `SecurityManager` for permission in four different ways with four different log lines. |
| [125](125-invisible-admin.md) | rejected | There is no administrator who may see and follow every channel without taking part in it. |
| [133](133-classification-model.md) | done | The confidentiality levels say how far information may travel, not how sensitive it is, so topics cannot be told apart and a tool cannot classify its results one by one. |
| [138](138-confidentiality-policy-in-ownership.md) | done | The confidentiality rules are applied by the ownership policy, so any other policy has to re-implement them or leave them out. |
| [142](142-roles.md) | in progress | Users carry roles as strings, but nothing says which roles exist or what they grant - clearances among them. |
| [149](149-special-tokens-in-content.md) | done | Special tokens typed into a message, or contained in a tool result, reach an Ollama model as real control tokens, so the content can open a fake system or agent turn. |
| [150](150-impersonation-towards-the-agent.md) | done | A participant can write another participant's sender marker into their own message, and models believe it, so the agent may act on a consent nobody gave. |
| [158](158-publish-question-built-five-times.md) | done | Whether something may flow into a channel is asked of the policy in five places by hand, not as an access question. |
| [167](167-origin-and-releases.md) | rejected | A classification says who may read something but not where it was written: a release overwrites the origin, releases cannot be told apart, and there is no rule for writing. |
| [168](168-turn-label-as-high-water-mark.md) | done | A turn writes under its channel's classification even when everything it saw was classified lower, so what it remembers is lost to lower channels. |
| [169](169-changing-classified-content.md) | open | Nothing says who may change something classified once it is written, or that what the writer has seen must fit into its classification. |
| [181](181-where-roles-decide.md) | open | Nobody has reviewed where a user's roles - and soon an agent's - enter a permission check; today only the clearance at a channel's door asks for them. |
| [198](198-security-scenarios.md) | done | Nothing says, end to end, which permissions an action needs; a member whose personality was withdrawn locks everyone out of its session, and some actions look at others before the actor's own permission. |
| [228](228-envelope-sender-not-enforced.md) | done | Each actor sets the sender and security context of its own envelopes, and the channel takes them as given, so an actor could act in someone else's name. |
| [255](255-whose-name-agents-act-in.md) | done | An agent reads in its owner's name and acts in the name of whoever asked it, one call often in both, and which applies follows from where the code sits rather than from a rule. |
| [264](264-agent-reads-channels-its-owner-may-not.md) | done | An agent reads another channel of its session by the flow rule alone, so it reads channels its owner may not, and membership hides nothing from a session's agents. |
| [286](286-groups-as-subjects.md) | done | A workspace or an artifact cannot be shared with a team that users make themselves, only with people one by one, a role, or the members of a session or channel. |
| [287](287-memberships-cache.md) | open | Every check of a membership or a role in an access list reads the entity store, for every event of a session or channel and every subscriber. |
| [289](289-attach-compares-access.md) | rejected | Attaching a workspace to a session shares it with that one session by an entry of its own, even where its access list already reaches everyone the session may take in. |
| [304](304-review-of-the-permission-concept.md) | done | Four kinds of permission - ownership, flow rules, actions, reachability - are checked in many places and several layers, and nobody has reviewed where each is asked, in which order, and whether the checks that mix two of them do so on purpose. |
| [306](306-user-records-have-no-writer.md) | done | A user's record is written by whichever provider authenticates, without an event, so the `UserController` never learns that roles changed and every check reads them from the store. |
| [308](308-artifacts-shared-with-a-team-not-listed.md) | done | An artifact shared with a team is read by its members but not listed among their artifacts, since the list asks the store only for artifacts naming the viewer or their roles. |
| [310](310-secrets-have-no-owner.md) | open | Any secret can be named by whoever writes a component: secrets have no owner and no access list, and the policy allows every `ReadSecret`. |
| [312](312-order-and-reach-not-everywhere.md) | done | Some operations ask about others before their own verb, or reach an object by id with fewer questions than the views, so a refusal tells more than it may and "by id" is not "by navigation". |
| [316](316-store-checks-artifact-without-versions.md) | done | The store checks an artifact against its own access list only, not its versions', so the views list and read artifacts in the runtime's name and decide themselves who may read them. |
| [317](317-teams-listed-by-a-scan.md) | done | `Views::groups` lists the viewer's teams by reading every group and asking the policy of each, while `Views::artifacts` finds them by a query. |
| [319](319-scoped-artifact-events-miss-version-readers.md) | done | An artifact deleted or restored with its session or channel is announced by its own access list only, so whoever reads it by a version's list does not hear of it. |
| [320](320-scoped-deletion-in-the-runtimes-name.md) | done | What lies in a session's or channel's scope is deleted and restored with it in the runtime's name, so the store asks nothing, another's things go without their owner being asked, and the deletion is recorded as the system's. |
| [321](321-channel-components-in-the-owners-name.md) | done | A channel's components act in its owner's name with the roles of when the channel was built, a context nobody decided on, so what they may read or use depends on when the channel was last built. |
| [322](322-clearance-decided-outside-the-store.md) | done | Whether one is cleared for a classified entity is asked by the views, three managers and the event bus each, besides the store, so that what a reader forgets to ask is found, and the deleted are reached by other rules than the live. |
| [332](332-policy-as-a-predicate.md) | done | The policy decides entity by entity, so a store filters a list after reading it and no database could put who may read into its query. |
| [339](339-default-access-of-new-versions.md) | open | Whom a new version of an artifact reaches is fixed to the channel it is published in, and neither a default for new versions nor a version's own list can be changed. |
| [340](340-teams-as-owners.md) | open | Only a person owns anything: no team can own what is done in its name, as a company does, so what is a team's result is one member's and goes with them. |
| [341](341-identity-ownership-delegation.md) | done | A security context cannot tell an agent's owner from the one it acts for: `principal_id` stands for both, so a check that means the owner silently asks whoever it acts for once they differ. |
| [343](343-users-bring-their-own-secrets.md) | open | A user cannot bring a secret of their own: every secret is the operator's, put into the configured providers, so an agent of one's own cannot reach a remote system with its owner's credential. |
| [347](347-entries-naming-purged-groups.md) | done | An access list keeps its entries for a group once the group is purged - a session long over, a team removed -, so lists grow by entries that name nobody and are cleaned up by hand. |
| [348](348-participants-groups-out-of-step-with-their-holder.md) | done | A session's or channel's deletion that fails half-way, or a restoring that does not reach its groups, leaves the group of its participants in the trash while it is there, and its members lose it. |
| [402](402-login-attempts-unlimited.md) | open | `/auth/login` takes any number of attempts from anyone, so passwords can be guessed without limit, and every attempt costs the server an Argon2 verification that a flood of requests turns into exhausted memory and CPU. |
| [432](432-envelope-carries-whole-security-context.md) | done | An envelope carries the whole `SecurityContext` of its sender - owner, principal, roles, limiting roles - to every actor and every frontend it reaches, though a receiver needs only who began the chain, and the channel takes the roles in it on trust. |

## Audit

| No. | Status | Issue |
|---|---|---|
| [05](05-postgres-event-recorder.md) | done | The `Recorder` hook is wired into every channel, but `BlackholeRecorder` is the only implementation, so nothing is recorded. |
| [06](06-context-in-message-for-auditability.md) | done | For a past message there is no way to reconstruct what the agent actually saw when it wrote it. |
| [07](07-session-deletion-audit.md) | in progress | Deletion works since `540e482`; what remains open is whether and how a deletion is recorded. |
| [140](140-audit-log.md) | open | Acts that change who may see what - releases, participants, approvals, deletions - leave no record of who did them and when. |
| [164](164-soft-deletion-and-former-members.md) | done | Deleting a session, channel or agent and removing a participant erase them from the store, so nothing can say later that they were there, who removed them or when. |

## Tools and approvals

| No. | Status | Issue |
|---|---|---|
| [13](13-approval-modes.md) | done | Approvals cannot be remembered — no "always approve" for a scope, no brave mode — so repeated prompts train the user to approve without reading. |
| [14](14-approval-privilege.md) | done | Any user in the channel can approve any tool call; ineligible responses cannot be ignored because eligibility does not exist. |
| [15](15-disallowed-tool-errors-to-llm.md) | done | A refused tool call does not reach the model as something it can act on, so the agent retries or proceeds as if it had succeeded. |
| [32](32-unavailable-tools-invisible-to-model.md) | done | A tool source that failed to load (e.g. an unreachable MCP server) leaves no trace in the model's context, so the agent acts as if the tools never existed. |
| [37](37-tool-name-collisions.md) | done | When two components of an agent provide a tool of the same name, the first one silently wins and the other is unreachable. |
| [49](49-plan-step-system-messages.md) | done | Plan step instructions are system messages in the middle of a turn, which some models move into the system prompt, losing their place and the prompt cache. |
| [52](52-split-tool-providers.md) | done | All built-in tool providers live in one file, and tools added later went to other places, so there is no rule for where a tool goes. |
| [78](78-mcp-requires-approval-ignored.md) | done | `McpToolConfig.requires_approval` is never read; every discovered MCP tool runs without approval. |
| [81](81-approver-double-id.md) | done | The approver gives every approval a second id it never uses, and the agent passes through `ExecutingTools` only to start waiting. |
| [82](82-tool-parameter-parsing.md) | done | `create_plan` answers malformed arguments with "the tool failed" instead of "correct the arguments". |
| [100](100-tool-executor-table-and-result.md) | done | The tool executor scans all providers on every call, and a tool result exists in three shapes between the executor and the model. |
| [111](111-tool-parameter-reading.md) | done | Every built-in tool reads its parameters in its own way, so schema and reading code are written twice and can disagree. |
| [118](118-malformed-call-put-up-for-approval.md) | done | A tool call with invalid arguments is put up for approval as "No description" and fails only after a user approved it. |
| [126](126-mcp-tools-discovered-once.md) | done | An MCP server's tools are discovered once, when the agent is built, so a replaced or temporarily unreachable server is not followed. |
| [127](127-tool-executor-not-replaceable.md) | done | The tool executor is fixed in the runtime, so how tools are executed cannot be tried out by configuring a different component. |
| [139](139-mcp-result-classification.md) | open | An MCP server cannot classify its results one by one, only be configured with one classification for all of them. |
| [145](145-ended-turns-leave-a-record.md) | done | A turn that fails, is stopped or is given up leaves no record in the store of the tool calls it made, though their effects remain. |
| [148](148-cancelled-call-without-result.md) | done | A turn cancelled while tool calls are pending is taken over with the calls but without results, so the transcript holds calls never answered. |
| [161](161-tools-cannot-be-interrupted.md) | done | A running tool cannot be interrupted, so a cancel waits for it and a stop during it aborts the agent without giving its turn up. |
| [172](172-pending-approvals-not-kept.md) | done | Pending approvals live only in the approver's memory and in the event stream, so a restart or a reload leaves a turn waiting for a decision nobody can give. |
| [180](180-outbound-flow-unchecked.md) | done | What an agent writes into the arguments of a remote MCP tool leaves the system with no confidentiality rule asked; only what comes back is checked. |
| [194](194-tool-sets.md) | done | An agent's tools reach the model and the UI as a flat list without the grouping and purpose of the provider they come from, and the UI shows them only per message, not for the agent. |
| [226](226-concurrent-tool-calls.md) | open | The tool calls of one step run one after the other, because every tool holds the turn while it runs, so a step of several slow calls takes the sum of their durations. |
| [246](246-mcp-tool-hints.md) | done | An MCP server's hints on its tools - read-only, destructive - are ignored, so all tools of a server need approval alike. |
| [247](247-tool-own-matcher.md) | done | A rule on tool calls can match an argument that is a language - a shell command, SQL - only by its text, which a chained command slips past. |
| [252](252-openapi-tool.md) | open | A tool server that describes itself by OpenAPI - as most HTTP services and Open WebUI's tool servers do - cannot be reached; only MCP servers can. |
| [254](254-commands-run-unbounded.md) | done | A command an agent's call hands back as running runs until the model ends it - past its turn, its session and the channel's access to the workspace. |
| [267](267-move-file.md) | done | Agents cannot move or rename a file of the workspace, although both storages can, except by reading, writing and deleting it or through a shell. |
| [273](273-publish-file.md) | done | An agent cannot point a user to a file of the workspace to download; it can only say in text where it lies. |
| [275](275-publish-file-without-approval.md) | done | `publish_file` asks for approval like the other file tools, though it changes nothing and gives the file to nobody who could not download it already. |
| [276](276-interrupt-one-tool-call.md) | done | A tool call that hangs can be interrupted only by ending its whole turn or agent, not on its own so that the agent goes on with the turn. |
| [281](281-tool-argument-visibility.md) | done | A tool's arguments share one visibility flag with its result, so a call cannot publish what it was asked and hide what it returned. |
| [290](290-tools-offered-by-the-turn.md) | done | A model is offered every tool of its components in every turn, also those it may not call where it is - a workspace's tools where there is none, or where its list grants reading alone. |
| [294](294-user-ends-a-command.md) | done | A command an agent's call handed back as running can be ended only by the model; a user who sees it hang has no way to end it. |
| [400](400-exact-matcher-long-value.md) | done | `ToolMatcher::exact` escapes every argument's whole value into a pattern, which for a value longer than `MAX_PATTERN_CHARS` fails the check, so the rule an approval card offers for such a call is refused - only after the call was answered. |
| [419](419-approval-recorded-with-the-call.md) | done | Once a tool call was approved, nothing stored says who approved it or by which rule, so neither the chat nor the record of the turn can show it. |
| [433](433-mcp-tools-list-changed.md) | open | An MCP server that announces a change of its tools (`notifications/tools/list_changed`) is not heard, so an agent follows the change only once its list of the server's tools expires. |

## User interaction

| No. | Status | Issue |
|---|---|---|
| [28](28-user-commands.md) | done | Everything a user types goes to the agents; there is no way to address the system itself with slash commands such as `/help`, `/wakeup` or `/compact`. |
| [29](29-mentions.md) | done | In a multi-agent channel the user cannot choose who answers; the moderator alone decides, and an `@name` in the text means nothing to it. |
| [35](35-sidebar-live-updates.md) | done | The web UI's session list is loaded once and never updated, so sessions and channels created or deleted elsewhere stay invisible until a reload. |
| [40](40-choose-session-template.md) | done | A new session in the web UI always uses the first template of an unordered list, and the user cannot choose another. |
| [41](41-show-message-context.md) | done | The web UI offers no way to see what an agent's model saw when it produced a message, although the server can answer it. |
| [43](43-message-time-and-statistics.md) | done | The web UI shows neither when a message was written nor the token counts and durations recorded for it. |
| [44](44-mentions-only-moderator.md) | done | Every moderator makes some agent answer every message; there is none under which agents respond only when they are `@`-mentioned. |
| [45](45-agent-mentions.md) | done | An `@`-mention written by an agent does not make the mentioned agent respond, so agents cannot hand questions to each other. |
| [56](56-response-language.md) | done | Agents often answer in English, and a user has no way to choose the language the agents of a channel answer in. |
| [80](80-stored-channel-response.md) | done | A channel that is not running is reported as not changeable and its agents' context as not readable, because the REST server picks and asks the access questions itself, in response builders spread over three modules. |
| [102](102-rest-channel-lookup.md) | done | The REST server repeats the session/channel lookup twelve times and decides itself what a viewer may see. |
| [103](103-rest-error-mappers.md) | done | Six REST functions map runtime errors to problem details with the same pattern. |
| [104](104-sse-stream-and-subscribe.md) | done | Both SSE handlers build their stream the same way, and the channel stream's membership loop is runtime knowledge in a handler. |
| [106](106-rest-small-findings.md) | done | Small inconsistencies in the REST server: hand-built responses, raw model types on the wire, stale docs, unused state. |
| [146](146-show-given-up-turns.md) | done | The web UI does not show the turns an agent gave up, so what their tools did can be read only through the API. |
| [162](162-ui-keeps-a-removed-agents-turn.md) | done | The web UI keeps showing the turn of an agent removed during it as under way, with its stop button, until the page is reloaded. |
| [165](165-ask-the-user-tool.md) | done | An agent cannot ask the user a question and wait for the answer - yes/no, a choice, free text - and the web UI has no way to show a tool call other than generically. |
| [170](170-context-panel-shows-the-label.md) | done | The context panel does not show how each part of a message's context was classified, so nobody can tell why the message was classified as it was. |
| [200](200-chain-limit-hint.md) | done | When a chain between agents reaches its limit and the moderator asks nobody to respond, the users are not told, so the mentioned agent looks as if it ignored the question. |
| [202](202-send-while-generating.md) | done | While an agent generates an answer, the web UI does not let the user send a message, though nothing in the runtime needs that. |
| [203](203-concurrent-answers-in-view.md) | done | The chat view keeps one answer under way per channel, so the streamed answers of agents answering at once run into each other or vanish. |
| [207](207-agent-history-in-ui.md) | done | An agent's history - all that was offered to it and what it said - cannot be shown; neither the views, the REST API nor the web UI hand it out. |
| [208](208-generated-artifacts.md) | open | Files and data an agent generates - an external agent's artifacts, a tool's images - are dropped; only their text is kept, and nothing can be stored or downloaded. |
| [216](216-show-reasoning.md) | done | The web UI shows an agent's reasoning nowhere - neither in the chat, while it streams or afterwards, nor in the context panel - although the server hands it out. |
| [218](218-paged-histories.md) | done | A channel's history and an agent's history are returned whole over REST, though both grow without a limit and only their end is shown first. |
| [219](219-usage-hover.md) | done | The token usage is shown in two hovers built differently - a box too narrow for lines that wrap, and the browser's plain tooltip - and neither looks good. |
| [220](220-user-stream-from-its-actor.md) | done | A user's stream is the channel's broadcast, not what their actor receives, so the moderator cannot decide what a user sees and a cut-off chain never reaches the web UI. |
| [225](225-cancel-leaves-answer-generating.md) | done | After a cancel the web UI keeps showing the answer as being generated, and the cancel button stays lit. |
| [227](227-agent-management.md) | done | Agents cannot be seen or managed on their own, only as members of channels: no list, no renaming, no trash in the web UI, nothing of what an agent has across channels. |
| [230](230-member-actions-menu.md) | done | The action buttons on every channel member in the web UI keep their place even when hidden, so the member's name is cut off almost entirely. |
| [231](231-resizable-panels.md) | done | The panels of the web UI - sidebar, channel members, an agent's context and history - have a fixed width that cannot be changed. |
| [232](232-empty-trash-shown.md) | done | The sections of deleted sessions and channels are shown in the sidebar even where they hold nothing the user may see, so every unfolded session carries an empty line. |
| [233](233-private-notes-panel.md) | done | An agent's private notes can be read only inside the context of one of its messages, as they stood then, not on their own, current and for every channel of the session. |
| [240](240-answers-under-way-not-in-snapshot.md) | done | A page loaded while an agent answers does not know the answer is under way, so it offers neither stop nor retry for it until its next message. |
| [242](242-retry-offered-for-external-agents.md) | done | The web UI offers to ask an external agent for another answer - of a message and while it answers - which it never gives. |
| [243](243-member-state-in-member-list.md) | done | The members panel does not show what an agent is doing - idle, asleep, working, waiting - nor offers to stop one that works. |
| [244](244-member-profile.md) | done | The web UI shows no participant's introduction, so a user learns of an agent in a channel only its name. |
| [263](263-plan-steps-not-in-chat.md) | done | The chat shows a plan only as the raw arguments of its call and its steps' messages as one block, without saying which message belongs to which step. |
| [265](265-file-list-and-search-presentations.md) | done | The web UI shows a listing and a search of the workspace as raw JSON, while reading, writing and replacing in a file are shown as what they do. |
| [266](266-workspace-on-new-session.md) | done | A new session's workspace cannot be chosen when it is made, and one for that session alone cannot be made without a template naming it. |
| [268](268-question-presentation.md) | done | An agent's question offers its choices side by side, their meanings in a list apart, and once answered is shown as raw JSON beside a record without the choices offered. |
| [269](269-agent-commands.md) | done | A user command can act on the channel but not on an agent - its transcript has one writer, the agent's actor - so neither `/compact` nor loading a skill for an agent can be built. |
| [274](274-multiline-message-input.md) | done | The message field of the web UI is a single line: a long message scrolls sideways instead of wrapping, and no line break can be entered. |
| [277](277-command-agent-by-name.md) | done | A command such as `/wakeup` finds the agent it names only by its exact handle, so `/wakeup Sassy` or `@Sassy` names no agent. |
| [278](278-nudge-command.md) | done | A user cannot ask an agent to respond to the conversation as it stands without writing a message, such as an empty `@sassy?`. |
| [282](282-message-attachments.md) | open | A user cannot attach an image or a file to a message, so a model that can see, such as Gemma 4, is never shown one. |
| [283](283-show-agent-components.md) | open | The web UI does not show what an agent was built from, so its owner cannot see which components it has or which came from an included list. |
| [291](291-share-artifacts-in-the-web-ui.md) | done | The web UI has no page of an artifact: its owner can neither see nor change whom it and its versions are shared with. |
| [298](298-pick-users-and-roles-to-share-with.md) | open | The web UI shares a workspace or an artifact only with the members of a session or channel, not with a user or a role by name, since the REST API lists neither. |
| [299](299-command-under-way-not-shown.md) | done | A command an agent carries out, such as `/compact`, is shown only once it is done: nothing tells that the agent began, and it is shown idle meanwhile. |
| [301](301-skill-list-presentation.md) | done | The skills an agent lists with `list_skills` are shown in the web UI as raw JSON, while loading, writing and changing a skill are shown as what they do. |
| [303](303-note-and-memory-presentations.md) | done | The web UI shows writing, changing, revoking and reading notes, private notes and memory entries as raw JSON, so a call that names an entry by number does not say which one. |
| [331](331-language-default-in-the-frontend.md) | done | The core holds a new channel or session in its creator's preferred language where nothing else is said, a decision that belongs to the frontend. |
| [384](384-malformed-query-answered-as-plain-text.md) | done | A malformed query string, such as `?limit=abc`, is answered with a plain-text `400` instead of problem details with a `code`, since axum's `Query` extractor is used unwrapped. |
| [401](401-mention-everyone.md) | done | A user who wants every participant of a channel to respond has to `@`-mention each of them by name; there is no `@all` or `@channel` that addresses them all at once. |
| [404](404-unread-and-mentions-in-overview.md) | done | The channel list shows only each channel's name, so a user does not see in which channels new messages arrived since they last looked, nor where someone `@`-mentioned them, without opening every channel. |
| [405](405-gauge-stale-after-compact-command.md) | done | After an agent compacted its context on `/compact`, the web UI's gauge still shows how full the context was before, until the page is loaded again. |
| [407](407-own-messages-by-role.md) | done | The web UI puts a message on the right, in the colour of one's own, whenever its role is `User`, so in a channel with several people the messages of all of them look like the viewer's own. |
| [411](411-call-limit-ends-turn-silently.md) | done | When a turn has asked its model `MAX_MODEL_CALLS` times, it ends as if it were done - no message, no reason, only a warning in the server's log - so the users see an agent stop in the middle of its work, and a plan under way is lost with the turn. |
| [415](415-messages-of-a-turn-not-grouped.md) | done | The chat shows every message of an agent as a bubble of its own, named and spaced like any other, so a turn of several messages cannot be told apart from several turns one after the other. |
| [425](425-session-language.md) | done | Only a channel can be held in a language, so the language of a session is set again on each of its channels, a channel added later is held in none, and nothing that belongs to the session as a whole - the formatting of times by its time zone among it - has a language to go by. |
| [443](443-errors-carry-only-an-english-text.md) | open | What an error tells a person - a problem detail of the REST API, what a failed turn or command tells its owner - is an English text written by the runtime, with nothing beside it from which a frontend could say the same in the person's language. |

## Context and knowledge

| No. | Status | Issue |
|---|---|---|
| [16](16-context-compression.md) | done | History and context grow without bound; nothing decides what to drop before the model's context window is exceeded. |
| [17](17-information-sharing-within-session.md) | done | Agents in one session share nothing but routed messages — there is no scratchpad one can write to and another read. |
| [18](18-knowledge-sharing.md) | done | Sessions are sealed: nothing an agent learns outlives the session, and cross-agent sharing is a harder question again. |
| [19](19-memory-tiers.md) | done | The four memory tiers from `TODO.md` are named but undefined — scope, lifetime, readers and invalidation are all open. |
| [22](22-skill-management.md) | done | Skills do not exist: there is no unit of procedural knowledge below a personality, and no rule for who may load one into a turn. |
| [47](47-prompt-parts-with-provenance.md) | done | What a model is given besides the conversation is placed and recorded on two separate paths, and can only be a system prompt, whose every change invalidates the prompt cache. |
| [48](48-shared-digest-chain.md) | done | History and context duplicate one digest-chained structure, and notes would become a third copy that branching would have to change separately. |
| [50](50-turn-context-naming.md) | done | `TurnContext` calls both the stored context and the input of one model call "context". |
| [51](51-agent-private-notes.md) | done | An agent knows in each channel only what was said there; it has no private knowledge that carries over to its other channels. |
| [54](54-private-notes-visible-to-users.md) | done | Private notes meant for the agent alone are visible to users, in the context panel and as the tool call in the chat. |
| [55](55-notes-by-reflection.md) | open | Notes are written only when the model thinks of calling a note tool while answering, so what an agent learnt in a turn is kept unreliably and never tidied up. |
| [57](57-prompt-catalogue.md) | done | The prompts the runtime gives models are English constants spread through the code, in one variant for every language and model. |
| [58](58-notes-in-every-prompt.md) | open | Every note is put into every prompt in full, which grows without bound, and the writing agent sees its notes a second time as tool calls - revoked ones included. |
| [59](59-retrieval-of-notes.md) | open | Knowledge reaches the model only in full or when the model itself searches for it; nothing selects what is relevant to the turn. |
| [60](60-note-ids-in-prompts.md) | done | Notes of its own that an agent is shown from other channels carry no usable id, so it cannot revoke them, and ids from other chains are ambiguous. |
| [61](61-reasoning-leaves-the-agent.md) | done | An agent's reasoning travels with its messages to every user, and can disclose what a hidden tool call or private knowledge withholds. |
| [63](63-reasoning-in-model-input.md) | done | Every model call gets the reasoning of all earlier messages back, other agents' included, although several reasoning models document that it must not be. |
| [65](65-generic-component-state.md) | rejected | The state of notes is hard-wired into the agent state, the turn, the stores and REST, so every further kind of component state needs changes to the core and branching must be taught each one. |
| [66](66-channel-level-in-prompt.md) | done | Private notes and results of tools the channel does not see are not marked as private for the model, which may quote them in its always-published answer. |
| [68](68-notes-in-agent-state.md) | done | An agent's notes are bound to one of its channels, although much of what it notes is knowledge of the agent itself, across channels and sessions. |
| [72](72-history-digest-ignores-limit.md) | done | What `read_history` records as read names the whole history of the channel, although a reader who joined later with a limit saw only part of it. |
| [92](92-ledger-for-notes-and-memory.md) | done | `NoteChain` and `MemoryChain` are the same type written twice. |
| [98](98-context-provider-facet.md) | done | `ContextProvider` is a facet with one implementation, and a second one would silently overwrite the first. |
| [101](101-chain-component-helpers.md) | done | The three chain components copy their helpers, and `AgentNotes`/`AgentMemory` are constructed only to be called once. |
| [117](117-message-prev-id.md) | done | `Message::prev_id` is excluded from the digest because the chain owns it, yet two places still set it. |
| [119](119-seen-item-and-context-item.md) | done | What a model saw when it produced a message and what its turn gave it are two types of the same thing. |
| [131](131-agent-memory-bypasses-policy.md) | done | Agent memory decides with the built-in rule which entries may be recalled in a channel, instead of asking the `SecurityManager` like the other reads across channels. |
| [135](135-sender-names-in-model-input.md) | done | In a channel with several participants the model sees their messages as `user` messages without their senders, so it cannot tell who said what. |
| [136](136-virtual-transcript.md) | done | A turn under way is resolved through another method, another reference type and another kind of placement than a recorded message, only because its messages have no digests yet. |
| [137](137-tools-in-recorded-context.md) | done | Which tools a model was offered, and how they were described to it, is recorded nowhere, so what an agent saw cannot be told in full. |
| [156](156-notes-and-memory-staged-scans.md) | done | Notes and memory answer "the chain with this turn's changes" with eight hand-written scans instead of the fold the chains already have. |
| [163](163-navigable-views.md) | done | There is no model to navigate from a session to what is in it: every read of history, notes, contexts or memory is put together anew, with its own conditions, by the REST server, the channel loop, the actors and `SessionReader`. |
| [166](166-memory-as-topic-documents.md) | open | Agent memory is a flat list of one-line entries, all of them in every prompt, with nothing to group, revise or fetch them by topic. |
| [176](176-participants-in-system-prompt.md) | done | An agent is told neither its own name nor who else is in its channel, and neither agents nor users have a description that says who they are. |
| [224](224-what-agents-hear-of-others.md) | done | What an agent hears of others is shaped by the moderator and again by the external agent, and an agent run here that joins takes others' answers from the history unshaped, as if they were its own. |
| [237](237-erasing-a-message.md) | open | A message written by mistake can be taken back but not erased: it stays in every participant's record and in the contexts built on it. |
| [249](249-prompt-part-titles.md) | done | Some prompt parts have no title and run into the others, and some have one that is a paragraph. |
| [253](253-workspace.md) | done | Nothing in Caelumora holds a file: agents and people have no workspace to keep, share and work on files in, and an executor would have to own them. |
| [256](256-workspace-versions.md) | open | A workspace has no versions: what a person, a file tool or a command overwrote cannot be taken back, and who changed what is not recorded. |
| [271](271-publishing-artifacts.md) | done | An agent cannot offer a file for download, and nothing it hands over outlives the workspace it lies in. |
| [272](272-artifacts-in-s3.md) | open | Artifacts can be kept only on the server's own disk, not in S3 or another object storage. |
| [279](279-agents-md-in-workspace.md) | done | An agent is not told of an `AGENTS.md` in its workspace, and there is no skill that helps it write one for an existing project. |
| [284](284-automatic-context-compaction.md) | done | An agent's context is compacted only when somebody asks for it; nothing compacts it before a model call that would not fit, least of all within the turn that makes it. |
| [300](300-compaction-keeps-long-turns-whole.md) | done | A compaction keeps the turn its newest messages reach into whole, so that a long plan is never summarised and almost nothing is freed. |
| [302](302-update-note.md) | done | A note cannot be changed, only revoked and written again under a new number, and `note` is not named after what it does like `revoke_note` and `read_notes`. |
| [305](305-skill-versions.md) | open | A skill keeps only its current text: every change overwrites what it said, nobody sees what changed, and an earlier version cannot be made current again. |
| [338](338-artifacts-go-with-their-session.md) | open | An artifact is published into its session's scope and is deleted with the session unless its owner keeps it elsewhere, though it is meant to outlive the conversation. |
| [389](389-knowledge-read-in-the-transcript.md) | done | A compaction finds what the agent read, to give it again, in the history, although the model's input is to be built from the transcript alone. |
| [390](390-context-gauge-profile-from-history.md) | done | How full an agent's context is between turns is estimated with the model profile of a message from its history, not with the rules of the model it is run with now. |
| [410](410-roster-changes-told-twice.md) | done | An external agent whose roster is told `changes_only` learns of a join or a leave twice - from the roster's "Since you were last told who is here" and from the channel's notice in what it is given - and the two can disagree. |
| [412](412-plan-budget.md) | done | A plan is carried out within one turn, and a turn may ask its model 25 times at most, so a plan of nine steps has fewer than three calls per step - too few for a step that writes a file, finds its mistake and writes it again - and a plan that runs out ends unfinished. |
| [413](413-compaction-restores-unused-skills.md) | open | After a compaction, the `skills` component gives the agent again every skill loaded in the part that was summarised, whether the work still needs it or not, so a context just made smaller is filled with instructions from an earlier task. |
| [424](424-time-in-the-prompt.md) | done | A model is given the messages of its channel without when they were said and is never told the current date and time, so it cannot tell a message of a minute ago from one of last week, answers questions about today from its training cutoff, and a session has no time zone that times could be given in. |
| [430](430-summary-given-times-in-utc.md) | done | `Turn::context_until`, which hands a component the transcript to work on - the summary of a compaction -, takes the turn's profile but not its time zone, so the model that summarizes reads the times of messages in UTC while the turn's own model reads them in the session's zone. |
| [438](438-agents-mention-without-knowing-it-asks.md) | done | An agent is told to mention others by their `@handle` to address them, but not that a mention of an agent asks that agent to respond, so agents greet and thank each other with `@` and every greeting starts another turn. |
| [442](442-agents-mention-each-other-despite-the-roster.md) | done | Agents greet, thank and praise each other with `@handle` although the roster says since issue 438 that a mention asks the agent to respond, because the rule stands once in the system prompt next to an invitation to address others by `@handle`, so nearly every turn in a channel of several agents is asked for by another agent's courtesy. |

## Maintenance

| No. | Status | Issue |
|---|---|---|
| [36](36-replace-serde-yaml.md) | done | The configuration is parsed with `serde_yaml`, which is deprecated and will receive no further fixes. |
| [53](53-facet-trait-placement.md) | done | The traits components implement as facets live in different places, some among the implementations, and `MessageProcessor` names two unrelated traits. |
| [71](71-access-helpers-on-entities.md) | done | Pure access questions such as `may_be_read_by`, `may_be_changed_by` and `history_visible_to` are public methods of the entities and bloat their interfaces. |
| [74](74-refactoring-epic.md) | done | Epic for the structure review of 2026-09-28: the bracket around issues 75–117 and their recommended order. |
| [96](96-config-unification.md) | done | `AppConfig` mixes tagged and untagged enums and `Option` with `default`. |
| [97](97-small-model-unifications.md) | done | Six helper functions in five modules define the default confidentiality level. |
| [105](105-seed-data-twice.md) | done | The demo data is seeded by both the REST server and the TUI, from two different definitions. |
| [109](109-code-hygiene.md) | done | `unwrap`, `unsafe`, infallible `Result`s, idle locks and per-chunk clones outside tests. |
| [110](110-docs-contradicting-code.md) | done | Doc comments and ARCHITECTURE.md name types and behaviour that the code does not have. |
| [112](112-id-types.md) | done | `ToolId` and `SessionTemplateId` are written by hand, and `ActorId::Approver` holds a bare `Uuid`. |
| [113](113-error-shapes.md) | done | "Forbidden" and "not found" have three shapes across the domain errors, and `ChannelError` lives in a crate that never constructs it. |
| [114](114-moderator-details.md) | done | `ModeratorActors` uses a concurrent map behind `&mut self`, downcasts for a kind the id already tells, and clones every message twice. |
| [115](115-filtering-event-sender.md) | done | `FilteringEventSender::send` drops a filtered envelope silently while `try_send` reports it as an error. |
| [129](129-documentation-structure.md) | done | AGENTS.md and ARCHITECTURE.md hold everything in two long files, so the architecture draws no overall picture and the details of one subject are hard to find. |
| [132](132-rest-and-web-ui-docs.md) | done | The REST server and the web UI have no document of their own; what they do is scattered across the documents of other subjects. |
| [151](151-agent-refactoring-epic.md) | done | Epic for the review of the agent actor of 2026-09-30: the bracket around issues 152–160 and 92, and their recommended order. |
| [160](160-agent-small-cleanups.md) | done | Around the agent actor, methods without callers, duplicated steps and stale comments are left from earlier changes. |
| [192](192-issue-179-small-duplicates.md) | done | Small duplicates were left from issue 179 in the member views, the reader's place, "not found means none" and `unheard`. |
| [313](313-docs-too-detailed.md) | done | The documents in `docs/` have about 130,000 tokens, mostly code retold - variants, fields, call orders - and inline issue history, so the concepts and rules are hard to find and every change has a second place to update. |
| [349](349-core-review-epic.md) | done | Epic for the review of agents, sessions and channels of 2026-10-14: the bracket around issues 350–361 and their recommended order. |
| [378](378-resource-management-epic.md) | open | Epic for resource management: the bracket around 376, 379–383 and 218, which reduce what the server holds in memory, and their recommended order. |
| [391](391-history-and-transcript-review-epic.md) | done | Epic for the review of the history and the transcript of 2026-10-16: the bracket around issues 392–398 and their recommended order. |
| [444](444-texts-compiled-in-cannot-be-exported.md) | open | A variant of the texts given to models is a directory of files of the form of those compiled in, but whoever runs the program without its sources has no copy of them to begin one with. |

## Code structure

| No. | Status | Issue |
|---|---|---|
| [83](83-turn-logic-on-agent-actor.md) | done | The logic of a turn lives in `AgentActor`, which drives `Turn` through eight to ten setters per step. |
| [84](84-agent-state-machine.md) | done | The agent's state machine has a state that only forwards and two handlers that differ in one branch. |
| [85](85-turn-state-in-agent-channel-state.md) | done | The working state of a turn is eleven loose fields of `AgentChannelState`, reset by three overlapping lists. |
| [86](86-controllers-in-the-model.md) | done | The five controllers write to the store and publish events from inside the model, each built differently. |
| [88](88-template-instantiation-in-the-model.md) | done | Instantiating a template does store I/O in the model, and both frontends drive the sequence by hand. |
| [89](89-channel-runtime-delegators.md) | rejected | `ChannelRuntime` is six methods that forward to `RuntimeManager`. |
| [91](91-double-actor-table.md) | done | `ChannelLoop` keeps its actors in two tables kept in step by a queue. |
| [99](99-component-factory-and-tags.md) | done | The component factory is synchronous while building is async, and component tags have no reader. |
| [107](107-public-interface.md) | done | Many `pub` items of the three crates have no caller, or one, outside their file. |
| [116](116-session-loop-channel-tables.md) | done | `SessionLoop` describes the one set of its channels with two tables and a queue. |
| [123](123-session-handle-in-channel-runtime.md) | done | The channel runtime carries a handle to the session that only the channel needs, filled in a second step outside the session. |
| [128](128-actor-runtime-in-components.md) | done | Components are given their actor's whole `ActorRuntime` and could store its state or act as it past the turn. |
| [144](144-runtime-and-entity-accessors.md) | done | Runtimes forward attributes of their own and of the entity above in several ways, with no rule for what the runtime answers and what the entity answers itself. |
| [147](147-actor-control-unchecked.md) | done | `Actor::cancel`, `wakeup` and `stop` check no permission, and `ChannelLoop::actor_by_id` hands them to any frontend, which could call them unchecked. |
| [152](152-three-signals-end-a-turn.md) | done | A turn is ended by three signals - a state switched from outside, a token and a flag - that the run loop reconciles with a precedence rule. |
| [153](153-three-give-up-paths.md) | done | A turn is given up along three paths that do the same, with two enums carrying one classification. |
| [154](154-model-step-mixes-four-things.md) | done | The model step mixes stamping messages, plan mode, streaming and plan progress in one function of 120 lines. |
| [155](155-approval-admission-on-the-actor.md) | done | The actor decides how a tool call is admitted and builds its `ToolCallState` by hand, though everything the rule is made of is in the tool guard. |
| [157](157-actor-holds-what-is-not-its-own.md) | done | The actor keeps the turn's count of model calls, which a resumed turn loses, wraps a constant in its mutex, and reads its state for others twice alike. |
| [159](159-split-agent-actor-file.md) | done | `actor_impl/agent.rs` holds the run loop, the model step, the tool step and the end of a turn in 1100 lines. |
| [177](177-metadata-on-some-entities.md) | done | Name, scope and description sit in `Metadata` on some entities only, which brings fields to entities that mean nothing there. |
| [189](189-agent-kind-decided-twice.md) | done | An agent's kind is decided from its personality in two places, by different rules for a missing personality. |
| [190](190-channel-seeds-actor-state.md) | done | The channel writes each actor kind's private state itself when a participant joins. |
| [191](191-channel-runtime-hands-out-runtime-manager.md) | done | `ChannelRuntime` hands out the whole `RuntimeManager` so that member lookups can be called with it. |
| [196](196-perceive-processors-write-state.md) | done | Whether an agent's history is complete depends on two configurable components, and an external agent has no stage to filter what it perceives. |
| [197](197-one-chain-one-model-of-change.md) | done | The ledger appends every change while the message chains amend entries in place, and six types stand between a chain and what it stores. |
| [201](201-component-nodes-wrapper.md) | done | Components are written under a `component_nodes` key inside their own key, a level that exists only because `ComponentConfig` is a struct with one field. |
| [206](206-one-agent-channel-state.md) | done | Agents run here and external agents keep their channel state in two types with separate store methods, change types and views, though history and transcript are the same in both. |
| [210](210-a2a-agent-structure.md) | done | The A2A agent is one file of 925 lines holding the protocol's types, their translation, the HTTP transport and the component, with what is sent built from untyped JSON. |
| [211](211-turn-and-agent-work-structure.md) | done | `Turn` is the agent's handle on its state between turns, the components' API and the turn's logic at once, the two kinds of agent publish and catch up through code written twice, and an `OpenTask` keeps less of its contexts than a `TurnState`. |
| [214](214-prompt-logic-in-the-context-provider.md) | done | What an agent is given besides the conversation is decided in a component, a free function and, for an external agent, its actor - and the component's facet writes into the turn instead of answering. |
| [229](229-services-fetched-through-the-runtime.md) | done | Central services - the `SecurityManager`, the stores, the event bus - are fetched through the runtime at every use, often through long chains, so no type says what it depends on. |
| [241](241-turn-membership-recorded-twice.md) | done | Which messages belong to a turn is recorded twice - as the turn's id on each message and as a count in its `ContextRef` - and read by different rules. |
| [248](248-validated-text-fields.md) | done | Text fields are `String`s checked by whoever writes them, if at all, so a field can hold what its rules forbid and a new writer has to remember the check. |
| [257](257-facet-and-components-in-two-modules.md) | done | A facet's trait and the components that provide it live in two sibling modules, `foo` and `foo_impl`, with no rule for what goes where. |
| [260](260-turn-messages-into-the-transcript.md) | done | A turn keeps its messages apart from the transcript until it ends, so two places hold what an agent said and every reader puts them together. |
| [270](270-personality-inheritance.md) | done | Every personality writes out its whole list of components, so what every agent should have is repeated in each and a later addition reaches only those touched by hand. |
| [280](280-tool-executor-trait.md) | done | `ToolExecutor` is a replaceable facet whose only implementation is `DefaultToolExecutor`, and its shape cannot serve the variations it was made replaceable for. |
| [285](285-set-context-with-knowledge.md) | done | A turn's prompt parts and the knowledge read for them are set in two calls, so every caller of the `ContextProvider` records the knowledge itself. |
| [296](296-workspace-operations-on-the-view.md) | done | The view of a workspace, which is for reading, hands out its terminal for writing and publishes and ends commands with it, while the scope of a command is made by every caller of its shell. |
| [309](309-options-none-only-in-tests.md) | done | Some fields are an `Option` that production code always sets - a turn's views only tests leave out, a component container's graph nobody does - so their readers handle a case that never occurs. |
| [311](311-split-channel-and-session-loops.md) | done | `channel.rs` and `session.rs` have about 5900 and 2750 lines, each holding several subjects in one file, and most of the channel's tests are about the actors that run in it. |
| [314](314-session-manager-handles-agents.md) | done | The `SessionManager` also deletes, restores and purges agents, though only deleting has to do with sessions, and agents have no owner of their own as groups do. |
| [315](315-view-and-session-test-fixtures.md) | done | The tests of the views and of sessions have fixtures of the same names and different signatures, so four tests of the views live with the sessions. |
| [323](323-kinds-enumerated-in-the-core.md) | done | Every stored kind is enumerated in the core - `ActionType`, `EntityKind`, `EntityStore`, `GlobalEvent`, the scope, the seed, `ApiError` and the routes - so a subject such as skills cannot be moved into a crate of its own. |
| [324](324-backends-enumerated-in-the-core.md) | done | The backends of a service - the storages of workspaces and of artifacts, the user and secret providers, the stores - are each a closed `enum` of the configuration, so none can be added from a crate of its own. |
| [325](325-mcp-in-a-crate-of-its-own.md) | done | The MCP integration is part of the runtime crate, so `rmcp` is built into everything that uses the runtime, though the core knows `mcp_tool` only as a component kind. |
| [326](326-a2a-in-a-crate-of-its-own.md) | done | The A2A protocol is part of the runtime crate, though the core reaches external agents only through the `ExternalAgent` facet and names no protocol. |
| [327](327-open-terminal-in-a-crate-of-its-own.md) | done | The Open Terminal storages of workspaces are part of the runtime crate and named in `StorageConfig`, so `bollard` is built into everything that uses the runtime. |
| [328](328-artifacts-in-a-crate-of-their-own.md) | done | Artifacts are enumerated in the core like skills, and need seams skills do not - the channel's view, an event that carries a version, a verb shared with workspaces, a dependency on workspaces. |
| [329](329-ollama-named-by-the-core.md) | done | The core names `ollama_client` as the default model, so the choice of a model provider is not the deployment's and a second provider cannot become the default without configuration. |
| [330](330-log-setup-in-the-runtime.md) | done | The runtime library installs the `tracing` subscriber for the binaries and so depends on `tracing-subscriber`, though setting up the log is the application's decision. |
| [333](333-workspaces-woven-into-sessions.md) | rejected | Workspaces are part of what sessions, channels and templates are - an attached workspace, its use per channel, one made by a template -, so they cannot leave the core as skills did. |
| [334](334-groups-are-part-of-the-access-model.md) | rejected | Groups are the participants of sessions and channels and the teams users make in one kind, which the policy reads, so only teams could become a plugin. |
| [335](335-channel-events-name-their-session.md) | done | The events of channels and their members are variants of their own beside `GlobalEvent::Entity`, which cannot say where an entity lies, so a plugin's kind that lies in a channel has no way to announce it. |
| [337](337-components-nobody-asks-for.md) | done | A container builds every component it is configured with, also those its owner never asks for, so a tool in a channel reads its owner's secret and connects for nothing. |
| [344](344-workspace-manager-owns-their-use.md) | done | The `WorkspaceManager` owns workspaces and also what is done in them - reaching terminals and shells for a caller, ending commands, holding the storages -, with the stores and policies that needs. |
| [345](345-global-event-has-one-variant.md) | done | `GlobalEvent` is an enum with the single variant `Entity`, so every reader takes it apart with an irrefutable `let` and the event streams wrap every event in `{"Entity": …}`. |
| [346](346-subscription-topics-for-every-kind.md) | open | The event bus has subscription topics only for sessions, channels, agents and users, so a plugin cannot follow a single entity - an artifact, say - without reading every event. |
| [350](350-lookups-written-out-again.md) | done | The same lookup - the actor's channel, an agent's attributes as stored, a permission with the channel's attributes - is written out again at several places around channels and actors. |
| [351](351-channel-events-announced-by-hand.md) | done | The lifecycle events of a channel are announced at ten places, each assembling the key, the attributes and the classification by hand. |
| [352](352-envelopes-built-by-hand.md) | done | The approver and the channel build their envelopes by hand beside `Answering`, the one way an actor publishes. |
| [353](353-rules-of-the-model-computed-in-the-runtime.md) | done | Rules of the model - what an approval strategy decides, what a history limit shows, how a stored agent state is made fit to resume, how an update event applies - are computed in the runtime. |
| [354](354-names-follow-several-schemes.md) | done | Names around sessions, channels and actors follow several schemes: manager verbs with and without the noun, three ways to find by id, and `Member` meaning three things. |
| [355](355-two-agent-actors-share-half-their-fields.md) | done | The two agent actors and `AgentCommands` hold the same half of an agent three times, and what both kinds of agent do alike is written twice. |
| [356](356-member-questions-threaded-with-their-services.md) | done | The questions on a channel's member take its session and channel and four services as parameters each time, so both loops hold the services and one method needs a clippy exception. |
| [357](357-actor-start-and-channel-construction-written-twice.md) | done | Starting a member's actor is written three times in `ChannelLoop`, and building a channel's loop twice in `SessionLoop`. |
| [358](358-controllers-apply-their-events-each-on-its-own.md) | done | The controllers apply, store and announce their update events each on their own, `updated_at` moves on one path only, and the loops forward the updates with uneven logging. |
| [359](359-actor-runtime-hands-out-the-runtime-manager.md) | done | `ActorRuntime` hands out the `RuntimeManager`, which two actors use to reach past the facade for the `ControllerManager`. |
| [360](360-channel-runtime-forwarders-again.md) | done | `ChannelRuntime` forwards ten services of the `RuntimeManager` it never uses itself, each read once by `ChannelLoop::new`; what 89 rejected, on code that has changed since. |
| [361](361-session-before-its-channels.md) | done | A channel is attached to its session in a second step after it is built, though the session's controller already travels through its runtime; the option 123 rejected, on code that has changed since. |
| [363](363-run-control-built-twice.md) | done | Where an agent's actor stands and what `cancel` and `stop` asked of it is built twice - `RunControl` for the `AgentActor`, three fields of its own for the `ExternalAgentActor` - so the two show and end their work differently, and the loop that carries out commands cannot be shared. |
| [364](364-managers-in-different-places.md) | done | A module's manager lives in `manager.rs` in some modules, in `mod.rs` or a flat file in others, and its error type now beside it, now in `mod.rs`. |
| [365](365-security-manager-is-an-access-policy.md) | done | `SecurityManager` decides who may do what and manages nothing: it is the access policy beside `ConfidentialityPolicy`, under a name that means a lifecycle everywhere else. |
| [366](366-runtime-rs-holds-four-types.md) | done | `runtime.rs` holds the runtime, how it is built, and what a channel and an actor are given of it, interleaved in one file of 847 lines. |
| [367](367-prompt-module.md) | done | The prompt facets live in a flat `context.rs` and their components among the components of no facet in `component_impl/`, unlike every other facet with a module of its own. |
| [368](368-component-module.md) | done | `component.rs` holds the registry, the container, the context of a factory and 550 lines of their tests in one file of 1541 lines. |
| [369](369-security-module.md) | done | `security.rs` holds the actions, the attributes, the context, the access policy and its implementations, without a description, in one file of 1331 lines. |
| [370](370-agent-logic-in-a-crate-of-its-own.md) | done | The runtime crate holds the environment agents run in and the agents' own logic - the turn, the tools, the model, the prompts, both actors - in one crate, so nothing enforces that an agent reaches the channel through `ActorRuntime` alone, and the channel cannot be tested without an agent. |
| [371](371-tool-executor-construction.md) | done | `ToolExecutor::new` takes seven parameters, five of which its one caller reads from the agent's `ActorRuntime` that it passes as well, so they can disagree with it. |
| [372](372-runtime-test-actors.md) | done | The runtime's tests implement `Actor` four times beside the scripted agent, each writing the trait out again for a test or two. |
| [373](373-agent-test-worlds.md) | done | The agents' test world mirrors the four constructors of the runtime's `World`, each a shorthand for the next, so a new way to set up a world is added in eight places. |
| [374](374-channel-controller-beside-the-actor-runtime.md) | done | Every actor is given its channel's controller as a parameter beside its `ActorRuntime`, its interface to that channel, so the two can disagree and every actor's constructor carries both. |
| [375](375-test-runtimes-built-by-hand.md) | done | About twenty tests in four crates build a runtime by hand, writing out every service it is made of, so each change to what a runtime is built from changes them all. |
| [392](392-history-readers-without-callers.md) | done | `History::messages`, `items` and `recorded_context_of` read a history held in memory, which nothing in production does any more, and two comments of the agent's run loop describe a history that keeps only final messages. |
| [393](393-history-record-wrappers.md) | done | `History` records a turn's end, a plan step, a command, an activity and a compaction through five methods that each only assert the variant of the entry they are handed. |
| [394](394-chain-branch-memory.md) | rejected | `Chain` remembers the branch last taken from every entry and the stores take in nodes changed in their place, for a switch between branches that nothing makes since 08 was rejected. |
| [395](395-message-change-applied-twice.md) | rejected | `HistoryEntry` and `TranscriptEntry` apply a modification, revocation or reclassification to a message, and digest it, by two copies of one rule. |
| [396](396-three-ways-to-read-a-message-as-it-stands.md) | done | One message as it stands is read by three functions, two of which fold everything said after it or given before it. |
| [397](397-agent-channel-state-follows-the-transcript.md) | done | Four methods of `AgentChannelState` move the turn's context after the transcript's head each for itself, and `compact` checks for a turn four times. |
| [398](398-given-means-four-things.md) | done | Around the transcript "given" names a message given to read, which messages the transcript holds, and the directory of the prompt texts and tool sets. |
| [399](399-turn-and-agent-work-overlap.md) | done | `Turn` passes on nine of `AgentWork`'s methods to read and not the others, and `AgentWork` names its session, its workspace and the workspace's terminal by no common rule and fails in three shapes, so neither the line between the two types nor the one between views and handles can be read off their methods. |
| [409](409-participant-and-member.md) | done | Who takes part in a session or channel is a participant in the model and its operations, but a member in the views, the REST API and the web UI, and "member" also names who belongs to a group; and that taking part began or ended is told as joined/left on the event bus but as added on the session. |
| [427](427-local-terminal-in-a-crate-of-its-own.md) | rejected | The `local` storage of workspaces, whose shell runs commands on Caelumora's host as its user, is built into the runtime crate, so no deployment can be built without it and the core names one storage of its own beside those of plugins. |
| [428](428-workspace-tools-in-a-crate-of-their-own.md) | done | The `terminal_tool` and the `workspace_prompt_provider`, which give an agent the session's workspace, are part of the agents crate, while the tools of other subjects - artifacts, skills, MCP - are plugins. |
| [429](429-message-framing-configured-three-times.md) | open | `ollama_client`, `a2a_agent` and `decision_policy` each declare `sender_format`, `sender_style` and - the first two - `message_times` with their defaults in a configuration of their own and copy them into a `ModelProfile` by hand, so that a new setting of the framing is added in three places and missing in one goes unnoticed. |
| [445](445-model-parameters-repeated-per-personality.md) | done | A personality that needs one parameter of its model changed - the variant of the texts it is given, say - has to write out a model of its own with every parameter of the configured default, which then no longer follows a change of the default. |
| [449](449-runtime-given-the-policies-twice.md) | open | `RuntimeBuilder::new` and `RuntimeManager::new` take the `AccessPolicy` and the `ConfidentialityPolicy` beside a `Persistence` that was made with them, and nothing but a doc comment says that they have to be the same instances - tests already run with stores that permit everything under a runtime that checks. |
| [452](452-guideline-rules-not-checked-by-lints.md) | done | Most rules of CODING-GUIDELINES.md that clippy could check - imports, no `unwrap`, `pub(crate)` by default, `expect` instead of `allow`, no `println!` - are only written down, and are missed; nothing notices when a function grows too complex. |
| [453](453-artifact-publish-takes-eight-parameters.md) | open | `ArtifactManager::publish` takes the caller, the content, its classification, the name, the description, the session and the channel one by one, and carries an `expect(clippy::too_many_arguments)`. |
| [454](454-rustdoc-warnings-unchecked.md) | open | `cargo doc --workspace` warns 31 times that public documentation links to a private item, and neither clippy nor the pre-commit hook runs rustdoc, so such warnings only grow. |
| [455](455-components-configuration-kept-as-entities.md) | done | The preconfigured components are copied once from the seed into a store of their own, read only to resolve a `ref` and written by nothing else, and theirs is the one kind whose id is not a UUID - so no store can key every kind by one. |
| [461](461-scope-members-repeat-their-loops.md) | open | The managers of agents, workspaces, skills and artifacts, and `Unowned` for the kinds without one, each implement `ScopeMember` with the same three loops - list what lies in the scope, delete, restore or purge each, count or skip what was refused -, differing only in what they announce and what they remove besides. |

## Integrations

| No. | Status | Issue |
|---|---|---|
| [179](179-external-agents-via-a2a.md) | done | An agent that runs elsewhere cannot be given a task or take part in a channel; only models and MCP tools can be reached from outside. |
| [183](183-external-events-folded-twice.md) | done | An external agent takes in the remote agent's events in two places that do the same with differences of no stated reason. |
| [184](184-followed-task-ends-said-wrongly.md) | done | When a task an external agent follows fails or its stream breaks, the agent records a message nobody saw or publishes a half answer. |
| [185](185-sse-decoded-per-chunk.md) | done | The A2A event stream is decoded chunk by chunk, so a character or a line end split by the network breaks the answer. |
| [186](186-a2a-requests-and-long-tasks.md) | done | An external agent without streaming may not deliver tasks longer than its request timeout, and a streamed request can wait forever. |
| [188](188-remote-helpers-on-llm-error.md) | done | What remote integrations share - redacting the endpoint, describing errors, the endpoint's configuration - is not in `remote`, and MCP errors show the URL unredacted. |
| [195](195-broken-answer-said-twice.md) | done | An external agent's answer that broke off before its task ended is published cut off, and said again in full when the followed task ends. |
| [204](204-external-agent-roster.md) | done | An external agent is not told who is in its channel nor how to address them, so it cannot mention another agent. |
| [205](205-external-agent-message-context.md) | done | What an external agent was given for one of its messages is not recorded, so the web UI cannot show it, and its transcript ties no answer to the call it answered. |
| [212](212-external-task-during-reclassification.md) | done | An external agent's task that began before its channel's classification was lowered is answered into the lowered channel, where a turn of an agent run here is given up. |
| [213](213-external-task-taken-up-keeps-old-work.md) | done | An external agent's task taken up again by a later call keeps the work of the call that began it - its authority, envelope, bound and context - unless the remote agent tells the task as begun again. |
| [217](217-external-agent-reasoning.md) | rejected | What an external agent reports while it works is dropped, so its messages carry no reasoning and the chat shows nothing until it answers. |
| [236](236-external-agent-not-told-of-changes.md) | done | A remote agent is not told when a message it was given is modified or taken back, so it goes on answering the old version. |
| [251](251-open-terminal.md) | done | Agents have no workspace to run commands and keep files in, and Open Terminal, which offers one, is reachable only as an MCP server without approval by kind of tool, interruption or a channel's scope. |
| [288](288-compaction-of-external-agents.md) | rejected | A personality of an external agent that lists `compaction` is given a default model and compacts what its remote agent is to be given next; nothing refuses it. |
