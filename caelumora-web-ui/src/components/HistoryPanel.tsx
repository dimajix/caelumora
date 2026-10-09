// All that was offered to an agent in a channel and what it said itself
// (issue 207): its history, for the agent as a whole rather than for one
// message. It may differ from what the channel shows - an agent that joined
// late or was asleep heard what it heard. What its perceive processors did
// not pass on to its transcript is marked; what each of its own messages was
// produced from opens as that message's context. A turn it gave up is shown
// where it ended, after what it said, with why (issues 146, 258), and a
// command it carried out where it did, with on whose word (issue 269).

import React, {useEffect, useState} from 'react';
import {ChevronRight, History, ScrollText, Undo2, X} from 'lucide-react';
import type {ActorId, ChannelMember, GivenUpReason, HistoryItem, HistoryMessage, Message} from '../types';
import {classificationLabel} from '../confidentiality';
import {cn} from '../classNames';
import {api} from '../api';
import {actorKey, actorUuid} from '../mention';
import {ApiRequestError, reportError} from '../errors';
import {MessageView} from './MessageView';
import {formatTimestamp, givenUpText, mergeToolResults} from '../messages';

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

/** A row of the panel: a message of the history, merged with its tool results, or a turn given up. */
type Row =
  | {kind: 'message'; message: Message}
  | {kind: 'givenUp'; turnId: string; reason: GivenUpReason; at: string}
  | {kind: 'command'; turnId: string; command: string; args: string; issuer: ActorId; at: string};

/**
 * The rows of `items`, in the order they happened: the messages, each run of
 * them with its tool results merged in, every turn given up where it ended,
 * and every command carried out where it was. A turn that finished shows
 * nothing of its own.
 */
function rowsOf(items: HistoryItem<HistoryMessage>[]): Row[] {
  const rows: Row[] = [];
  let pending: Message[] = [];
  const flush = () => {
    rows.push(...mergeToolResults(pending).map((message): Row => ({kind: 'message', message})));
    pending = [];
  };
  for (const item of items) {
    if ('said' in item) {
      pending.push(item.said.message);
    } else if ('turn_given_up' in item) {
      flush();
      const {turn_id, reason, at} = item.turn_given_up;
      rows.push({kind: 'givenUp', turnId: turn_id, reason, at});
    } else if ('command_executed' in item) {
      flush();
      const {turn_id, command, args, issuer, at} = item.command_executed;
      rows.push({kind: 'command', turnId: turn_id, command, args, issuer, at});
    }
  }
  flush();
  return rows;
}

export const HistoryPanel: React.FC<HistoryPanelProps> = ({sessionId, channelId, member, members, version, onShowContext, onClose}) => {
  const actor = actorUuid(member.id);
  const [items, setItems] = useState<HistoryItem<HistoryMessage>[] | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  useEffect(() => {
    if (!actor) return;
    let current = true;
    api.getAgentHistory(sessionId, channelId, actor)
      .then(items => { if (current) setItems(items); })
      .catch(error => {
        if (error instanceof ApiRequestError && error.code === 'context_forbidden') {
          if (current) setNotice('Only the owner of this agent can see what it heard.');
        } else {
          reportError('Failed to load the history', error);
        }
      });
    return () => { current = false; };
  }, [sessionId, channelId, actor, version]);

  const marks = new Map((items ?? []).flatMap(item => 'said' in item ? [[item.said.message.id, item.said] as const] : []));
  const rows = rowsOf(items ?? []);
  const messageRows = rows.flatMap((row, index) => row.kind === 'message' ? [index] : []);
  const hidden = showAll ? 0 : Math.max(0, messageRows.length - SHOWN_AT_FIRST);
  const firstShown = hidden > 0 ? messageRows[hidden] : 0;

  const messageView = (message: Message, key: React.Key) => {
    const mark = message.id ? marks.get(message.id) : undefined;
    const passedOn = mark?.in_transcript ?? true;
    return (
      <MessageView
        key={key}
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
  };

  const givenUpView = (reason: GivenUpReason, at: string, key: React.Key) => {
    const time = formatTimestamp(at);
    return (
      <div key={key} className="flex items-start gap-2 p-2 rounded-lg border border-dashed border-amber-400 dark:border-amber-700 bg-amber-50/60 dark:bg-amber-900/20 text-xs text-amber-900 dark:text-amber-200">
        <Undo2 size={14} className="mt-0.5 shrink-0" />
        <span className="flex-1">
          <span className="font-semibold">Turn given up</span>
          {time && <span className="opacity-70" title={time.full}> · {time.short}</span>}
          <span className="block">{givenUpText(reason)} What its tools did stays done.</span>
        </span>
      </div>
    );
  };

  // A command the agent carried out (issue 269): which, on whose word, when.
  const commandView = (row: Extract<Row, {kind: 'command'}>, key: React.Key) => {
    const time = formatTimestamp(row.at);
    const issuer = members.find(m => actorKey(m.id) === actorKey(row.issuer))?.name ?? 'someone';
    return (
      <div key={key} className="flex items-start gap-2 p-2 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-50 dark:bg-gray-800/60 text-xs text-gray-700 dark:text-gray-300">
        <ChevronRight size={14} className="mt-0.5 shrink-0" />
        <span className="flex-1">
          <span className="font-semibold">Carried out <span className="font-mono">/{row.command}{row.args ? ` ${row.args}` : ''}</span> for {issuer}</span>
          {time && <span className="opacity-70" title={time.full}> · {time.short}</span>}
        </span>
      </div>
    );
  };

  return (
    <div className="w-full border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex flex-col min-h-0">
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
          Greyed out is what it did not pass on to its transcript; a turn it gave up is marked where it ended.
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {notice && <div className="text-sm text-gray-500 dark:text-gray-400">{notice}</div>}
        {!items && !notice && <div className="text-sm text-gray-500 dark:text-gray-400">Loading…</div>}
        {items && (
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
            {rows.slice(firstShown).map((row, idx) => row.kind === 'givenUp'
              ? givenUpView(row.reason, row.at, `given-up-${row.turnId}`)
              : row.kind === 'command'
                ? commandView(row, `command-${row.turnId}`)
                : messageView(row.message, row.message.id ?? `message-${idx}`))}
            {rows.length === 0 && (
              <div className="text-sm text-gray-500 dark:text-gray-400">The agent has heard nothing here yet.</div>
            )}
          </>
        )}
      </div>
    </div>
  );
};
