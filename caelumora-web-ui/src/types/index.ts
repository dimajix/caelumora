import type {Classification} from '../confidentiality';
import type {PrivacyPolicy} from '../privacy';

export interface SessionId {
  0: string;
}

export interface ChannelId {
  0: string;
}

export interface UserResponse {
  id: string;
  username: string;
  /** Lifetime of a session token in seconds; how often the UI renews. */
  token_lifetime_secs: number;
}

/**
 * The logged-in user's profile (issue 176): how they introduce themselves to
 * the agents of every channel they take part in.
 */
export interface ProfileResponse {
  introduction: string | null;
  max_introduction_chars: number;
  /** The language you prefer, a BCP 47 tag such as `de` (issue 56); the web UI holds your new channels in it (issue 331). */
  preferred_language: string | null;
  /** Your answers to tool calls given in advance (issue 13): rules that allow or deny, each within a reach. */
  approval_rules: ApprovalRule[];
}

/** Your profile as it is to be: both fields are replaced, `null` for none. */
export interface ProfileRequest {
  introduction: string | null;
  preferred_language: string | null;
}

export interface SessionResponse {
  id: string;
  name: string;
  /** What it is for, in a line (issue 245); absent if nobody said. */
  summary?: string;
  /** Whom the session lets read what was said in it (issue 134). */
  privacy: PrivacyPolicy;
}

/** What a session or channel is for (issue 245), as it is set: `null` removes one. */
export interface Description {
  summary: string | null;
  briefing: string | null;
}

/** Serialized `ActorId`: `"None"` or an object like `{ "Agent": "<uuid>" }`. */
export type ActorId = string | Record<string, string>;

/** What kind of participant an actor is. */
export type ParticipantKind = 'user' | 'agent' | 'approver';

/** Whether an agent runs here or elsewhere (issues 179, 244). */
export type PersonalityKind = 'internal' | 'external';

/** Where an agent stands (issue 243), as `StateChanged` and a participant's `state` name it. */
export type ActorState = 'Idle' | 'Sleeping' | 'Planning' | 'ExecutingTools' | 'WaitingForUser' | 'Reflecting' | 'Cancelling' | 'CarryingOutCommand' | 'Busy';

/** A participant of a channel. */
/**
 * What an agent's model calls in a channel used (issue 209): the tokens of
 * all calls - every call is given the whole context again, so this is what
 * was used, not what is held - and those of the last call against the size
 * of its context, how full the context is.
 */
export interface TokenUsage {
  calls: number;
  prompt_tokens: number;
  completion_tokens: number;
  /** Absent if not known. */
  context_size?: number;
  /** How many tokens the context holds now (issue 284) - after a compaction no longer the last call's. */
  context_tokens: number;
}

export interface ChannelParticipant {
  id: ActorId;
  name: string;
  /** The handle it is `@`-mentioned by, or null if it cannot be mentioned. */
  handle: string | null;
  kind: ParticipantKind;
  /** Whether you may see what this participant saw when it wrote a message (issue 41). */
  can_read_context: boolean;
  /** Whether you may see which tools it has now - only ever an agent (issue 194). */
  can_read_tools: boolean;
  /** Whether you may see which components it was built from - only ever an agent of yours (issue 270). */
  can_read_components: boolean;
  /**
   * Whether you may approve its tool calls - only ever an agent taking part,
   * yours or one you contribute to (issue 14). Everybody sees what it waits
   * for; only who may approve is asked.
   */
  can_approve: boolean;
  /**
   * Whether you may ask it to answer again instead of one of its messages or
   * of its answer under way (issues 234, 235, 242) - only ever an agent taking
   * part that answers again, not one that runs elsewhere. Whether it does is
   * the agent's to decide.
   */
  can_retry: boolean;
  /** Whether you may end its answer under way - only ever an agent taking part (issues 235, 243). */
  can_cancel: boolean;
  /**
   * Where it stands now (issue 243), kept up by `StateChanged`; absent for a
   * user or an approver, and in a channel that does not run.
   */
  state?: ActorState;
  /**
   * The commands an agent carries out itself that you may see (issue 269),
   * written `/name @handle …`; absent if none, and in a channel that does
   * not run.
   */
  commands?: Command[];
  /**
   * The turn of the answer it is giving now (issue 240), as `ResponseStarted`
   * named it; absent if none - also for an external agent.
   */
  turn_under_way?: string;
  /**
   * What its model calls used (issue 209): sent with the channel stream's
   * snapshot and by `getParticipant`, not where channels are listed; absent
   * for a participant whose calls report no tokens.
   */
  usage?: TokenUsage;
  /** For a former participant (issue 164): who removed it and when. */
  left?: Removal;
  /** Whether you may delete it - only ever an agent of yours (issue 164). */
  can_delete: boolean;
  /**
   * How it is introduced (issues 176, 244): an agent by its personality, a
   * user in their own words. Sent by `getParticipant` only, absent if none.
   */
  introduction?: string;
  /** The name of an agent's personality (issue 244) - by `getParticipant` only. */
  personality?: string;
  /** Whether an agent runs here or elsewhere (issue 244) - by `getParticipant` only. */
  personality_kind?: PersonalityKind;
  /** Who owns an agent: whose authority it acts with (issue 244) - by `getParticipant` only. */
  owner?: ActorId;
  /** The owner's name. */
  owner_name?: string;
}

/** That something was deleted, or someone left: by whom and when (issue 164). */
export interface Removal {
  by: ActorId;
  at: string;
}

/**
 * Something deleted - a session, a channel - in the trash (issue 164): who
 * deleted it and when, and whether you may restore it or remove it for good.
 */
