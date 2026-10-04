// All that was offered to an agent in a channel and what it said itself
// (issue 207): its history, for the agent as a whole rather than for one
// message. It may differ from what the channel shows - an agent that joined
// late or was asleep heard what it heard. What its perceive processors did
// not pass on to its transcript is marked; what each of its own messages was
// produced from opens as that message's context.

import React, {useEffect, useState} from 'react';
import {History, ScrollText, X} from 'lucide-react';
import type {ChannelMember, HistoryMessage, Message} from '../types';
import {classificationLabel} from '../confidentiality';
import {clsx as cn} from 'clsx';
import {api} from '../api';
import {actorUuid} from '../mention';
import {ApiRequestError, reportError} from '../errors';
import {MessageView} from './MessageView';
import {mergeToolResults} from '../messages';

/** How many of the latest messages are shown before "show earlier". */
const SHOWN_AT_FIRST = 20;

interface HistoryPanelProps {
  sessionId: string;
  channelId: string;
  /** The agent whose history is shown. */
  member: ChannelMember;
  members: ChannelMember[];
  /** Changes whenever the history may have grown, to load it again. */
  version: number;
  /** Shows the context of one of the agent's messages. */
  onShowContext: (message: Message) => void;
  onClose: () => void;
}

export const HistoryPanel: React.FC<HistoryPanelProps> = ({sessionId, channelId, member, members, version, onShowContext, onClose}) => {
  const actor = actorUuid(member.id);
  const [history, setHistory] = useState<HistoryMessage[] | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  useEffect(() => {
    if (!actor) return;
    let current = true;
    api.getAgentHistory(sessionId, channelId, actor)
      .then(result => {
        if (current) setHistory(result);
      })
      .catch(error => {
        if (error instanceof ApiRequestError && error.code === 'context_forbidden') {
          if (current) setNotice('Only the owner of this agent can see what it heard.');
        } else {
          reportError('Failed to load the history', error);
        }
      });
    return () => { current = false; };
  }, [sessionId, channelId, actor, version]);

  const marks = new Map((history ?? []).map(entry => [entry.message.id, entry]));
  const messages = mergeToolResults((history ?? []).map(entry => entry.message));
  const hidden = showAll ? 0 : Math.max(0, messages.length - SHOWN_AT_FIRST);

  return (
    <div className="w-[28rem] max-w-full border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex flex-col min-h-0">
      <div className="p-4 border-b border-gray-200 dark:border-gray-700 space-y-2">
        <div className="flex items-center gap-2 text-gray-700 dark:text-gray-200">
          <History size={18} />
          <h3 className="font-semibold text-sm flex-1">What {member.name} heard and said</h3>
          <button onClick={onClose} className="p-1 hover:bg-gray-100 dark:hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>
        <div className="text-xs text-gray-500 dark:text-gray-400">
          Everything offered to the agent in this channel, which may differ from what the channel shows.
          Greyed out is what it did not pass on to its transcript.
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {notice && <div className="text-sm text-gray-500 dark:text-gray-400">{notice}</div>}
        {!history && !notice && <div className="text-sm text-gray-500 dark:text-gray-400">Loading…</div>}
        {history && (
          <>
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
            {messages.slice(hidden).map((message, idx) => {
              const mark = message.id ? marks.get(message.id) : undefined;
              const passedOn = mark?.in_transcript ?? true;
              return (
                <MessageView
                  key={message.id ?? idx}
                  message={message}
                  members={members}
                  className={cn('max-w-full', !passedOn && 'opacity-50')}
                  badges={[
                    ...(message.internal ? ['internal'] : []),
                    ...(message.classification ? [classificationLabel(message.classification)] : []),
                    ...(passedOn ? [] : ['not passed on']),
                  ]}
                  actions={mark?.has_context ? (
                    <button
                      onClick={() => onShowContext(message)}
                      className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
                      title="Show what the agent saw"
                      aria-label="Show what the agent saw"
                    >
                      <ScrollText size={13} />
                    </button>
                  ) : undefined}
                />
              );
            })}
            {messages.length === 0 && (
              <div className="text-sm text-gray-500 dark:text-gray-400">The agent has heard nothing here yet.</div>
            )}
          </>
        )}
      </div>
    </div>
  );
};
