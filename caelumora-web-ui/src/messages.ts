// Helpers for showing messages, shared by the chat and the context panel.

import type {ActorId, GivenUpReason, HistoryItem, Message, TokenUsage} from './types';
import {classificationLabel} from './confidentiality';

export function formatArguments(args: unknown): string {
  return typeof args === 'string' ? args : JSON.stringify(args);
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

/** Why a turn was given up (issues 145, 258), in this page's words. */
export function givenUpText(reason: GivenUpReason): string {
  switch (reason.kind) {
    case 'failed':
      return `It failed: ${reason.message}`;
    case 'lowered':
      return `The channel was lowered to ${classificationLabel(reason.channel)} while it ran.`;
    case 'stopped':
      return 'The agent was stopped, leaving the channel.';
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
 * The chat's messages for `items` of a channel's history (issue 258): each
 * message said, and the notice of a turn given up - whose, as `nameOf`
 * names them. A turn that finished shows nothing of its own.
 */
export function chatMessages(items: HistoryItem<Message>[], nameOf: (actor: ActorId) => string | undefined): Message[] {
  return items.flatMap((item): Message[] => {
    if ('said' in item) return [item.said];
    if ('turn_given_up' in item) {
      const {turn_id, actor, reason, at} = item.turn_given_up;
      return [givenUpNotice(turn_id, reason, actor, nameOf(actor), at)];
    }
    return [];
  });
}

/**
 * A row of the chat: a message, or the steps of a plan (issue 258) - a run of
 * internal messages of one turn, shown together and folded.
 */
export type ChatRow =
  | {kind: 'message'; message: Message}
  | {kind: 'steps'; turnId: string | null; messages: Message[]};

/**
 * The rows `messages` are shown as: each internal message joins the steps
 * right before it if they are of its turn, else begins steps of its own.
 * Tool results are merged first (`mergeToolResults`), so that a step's call
 * and its result stay one message.
 */
export function chatRows(messages: Message[]): ChatRow[] {
  const rows: ChatRow[] = [];
  for (const message of messages) {
    const last = rows[rows.length - 1];
    const turnId = message.turn_id ?? null;
    if (!message.internal) {
      rows.push({kind: 'message', message});
    } else if (last?.kind === 'steps' && last.turnId === turnId) {
      last.messages.push(message);
    } else {
      rows.push({kind: 'steps', turnId, messages: [message]});
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
 * What an agent's calls used (issue 209), in two groups: its context - the
 * last call only, against the context's size - and its usage, summed over
 * all calls, which grows much faster since every call is given the whole
 * context again.
 */
export function tokenUsageGroups(usage: TokenUsage): StatGroup[] {
  const level = fillLevel(usage.last_call_tokens, usage.context_size);
  const context: Array<[string, string]> = [['Tokens', formatTokens(usage.last_call_tokens)]];
  if (level === null) {
    context.push(['Size', 'unknown']);
  } else {
    context.push(['Size', formatTokens(usage.context_size!)], ['Filled', percent(level)]);
  }
  return [
    {title: 'Context of the last call', rows: context},
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
