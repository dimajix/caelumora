// What an agent's model saw when it produced a message (issue 41): the
// system prompt of that turn and the messages of its context, in the order
// they were sent - including internal steps and tool results the chat hides.

import React, {useEffect, useState} from 'react';
import {Copy, ScrollText, X} from 'lucide-react';
import type {ChannelMember, Message, MessageContextResponse} from '../types';
import {api} from '../api';
import {ApiRequestError, reportError} from '../errors';
import {MessageView} from './MessageView';
import {mergeToolResults} from '../messages';

/** How many of the latest messages are shown before "show earlier". */
const SHOWN_AT_FIRST = 10;

// A message's context never changes: what was fetched once is kept for as
// long as the page runs.
const cache = new Map<string, MessageContextResponse>();

/** The part of an `ActorId` the REST API addresses an actor by. */
function actorUuid(id: Message['sender_id']): string | null {
  if (!id || typeof id === 'string') return null;
  return Object.values(id)[0] ?? null;
}

/** Why a context is not shown, if the server answered that it cannot be. */
function refusal(error: unknown): string | null {
  if (!(error instanceof ApiRequestError)) return null;
  switch (error.code) {
    case 'context_forbidden':
      return 'Only the owner of this agent can see what it saw.';
    case 'context_not_recorded':
      return 'No context has been recorded for this message (yet).';
    case 'context_unavailable':
      return 'The context of this message is no longer available.';
    default:
      return null;
  }
}

const DigestLine: React.FC<{label: string; digest: string | null}> = ({label, digest}) => (
  <div className="flex items-center gap-2 text-[11px] text-gray-500 dark:text-gray-400">
    <span className="w-24 shrink-0">{label}</span>
    {digest ? (
      <>
        <code className="font-mono">{digest.slice(0, 12)}…</code>
        <button
          onClick={() => void navigator.clipboard?.writeText(digest)}
          className="p-0.5 hover:text-gray-800 dark:hover:text-gray-200"
          title={`Copy ${digest}`}
        >
          <Copy size={12} />
        </button>
      </>
    ) : (
      <span className="italic">none</span>
    )}
  </div>
);

interface ContextPanelProps {
  sessionId: string;
  channelId: string;
  /** The agent's message whose context is shown. */
  message: Message;
  members: ChannelMember[];
  onClose: () => void;
}

export const ContextPanel: React.FC<ContextPanelProps> = ({sessionId, channelId, message, members, onClose}) => {
  const actor = actorUuid(message.sender_id);
  const key = `${actor}:${message.id}`;
  const [context, setContext] = useState<MessageContextResponse | null>(() => cache.get(key) ?? null);
  const [notice, setNotice] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  useEffect(() => {
    if (!actor || !message.id || cache.has(key)) return;
    let current = true;
    api.getMessageContext(sessionId, channelId, actor, message.id)
      .then(result => {
        cache.set(key, result);
        if (current) setContext(result);
      })
      .catch(error => {
        const why = refusal(error);
        if (why) {
          if (current) setNotice(why);
        } else {
          reportError('Failed to load the context', error);
        }
      });
    return () => { current = false; };
  }, [sessionId, channelId, actor, message.id, key]);

  const messages = context ? mergeToolResults(context.messages) : [];
  const hidden = showAll ? 0 : Math.max(0, messages.length - SHOWN_AT_FIRST);

  return (
    <div className="w-[28rem] max-w-full border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex flex-col min-h-0">
      <div className="p-4 border-b border-gray-200 dark:border-gray-700 space-y-2">
        <div className="flex items-center gap-2 text-gray-700 dark:text-gray-200">
          <ScrollText size={18} />
          <h3 className="font-semibold text-sm flex-1">What {message.sender_name || 'the agent'} saw</h3>
          <button onClick={onClose} className="p-1 hover:bg-gray-100 dark:hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>
        {context && (
          <div className="space-y-0.5">
            <DigestLine label="Context" digest={context.context_digest} />
            <DigestLine label="System prompt" digest={context.system_prompt_digest} />
          </div>
        )}
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {notice && <div className="text-sm text-gray-500 dark:text-gray-400">{notice}</div>}
        {!context && !notice && <div className="text-sm text-gray-500 dark:text-gray-400">Loading…</div>}
        {context && (
          <>
            <details className="rounded-md border border-gray-200 dark:border-gray-700 text-sm">
              <summary className="cursor-pointer px-3 py-2 font-semibold text-gray-700 dark:text-gray-200">System prompt</summary>
              <pre className="px-3 pb-3 whitespace-pre-wrap text-xs text-gray-700 dark:text-gray-300">
                {context.system_prompt ?? 'None'}
              </pre>
            </details>
            {hidden > 0 && (
              <div className="text-center">
                <button
                  onClick={() => setShowAll(true)}
                  className="text-xs px-3 py-1 rounded-md border border-gray-300 dark:border-gray-600 text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-800"
                >
                  Show {hidden} earlier {hidden === 1 ? 'message' : 'messages'}
                </button>
              </div>
            )}
            {messages.slice(hidden).map((msg, idx) => (
              <MessageView
                key={msg.id ?? idx}
                message={msg}
                members={members}
                className="max-w-full"
                badges={[
                  ...(msg.internal ? ['internal'] : []),
                  ...(msg.role === 'Tool' ? ['tool result'] : []),
                ]}
              />
            ))}
            {messages.length === 0 && (
              <div className="text-sm text-gray-500 dark:text-gray-400">The context was empty.</div>
            )}
          </>
        )}
      </div>
    </div>
  );
};
