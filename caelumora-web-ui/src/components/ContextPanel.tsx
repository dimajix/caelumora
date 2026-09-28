// What an agent's model saw when it produced a message (issue 41): the
// messages of its context and the prompt parts of that turn, in the order the
// model was given them (issue 47) - including internal steps and tool
// results the chat hides.

import React, {useEffect, useState} from 'react';
import {Copy, ScrollText, X} from 'lucide-react';
import type {ChannelMember, KnowledgeRef, Message, MessageContextResponse, SeenPrompt} from '../types';
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
    <span className="min-w-24 shrink-0">{label}</span>
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

/** What the panel shows, in order: prompt parts, and runs of messages with their tool results merged. */
type Row = SeenPrompt | {kind: 'message'; message: Message};

function rowsOf(items: MessageContextResponse['items']): Row[] {
  const rows: Row[] = [];
  let run: Message[] = [];
  const flush = () => {
    rows.push(...mergeToolResults(run).map(message => ({kind: 'message' as const, message})));
    run = [];
  };
  for (const item of items) {
    if (item.kind === 'message') {
      run.push(item.message);
    } else {
      flush();
      rows.push(item);
    }
  }
  flush();
  return rows;
}

const PromptView: React.FC<{prompt: SeenPrompt; leading: boolean}> = ({prompt, leading}) => (
  <details className="rounded-md border border-gray-200 dark:border-gray-700 text-sm">
    <summary className="cursor-pointer px-3 py-2 font-semibold text-gray-700 dark:text-gray-200">
      {leading && prompt.role === 'System' ? 'System prompt' : `Prompt part (as ${prompt.role.toLowerCase()} message)`}
    </summary>
    <div className="px-3 pb-2">
      <DigestLine label="Digest" digest={prompt.digest} />
    </div>
    <pre className="px-3 pb-3 whitespace-pre-wrap text-xs text-gray-700 dark:text-gray-300">{prompt.text}</pre>
  </details>
);

/** What a knowledge reference read, in words. */
function knowledgeLabel(knowledge: KnowledgeRef): string {
  const channel = knowledge.channel_name ? `#${knowledge.channel_name}` : 'a deleted channel';
  const agent = knowledge.agent_name ?? 'an agent';
  switch (knowledge.source) {
    case 'notes':
      return `Notes of ${agent} in ${channel}`;
    case 'private_notes':
      return `Private notes of ${agent} in ${channel}`;
    case 'memory':
      return `Memory of ${agent}, as recalled in ${channel}`;
    default:
      return `History of ${channel}`;
  }
}

const KnowledgeList: React.FC<{knowledge: KnowledgeRef[]}> = ({knowledge}) => (
  <details className="rounded-md border border-gray-200 dark:border-gray-700 text-sm" open>
    <summary className="cursor-pointer px-3 py-2 font-semibold text-gray-700 dark:text-gray-200">
      Knowledge read ({knowledge.length})
    </summary>
    <div className="px-3 pb-2 space-y-1">
      {knowledge.map((k, idx) => (
        <DigestLine key={idx} label={knowledgeLabel(k)} digest={k.digest} />
      ))}
    </div>
  </details>
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

  const rows = context ? rowsOf(context.items) : [];
  // Prompt parts before everything - the system prompt - stay on top; the
  // rest is shortened to its end.
  const leading = rows.findIndex(row => row.kind !== 'prompt');
  const pinned = leading === -1 ? rows.length : leading;
  const rest = rows.slice(pinned);
  const hidden = showAll ? 0 : Math.max(0, rest.length - SHOWN_AT_FIRST);

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
          </div>
        )}
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {notice && <div className="text-sm text-gray-500 dark:text-gray-400">{notice}</div>}
        {!context && !notice && <div className="text-sm text-gray-500 dark:text-gray-400">Loading…</div>}
        {context && (
          <>
            {context.knowledge?.length > 0 && <KnowledgeList knowledge={context.knowledge} />}
            {rows.slice(0, pinned).map(row => row.kind === 'prompt' && (
              <PromptView key={row.digest} prompt={row} leading />
            ))}
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
            {rest.slice(hidden).map((row, idx) => row.kind === 'prompt' ? (
              <PromptView key={`${row.digest}-${idx}`} prompt={row} leading={false} />
            ) : (
              <MessageView
                key={row.message.id ?? idx}
                message={row.message}
                members={members}
                className="max-w-full"
                badges={[
                  ...(row.message.internal ? ['internal'] : []),
                  ...(row.message.role === 'Tool' ? ['tool result'] : []),
                ]}
              />
            ))}
            {rest.length === 0 && (
              <div className="text-sm text-gray-500 dark:text-gray-400">The context was empty.</div>
            )}
          </>
        )}
      </div>
    </div>
  );
};
