export interface SessionId {
  0: string;
}

export interface ChannelId {
  0: string;
}

export interface UserResponse {
  id: string;
  username: string;
}

export interface SessionResponse {
  id: string;
  name: string;
}

/** Serialized `ActorId`: `"None"` or an object like `{ "Agent": "<uuid>" }`. */
export type ActorId = string | Record<string, string>;

/** A participant of a channel. */
export interface ChannelMember {
  id: ActorId;
  name: string;
  /** The handle it is `@`-mentioned by, or null if it cannot be mentioned. */
  handle: string | null;
  kind: string;
}

export interface ChannelResponse {
  id: string;
  name: string;
  /** All participants, in no particular order. */
  members: ChannelMember[];
}

export interface ToolCall {
  id?: string;
  function_name: string;
  arguments: unknown;
}

/** How a tool call ended; absent means `ok`. */
export type ToolResultStatus =
  | 'ok' | 'denied' | 'forbidden' | 'timed_out' | 'invalid_arguments' | 'failed' | 'unknown_tool';

export interface ToolResult {
  id?: string;
  tool_name: string;
  /** The tool's result, or for any other status a JSON object with `status` and `message`. */
  content: string;
  status?: ToolResultStatus;
}

export interface Message {
  id?: string;
  role: 'User' | 'Assistant' | 'System' | 'Tool';
  content: string | null;
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
  // Usage statistics
  prompt_tokens?: number;
  completion_tokens?: number;
  prompt_duration?: number; // ns
  completion_duration?: number; // ns
  total_duration?: number; // ns
  [key: string]: any;
}

export interface PlanStepEvent {
  step_index: number;
  total_steps: number;
  description: string;
}

export interface ActorEvent {
  ResponseStarted?: any;
  PartialMessageGenerated?: Message;
  CompleteMessageGenerated?: Message;
  ApprovalRequested?: ApprovalRequest;
  ApprovalGranted?: ApprovalResponse;
  ApprovalDenied?: ApprovalResponse;
  ResponseRequested?: any;
  FullResponseGenerated?: Message[];
  WakeupRequested?: any;
  IntermediatePartialGenerated?: Message;
  IntermediateMessageGenerated?: Message;
  PlanStepStarted?: PlanStepEvent;
  PlanStepFinished?: PlanStepEvent;
  /** An agent's turn was aborted by an error; the agent is idle again. */
  TurnFailed?: { message: string };
}

export interface EventContext {
  session_id: string;
  channel_id: string;
  sender_id: any;
  sender_name: string;
  event_id: string;
  correlation_id: string;
  span_id: string;
  span_depth: number;
}

export interface Envelope {
  timestamp: string;
  event: ActorEvent;
  context: EventContext;
  security: any;
}

export interface SessionTemplate {
  id: string;
  metadata: {
    name: string;
    description?: string;
  };
}

export interface CreateSessionRequest {
  template_id: string;
  name: string;
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
