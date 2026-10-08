// All that was offered to an agent in a channel and what it said itself
// (issue 207): its history, for the agent as a whole rather than for one
// message. It may differ from what the channel shows - an agent that joined
// late or was asleep heard what it heard. What its perceive processors did
// not pass on to its transcript is marked; what each of its own messages was
// produced from opens as that message's context. The turns it gave up (issue
// 146) are shown where they branched off, folded, with why they ended and
// what they did.

import React, {useEffect, useState} from 'react';
import {ChevronDown, ChevronRight, History, ScrollText, Undo2, X} from 'lucide-react';
import type {ChannelMember, GivenUpTurn, HistoryMessage, Message} from '../types';
import {classificationLabel} from '../confidentiality';
import {clsx as cn} from 'clsx';
import {api} from '../api';
import {actorUuid} from '../mention';
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
  /** Opens the latest turn given up - when the panel was asked what a failed turn did. */
  openLatestGivenUp?: boolean;
  /** Shows the context of one of the agent's messages. */
  onShowContext: (message: Message) => void;
  onClose: () => void;
}

/** A row of the panel: messages of the history, merged with their tool results, or a turn given up. */
type Row = {kind: 'message'; message: Message} | {kind: 'turn'; turn: GivenUpTurn; index: number};

/**
 * The rows of `history` with each of `turns` after the message it follows -
 * first if after none, last if the history does not hold that message. The
 * history holds a turn's messages as well (issue 258); they are shown in the
 * turn's block alone.
 */
function rowsOf(history: HistoryMessage[], turns: GivenUpTurn[]): Row[] {
  const ofTurns = new Set(turns.flatMap(turn => turn.messages.map(entry => entry.message.id)));
  history = history.filter(entry => !ofTurns.has(entry.message.id));
  const held = new Set(history.map(entry => entry.message.id));
  const turnRows = (after: string | null | undefined) => turns
    .map((turn, index): Row => ({kind: 'turn', turn, index}))
    .filter(row => row.kind === 'turn' && (after === undefined ? row.turn.after !== null && !held.has(row.turn.after) : row.turn.after === after));
  const rows: Row[] = turnRows(null);
  let pending: Message[] = [];
  const flush = () => {
    rows.push(...mergeToolResults(pending).map((message): Row => ({kind: 'message', message})));
    pending = [];
  };
  for (const entry of history) {
    pending.push(entry.message);
    const after = turnRows(entry.message.id);
    if (after.length > 0) {
      flush();
      rows.push(...after);
    }
  }
  flush();
  rows.push(...turnRows(undefined));
  return rows;
}

export const HistoryPanel: React.FC<HistoryPanelProps> = ({sessionId, channelId, member, members, version, openLatestGivenUp, onShowContext, onClose}) => {
  const actor = actorUuid(member.id);
  const [history, setHistory] = useState<HistoryMessage[] | null>(null);
  const [turns, setTurns] = useState<GivenUpTurn[]>([]);
  const [open, setOpen] = useState<Set<number>>(new Set());
  const [notice, setNotice] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  useEffect(() => {
    if (!actor) return;
    let current = true;
    Promise.all([api.getAgentHistory(sessionId, channelId, actor), api.getGivenUpTurns(sessionId, channelId, actor)])
      .then(([history, turns]) => {
        if (!current) return;
        setHistory(history);
        setTurns(turns);
        if (openLatestGivenUp && turns.length > 0) setOpen(prev => new Set(prev).add(turns.length - 1));
      })
      .catch(error => {
        if (error instanceof ApiRequestError && error.code === 'context_forbidden') {
          if (current) setNotice('Only the owner of this agent can see what it heard.');
        } else {
          reportError('Failed to load the history', error);
        }
      });
    return () => { current = false; };
  }, [sessionId, channelId, actor, version, openLatestGivenUp]);

  const marks = new Map([...(history ?? []), ...turns.flatMap(turn => turn.messages)].map(entry => [entry.message.id, entry]));
  const rows = rowsOf(history ?? [], turns);
  const messageRows = rows.flatMap((row, index) => row.kind === 'message' ? [index] : []);
  const hidden = showAll ? 0 : Math.max(0, messageRows.length - SHOWN_AT_FIRST);
  const firstShown = hidden > 0 ? messageRows[hidden] : 0;

  const toggle = (index: number) => setOpen(prev => {
    const next = new Set(prev);
    if (!next.delete(index)) next.add(index);
    return next;
  });

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

  const turnView = (turn: GivenUpTurn, index: number) => {
    const expanded = open.has(index);
    const at = formatTimestamp(turn.at);
    return (
      <div key={`turn-${index}`} className="rounded-lg border border-dashed border-amber-400 dark:border-amber-700 bg-amber-50/60 dark:bg-amber-900/20">
        <button
          onClick={() => toggle(index)}
          className="w-full flex items-start gap-2 p-2 text-left text-xs text-amber-900 dark:text-amber-200"
          aria-expanded={expanded}
        >
          {expanded ? <ChevronDown size={14} className="mt-0.5 shrink-0" /> : <ChevronRight size={14} className="mt-0.5 shrink-0" />}
          <Undo2 size={14} className="mt-0.5 shrink-0" />
          <span className="flex-1">
            <span className="font-semibold">Turn given up</span>
            {at && <span className="opacity-70" title={at.full}> · {at.short}</span>}
            <span className="block">{givenUpText(turn.reason)}</span>
          </span>
        </button>
        {expanded && (
          <div className="p-2 pt-0 space-y-3">
            <div className="text-xs text-gray-500 dark:text-gray-400">
              What the turn produced. It is not part of what the agent heard and said, but what its tools did stays done.
            </div>
            {mergeToolResults(turn.messages.map(entry => entry.message)).map((message, idx) => messageView(message, message.id ?? idx))}
          </div>
        )}
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
          Greyed out is what it did not pass on to its transcript; the turns it gave up are folded in where they began.
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
            {rows.slice(firstShown).map((row, idx) => row.kind === 'turn'
              ? turnView(row.turn, row.index)
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