export interface Deleted {
  id: string;
  name: string;
  deleted: Removal;
  can_restore: boolean;
  can_purge: boolean;
}

export interface ChannelResponse {
  id: string;
  name: string;
  /** What it is for, in a line (issue 245); absent if nobody said. */
  summary?: string;
  /**
   * What an agent should know before it works here (issue 245), Markdown:
   * sent where one session or channel is fetched, not where they are listed;
   * absent if nobody wrote one.
   */
  briefing?: string;
  /** All participants, in no particular order. */
  participants: ChannelParticipant[];
  /** The kind of the moderator, e.g. `policy_moderator`; `null` if preconfigured (issue 174). */
  moderator: string | null;
  /** The kind of the policy in the moderator's slot - who answers - e.g. `round_robin_policy` or `mentions_only_policy` (issues 44, 174); `null` if preconfigured. */
  responder_policy: string | null;
  /** The classification of the channel's contents (issue 133). */
  classification: Classification;
  /** The language its conversation is held in, a BCP 47 tag (issue 56); `null` for none in particular. */
  language: string | null;
  /** What it restricts of the tool calls made in it (issue 13): rules that ask or deny. */
  approval_rules: ApprovalRule[];
  /** Whether its participants work on the session's workspace here (issue 251), where the classifications let them. */
  uses_workspace: boolean;
  /** The session's workspace, if the channel uses it and you may read it (issue 23). */
  workspace_id?: string;
  /** Whether you may rename it, add or remove participants (issue 46) and set its approval rules. */
  can_change: boolean;
  /** Who took part and left (issue 164), in the order they left, each with `left`. */
  former_participants: ChannelParticipant[];
  /** What is new in it to you (issue 404), as you open or list it. */
  unread?: Unread;
}

/** What is new in a channel to you (issue 404): the messages of others said since you last read it. */
export interface Unread {
  /** How many, counted up to 100: more than 99 are shown as such. */
  messages: number;
  /** Whether one of them mentions you, `@all` included. */
  mentioned: boolean;
  /** The last message you read, after which the new ones begin; absent while you read nothing of the channel. */
  read_up_to?: string;
}

/** Who sees a part of what an agent produces (issue 133): the channel, or only the agent's owner. */
export type Visibility = 'channel' | 'owner';

/**
 * Who sees a tool call (issues 54, 133, 281): the call itself, its arguments
 * and its result. Absent means all `channel`.
 */
export interface ToolVisibility {
  call: Visibility;
  arguments: Visibility;
  result: Visibility;
}

/**
 * How a tool's calls and their results are shown (issue 165), declared by the tool and stamped on
 * each call; absent means `default`. Display only.
 */
export type ToolPresentation = 'default' | 'shell' | 'plan' | 'file_read' | 'file_write' | 'file_edit' | 'file_list' | 'file_search' | 'file_move' | 'questions' | 'file_publish' | 'artifact_publish' | 'skill' | 'skill_list' | 'note' | 'note_list';

export interface ToolCall {
  id?: string;
  function_name: string;
  /** `null` where the call was published without its arguments. */
  arguments: unknown;
  visibility?: ToolVisibility;
  presentation?: ToolPresentation;
}

/** How a tool call ended; absent means `ok`. */
export type ToolResultStatus =
  | 'ok' | 'denied' | 'forbidden' | 'timed_out' | 'declined' | 'invalid_arguments' | 'failed' | 'not_executed' | 'interrupted' | 'unknown_tool' | 'withheld';

export interface ToolResult {
  id?: string;
  tool_name: string;
  /** The tool's result, or for any other status a JSON object with `status` and `message`. */
  content: string;
  status?: ToolResultStatus;
  /** Empty `content` where the call was published without its result. */
  visibility?: ToolVisibility;
  /** How sensitive `content` is and for whom (issue 133). */
  classification: Classification;
  /** When the tool started and finished running; absent for a call that did not run. */
  started_at?: string;
  finished_at?: string;
  /**
   * The questions the call asked and how they were answered or declined (issue 165), for the
   * record; absent for a call that asked nothing, for sensitive questions, and where the content is
   * not shown.
   */
  answers?: Answers;
}

/** The state of a channel when an event stream connects (issue 34). */
export interface ChannelSnapshot {
  channel: ChannelResponse;
  /** The last page of its history: messages, and how turns ended (issue 258). */
  items: HistoryItem<Message>[];
  /** What to read the page before it with over REST (issue 218); `null` if there is none. */
  earlier: number | null;
  /** The notes every agent of the channel published (issue 17). */
  notes: PublishedNotes[];
  /** The approvals the channel's agents wait for (issue 172): the stream tells only of requests made while it listens. */
  pending_approvals?: ApprovalRequest[];
  /** The questions the channel's agents wait for the answers to (issue 165), as for approvals. */
  pending_questions?: QuestionRequest[];
}

/** A note as it stands (issue 17). */
export interface Note {
  /** Its number in the agent's notes. */
  id: number;
  text: string;
  /** The message it follows. */
  after: string;
  written_at: string;
  /** Where it was noted: the digest of the agent's transcript then. */
  origin: { transcript: string | null };
  /** The label of the turn that wrote it, unless it was reclassified since. */
  classification: Classification;
}

/** The private notes an agent wrote in one channel of a session (issue 233). */
export interface PrivateNotes {
  channel_id: string;
  channel_name: string;
  notes: Note[];
  digest: string | null;
}

/** The notes one agent published in a channel. */
export interface PublishedNotes {
  agent_id: ActorId;
  name: string;
  notes: Note[];
}

