// Helpers for showing messages, shared by the chat and the context panel.

import type {ActorId, GivenUpReason, HistoryItem, Message, PlanStep, TokenUsage, ToolResult, ToolVisibility} from './types';
import {classificationLabel} from './confidentiality';
import {actorKey, handleOf} from './mention';

/** Whether `message` was said by `me` (issue 407); one without a sender is no one's own. */
export function isOwn(message: Message, me: ActorId): boolean {
  return message.sender_id !== undefined && actorKey(message.sender_id) === actorKey(me);
}

export function formatArguments(args: unknown): string {
  return typeof args === 'string' ? args : JSON.stringify(args);
}

/**
 * What of a tool's calls only the agent's owner sees (issues 54, 133, 281) - `call`, `arguments and
 * result`, `arguments` or `result` - or `null` if the channel sees all of it.
 */
export function ownerOnly(visibility: ToolVisibility | undefined): string | null {
  if (!visibility) return null;
  if (visibility.call === 'owner') return 'call';
  const parts = [visibility.arguments === 'owner' ? 'arguments' : null, visibility.result === 'owner' ? 'result' : null];
  const owners = parts.filter(part => part !== null);
  return owners.length > 0 ? owners.join(' and ') : null;
}

/**
 * `messages` with every tool result attached to the message holding the
 * tool call it answers, so that a call and its result are shown together.
 */
export function mergeToolResults(messages: Message[]): Message[] {
  return messages.reduce((acc: Message[], msg) => {
    if (msg.tool_results && msg.tool_results.length > 0) {
      const lastMsg = acc[acc.length - 1];
      if (lastMsg && lastMsg.tool_calls && lastMsg.tool_calls.some(tc => msg.tool_results?.some(tr => tr.id === tc.id))) {
        const merged = {...lastMsg, tool_results: [...(lastMsg.tool_results || []), ...msg.tool_results]};
        return [...acc.slice(0, -1), merged];
      }
    }
    return [...acc, msg];
  }, []);
}

/** What a shell tool answers of a command (issue 251). */
export interface CommandResult {
  id: string;
  status: 'running' | 'completed' | 'killed' | 'failed';
  output: string;
  truncated: boolean;
  exit_code?: number;
}

/** The result of a successful call of a shell tool, or `null` for any other. */
export function commandResult(result: ToolResult): CommandResult | null {
  if (result.status && result.status !== 'ok') return null;
  try {
    const parsed = JSON.parse(result.content);
    return typeof parsed?.id === 'string' && typeof parsed?.status === 'string' && typeof parsed?.output === 'string' ? parsed : null;
  } catch {
    return null;
  }
}

/** The commands that results among `messages` show ended (issue 294): those of a shell tool's result not `running`. */
export function endedCommands(messages: Message[]): Set<string> {
  const ended = new Set<string>();
  for (const msg of messages) {
    for (const call of msg.tool_calls ?? []) {
      const result = call.presentation === 'shell' ? msg.tool_results?.find(r => r.id === call.id) : undefined;
      const command = result && commandResult(result);
      if (command && command.status !== 'running') ended.add(command.id);
    }
  }
  return ended;
}

/** Why a turn was given up (issues 145, 258), in this page's words. */
export function givenUpText(reason: GivenUpReason): string {
  switch (reason.kind) {
    case 'failed':
      return `It failed: ${reason.message}`;
    case 'lowered':
      return `The channel was lowered to ${classificationLabel(reason.channel)} while it ran.`;
    case 'left':
      return 'The agent left the channel.';
    case 'retried':
      return 'Its answer was asked for again while it was under way.';
  }
}

/**
 * The notice of a turn given up (issues 145, 258), as the chat shows it -
 * live and from the history alike, named for its turn so that it is shown
 * once: whose turn it was, why, and when.
 */
export function givenUpNotice(turnId: string, reason: GivenUpReason, actor: ActorId, name: string | undefined, at: string, spanId?: string): Message {
  return {
    id: `turn-given-up:${turnId}`,
    role: 'System',
    content: givenUpText(reason),
    sender_id: actor,
    sender_name: name,
    turn_id: turnId,
    givenUp: reason,
    span_id: spanId,
    timestamp: at,
  };
}

