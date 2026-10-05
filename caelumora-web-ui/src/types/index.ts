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
}

export interface SessionResponse {
  id: string;
  name: string;
  /** Whom the session lets read what was said in it (issue 134). */
  privacy: PrivacyPolicy;
}

/** Serialized `ActorId`: `"None"` or an object like `{ "Agent": "<uuid>" }`. */
export type ActorId = string | Record<string, string>;

/** What kind of participant an actor is. */
export type ParticipantKind = 'user' | 'agent' | 'approver';

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
  last_call_tokens: number;
  /** Absent if not known. */
  context_size?: number;
}

export interface ChannelMember {
  id: ActorId;
  name: string;
  /** The handle it is `@`-mentioned by, or null if it cannot be mentioned. */
  handle: string | null;
  kind: ParticipantKind;
  /** Whether you may see what this participant saw when it wrote a message (issue 41). */
  can_read_context: boolean;
  /**
   * What its model calls used (issue 209): sent with the channel stream's
   * snapshot and by `getParticipant`, not where channels are listed; absent
   * for a member whose calls report no tokens.
   */
  usage?: TokenUsage;
}

export interface ChannelResponse {
  id: string;
  name: string;
  /** All participants, in no particular order. */
  members: ChannelMember[];
  /** The kind of the moderator, e.g. `policy_moderator`; `null` if preconfigured (issue 174). */
  moderator: string | null;
  /** The kind of the policy in the moderator's slot - who answers - e.g. `round_robin_policy` or `mentions_only_policy` (issues 44, 174); `null` if preconfigured. */
  responder_policy: string | null;
  /** The classification of the channel's contents (issue 133). */
  classification: Classification;
  /** Whether you may rename it and add or remove participants (issue 46). */
  can_change: boolean;
}

/** Who sees a part of what an agent produces (issue 133): the channel, or only the agent's owner. */
export type Visibility = 'channel' | 'owner';

/**
 * Who sees a tool call (issues 54, 133): the call itself, and its arguments
 * and result. Absent means both `channel`.
 */
export interface ToolVisibility {
  call: Visibility;
  content: Visibility;
}

export interface ToolCall {
  id?: string;
  function_name: string;
  /** `null` where the call was published without its content. */
  arguments: unknown;
  visibility?: ToolVisibility;
}

/** How a tool call ended; absent means `ok`. */
export type ToolResultStatus =
  | 'ok' | 'denied' | 'forbidden' | 'timed_out' | 'invalid_arguments' | 'failed' | 'not_executed' | 'interrupted' | 'unknown_tool' | 'withheld';

export interface ToolResult {
  id?: string;
  tool_name: string;
  /** The tool's result, or for any other status a JSON object with `status` and `message`. */
  content: string;
  status?: ToolResultStatus;
  /** Empty `content` where the call was published without its content. */
  visibility?: ToolVisibility;
  /** How sensitive `content` is and for whom (issue 133). */
  classification: Classification;
  /** When the tool started and finished running; absent for a call that did not run. */
  started_at?: string;
  finished_at?: string;
}