/** A session with its channels: an entry of the global snapshot, and `GET /sessions/{id}`. */
export interface SessionSnapshot {
  id: string;
  name: string;
  /** What it is for, in a line (issue 245); absent if nobody said. */
  summary?: string;
  /**
   * What an agent should know before it works here (issue 245), Markdown:
   * sent where one session or channel is fetched, not where they are listed;
   * absent if nobody wrote one.
   */
  briefing?: string;
  /** Whom the session lets read what was said in it (issue 134). */
  privacy: PrivacyPolicy;
  /** What it restricts of the tool calls made in any of its channels (issue 13): rules that ask or deny. */
  approval_rules: ApprovalRule[];
  /** Whether you may change it - rename it, change its privacy, set its approval rules. */
  can_change: boolean;
  channels: ChannelResponse[];
  /** Everyone who takes part in the session - whom a new channel can be made of (issue 09). */
  participants: SessionParticipant[];
  /** The workspace attached to it (issue 253), if you may read it - its owner; absent otherwise. */
  workspace?: WorkspaceResponse;
}

/** Where a workspace (issue 251), a skill (issue 22) or an artifact (issue 271) lies: of one session, the owner's, or global. */
export type ScopeKind = 'session' | 'user' | 'global';

/** What an entry of an access list grants (issue 23): reading, changing the files too, or - for a session or channel - joining it (issue 221). */
export type Grant = 'read' | 'write' | 'join';

/**
 * An entry of a workspace's access list (issue 23): whom - a user, a role,
 * the members of a session or a channel, or a team (issue 286) - it grants
 * what, with how whom is called where the server knows it.
 */
export interface AccessEntry {
  kind: 'user' | 'role' | 'session' | 'channel' | 'group' | 'everyone';
  /** Absent for everyone (issue 221). */
  id?: string;
  grant: Grant;
  name?: string;
}

/** A workspace (issue 253), as whoever its access list lets read it sees it (issue 23). */
export interface WorkspaceResponse {
  id: string;
  name: string;
  scope: ScopeKind;
  /** The session it was made for, if it lies in a session's scope. */
  session_id?: string;
  /** The storage its files lie in, by its configured name. */
  storage: string;
  description?: string;
  classification: Classification;
  /** Whether its storage gives it a shell: agents can run commands in it. */
  has_shell: boolean;
  /** Whether you may change it, share it and attach it to a session: its owner. */
  can_change: boolean;
  /** Whether you may change its files (issue 23); who may not, reads them. */
  can_write: boolean;
  /** Whom it is shared with - only for its owner. */
  access?: AccessEntry[];
}

/**
 * A skill (issue 22): a procedure written down for agents, as whoever may read
 * it sees it - with its `body` where it is fetched alone, not where skills are
 * listed.
 */
export interface SkillResponse {
  id: string;
  name: string;
  scope: ScopeKind;
  /** The session it was written for, if it lies in a session's scope. */
  session_id?: string;
  /** What it is for, in a line: what agents are shown to choose it by. */
  description: string;
  classification: Classification;
  /** Whose it is: a user, or the system for a skill of the seed. */
  owner: ActorId;
  /** Whether you may change it and delete it: its owner. */
  can_change: boolean;
  body?: string;
}

export interface CreateSkillRequest {
  name: string;
  /** `user` unless given; one of a session's scope is written by an agent in it. */
  scope?: 'user' | 'global';
  description: string;
  body: string;
  classification?: Classification;
}

/**
 * A group (issue 286), as its owner and its members see it: a team a user
 * made, or the participants of a session or a channel.
 */
export interface GroupResponse {
  id: string;
  name: string;
  /** `user` or `global` for a team; `session` for the participants of the session `session_id` - or of its channel `channel_id`. */
  scope: ScopeKind;
  session_id?: string;
  channel_id?: string;
  /** Whose it is. */
  owner: ActorId;
  /** Who belongs to it - users alone in a team. */
  members: GroupMemberResponse[];
  /** Whether you may rename it, change its members and delete it: the owner of a team. */
  can_change: boolean;
}

/** A member of a group (issue 286), with their name where the server knows it. */
export interface GroupMemberResponse {
  id: ActorId;
  name?: string;
}

/** A team to make (issue 286): with nobody in it yet. */
export interface CreateGroupRequest {
  name: string;
  /** `user` unless given; the participants of a session or a channel are a group made with it. */
  scope?: 'user' | 'global';
}

/** An agent the user may read (issues 164, 227), on its own rather than as a participant of a channel. */
export interface AgentResponse {
  id: string;
  name: string;
  /** `user` or `global` for an agent of its own; `session` for one that takes part only in the session `session_id` - or in its channel `channel_id`. */
  scope: ScopeKind;
  session_id?: string;
  channel_id?: string;
  /** Whose it is: whose rights it acts with. */
  owner: ActorId;
  personality_id: string;
  /** Its personality's name; absent if the personality is gone. */
  personality?: string;
  /** Whether you may rename it. */
  can_change: boolean;
  can_delete: boolean;
  /** The channels it takes part in that you may read - by `getAgent` only. */
  channels?: AgentChannelResponse[];
}

/** A channel an agent takes part in, with its session. */
export interface AgentChannelResponse {
  session_id: string;
  session_name: string;
  channel_id: string;
  channel_name: string;
}

/** An agent to make of a personality (issue 227): yours, in your scope unless `global` - or in a session's, `session_id`, or one of its channels', `channel_id`, as far as the personality admits it. */
export interface CreateAgentRequest {
  name: string;
  personality_id: string;
  scope?: ScopeKind;
  session_id?: string;
  channel_id?: string;
}

