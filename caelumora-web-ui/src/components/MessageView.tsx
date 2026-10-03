// One message as the chat shows it - reasoning, content, tool calls with
// their status and result - shared by the chat and the context panel (issue 41).

import React, {useState} from 'react';
import {Brain, Check, CheckCircle2, ChevronDown, ChevronRight, Copy, EyeOff, Info, Terminal, Wrench, XCircle} from 'lucide-react';
import type {ChannelMember, Message, ToolResult, ToolResultStatus, ToolVisibility} from '../types';
import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';
import {MarkdownMessage} from './MarkdownMessage';
import {mentionedHandles} from '../mention';
import {formatArguments, formatTimestamp, usageRows} from '../messages';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

const TOOL_STATUS_LABELS: Record<Exclude<ToolResultStatus, 'ok'>, string> = {
  denied: 'Denied',
  forbidden: 'Not permitted',
  timed_out: 'Timed out',
  invalid_arguments: 'Invalid arguments',
  failed: 'Failed',
  not_executed: 'Not executed',
  unknown_tool: 'Unknown tool',
  withheld: 'Withheld',
};

/**
 * What of a call only the agent's owner sees (issues 54, 133), or `null`
 * if the channel sees all of it. The channel shows such a call without
 * arguments and result, or not at all; the owner sees it in full in the
 * context panel.
 */
function visibilityLabel(labels: ToolVisibility | undefined): string | null {
  if (!labels || (labels.call === 'channel' && labels.content === 'channel')) return null;
  return labels.call === 'channel' ? 'Arguments and result: owner only' : 'Call: owner only';
}

