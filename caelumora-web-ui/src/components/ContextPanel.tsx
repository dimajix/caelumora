// What an agent was given when it produced a message (issue 41): the
// messages of its context and the prompt parts of that turn, in the order the
// model was given them (issue 47) - including internal steps and tool
// results the chat hides - and the tools it was offered (issue 137). Every
// part shows how it was classified, and those that set the message's label
// are marked (issue 170). For an external agent, which keeps its
// conversation itself, it is what the remote agent was given in the one call
// that produced the message (issue 205): worded so that it holds for both.

import React, {useEffect, useState} from 'react';
import {Copy, ScrollText, X} from 'lucide-react';
import type {ActorId, ChannelParticipant, KnowledgeRef, Message, MessageContextResponse, SeenMessage, SeenPrompt, ToolSet} from '../types';
import {type Classification, classificationDescription, classificationLabel} from '../confidentiality';
import {cn} from '../classNames';
import {api, type ChannelAttachments} from '../api';
import {actorUuid} from '../actor';
import {ApiRequestError, reportError} from '../errors';
import {ToolSetList} from './ToolSetList';
import {MessageView} from './MessageView';
import {mergeToolResultsAmong} from '../messages';

/** How many of the latest messages are shown before "show earlier". */
const SHOWN_AT_FIRST = 10;

// A message's context never changes: what was fetched once is kept for as
// long as the page runs.
const cache = new Map<string, MessageContextResponse>();