/** A personality the user may read (issue 227): what they may make an agent of. */
export interface PersonalityResponse {
  id: string;
  name: string;
  /** Where it lies, as an agent's: an agent made of it lies there or within it. */
  scope: ScopeKind;
  session_id?: string;
  channel_id?: string;
  kind: PersonalityKind;
  description?: string;
}

/** An artifact (issue 271), as whoever reads any of its versions sees it (issue 23): with those versions alone. */
export interface ArtifactResponse {
  id: string;
  name: string;
  /** `session`: published in it, and gone with it unless kept. */
  scope: ScopeKind;
  session_id?: string;
  /** What it is as a whole, as its owner describes it. */
  description?: string;
  classification: Classification;
  /** Oldest first; those you may read alone (issue 23). */
  versions: ArtifactVersion[];
  /** Whether you may keep, describe, share or delete it: its owner. */
  can_change: boolean;
  /** Whether you may add a version (issue 23). */
  can_write: boolean;
  /** Whom it is shared with as a whole - only for its owner. */
  access?: AccessEntry[];
}

/** A version of an artifact: what was published, where and by whom. */
export interface ArtifactVersion {
  number: number;
  size: number;
  digest: string;
  media_type?: string;
  /** What it is, as it was described when it was published (issue 23). */
  description?: string;
  session_id: string;
  channel_id: string;
  published_by: ActorId;
  published_at: string;
  /**
   * The names of where it was published and of who published it (issue 291) - where one artifact
   * is fetched, for whoever may read that channel; `published_by_name` is absent too for a
   * publisher gone.
   */
  session_name?: string;
  channel_name?: string;
  published_by_name?: string;
  /** Who reads it besides whom the artifact is shared with - only for its owner. */
  access?: AccessEntry[];
}

export interface CreateWorkspaceRequest {
  name: string;
  /** `user` unless given; one of a session's scope is made with the session. */
  scope?: 'user' | 'global';
  storage: string;
  description?: string;
  classification?: Classification;
}

/** A configured storage a workspace can be made in. */
export interface StorageResponse {
  name: string;
  /** Whether its workspaces have a shell: agents can run commands in them (issue 266). */
  has_shell: boolean;
}

/** An entry of a directory of a workspace. */
export interface FileEntry {
  name: string;
  is_dir: boolean;
  size: number;
  modified?: string;
}

export interface SessionParticipant {
  id: ActorId;
  name: string;
  kind: ParticipantKind;
}

/** The state of all sessions when the global event stream connects (issue 34). */
export interface GlobalSnapshot {
  sessions: SessionSnapshot[];
}

/**
 * What happened to an entity: one of the four of its life, the same for
 * every kind, or something particular to its kind under a name of its own -
 * a session `started`, an agent's `state_changed`.
 */
export type EntityChange =
  | 'Created'
  | 'Deleted'
  | 'Restored'
  | {Updated: unknown}
  | {Custom: {name: string; data: unknown}};

/**
 * An event of the global stream (`globalEvents.ts`), about an entity named by
 * its kind and its own id, and for a kind that lies in an entity by that
 * entity's id as well (issue 335): `{"kind": "session", "id": "<id>",
 * "change": {"Updated": {"NameChanged": "new name"}}}`, `{"kind": "channel",
 * "id": "<channel id>", "within": "<session id>", "change": "Deleted"}`.
 */
export interface GlobalEvent {
  kind: string;
  id: string;
  within?: string;
  change: EntityChange;
}

export interface Message {
  id?: string;
  role: 'User' | 'Assistant' | 'System' | 'Tool';
  /**
   * How sensitive the message is and for whom (issue 133). Absent only on
   * what the UI shows without the server - a command's answer, a failed turn.
   */
  classification?: Classification;
  sender_id?: ActorId;
  /**
   * A step of a plan under way - its answer, a tool call or result - rather
   * than the answer itself: the chat folds a turn's steps together (issue 258).
   */
  internal?: boolean;
  /**
   * The turn of an agent that produced it (issue 235), as `ResponseStarted`
   * announced it; absent for a user's message.
   */
  turn_id?: string;
  content: string | null;
  /** The model's reasoning (issue 216), if it may be seen here. */
  reasoning?: string;
  /** Who sees `reasoning` (issue 61); absent if the channel does. */
  reasoning_visibility?: Visibility;
  sender_name?: string;
  timestamp?: string;
  tool_calls?: ToolCall[];
  tool_results?: ToolResult[];
  /** Participants addressed by `@`-mentions, resolved by the server. */
  mentions?: ActorId[];
  /** When its author last modified it (issue 234); absent if never. */
  modified?: string;
  /**
   * When its author took it back (issue 234); absent if not. A message taken
   * back keeps its place without what it said.
   */
  revoked?: string;
  // UI only: a turn given up, shown in the chat (issue 258; not yet sent by
  // the server as part of the history), with the `span_id` under which a
  // failure is in the server log.
  givenUp?: GivenUpReason;
  span_id?: string;
  // UI only: the answer to a user command. `command` is its name;
  // `private` if only the issuer sees it. One an agent carried out (issue
  // 269) also names the `agent`, its `args` and, from the history, the
  // `issuer`; `failed` if the agent did not carry it out, `underWay` while
  // it carries it out (issue 299). Part of the history, with what it
  // answered, for whom it was answered (issue 417).
  commandOutput?: {command: string; private: boolean; args?: string; agent?: string; issuer?: string; failed?: boolean; underWay?: boolean};
  // UI only: something an agent did of its own accord (issue 284) - its
  // words, as "compacting its context" -, `underWay` until it ended. One
  // ended is part of the history, with what came of it as the content.
  activity?: {activity: string; agent: string; underWay?: boolean};
  // UI only: someone `joined` or `left` the channel (issue 408) - the
  // content says who and by whose act. Part of the history.
  participant?: 'joined' | 'left';
  // UI only: its sender was removed from the channel before its tool calls
  // were answered (issue 162) - those without a result never will be.
  abandoned?: boolean;
  // UI only: a chain between agents was cut off at its limit (issue 200);
  // `notAsked` are the agents mentioned. Part of the history (issue 417).
  chainCutOff?: {maxAgentHops: number; notAsked: ActorId[]};
  // UI only: `sender_name` asks you to respond (issue 278), part of the
  // history as the request it is (issue 417).
  nudged?: boolean;
  // UI only: the sender would not answer again instead of a message (issue
  // 234). Part of the history (issue 417).
  retryRejected?: RetryRefused;
  // UI only: the channel did not let this message of the user's in, for
  // `rejected` (issue 304) - it is in no history, and gone on reload.
  rejected?: Rejection;
  // UI only: the channel did not let in what the user sent (issue 304).
  // Part of the history, also for a message of theirs (issue 417).
  messageRejected?: {rejected: string; reason: Rejection};
  // UI only: a step of the plan of the turn `turn_id` began here (issue
  // 263) - not shown itself, but heading the messages of the step.
  planStep?: PlanStep;
  // Usage statistics
  prompt_tokens?: number;
  completion_tokens?: number;
  prompt_duration?: number; // ns
  completion_duration?: number; // ns
  total_duration?: number; // ns
  /** How many tokens the context of the call that produced it held (issue 209). */
  context_size?: number;
  [key: string]: unknown;
}