/** The text to show for a tool result: the message of an unsuccessful one, else the content. */
/** How long a tool ran, e.g. `240 ms`, `3.1 s` or `2 min 5 s`; `null` if it did not run. */
function toolDuration(result: ToolResult): string | null {
  if (!result.started_at || !result.finished_at) return null;
  const ms = Date.parse(result.finished_at) - Date.parse(result.started_at);
  if (Number.isNaN(ms) || ms < 0) return null;
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.floor(ms / 60_000)} min ${Math.round((ms % 60_000) / 1000)} s`;
}

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

/** When a message was written: date and time, the full form on hover (issue 43). */
export const MessageTime: React.FC<{timestamp?: string; className?: string}> = ({timestamp, className}) => {
  const time = formatTimestamp(timestamp);
  if (!time) return null;
  return (
    <time dateTime={timestamp} title={time.full} className={cn("normal-case tracking-normal font-normal", className)}>
      {time.short}
    </time>
  );
};

/**
 * An "(i)" that reveals a message's usage statistics - on hover, on keyboard
 * focus, and on tap, where there is no hover (issue 43). Nothing for messages
 * without statistics.
 */
const UsageInfo: React.FC<{message: Message}> = ({message}) => {
  const [open, setOpen] = useState(false);
  const rows = usageRows(message);
  if (rows.length === 0) return null;
  return (
    <span className="relative group normal-case tracking-normal font-normal">
      <button
        type="button"
        onClick={() => setOpen(o => !o)}
        onBlur={() => setOpen(false)}
        aria-label="Usage statistics"
        aria-expanded={open}
        className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100 align-middle"
      >
        <Info size={13} />
      </button>
      <span
        role="tooltip"
        className={cn(
          "absolute left-0 bottom-full mb-1 z-20 w-56 rounded-md border border-gray-200 dark:border-gray-600 bg-white dark:bg-gray-800 text-gray-800 dark:text-gray-100 shadow-lg p-2 text-xs",
          open ? "block" : "hidden group-hover:block group-focus-within:block"
        )}
      >
        {rows.map(([label, value]) => (
          <span key={label} className="flex justify-between gap-3">
            <span className="text-gray-500 dark:text-gray-400">{label}</span>
            <span className="font-mono">{value}</span>
          </span>
        ))}
      </span>
    </span>
  );
};

/**
 * A message's reasoning (issue 216), folded under a "Thought" toggle that the
 * user opens - it is often longer than the answer. `thinking` while it is
 * still streaming in and nothing of the answer has followed; `ownerOnly` if
 * only the agent's owner sees it, which only the context panel shows.
 */
export const Reasoning: React.FC<{text: string; thinking?: boolean; ownerOnly?: boolean}> = ({text, thinking, ownerOnly}) => {
  const [open, setOpen] = useState(false);
  return (
    <div className="mb-2">
      <button
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-expanded={open}
        className="flex items-center gap-1 text-xs opacity-60 hover:opacity-100 focus:opacity-100"
      >
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <Brain size={13} />
        {thinking ? (
          <span className="inline-flex items-center gap-1.5">
            Thinking
            <span className="w-1.5 h-1.5 bg-current rounded-full animate-pulse" />
          </span>
        ) : 'Thought'}
        {ownerOnly && (
          <span className="flex items-center gap-1 text-[11px] ml-1" title="Who sees this reasoning">
            <EyeOff size={11} />
            owner only
          </span>
        )}
      </button>
      {open && (
        <div className="mt-1 pl-3 border-l-2 border-gray-200 dark:border-gray-600 text-sm opacity-75">
          <MarkdownMessage content={text} />
        </div>
      )}
    </div>
  );
};

/** Copies the message's text - as stored, Markdown included - and confirms it briefly. */
const CopyButton: React.FC<{text: string}> = ({text}) => {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (error) {
      console.error('Copying to the clipboard failed', error);
    }
  };
  return (
    <button
      type="button"
      onClick={copy}
      className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
      title={copied ? 'Copied' : 'Copy to clipboard'}
      aria-label="Copy to clipboard"
    >
      {copied ? <Check size={13} /> : <Copy size={13} />}
    </button>
  );
};

/**
 * The bottom row of a message: when it was written and the icons about it -
 * usage statistics, copying, and whatever `actions` the caller adds. Dimmed
 * per item, not as a whole: opacity on the row would make the usage tooltip
 * in it translucent too.
 */
export const MessageFooter: React.FC<{message: Message; actions?: React.ReactNode}> = ({message, actions}) => (
  <div className="mt-2 flex items-center gap-2 text-[11px]">
    <MessageTime timestamp={message.timestamp} className="opacity-60" />
    <UsageInfo message={message} />
    {message.content && <CopyButton text={message.content} />}
    {actions}
  </div>
);

interface MessageViewProps {
  message: Message;
  /** The channel's members, for highlighting `@`-mentions. */
  members: ChannelMember[];
  /** Labels shown next to the sender, e.g. "internal". */
  badges?: string[];
  /** Further icons for the footer, e.g. showing the message's context. */
  actions?: React.ReactNode;
  className?: string;
}

export const MessageView: React.FC<MessageViewProps> = ({message: msg, members, badges, actions, className}) => (
  <div
    className={cn(
      "max-w-[80%] p-3 rounded-lg shadow-xs",
      msg.role === 'User'
        ? "ml-auto bg-blue-600 text-white rounded-br-none"
        : "bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 rounded-bl-none border border-gray-200 dark:border-gray-600",
      className
    )}
  >
    <div className="text-xs font-semibold mb-1 uppercase tracking-wider flex items-center gap-2">
      <span className="opacity-70">{msg.sender_name || msg.role}</span>
      {badges?.map(badge => (
        <span key={badge} className="opacity-70 normal-case font-normal px-1.5 rounded-sm border border-current">{badge}</span>
      ))}
    </div>

    {msg.reasoning && <Reasoning text={msg.reasoning} ownerOnly={msg.reasoning_visibility === 'owner'} />}

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
                  {visibilityLabel(call.visibility) && (
                    <span className="flex items-center gap-1 font-sans text-[11px] opacity-60" title="Who sees this call">
                      <EyeOff size={11} />
                      {visibilityLabel(call.visibility)}
                    </span>
                  )}
                </div>
                <div className="flex items-center gap-2">
                {result && toolDuration(result) && (
                  <span
                    className="text-[11px] font-mono opacity-60"
                    title={result.started_at ? `Ran from ${formatTimestamp(result.started_at)?.full}` : undefined}
                  >
                    {toolDuration(result)}
                  </span>
                )}
                {result && result.status && result.status !== 'ok' ? (
                  <div className="flex items-center gap-1 text-amber-600 dark:text-amber-400">
                    <XCircle size={12} />
                    <span className="text-[11px] uppercase">{TOOL_STATUS_LABELS[result.status]}</span>
                  </div>
                ) : result ? (
                  <CheckCircle2 size={12} className="text-green-500" />
                ) : msg.abandoned ? (
                  <div className="flex items-center gap-1 opacity-60" title="Its caller was removed from the channel">
                    <XCircle size={12} />
                    <span className="text-[11px] uppercase">Abandoned</span>
                  </div>
                ) : (
                  <div className="flex items-center gap-1">
                    <div className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse" />
                    <span className="text-[11px] text-blue-500 uppercase">Executing</span>
                  </div>
                )}
                </div>
              </div>
              <div className="p-2">
                <div className="flex items-start gap-2 mb-1">
                  <Terminal size={12} className="mt-0.5 opacity-50" />
                  {call.arguments === null ? (
                    <span className="text-[11px] italic opacity-60">Arguments not shown</span>
                  ) : (
                    <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>
                  )}
                </div>
                {result && result.content === '' && (result.visibility?.content ?? 'channel') !== 'channel' ? (
                  <div className="mt-2 pt-2 border-t border-gray-200 dark:border-gray-700 text-[11px] italic opacity-60">
                    Result not shown
                  </div>
                ) : result && (
                  <div className="mt-2 pt-2 border-t border-gray-200 dark:border-gray-700">
                    <div className="text-[11px] uppercase opacity-50 mb-1">Result</div>
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

    <MessageFooter message={msg} actions={actions} />
  </div>
);
