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