/** A step of a plan, as the steps of an agent's turn are numbered (issue 263). */
export interface PlanStep {
  step_index: number;
  total_steps: number;
  description: string;
}

export interface PlanStepEvent extends PlanStep {
  turn_id: string;
}

/**
 * Events without a payload. The server sends them as a bare string
 * (`"WakeupRequested"`), not as an object like the other events. A user is
 * sent `ResponseRequested` only when someone nudges them (issue 278).
 */
export type UnitActorEvent = 'ResponseRequested' | 'WakeupRequested';

export interface ActorEvent {
  /**
   * An answer of the sender begins: its turn (issue 235), whose messages
   * carry the same `turn_id`.
   */
  ResponseStarted?: { turn_id: string };
  /** The sender, an agent, now stands at this state (issue 243). */
  StateChanged?: ActorState;
  PartialMessageGenerated?: Message;
  CompleteMessageGenerated?: Message;
  ApprovalRequested?: ApprovalRequest;
  ApprovalGranted?: ApprovalResponse;
  ApprovalDenied?: ApprovalResponse;
  ApprovalTimedOut?: ApprovalResponse;
  /**
   * The agent no longer waits for the decision on request `id`: its turn
   * ended first (issue 148). Nobody is asked any longer.
   */
  ApprovalRequestWithdrawn?: { id: string };
  /** An agent's call asks questions of users of the channel (issue 165). */
  QuestionRequested?: QuestionRequest;
  /** The agent no longer waits for the answers to questions `id`: answered, declined, or its turn ended. */
  QuestionClosed?: { id: string; shown_to?: ActorId[] | null };
  /**
   * The turn `turn_id` ended with its answer, `messages` (issue 258), each
   * said before as a `CompleteMessageGenerated`. A message said outside of
   * any turn - a user's - has no end.
   */
  TurnFinished?: { turn_id: string; messages: Message[] };
  IntermediatePartialGenerated?: Message;
  IntermediateMessageGenerated?: Message;
  PlanStepStarted?: PlanStepEvent;
  PlanStepFinished?: PlanStepEvent;
  /** An agent's turn was aborted by an error; the agent is idle again. */
  /** An agent gave its turn up (issues 145, 258); one whose session merely stops keeps it and sends none (issue 377). */
  TurnGivenUp?: { turn_id: string; reason: GivenUpReason };
  /**
   * A command was carried out (issues 28, 269): by the session, sent by the
   * user who ran it when its answer concerns the whole channel; or by an
   * agent, the sender, in its turn `turn_id`, for the issuer alone (`to`) or
   * for everybody.
   */
  CommandExecuted?: { command: string; args: string; output: string; to: ActorId | null; turn_id: string | null };
  /**
   * The agent began to carry out a command (issue 299), in its turn
   * `turn_id`, whose answer ends it; told to the issuer alone.
   */
  CommandStarted?: { command: string; args: string; to: ActorId; turn_id: string };
  /**
   * The agent did not carry out the command (issue 269); shown to the issuer
   * alone. `turn_id` is the turn it had begun in, if it said so.
   */
  CommandFailed?: { command: string; reason: string; to: ActorId; turn_id: string | null };
  ActivityStarted?: { turn_id: string; activity: string };
  ActivityFinished?: { turn_id: string; activity: string; output: string };
  /**
   * `participant`, called `name`, joined the channel (issue 408), by the act
   * of the envelope's sender: themselves, someone who added them, or the
   * system. Part of the history.
   */
  ParticipantJoined?: { participant: ActorId; name: string };
  /**
   * `participant`, called `name`, left the channel (issues 162, 408), by the
   * act of the envelope's sender, as `ParticipantJoined`; nothing of it
   * follows, so whatever of it was under way has ended. Part of the history.
   */
  ParticipantLeft?: { participant: ActorId; name: string };
  /**
   * `participant` was given the history from the position `from` on as it
   * joined (issue 417), by the act of the envelope's sender; sent to the
   * newcomer alone, right before `ParticipantJoined`. Not shown.
   */
  HistoryShared?: { participant: ActorId; from: number };
  /**
   * A chain between agents reached its limit (issue 200): the response ending
   * in `message` was delivered, but nobody was asked to respond to it -
   * `not_asked` are the agents it mentioned. Sent in the name of the system.
   */
  ChainCutOff?: { message: string; max_agent_hops: number; not_asked: ActorId[] };
  /**
   * The message `id` reads as this from now on (issue 234): its author
   * modified it; `timestamp` is when.
   */
  MessageModified?: Message;
  /** Its author took the message `message` back at `at` (issue 234). */
  MessageRevoked?: { message: string; at: string };
  /**
   * The message `message` is classified `classification` from now on
   * (issues 133, 239): reclassified by its author or the channel's owner.
   */
  MessageReclassified?: { message: string; classification: Classification };
  /** The sender answers again instead of `target` (issues 234, 235). */
  RetryAccepted?: { target: RetryTarget };
  /** The sender does not answer again instead of `target`, for `reason` (issues 234, 235). */
  RetryRejected?: { target: RetryTarget; reason: RetryRefused };
  /**
   * The channel did not let in the user's own event of the kind `rejected`
   * (issue 304), about the message `message` if it was one, for `reason`.
   * Sent by the channel to its sender alone, never part of the history.
   */
  MessageRejected?: { rejected: string; message: string | null; reason: Rejection };
}