/** Why a context is not shown, if the server answered that it cannot be. */
function whyNotShown(error: unknown): string | null {
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

/** What the panel shows, in order: prompt parts, and messages with their tool results merged. */
type Row = SeenPrompt | SeenMessage;

/**
 * `items` with the tool results merged into the messages that called them, across the prompt parts
 * (`mergeToolResultsAmong`); a merged message set the label if any message merged into it did.
 */
function rowsOf(items: MessageContextResponse['items']): Row[] {
  const merged = mergeToolResultsAmong<Row>(items, item => item.kind === 'message' ? item.message : undefined, (item, message) => ({kind: 'message', message, sets_label: item.kind === 'message' && item.sets_label}));
  return merged.map(row => {
    if (row.kind !== 'message') return row;
    const results = new Set((row.message.tool_results ?? []).map(result => result.id));
    const sets_label = items.some(item => item.kind === 'message' && item.sets_label
      && (item.message.id === row.message.id || (item.message.tool_results ?? []).some(result => results.has(result.id))));
    return {...row, sets_label};
  });
}

/** The border of a part of the context: marked if it set the message's label. */
const partBorder = (setsLabel: boolean) => setsLabel
  ? 'border-amber-400 dark:border-amber-500'
  : 'border-gray-200 dark:border-gray-700';

/**
 * How a part of the context was classified - `null`: not known, so it
 * counted as the channel - marked if it set the message's label.
 */
const ClassificationTag: React.FC<{classification: Classification | null; setsLabel: boolean}> = ({classification, setsLabel}) => (
  <span
    className={cn(
      'shrink-0 px-1.5 rounded-sm border text-[11px] font-normal',
      setsLabel
        ? 'border-amber-400 text-amber-700 bg-amber-50 dark:border-amber-500 dark:text-amber-300 dark:bg-amber-950'
        : 'border-gray-300 text-gray-500 dark:border-gray-600 dark:text-gray-400',
    )}
    title={[
      classification ? classificationDescription(classification) : 'Not classified: counts as the channel.',
      ...(setsLabel ? ['This set the label of the message.'] : []),
    ].join(' ')}
  >
    {classification ? classificationLabel(classification) : 'counts as the channel'}
  </span>
);

const PromptView: React.FC<{prompt: SeenPrompt; leading: boolean}> = ({prompt, leading}) => (
  <details className={cn('rounded-md border text-sm', partBorder(prompt.sets_label))}>
    <summary className="cursor-pointer px-3 py-2 font-semibold text-gray-700 dark:text-gray-200">
      {leading && prompt.role === 'System' ? 'System prompt' : `Prompt part (as ${prompt.role.toLowerCase()} message)`}
      {' '}
      <ClassificationTag classification={prompt.classification} setsLabel={prompt.sets_label} />
    </summary>
    <div className="px-3 pb-2">
      <DigestLine label="Digest" digest={prompt.digest} />
    </div>
    <pre className="px-3 pb-3 whitespace-pre-wrap text-xs text-gray-700 dark:text-gray-300">{prompt.text}</pre>
  </details>
);

/** What a knowledge reference read, in words; `sessionId` is the session the panel is shown in. */
function knowledgeLabel(knowledge: KnowledgeRef, sessionId: string): string {
  const channel = knowledge.session_id !== sessionId
    ? 'a channel of another session'
    : knowledge.channel_name ? `#${knowledge.channel_name}`
    : knowledge.channel_deleted ? 'a deleted channel' : 'a channel you cannot see';
  const agent = knowledge.agent_name ?? 'an agent';
  switch (knowledge.source) {
    case 'notes':
      return `Notes of ${agent} in ${channel}`;
    case 'private_notes':
      return `Private notes of ${agent} in ${channel}`;
    case 'memory':
      return `Memory of ${agent}, from ${channel}`;
    case 'workspace':
      // The digest is a file's content as read; none for a listing or a search.
      return knowledge.digest ? `A file of the workspace, read in ${channel}` : `The files of the workspace, listed or searched in ${channel}`;
    case 'skill':
      // The digest is the skill's text as it was loaded (issue 22).
      return `A skill, loaded in ${channel}`;
    case 'history':
      // A reader who joined later saw only part of it (issue 72).
      return knowledge.history_start ? `History of ${channel}, from when it was granted` : `History of ${channel}`;
    default:
      // An entity of a kind this view does not know (issue 323).
      return `A ${knowledge.source.replace(/_/g, ' ')}, read in ${channel}`;
  }
}

const KnowledgeList: React.FC<{knowledge: KnowledgeRef[]; sessionId: string}> = ({knowledge, sessionId}) => (
  <details className="rounded-md border border-gray-200 dark:border-gray-700 text-sm" open>
    <summary className="cursor-pointer px-3 py-2 font-semibold text-gray-700 dark:text-gray-200">
      Knowledge read ({knowledge.length})
    </summary>
    <div className="px-3 pb-2 space-y-1">
      {knowledge.map((k, idx) => (
        <div key={idx} className="flex items-center gap-2">
          <div className="flex-1 min-w-0">
            <DigestLine label={knowledgeLabel(k, sessionId)} digest={k.digest} />
          </div>
          <ClassificationTag classification={k.classification} setsLabel={k.sets_label} />
        </div>
      ))}
    </div>
  </details>
);

/** The tools offered, by the set they came from (issue 194). */
const ToolList: React.FC<{sets: ToolSet[]}> = ({sets}) => (
  <details className="rounded-md border border-gray-200 dark:border-gray-700 text-sm">
    <summary className="cursor-pointer px-3 py-2 font-semibold text-gray-700 dark:text-gray-200">
      Tools offered ({sets.reduce((count, set) => count + set.tools.length, 0)})
    </summary>
    <div className="px-3 pb-2">
      <ToolSetList sets={sets} />
    </div>
  </details>
);

interface ContextPanelProps {
  sessionId: string;
  channelId: string;
  /** The attachments of the channel's messages, where what its messages carry is downloaded from (issue 282). */
  attachments: ChannelAttachments;
  /** The agent's message whose context is shown. */
  message: Message;
  /** Who views it, whose messages are their own (issue 407). */
  me: ActorId;
  participants: ChannelParticipant[];
  onClose: () => void;
}

export const ContextPanel: React.FC<ContextPanelProps> = ({sessionId, channelId, attachments, message, me, participants, onClose}) => {
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
        const why = whyNotShown(error);
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
    <div className="w-full border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex flex-col min-h-0">
      <div className="p-4 border-b border-gray-200 dark:border-gray-700 space-y-2">
        <div className="flex items-center gap-2 text-gray-700 dark:text-gray-200">
          <ScrollText size={18} />
          <h3 className="font-semibold text-sm flex-1">What {message.sender_name || 'the agent'} was given</h3>
          <button onClick={onClose} className="p-1 hover:bg-gray-100 dark:hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>
        {message.classification && (
          <div className="text-xs text-gray-500 dark:text-gray-400">
            Labelled <span className="text-gray-700 dark:text-gray-300">{classificationLabel(message.classification)}</span> by
            what it had seen; marked is what of this set the label.
          </div>
        )}
        {context && (
          <div className="space-y-0.5">
            <DigestLine label="Transcript" digest={context.transcript_digest} />
            <DigestLine label="Tools" digest={context.tools_digest} />
          </div>
        )}
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {notice && <div className="text-sm text-gray-500 dark:text-gray-400">{notice}</div>}
        {!context && !notice && <div className="text-sm text-gray-500 dark:text-gray-400">Loading…</div>}
        {context && (
          <>
            {context.knowledge?.length > 0 && <KnowledgeList knowledge={context.knowledge} sessionId={sessionId} />}
            {context.tool_sets?.length > 0 && <ToolList sets={context.tool_sets} />}
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
                me={me}
                participants={participants}
                attachments={attachments}
                className={cn('max-w-full', row.sets_label && 'ring-2 ring-amber-400 dark:ring-amber-500')}
                badges={[
                  ...(row.message.internal ? ['internal'] : []),
                  ...(row.message.role === 'Tool' ? ['tool result'] : []),
                  ...(row.message.classification ? [classificationLabel(row.message.classification)] : []),
                  ...(row.sets_label ? ['set the label'] : []),
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
