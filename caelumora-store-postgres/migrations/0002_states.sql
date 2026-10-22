-- The states (issue 04): each a document, as serde writes it - without its
-- chain and what else is kept apart -, and the chains a row per node, in the
-- order the nodes were added (`seq`), with their heads in a table of their
-- own. Ids and digests are kept as text, as they are written (`Display`).

CREATE TABLE agent_state (
    agent_id text PRIMARY KEY,
    body jsonb NOT NULL
);

CREATE TABLE channel_state (
    channel_id text PRIMARY KEY,
    body jsonb NOT NULL
);

CREATE TABLE user_channel_state (
    user_id text NOT NULL,
    channel_id text NOT NULL,
    body jsonb NOT NULL,
    PRIMARY KEY (user_id, channel_id)
);
CREATE INDEX user_channel_state_channel ON user_channel_state (channel_id);

-- An agent's state in a channel, and whether it has work under way (issue
-- 388): what a restart starts again, read from the partial index.
CREATE TABLE agent_channel_state (
    agent_id text NOT NULL,
    channel_id text NOT NULL,
    has_work_under_way boolean NOT NULL,
    body jsonb NOT NULL,
    PRIMARY KEY (agent_id, channel_id)
);
CREATE INDEX agent_channel_state_channel ON agent_channel_state (channel_id);
CREATE INDEX agent_channel_state_under_way ON agent_channel_state (channel_id) WHERE has_work_under_way;

-- The transcripts of the agents (issue 380), every branch: a node by its
-- position, with the message it holds or changes, by which it is found.
CREATE TABLE transcripts (
    agent_id text NOT NULL,
    channel_id text NOT NULL,
    head text,
    PRIMARY KEY (agent_id, channel_id)
);
CREATE INDEX transcripts_channel ON transcripts (channel_id);

CREATE TABLE transcript_entries (
    agent_id text NOT NULL,
    channel_id text NOT NULL,
    seq bigint NOT NULL,
    message_id text,
    node jsonb NOT NULL,
    PRIMARY KEY (agent_id, channel_id, seq)
);
CREATE INDEX transcript_entries_channel ON transcript_entries (channel_id);
CREATE INDEX transcript_entries_message ON transcript_entries (agent_id, channel_id, message_id) WHERE message_id IS NOT NULL;

-- What an agent's model was given beside its transcript (issues 383, 426),
-- each once by its digest.
CREATE TABLE prompt_texts (
    agent_id text NOT NULL,
    channel_id text NOT NULL,
    digest text NOT NULL,
    text text NOT NULL,
    PRIMARY KEY (agent_id, channel_id, digest)
);
CREATE INDEX prompt_texts_channel ON prompt_texts (channel_id);

CREATE TABLE offered_tools (
    agent_id text NOT NULL,
    channel_id text NOT NULL,
    digest text NOT NULL,
    sets jsonb NOT NULL,
    PRIMARY KEY (agent_id, channel_id, digest)
);
CREATE INDEX offered_tools_channel ON offered_tools (channel_id);

CREATE TABLE frames (
    agent_id text NOT NULL,
    channel_id text NOT NULL,
    digest text NOT NULL,
    frame jsonb NOT NULL,
    PRIMARY KEY (agent_id, channel_id, digest)
);
CREATE INDEX frames_channel ON frames (channel_id);

-- The histories of the channels (issue 379): the chain, its head and its
-- memberships (issue 417) - kept whole, as `Memberships` takes them in,
-- none before an entry is -,
-- and the items derived from it, each as it stands, found by the message it
-- said.
CREATE TABLE histories (
    channel_id text PRIMARY KEY,
    head text,
    memberships jsonb
);

CREATE TABLE history_entries (
    channel_id text NOT NULL,
    seq bigint NOT NULL,
    node jsonb NOT NULL,
    PRIMARY KEY (channel_id, seq)
);

CREATE TABLE history_items (
    channel_id text NOT NULL,
    seq bigint NOT NULL,
    message_id text,
    item jsonb NOT NULL,
    PRIMARY KEY (channel_id, seq)
);
CREATE INDEX history_items_message ON history_items (channel_id, message_id) WHERE message_id IS NOT NULL;