/**
 * Why the channel did not let in what a user sent (issue 304): a known few,
 * which the UI words itself; one it does not know is shown as such.
 */
export type Rejection = 'forbidden' | 'classification_refused' | 'message_not_found' | 'message_unchangeable' | 'invalid';

/**
 * What a request to answer again is about (issue 235): a message of the
 * agent's, or its answer under way, by its turn.
 */
export type RetryTarget = { message: string } | { turn: string };

/**
 * Why an agent does not answer again (issues 234, 235): a known few, which
 * the UI words itself; one it does not know is shown as such.
 */
export type RetryRefused = 'not_its_answer' | 'not_last' | 'calls_ran' | 'asked_again' | 'elsewhere' | 'ended';

/** A user command, as `/help` and completion show it (issue 28). */
export interface Command {
  name: string;
  description: string;
  usage: string | null;
  /** Of an agent's command (issue 269): whether it is for the agent's owners alone. */
  owners_only?: boolean;
}

/**
 * What a command answered; `output` only if it was for the caller alone.
 * `span_id` is set for a command sent to agents (issue 269), whose answers
 * come on the stream - `output` then names those that were not asked.
 */
export interface CommandReply {
  command: string;
  output: string | null;
  span_id?: string;
}

export interface EventContext {
  session_id: string;
  channel_id: string;
  sender_id: ActorId;
  sender_name: string;
  event_id: string;
  correlation_id: string;
  span_id: string;
  span_depth: number;
  /**
   * The position in the channel's history of the entry the event was
   * delivered as (issue 417): the `seq` of the item it is read as later.
   * Absent for what the history does not record.
   */
  position?: number;
}

export interface Envelope {
  timestamp: string;
  event: ActorEvent | UnitActorEvent;
  context: EventContext;
  security: unknown;
}

/** A compartment a classification may name, as `GET /compartments` offers it (issue 133). */
export interface Compartment {
  name: string;
}

/** A session template, as `GET /templates` offers it. */
export interface SessionTemplate {
  id: string;
  name: string;
  description: string;
  /** The workspace its sessions have unless another is chosen (issue 266); absent if it names none. */
  workspace?: TemplateWorkspace;
}

/** The workspace a template names: found by its name where it lies, or made there. */
export interface TemplateWorkspace {
  name: string;
  /** `session`: made for every session anew. */
  scope: ScopeKind;
  storage: string;
}

export interface CreateSessionRequest {
  template_id: string;
  name: string;
  /** Its workspace in place of the template's (issue 266); the template's if absent. */
  workspace?: SessionWorkspaceRequest;
}

/** The workspace a new session is given in place of its template's (issue 266). */
export type SessionWorkspaceRequest =
  | {kind: 'none'}
  /** One of the user's own, shared with the session's members as far as `share` says (issue 23). */
  | {kind: 'existing'; workspace_id: string; share?: Grant}
  /** Made for the session alone and named as it; `internal` unless classified. */
  | {kind: 'new'; storage: string; classification?: Classification};

/** A channel to create in a running session (issue 09). */
export interface CreateChannelRequest {
  name: string;
  /** The kind of the moderator; the server's default if absent (issue 174). */
  moderator?: string;
  /** The kind of the policy in the moderator's slot; the server's default if absent. */
  responder_policy?: string;
  /** Participants of the session; you take part anyway. */
  participants: ActorId[];
  /** The classification of the channel's contents; the server's default if absent. */
  classification?: Classification;
  /** The language its conversation is held in (issue 56); none in particular if absent (issue 331). */
  language?: string;
  /** Whether its participants work on the session's workspace in it (issue 253); yes if absent. */
  uses_workspace?: boolean;
}

/**
 * How much of a channel's history a participant added later may see
 * (issue 46): all of it, none, the last `count` messages or days.
 */
export type HistoryAccess =
  | {kind: 'all'}
  | {kind: 'none'}
  | {kind: 'last_messages'; count: number}
  | {kind: 'last_days'; count: number};

export interface AddParticipantRequest {
  /** A user or agent; one not in the session yet is taken into it, too (issue 221). */
  participant: ActorId;
  history: HistoryAccess;
}

/** Why someone cannot be taken into a session or channel (issue 221) - coarse, never which clearance is missing. */
export type CandidateRefusal = 'not_cleared' | 'out_of_scope' | 'cannot_run';