/**
 * The mark of a step of a plan beginning (issue 263), as the chat keeps it -
 * live and from the history alike, named for its turn and step so that it
 * is kept once: whose turn it is, which step, and when.
 */
export function planStepMark(turnId: string, step: PlanStep, actor: ActorId, name: string | undefined, at: string): Message {
  return {
    id: `plan-step:${turnId}:${step.step_index}`,
    role: 'System',
    content: null,
    sender_id: actor,
    sender_name: name,
    turn_id: turnId,
    planStep: {step_index: step.step_index, total_steps: step.total_steps, description: step.description},
    timestamp: at,
  };
}

/**
 * The notice of a command an agent carried out (issue 269), as the chat
 * shows it - live and from the history alike, named for the turn it was
 * carried out in so that it is shown once: which command, by which agent,
 * when, on whose word where that is known, and - live, to whom it was
 * answered - what it answered, which the history does not keep.
 */
export function commandNotice(turnId: string, command: string, args: string, agent: ActorId, agentName: string | undefined, issuer: string | undefined, at: string, answer?: {output: string; private: boolean}): Message {
  return {
    id: `command:${turnId}`,
    role: 'System',
    content: answer?.output ?? null,
    sender_id: agent,
    sender_name: agentName,
    turn_id: turnId,
    commandOutput: {command, args, private: answer?.private ?? false, agent: agentName ?? 'An agent', issuer},
    timestamp: at,
  };
}

/**
 * The notice of something an agent does of its own accord (issue 284), as
 * the chat shows it: under way, kept by its turn until it ends - one turn
 * may do it more than once -, then ended with what came of it, `output`,
 * named for when it ended so that it is shown once, live and from the
 * history alike.
 */
export function activityNotice(turnId: string, activity: string, agent: ActorId, agentName: string | undefined, at: string, output?: string): Message {
  return {
    id: output === undefined ? activityUnderWay(turnId) : `activity:${turnId}:${at}`,
    role: 'System',
    content: output ?? null,
    sender_id: agent,
    sender_name: agentName,
    turn_id: turnId,
    activity: {activity, agent: agentName ?? 'An agent', underWay: output === undefined},
    timestamp: at,
  };
}

/** The id of the notice of the activity of turn `turnId` under way. */
export function activityUnderWay(turnId: string): string {
  return `activity:${turnId}:under-way`;
}

/** How the chat names someone called `name` in a notice: by handle, as a mention would. */
function named(name: string): string {
  const handle = handleOf(name);
  return handle ? `@${handle}` : name;
}

/**
 * The notice that `participant`, called `name`, joined or left the channel
 * (issue 408), as the chat shows it - live and from the history alike, named
 * for whom and when so that it is shown once: by their own act, the system's,
 * or that of `by`, called `byName`.
 */
export function participantNotice(change: 'joined' | 'left', participant: ActorId, name: string, by: ActorId, byName: string, at: string): Message {
  const who = named(name);
  const joined = change === 'joined';
  const content = actorKey(by) === actorKey(participant)
    ? `${who} has ${change} the channel`
    : actorKey(by) === 'System'
      ? `${who} was ${joined ? 'added to' : 'removed from'} the channel`
      : `${named(byName)} has ${joined ? 'added' : 'removed'} ${who} ${joined ? 'to' : 'from'} the channel`;
  return {
    id: `participant-${change}:${actorKey(participant)}:${at}`,
    role: 'System',
    content,
    sender_id: by,
    sender_name: byName,
    participant: change,
    timestamp: at,
  };
}

/**
 * The chat's messages for `items` of a channel's history (issue 258): each
 * message said, the mark of each step of a plan begun (issue 263), the
 * notice of a turn given up, that of a command an agent carried out
 * (issue 269), that of what it did of its own accord (issue 284) -
 * whose, as `nameOf` names them - and that of who joined or left (issue
 * 408), named as they were then. A turn that finished shows nothing of its
 * own.
 */
