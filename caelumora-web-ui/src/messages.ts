// Helpers for showing messages, shared by the chat and the context panel.

import type {Message} from './types';

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

const DATE_TIME = new Intl.DateTimeFormat(undefined, {
  year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit',
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
  if (message.completion_tokens != null && message.completion_duration) {
    rows.push(['Speed', `${(message.completion_tokens / (message.completion_duration / 1e9)).toFixed(1)} tokens/s`]);
  }
  return rows;
}