/** Someone found by name to take into a session (issue 221). */
export interface Candidate {
  id: ActorId;
  name: string;
  kind: ParticipantKind;
  in_session: boolean;
  /** In the channel asked about. */
  in_channel: boolean;
  /** Why they cannot be taken in; absent if they can. */
  refusal?: CandidateRefusal;
}

export type ApprovalStatus = 'Approved' | 'Denied';

/** An answer offered to a question (issue 165). */
export interface Choice {
  label: string;
  description?: string;
  /** The asker recommends it. */
  recommended?: boolean;
}

/** One question: answered with text without `choices`, else by choosing - several with `multi_select` - or writing another answer. */
export interface Question {
  /** A short label, at most 12 characters. */
  header: string;
  question: string;
  choices?: Choice[];
  multi_select?: boolean;
}

/** Whom questions are put to; as the server sends them, every one found among the users: `{actor}`. */
export type Addressee = 'owner' | { handle: string } | { actor: ActorId };

/** What a call asks (issue 165). */
export interface Questions {
  questions: Question[];
  ask: Addressee[];
  /** The answers are for the tool alone: masked, shown to those asked alone. */
  sensitive?: boolean;
}

/** The response to one question, in the order of the questions (`Response` on the server). */
export type QuestionResponse =
  | { kind: 'chosen'; chosen?: string[]; other?: string }
  | { kind: 'text'; text: string }
  | { kind: 'declined' };

/** The answers to a call's questions, with the questions and who gave them. */
export interface Answers {
  questions: Questions;
  answered_by: ActorId;
  answered_by_name: string;
  responses: QuestionResponse[];
}

/** Questions an agent's call waits for the answers to (issue 165). */
export interface QuestionRequest {
  id: string;
  session_id: string;
  channel_id: string;
  agent_id: string;
  tool_call: ToolCall;
  questions: Questions;
}

export interface ApprovalRequest {
  id: string;
  session_id: string;
  channel_id: string;
  agent_id: string;
  tool_call: ToolCall;
  description: string;
  /** A rule of the session or channel asked for this very call to be decided by a person (issue 13); left out when not. */
  explicit?: boolean;
  /** What other calls are like this one, as its tool says (issue 13): what a rule derived from it is offered with; left out where no rule can be (issue 400). */
  suggested?: ToolMatcher;
}

/** What calls a rule is about: the tool by name - `*` for every tool - and, per argument, how its value is compared. */
export interface ToolMatcher {
  tool: string;
  arguments?: Record<string, ArgumentMatcher>;
}

/**
 * How the value of one argument is compared (issue 247): a pattern - a regular expression its whole value must
 * match -, for a bash command line the words each of its simple commands must begin with, or the whole value as it
 * is, for one too long for a pattern (issue 400).
 */
export type ArgumentMatcher = string | ShellWords | ExactValue;

/** The words each simple command of a bash command line must begin with, as they run. */
export interface ShellWords {
  shell: string[];
}

/** The whole value as it is: a string as itself, any other value as its compact JSON. */
export interface ExactValue {
  equals: string;
}

/** What a rule decides of the calls it is about (issue 13). A session or channel only asks or denies. */
export type RuleDecision = 'allow' | 'ask' | 'deny';

/** Where a rule applies within the place it is kept; `everywhere` when left out. */
export type Reach = 'everywhere' | {session: string} | {channel: string};

/** A standing decision on tool calls (issue 13): the calls its matcher is about, what it decides of them, where and how long. */
export interface ApprovalRule {
  /** Made by the server when left out. */
  id?: string;
  matcher: ToolMatcher;
  decision: RuleDecision;
  reach?: Reach;
  /** When it stops applying, an RFC 3339 time; never when left out. */
  expires_at?: string;
}

export interface ApprovalResponse {
  id: string;
  agent_id: string;
  /** The rule of yours that gave the answer (issue 13); only on your own answers, as your actor shows them to you. */
  by_rule?: string;
}

/** A prompt part the model was given besides the conversation (issue 47). */
export interface SeenPrompt {
  kind: 'prompt';
  digest: string;
  role: Message['role'];
  text: string;
  /** How the part was classified; null if not known - it counts as the channel (issue 168). */
  classification: Classification | null;
  /** Whether it set the label of the message (issue 170). */
  sets_label: boolean;
}

/** A message of a context, with whether it - or a tool result in it - set the label of the message (issue 170). */
export interface SeenMessage {
  kind: 'message';
  message: Message;
  sets_label: boolean;
}

/**
 * A message of an agent's history (issue 207): offered to it or said by it,
 * whether its context can be shown, and whether it was passed on to the
 * agent's transcript - not if its perceive processors dropped it.
 */
export interface HistoryMessage {
  message: Message;
  has_context: boolean;
  in_transcript: boolean;
}

/** Why an agent gave up a turn (issue 145). */
export type GivenUpReason =
  | {kind: 'failed'; message: string}
  | {kind: 'lowered'; channel: Classification}
  | {kind: 'left'}
  | {kind: 'retried'}
  | {kind: 'call_limit'; calls: number};

/**
 * An item of a history (issue 258): a message said - `M`, a `Message` of the
 * channel or a `HistoryMessage` of an agent - or how a turn ended, and whose
 * it was; a notice its reader was given (issue 417). `seq` is the position
 * of the entry that added it: what a live event that announced it carries
 * as its `position`.
 */
export type HistoryItem<M> = {seq: number} & HistoryItemKind<M>;