export function chatMessages(items: HistoryItem<Message>[], nameOf: (actor: ActorId) => string | undefined): Message[] {
  return items.flatMap((item): Message[] => {
    if ('said' in item) return [item.said];
    if ('plan_step_started' in item) {
      const {turn_id, actor, at, ...step} = item.plan_step_started;
      return [planStepMark(turn_id, step, actor, nameOf(actor), at)];
    }
    if ('turn_given_up' in item) {
      const {turn_id, actor, reason, at} = item.turn_given_up;
      return [givenUpNotice(turn_id, reason, actor, nameOf(actor), at)];
    }
    if ('command_executed' in item) {
      const {turn_id, actor, issuer, command, args, at} = item.command_executed;
      return [commandNotice(turn_id, command, args, actor, nameOf(actor), nameOf(issuer), at)];
    }
    if ('activity_finished' in item) {
      const {turn_id, actor, activity, output, at} = item.activity_finished;
      return [activityNotice(turn_id, activity, actor, nameOf(actor), at, output)];
    }
    if ('participant_joined' in item) {
      const {participant, name, by, by_name, at} = item.participant_joined;
      return [participantNotice('joined', participant, name, by, by_name, at)];
    }
    if ('participant_left' in item) {
      const {participant, name, by, by_name, at} = item.participant_left;
      return [participantNotice('left', participant, name, by, by_name, at)];
    }
    return [];
  });
}

/**
 * A row of the chat: a message, or a step of a plan (issues 258, 263) - the
 * internal messages of one step of a turn, shown together under the step
 * and folded, with the name of whose turn it is. `step` is `null` where it
 * is not known: a history from before steps were recorded, or one cut off in
 * the middle of a step.
 */
export type ChatRow =
  | {kind: 'message'; message: Message}
  | {kind: 'steps'; turnId: string | null; sender: string | undefined; step: PlanStep | null; messages: Message[]};

/**
 * The rows `messages` are shown as: the mark of a step begins a row of its
 * own, and each internal message joins the row right before it if that is
 * of its turn. Interrupted by another's - agents answering at once - it
 * begins a row of its own, under the step its turn was last in. Tool
 * results are merged first (`mergeToolResults`), so that a step's call and
 * its result stay one message.
 */
export function chatRows(messages: Message[]): ChatRow[] {
  const rows: ChatRow[] = [];
  // The step each turn is in, as its last mark said.
  const steps = new Map<string | null, PlanStep>();
  for (const message of messages) {
    const last = rows[rows.length - 1];
    const turnId = message.turn_id ?? null;
    if (message.planStep) {
      steps.set(turnId, message.planStep);
      rows.push({kind: 'steps', turnId, sender: message.sender_name, step: message.planStep, messages: []});
    } else if (!message.internal) {
      rows.push({kind: 'message', message});
    } else if (last?.kind === 'steps' && last.turnId === turnId) {
      last.messages.push(message);
    } else {
      rows.push({kind: 'steps', turnId, sender: message.sender_name, step: steps.get(turnId) ?? null, messages: [message]});
    }
  }
  return rows;
}

// The month by name ("29. Sept. 2026, 15:24"): at the small size timestamps
// are shown in, dots between digits are hard to make out.
const DATE_TIME = new Intl.DateTimeFormat(undefined, {
  year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit',
});
const FULL_DATE_TIME = new Intl.DateTimeFormat(undefined, {dateStyle: 'full', timeStyle: 'medium'});

/**
 * A message's timestamp for display: date and time in the browser's time
 * zone and locale, and the full form with seconds for a tooltip - or `null`
 * if it is missing or unreadable.
 */
export function formatTimestamp(timestamp: string | undefined): {short: string; full: string} | null {
  if (!timestamp) return null;
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return null;
  return {short: DATE_TIME.format(date), full: FULL_DATE_TIME.format(date)};
}

