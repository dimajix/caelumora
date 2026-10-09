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
  /** The language you prefer, a BCP 47 tag such as `de` (issue 56); your new channels are held in it. */
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

/** Where an agent stands (issue 243), as `StateChanged` and a member's `state` name it. */
export type ActorState = 'Idle' | 'Sleeping' | 'Planning' | 'ExecutingTools' | 'WaitingForUser' | 'Reflecting' | 'Cancelling';

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
  /** Whether you may see which tools it has now - only ever an agent (issue 194). */
  can_read_tools: boolean;
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
   * The turn of the answer it is giving now (issue 240), as `ResponseStarted`
   * named it; absent if none - also for an external agent.
   */
  turn_under_way?: string;
  /**
   * What its model calls used (issue 209): sent with the channel stream's
   * snapshot and by `getParticipant`, not where channels are listed; absent
   * for a member whose calls report no tokens.
   */
  usage?: TokenUsage;
  /** For a former member (issue 164): who removed it and when. */
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
  members: ChannelMember[];
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
  /** Whether its members work on the session's workspace here (issue 251), where the classifications let them. */
  uses_workspace: boolean;
  /** Whether you may rename it, add or remove participants (issue 46) and set its approval rules. */
  can_change: boolean;
  /** Who took part and left (issue 164), in the order they left, each with `left`. */
  former_members: ChannelMember[];
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

/**
 * How a tool's calls and their results are shown (issue 165), declared by the tool and stamped on
 * each call; absent means `default`. Display only.
 */
export type ToolPresentation = 'default' | 'shell' | 'plan' | 'file_read' | 'file_write' | 'file_edit' | 'file_list' | 'file_search' | 'file_move';

export interface ToolCall {
  id?: string;
  function_name: string;
  /** `null` where the call was published without its content. */
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
  /** Empty `content` where the call was published without its content. */
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
  /** The end of its history: messages, and how turns ended (issue 258). */
  items: HistoryItem<Message>[];
  /** Whether the history is longer than the snapshot; the rest comes over REST. */
  has_more: boolean;
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

/** Where a workspace lies (issue 251): made for one session, the owner's, or global. */
export type WorkspaceScope = 'session' | 'user' | 'global';

/** A workspace (issue 253), as its owner or a member of a channel using it sees it. */
export interface WorkspaceResponse {
  id: string;
  name: string;
  scope: WorkspaceScope;
  /** The session it was made for, if it lies in a session's scope. */
  session_id?: string;
  /** The storage its files lie in, by its configured name. */
  storage: string;
  description?: string;
  classification: Classification;
  /** Whether its storage gives it a shell: agents can run commands in it. */
  has_shell: boolean;
  /** Whether you may change it and attach it to a session: its owner. */
  can_change: boolean;
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
  // UI only: the answer to a user command (never part of the history).
  // `command` is its name; `private` if only the issuer sees it.
  commandOutput?: {command: string; private: boolean};
  // UI only: its sender was removed from the channel before its tool calls
  // were answered (issue 162) - those without a result never will be.
  abandoned?: boolean;
  // UI only: a chain between agents was cut off at its limit (issue 200),
  // a notice that is gone on reload; `notAsked` are the agents mentioned.
  chainCutOff?: {maxAgentHops: number; notAsked: ActorId[]};
  // UI only: the sender would not answer again instead of a message (issue
  // 234), a notice that is gone on reload.
  retryRejected?: RetryRefused;
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
 * (`"WakeupRequested"`), not as an object like the other events.
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
  /** An agent gave its turn up (issues 145, 258); a stopped agent sends none. */
  TurnGivenUp?: { turn_id: string; reason: GivenUpReason };
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
}

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
  /** The workspace its sessions have unless another is chosen (issue 266); absent if it names none. */
  workspace?: TemplateWorkspace;
}

/** The workspace a template names: found by its name where it lies, or made there. */
export interface TemplateWorkspace {
  name: string;
  /** `session`: made for every session anew. */
  scope: WorkspaceScope;
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
  | {kind: 'existing'; workspace_id: string}
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
  /** The language its conversation is held in (issue 56); the one you prefer if absent. */
  language?: string;
  /** Whether its members work on the session's workspace in it (issue 253); yes if absent. */
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
  /** A participant of the session. */
  participant: ActorId;
  history: HistoryAccess;
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
  /** What other calls are like this one, as its tool says (issue 13): what a rule derived from it is offered with. */
  suggested?: ToolMatcher;
}

/** What calls a rule is about: the tool by name - `*` for every tool - and, per argument, a pattern its whole value must match. */
export interface ToolMatcher {
  tool: string;
  arguments?: Record<string, string>;
}

/** What a rule decides of the calls it is about (issue 13). A session or channel only asks or denies. */
export type RuleDecision = 'allow' | 'ask' | 'deny';

/** Where a rule applies within the place it is kept; `everywhere` when left out. */
export type Reach = 'everywhere' | {session: string} | {channel: string};

/**
 * A standing decision on tool calls (issue 13): the tool by name - `*` for every tool - and, per
 * argument, a regular expression its whole value must match.
 */
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
  | {kind: 'stopped'}
  | {kind: 'retried'};

/**
 * An item of a history (issue 258): a message said - `M`, a `Message` of the
 * channel or a `HistoryMessage` of an agent - or how a turn ended, and whose
 * it was.
 */
export type HistoryItem<M> =
  | {said: M}
  | {plan_step_started: {turn_id: string; actor: ActorId; step_index: number; total_steps: number; description: string; at: string}}
  | {turn_finished: {turn_id: string; actor: ActorId; at: string}}
  | {turn_given_up: {turn_id: string; actor: ActorId; reason: GivenUpReason; at: string}};

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
  source: 'notes' | 'private_notes' | 'history' | 'memory' | 'workspace';
  /** The workspace whose files were read, for `workspace` (issue 251). */
  workspace_id?: string;
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