/** What an item of a history is (`HistoryItem`), named by its kind. */
export type HistoryItemKind<M> =
  | {said: M}
  | {plan_step_started: {turn_id: string; actor: ActorId; step_index: number; total_steps: number; description: string; at: string}}
  | {turn_finished: {turn_id: string; actor: ActorId; at: string}}
  | {turn_given_up: {turn_id: string; actor: ActorId; reason: GivenUpReason; at: string}}
  /**
   * A command was carried out (issues 269, 417): by an agent in its turn, or
   * by the session without one; what it answered, to `to` alone if private.
   */
  | {command_executed: {turn_id: string | null; actor: ActorId; issuer: ActorId; command: string; args: string; output: string; to: ActorId | null; at: string}}
  /** An agent ended something it did of its own accord, and what came of it (issue 284). */
  | {activity_finished: {turn_id: string; actor: ActorId; activity: string; output: string; at: string}}
  /** An agent replaced its context `from` by `to` (issue 16); in an agent's history only. */
  | {compacted: {turn_id: string; actor: ActorId; from: string; to: string; at: string}}
  /** Someone joined the channel, by the act of `by` (issue 408); the names as they were then. */
  | {participant_joined: {participant: ActorId; name: string; by: ActorId; by_name: string; at: string}}
  /** Someone left the channel, by the act of `by` (issue 408). */
  | {participant_left: {participant: ActorId; name: string; by: ActorId; by_name: string; at: string}}
  /** An agent did not carry out a command of `to`'s (issue 269). */
  | {command_failed: {turn_id: string | null; actor: ActorId; command: string; reason: string; to: ActorId; at: string}}
  /** An agent answers again instead of `target` (issue 234). */
  | {retry_accepted: {target: RetryTarget; actor: ActorId; at: string}}
  /** An agent does not answer again instead of `target` (issue 234). */
  | {retry_rejected: {target: RetryTarget; reason: RetryRefused; actor: ActorId; at: string}}
  /** A chain between agents reached its limit (issue 200). */
  | {chain_cut_off: {message: string; max_agent_hops: number; not_asked: ActorId[]; at: string}}
  /** What the reader sent was not let into the channel (issue 304). */
  | {message_rejected: {rejected: string; message: string | null; reason: Rejection; at: string}}
  /** The reader was asked to respond, to what `by` said or nudged (issues 278, 417). */
  | {response_requested: {by: ActorId; at: string}};

/**
 * A page of a history (issue 218): its items, oldest first, and `earlier`,
 * what to read the page before it with (`before`) - `null` if there is none.
 */
export interface HistoryPage<M> {
  items: HistoryItem<M>[];
  earlier: number | null;
}

/** What an agent's model saw when it produced a message (issues 06, 47). */
export interface MessageContextResponse {
  message_id: string;
  actor_id: ActorId;
  transcript_digest: string | null;
  /** The messages of the context and the prompt parts, in the order the model was given them. */
  items: (SeenPrompt | SeenMessage)[];
  /** What was read from notes and histories before the message was produced (issue 17). */
  knowledge: KnowledgeRef[];
  /** The digest of the tool sets the model was offered, null if none (issue 137). */
  tools_digest: string | null;
  /** The tools the model was offered, as it was told of them, in their sets (issue 194). */
  tool_sets: ToolSet[];
}

/**
 * The tools of one source of an agent - a component, e.g. an MCP server - with what they are for
 * (issue 194). A record made before has one set without a name.
 */
export interface ToolSet {
  /** The component the tools come from; empty in a record made before tool sets. */
  name: string;
  description: string;
  tools: Tool[];
  /** Why the source gives no tools now; absent if it does. */
  unavailable?: string;
  /** Where the arguments of its calls go (issue 180); absent if they stay in the system. */
  destination?: Classification;
}

/**
 * A tool of an agent (issues 137, 194): what its model is told of it - name, description, parameters -
 * and what only the runtime acts on.
 */
export interface Tool {
  name: string;
  description: string;
  /** Its parameters, as the model is told of them (`ToolParameters`). */
  parameters: unknown;
  requires_approval: boolean;
  /** Absent: its calls are shown to the channel. */
  visibility?: ToolVisibility;
  /** How its results are classified where they do not say; absent: as the channel. */
  classification?: Classification;
  /** How its calls are shown; absent: `default`. */
  presentation?: ToolPresentation;
}

/** The tools an agent has now in a channel (issue 194); none where it has none here - no agent run here, left, or the channel does not run. */
export interface AgentToolsResponse {
  tool_sets: ToolSet[];
}

/** Knowledge a message was produced with (issue 17). */
export interface KnowledgeRef {
  /** The session of the channel: another one's, for what an agent recalled from its memory (issue 134). */
  session_id: string;
  channel_id: string;
  /** Null for a channel that is gone, or of another session. */
  channel_name: string | null;
  /** Whether the channel was deleted (issue 164) - told only to whoever could read it were it there. */
  channel_deleted: boolean;
  /**
   * What was read: `notes`, `private_notes`, `history`, `memory`, or an entity
   * by its kind (issue 323) - a `workspace`'s files (issue 251), a `skill`
   * loaded (issue 22), or a kind of a plugin.
   */
  source: string;
  /** The entity read, for a kind. */
  entity_id?: string;
  /** Whose notes or memory, for `notes`, `private_notes` and `memory`. */
  agent_id: ActorId | null;
  agent_name: string | null;
  digest: string | null;
  /**
   * For `history`: the position the reader's view began at if they joined
   * later with a limit (issues 72, 417); absent means all of it.
   */
  history_start?: number;
  /** How what was read was classified when it was read; null if not known - it counts as the channel (issue 168). */
  classification: Classification | null;
  /** Whether it set the label of the message (issue 170). */
  sets_label: boolean;
}