const SIZE = new Intl.NumberFormat(undefined, {maximumFractionDigits: 1});

/** A size in bytes as people read it, e.g. `1.4 MB`. */
export function formatSize(bytes: number): string {
  const units = ['B', 'kB', 'MB', 'GB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return `${SIZE.format(value)} ${units[unit]}`;
}

const TOKENS = new Intl.NumberFormat();
const SHORT_TOKENS = new Intl.NumberFormat(undefined, {notation: 'compact', maximumFractionDigits: 1});

/** A number of tokens in full, e.g. `40,512`. */
export function formatTokens(tokens: number): string {
  return TOKENS.format(tokens);
}

/** A number of tokens in short, e.g. `40.5K`. */
export function formatTokensShort(tokens: number): string {
  return SHORT_TOKENS.format(tokens);
}

/**
 * How full a context was, as a share of its size between 0 and 1 - `tokens`
 * of prompt and answer against `size` - or `null` if the size is not known.
 */
export function fillLevel(tokens: number, size: number | undefined): number | null {
  return size ? Math.min(tokens / size, 1) : null;
}

/** Usage statistics under a heading, as label/value rows (issue 219). */
export interface StatGroup {
  title: string;
  rows: Array<[string, string]>;
}

/** A share of a context, e.g. `8 %`. */
function percent(level: number): string {
  return `${Math.round(level * 100)} %`;
}

/**
 * What an agent's calls used (issue 209), in two groups: its context - what
 * it holds now, against the context's size (issue 284) - and its usage,
 * summed over all calls, which grows much faster since every call is given
 * the whole context again.
 */
export function tokenUsageGroups(usage: TokenUsage): StatGroup[] {
  const level = fillLevel(usage.context_tokens, usage.context_size);
  const context: Array<[string, string]> = [['Tokens', formatTokens(usage.context_tokens)]];
  if (level === null) {
    context.push(['Size', 'unknown']);
  } else {
    context.push(['Size', formatTokens(usage.context_size!)], ['Filled', percent(level)]);
  }
  return [
    {title: 'Context', rows: context},
    {title: `Usage of all ${formatTokens(usage.calls)} ${usage.calls === 1 ? 'call' : 'calls'}`, rows: [
      ['Total input tokens', formatTokens(usage.prompt_tokens)],
      ['Total output tokens', formatTokens(usage.completion_tokens)],
    ]},
  ];
}

/** A duration the LLM backend reported in nanoseconds, in seconds. */
function seconds(nanoseconds: number): string {
  return `${(nanoseconds / 1e9).toFixed(2)} s`;
}

/**
 * The usage statistics of a message, grouped into its tokens and its
 * times, leaving out what was not reported; empty for messages that have
 * none (issue 43).
 */
export function usageGroups(message: Message): StatGroup[] {
  const tokens: Array<[string, string]> = [];
  if (message.prompt_tokens != null) tokens.push(['Input tokens', formatTokens(message.prompt_tokens)]);
  if (message.completion_tokens != null) tokens.push(['Output tokens', formatTokens(message.completion_tokens)]);
  const level = fillLevel((message.prompt_tokens ?? 0) + (message.completion_tokens ?? 0), message.context_size);
  if (level !== null && message.context_size) {
    tokens.push(['Context', `${percent(level)} of ${formatTokens(message.context_size)}`]);
  }
  const times: Array<[string, string]> = [];
  if (message.prompt_duration != null) times.push(['Prompt time', seconds(message.prompt_duration)]);
  if (message.completion_duration != null) times.push(['Completion time', seconds(message.completion_duration)]);
  if (message.total_duration != null) times.push(['Total time', seconds(message.total_duration)]);
  if (message.completion_tokens != null && message.completion_duration) {
    times.push(['Speed', `${(message.completion_tokens / (message.completion_duration / 1e9)).toFixed(1)} tokens/s`]);
  }
  return [
    {title: 'Tokens', rows: tokens},
    {title: 'Time', rows: times},
  ].filter(group => group.rows.length > 0);
}
