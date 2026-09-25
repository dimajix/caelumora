// One message as the chat shows it - content, tool calls with their status
// and result - shared by the chat and the context panel (issue 41).

import React from 'react';
import {CheckCircle2, Terminal, Wrench, XCircle} from 'lucide-react';
import type {ChannelMember, Message, ToolResult, ToolResultStatus} from '../types';
import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';
import {MarkdownMessage} from './MarkdownMessage';
import {mentionedHandles} from '../mention';
import {formatArguments} from '../messages';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

const TOOL_STATUS_LABELS: Record<Exclude<ToolResultStatus, 'ok'>, string> = {
  denied: 'Denied',
  forbidden: 'Not permitted',
  timed_out: 'Timed out',
  invalid_arguments: 'Invalid arguments',
  failed: 'Failed',
  unknown_tool: 'Unknown tool',
};

/** The text to show for a tool result: the message of an unsuccessful one, else the content. */
function toolResultText(result: ToolResult): string {
  if (!result.status || result.status === 'ok') return result.content;
  try {
    const parsed = JSON.parse(result.content);
    if (typeof parsed?.message === 'string') return parsed.message;
  } catch {
    // Not the JSON object the server writes for unsuccessful results.
  }
  return result.content;
}

interface MessageViewProps {
  message: Message;
  /** The channel's members, for highlighting `@`-mentions. */
  members: ChannelMember[];
  /** Labels shown next to the sender, e.g. "internal". */
  badges?: string[];
  /** An action in the header, e.g. showing the message's context. */
  action?: React.ReactNode;
  className?: string;
}

export const MessageView: React.FC<MessageViewProps> = ({message: msg, members, badges, action, className}) => (
  <div
    className={cn(
      "max-w-[80%] p-3 rounded-lg shadow-xs",
      msg.role === 'User'
        ? "ml-auto bg-blue-600 text-white rounded-br-none"
        : "bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 rounded-bl-none border border-gray-200 dark:border-gray-600",
      className
    )}
  >
    <div className="text-xs font-semibold mb-1 opacity-70 uppercase tracking-wider flex items-center gap-2">
      <span>{msg.sender_name || msg.role}</span>
      {badges?.map(badge => (
        <span key={badge} className="normal-case font-normal px-1.5 rounded-sm border border-current">{badge}</span>
      ))}
      {action && <span className="ml-auto">{action}</span>}
    </div>

    {msg.content && (
      <MarkdownMessage
        content={msg.content}
        invert={msg.role === 'User'}
        mentions={mentionedHandles(msg, members)}
      />
    )}

    {msg.tool_calls && msg.tool_calls.length > 0 && (
      <div className="mt-2 space-y-2">
        {msg.tool_calls.map((call, cidx) => {
          const result = msg.tool_results?.find(r => r.id === call.id);
          return (
            <div key={cidx} className="bg-gray-50 dark:bg-gray-800 rounded-sm border border-gray-200 dark:border-gray-600 overflow-hidden">
              <div className="bg-gray-100 dark:bg-gray-900 px-2 py-1 flex items-center justify-between border-b border-gray-200 dark:border-gray-600">
                <div className="flex items-center gap-2 text-xs font-mono">
                  <Wrench size={12} className="text-blue-500" />
                  <span>{call.function_name}</span>
                </div>
                {result && result.status && result.status !== 'ok' ? (
                  <div className="flex items-center gap-1 text-amber-600 dark:text-amber-400">
                    <XCircle size={12} />
                    <span className="text-[10px] uppercase">{TOOL_STATUS_LABELS[result.status]}</span>
                  </div>
                ) : result ? (
                  <CheckCircle2 size={12} className="text-green-500" />
                ) : (
                  <div className="flex items-center gap-1">
                    <div className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse" />
                    <span className="text-[10px] text-blue-500 uppercase">Executing</span>
                  </div>
                )}
              </div>
              <div className="p-2">
                <div className="flex items-start gap-2 mb-1">
                  <Terminal size={12} className="mt-0.5 opacity-50" />
                  <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>
                </div>
                {result && (
                  <div className="mt-2 pt-2 border-t border-gray-200 dark:border-gray-700">
                    <div className="text-[10px] uppercase opacity-50 mb-1">Result</div>
                    <pre className="text-[11px] font-mono whitespace-pre-wrap break-all bg-white dark:bg-gray-950 p-1.5 rounded-sm border border-gray-100 dark:border-gray-900">
                      {toolResultText(result)}
                    </pre>
                  </div>
                )}
              </div>
            </div>
          );
        })}
      </div>
    )}
  </div>
);