/** The state of a channel when an event stream connects (issue 34). */
export interface ChannelSnapshot {
  channel: ChannelResponse;
  messages: Message[];
  /** Whether the history is longer than the snapshot; the rest comes over REST. */
  has_more: boolean;
  /** The notes every agent of the channel published (issue 17). */
  notes: PublishedNotes[];
  /** The approvals the channel's agents wait for (issue 172): the stream tells only of requests made while it listens. */
  pending_approvals?: ApprovalRequest[];
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
  /** Whom the session lets read what was said in it (issue 134). */
  privacy: PrivacyPolicy;
  /** Whether you may change it - rename it, change its privacy. */
  can_change: boolean;
  channels: ChannelResponse[];
  /** Everyone who takes part in the session - whom a new channel can be made of (issue 09). */
  participants: SessionParticipant[];
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
 * An event of the global stream, serialized as `{ "<Variant>": value }`: the
 * value is the session's id, or an array starting with it, e.g.
 * `{"SessionUpdated": ["<id>", {"NameChanged": "new name"}]}`.
 */
export type GlobalEvent = Record<string, unknown>;

export interface Message {
  id?: string;
  role: 'User' | 'Assistant' | 'System' | 'Tool';
  /**
   * How sensitive the message is and for whom (issue 133). Absent only on
   * what the UI shows without the server - a command's answer, a failed turn.
   */
  classification?: Classification;
  sender_id?: ActorId;
  /** Internal scaffolding (deliberation, plan steps, mid-plan tool steps): never shown in the chat. */
  internal?: boolean;
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
  // UI only: a failed turn shown in the chat (never sent by the server as a
  // message), with the `span_id` under which the error is in the server log.
  turnFailed?: boolean;
  span_id?: string;
  // UI only: the answer to a user command (never part of the history).
  // `command` is its name; `private` if only the issuer sees it.
  commandOutput?: {command: string; private: boolean};
  // UI only: its sender was removed from the channel before its tool calls
  // were answered (issue 162) - those without a result never will be.
  abandoned?: boolean;
  // UI only: a chain between agents was cut off at its limit (issue 200),
  // a notice that is gone on reload; `notAsked` are the agents mentioned.
  chainCutOff?: {maxAgentHops: number; notAsked: ActorId[]};
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

export interface PlanStepEvent {
  step_index: number;
  total_steps: number;
  description: string;
}

/**
 * Events without a payload. The server sends them as a bare string
 * (`"ResponseStarted"`), not as an object like the other events.
 */
export type UnitActorEvent = 'ResponseRequested' | 'ResponseStarted' | 'WakeupRequested';

export interface ActorEvent {
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
  FullResponseGenerated?: Message[];
  IntermediatePartialGenerated?: Message;
  IntermediateMessageGenerated?: Message;
  PlanStepStarted?: PlanStepEvent;
  PlanStepFinished?: PlanStepEvent;
  /** An agent's turn was aborted by an error; the agent is idle again. */
  TurnFailed?: { message: string };
  /** A user ran a command whose answer concerns the whole channel (issue 28). */
  CommandExecuted?: { command: string; output: string };
  /**
   * `participant` was removed from the channel by the envelope's sender
   * (issue 162); nothing of it follows, so whatever of it was under way has
   * ended.
   */
  ParticipantRemoved?: { participant: ActorId };
  /**
   * A chain between agents reached its limit (issue 200): the response ending
   * in `message` was delivered, but nobody was asked to respond to it -
   * `not_asked` are the agents it mentioned. Sent in the name of the system.
   */
  ChainCutOff?: { message: string; max_agent_hops: number; not_asked: ActorId[] };
}

/** A user command, as `/help` and completion show it (issue 28). */
export interface Command {
  name: string;
  description: string;
  usage: string | null;
}

/** What a command answered; `output` only if it was for the caller alone. */
export interface CommandReply {
  command: string;
  output: string | null;
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
}

export interface CreateSessionRequest {
  template_id: string;
  name: string;
}

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
  /** A participant of the session. */
  participant: ActorId;
  history: HistoryAccess;
}

export type ApprovalStatus = 'Approved' | 'Denied';

export interface ApprovalRequest {
  id: string;
  session_id: string;
  channel_id: string;
  agent_id: string;
  tool_call: ToolCall;
  description: string;
}

export interface ApprovalResponse {
  id: string;
  agent_id: string;
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
  | {kind: 'stopped'};

/**
 * A turn an agent gave up (issues 145, 146): why and when, the message of
 * its history it branched off after (null if it began before anything was
 * said), and its messages, which the agent's history no longer holds.
 */
export interface GivenUpTurn {
  reason: GivenUpReason;
  at: string;
  after: string | null;
  messages: HistoryMessage[];
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
  /** The digest of the set of tools the model was offered, null if none (issue 137). */
  tools_digest: string | null;
  /** The tools the model was offered, as it was told of them. */
  tools: ToolDefinition[];
}

/** A tool as a model is offered it (issue 137). */
export interface ToolDefinition {
  name: string;
  description: string;
  /** Its parameters, as the model was told of them (`ToolParameters`). */
  parameters: unknown;
}

/** Knowledge a message was produced with (issue 17). */
export interface KnowledgeRef {
  /** The session of the channel: another one's, for what an agent recalled from its memory (issue 134). */
  session_id: string;
  channel_id: string;
  /** Null for a channel that is gone, or of another session. */
  channel_name: string | null;
  source: 'notes' | 'private_notes' | 'history' | 'memory';
  /** Whose notes or memory, for `notes`, `private_notes` and `memory`. */
  agent_id: ActorId | null;
  agent_name: string | null;
  digest: string | null;
  /**
   * For `history`: where the reader's view began if they joined later with a
   * limit (issue 72) - from or after that message; absent means all of it.
   */
  history_start?: {from: string} | {after: string};
  /** How what was read was classified when it was read; null if not known - it counts as the channel (issue 168). */
  classification: Classification | null;
  /** Whether it set the label of the message (issue 170). */
  sets_label: boolean;
}

