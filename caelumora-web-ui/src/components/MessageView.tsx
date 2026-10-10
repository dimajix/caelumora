// One message as the chat shows it - reasoning, content, tool calls with
// their status and result - shared by the chat and the context panel (issue 41).

import React, {useState} from 'react';
import {Brain, Check, ChevronDown, ChevronRight, Copy, EyeOff, Info} from 'lucide-react';
import type {ChannelMember, Message, ToolCall} from '../types';
import type {ChannelArtifacts, WorkspaceFiles} from '../api';
import {cn} from '../classNames';
import {MarkdownMessage} from './MarkdownMessage';
import {mentionedHandles} from '../mention';
import {formatTimestamp, usageGroups} from '../messages';
import {HoverCard} from './HoverCard';
import {UsageStats} from './UsageStats';
import {ToolCallView} from './ToolCallView';

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
 * focus, and on tap, where there is no hover (issues 43, 219). Nothing for
 * messages without statistics.
 */
const UsageInfo: React.FC<{message: Message}> = ({message}) => {
  const groups = usageGroups(message);
  if (groups.length === 0) return null;
  return (
    <HoverCard content={<UsageStats groups={groups} />} focusable={false}>
      <button
        type="button"
        aria-label="Usage statistics"
        className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100 align-middle"
      >
        <Info size={13} />
      </button>
    </HoverCard>
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
    {message.modified && (
      <span className="opacity-60" title={`Edited ${formatTimestamp(message.modified)?.full ?? ''}`}>edited</span>
    )}
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
  /** The files of the channel's workspace, where its calls may offer one for download (issue 273). */
  files?: WorkspaceFiles;
  /** Changes whenever those files may have changed. */
  filesVersion?: number;
  /** The artifacts of the channel, where its calls published one (issue 271). */
  artifacts?: ChannelArtifacts;
  /** Opens a skill's page, where its calls name one (issue 281). */
  onSelectSkill?: (skillId: string) => void;
  /** Interrupts one of its calls alone while it runs (issue 276); offered where given. */
  onInterruptCall?: (call: ToolCall) => void;
}

export const MessageView: React.FC<MessageViewProps> = ({message: msg, members, badges, actions, className, files, filesVersion, artifacts, onSelectSkill, onInterruptCall}) => (
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
        hardBreaks={msg.role === 'User'}
        mentions={mentionedHandles(msg, members)}
      />
    )}

    {msg.tool_calls && msg.tool_calls.length > 0 && (
      <div className="mt-2 space-y-2">
        {msg.tool_calls.map((call, cidx) => (
          <ToolCallView key={cidx} call={call} result={msg.tool_results?.find(r => r.id === call.id)} abandoned={msg.abandoned} files={files} filesVersion={filesVersion} artifacts={artifacts} onSelectSkill={onSelectSkill}
            onInterrupt={onInterruptCall && call.id ? () => onInterruptCall(call) : undefined} />
        ))}
      </div>
    )}

    <MessageFooter message={msg} actions={actions} />
  </div>
);
