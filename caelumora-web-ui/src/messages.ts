// Helpers for showing messages, shared by the chat and the context panel.

import type {Message, TokenUsage} from './types';

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

/** The usage of an agent's calls as label/value rows (issue 209). */
export function tokenUsageRows(usage: TokenUsage): Array<[string, string]> {
  const level = fillLevel(usage.last_call_tokens, usage.context_size);
  return [
    ['Context', level === null
      ? `${formatTokens(usage.last_call_tokens)} tokens, size unknown`
      : `${formatTokens(usage.last_call_tokens)} of ${formatTokens(usage.context_size!)} (${Math.round(level * 100)} %)`],
    ['Calls', formatTokens(usage.calls)],
    ['Prompt tokens', formatTokens(usage.prompt_tokens)],
    ['Completion tokens', formatTokens(usage.completion_tokens)],
  ];
}

/** A duration the LLM backend reported in nanoseconds, in seconds. */
function seconds(nanoseconds: number): string {
  return `${(nanoseconds / 1e9).toFixed(2)} s`;
}

/**
 * The usage statistics of a message as label/value rows, leaving out what
 * was not reported; empty for messages that have none (issue 43).
 */
export function usageRows(message: Message): Array<[string, string]> {
  const rows: Array<[string, string]> = [];
  if (message.prompt_tokens != null) rows.push(['Prompt tokens', String(message.prompt_tokens)]);
  if (message.completion_tokens != null) rows.push(['Completion tokens', String(message.completion_tokens)]);
  if (message.prompt_duration != null) rows.push(['Prompt time', seconds(message.prompt_duration)]);
  if (message.completion_duration != null) rows.push(['Completion time', seconds(message.completion_duration)]);
  if (message.total_duration != null) rows.push(['Total time', seconds(message.total_duration)]);
  const level = fillLevel((message.prompt_tokens ?? 0) + (message.completion_tokens ?? 0), message.context_size);
  if (level !== null && message.context_size) {
    rows.push(['Context', `${Math.round(level * 100)} % of ${formatTokens(message.context_size)}`]);
  }
  if (message.completion_tokens != null && message.completion_duration) {
    rows.push(['Speed', `${(message.completion_tokens / (message.completion_duration / 1e9)).toFixed(1)} tokens/s`]);
  }
  return rows;
}
