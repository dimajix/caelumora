// All that was offered to an agent in a channel and what it said itself
// (issue 207): its history, for the agent as a whole rather than for one
// message. It may differ from what the channel shows - an agent that joined
// late or was asleep heard what it heard. What its perceive processors did
// not pass on to its transcript is marked; what each of its own messages was
// produced from opens as that message's context. A turn it gave up is shown
// where it ended, after what it said, with why (issues 146, 258), a command
// it carried out where it did, with on whose word (issue 269), and where it
// compacted its context (issue 16). It is read in pages from the end
// (issue 218), older ones on request.

import React, {useEffect, useRef, useState} from 'react';
import {ChevronRight, FoldVertical, History, ScrollText, Undo2, UserMinus, UserPlus, X} from 'lucide-react';
import type {ActorId, ChannelParticipant, GivenUpReason, HistoryItem, HistoryMessage, HistoryPage, Message} from '../types';
import {classificationLabel} from '../confidentiality';
import {cn} from '../classNames';
import {api, type ChannelAttachments} from '../api';
import {actorUuid, sameActor} from '../actor';
import {ApiRequestError, reportError} from '../errors';
import {MessageView} from './MessageView';
import {formatTimestamp, givenUpText, mergeToolResultsAmong, participantNotice} from '../messages';

interface HistoryPanelProps {
  sessionId: string;
  channelId: string;
  /** The attachments of the channel's messages, where what its messages carry is downloaded from (issue 282). */
  attachments: ChannelAttachments;
  /** The agent whose history is shown. */
  participant: ChannelParticipant;
  /** Who views it, whose messages are their own (issue 407). */
  me: ActorId;
  participants: ChannelParticipant[];
  /** Changes whenever the history may have grown, to load it again. */
  version: number;
  /** Shows the context of one of the agent's messages. */
  onShowContext: (message: Message) => void;
  onClose: () => void;
}

/**
 * A row of the panel: a message of the history, merged with its tool results,
 * a turn given up, a command carried out, the context compacted, or the
 * notice of someone who joined or left (issue 408).
 */
type Row =
  | {kind: 'message'; message: Message}
  | {kind: 'givenUp'; turnId: string; reason: GivenUpReason; at: string}
  | {kind: 'command'; turnId: string | null; command: string; args: string; issuer: ActorId; at: string}
  | {kind: 'compacted'; turnId: string; at: string}
  | {kind: 'participant'; notice: Message};

/**
 * The pages of a history read again from its end back to `until` - the
 * `earlier` of the oldest page shown, `null` if all of it was - one after the
 * other with `read`, joined into one: what is shown, as it stands now, with
 * what was added since.
 */
async function readBack<M>(read: (before: number | null) => Promise<HistoryPage<M>>, until: number | null): Promise<HistoryPage<M>> {
  let page = await read(null);
  let items = page.items;
  while (page.earlier !== null && (until === null || page.earlier > until)) {
    page = await read(page.earlier);
    items = [...page.items, ...items];
  }
  return {items, earlier: page.earlier};
}

/**
 * The rows of `items`, in the order they happened: the messages, with their tool results merged in
 * (`mergeToolResultsAmong`), every turn given up where it ended, every command carried out where
 * it was, every compaction where it was made, and who joined or left where they did. A turn that
 * finished shows nothing of its own.
 */
function rowsOf(items: HistoryItem<HistoryMessage>[]): Row[] {
  const rows = items.flatMap((item): Row[] => {
    if ('said' in item) return [{kind: 'message', message: item.said.message}];
    if ('turn_given_up' in item) {
      const {turn_id, reason, at} = item.turn_given_up;
      return [{kind: 'givenUp', turnId: turn_id, reason, at}];
    }
    if ('command_executed' in item) {
      const {turn_id, command, args, issuer, at} = item.command_executed;
      return [{kind: 'command', turnId: turn_id, command, args, issuer, at}];
    }
    if ('compacted' in item) {
      const {turn_id, at} = item.compacted;
      return [{kind: 'compacted', turnId: turn_id, at}];
    }
    if ('participant_joined' in item) {
      const {participant, name, by, by_name, at} = item.participant_joined;
      return [{kind: 'participant', notice: participantNotice('joined', participant, name, by, by_name, at)}];
    }
    if ('participant_left' in item) {
      const {participant, name, by, by_name, at} = item.participant_left;
      return [{kind: 'participant', notice: participantNotice('left', participant, name, by, by_name, at)}];
    }
    return [];
  });
  return mergeToolResultsAmong(rows, row => row.kind === 'message' ? row.message : undefined, (_, message) => ({kind: 'message', message}));
}

export const HistoryPanel: React.FC<HistoryPanelProps> = ({sessionId, channelId, attachments, participant, me, participants, version, onShowContext, onClose}) => {
  const actor = actorUuid(participant.id);
  const [items, setItems] = useState<HistoryItem<HistoryMessage>[] | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // What to read the page before those shown with; `null` once there is none.
  const [earlier, setEarlier] = useState<number | null>(null);
  const [isLoadingEarlier, setIsLoadingEarlier] = useState(false);
  // How far back what is shown goes, for reading it again when it grew:
  // `undefined` while nothing is shown. The panel is made anew for another
  // agent.
  const shownBack = useRef<number | null | undefined>(undefined);

  const show = (page: HistoryPage<HistoryMessage>) => {
    setItems(page.items);
    setEarlier(page.earlier);
    shownBack.current = page.earlier;
  };

  useEffect(() => {
    if (!actor) return;
    let current = true;
    const read = (before: number | null) => api.getAgentHistory(sessionId, channelId, actor, before);
    const until = shownBack.current;
    (until === undefined ? read(null) : readBack(read, until))
      .then(page => { if (current) show(page); })
      .catch(error => {
        if (error instanceof ApiRequestError && error.code === 'context_forbidden') {
          if (current) setNotice('Only the owner of this agent can see what it heard.');
        } else {
          reportError('Failed to load the history', error);
        }
      });
    return () => { current = false; };
  }, [sessionId, channelId, actor, version]);

  /** Puts the page before those shown in front of them. */
  const loadEarlier = async () => {
    if (!actor || earlier === null) return;
    setIsLoadingEarlier(true);
    try {
      const page = await api.getAgentHistory(sessionId, channelId, actor, earlier);
      show({items: [...page.items, ...(items ?? [])], earlier: page.earlier});
    } catch (error) {
      reportError('Failed to load earlier messages', error);
    } finally {
      setIsLoadingEarlier(false);
    }
  };

  const marks = new Map((items ?? []).flatMap(item => 'said' in item ? [[item.said.message.id, item.said] as const] : []));
  const rows = rowsOf(items ?? []);

  const messageView = (message: Message, key: React.Key) => {
    const mark = message.id ? marks.get(message.id) : undefined;
    const passedOn = mark?.in_transcript ?? true;
    return (
      <MessageView
        key={key}
        message={message}
        me={me}
        participants={participants}
        attachments={attachments}
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
    const issuer = participants.find(m => sameActor(m.id, row.issuer))?.name ?? 'someone';
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

  // A compaction (issue 16): what follows replaced the older messages.
  const compactedView = (at: string, key: React.Key) => {
    const time = formatTimestamp(at);
    return (
      <div key={key} className="flex items-start gap-2 p-2 rounded-lg border border-dashed border-sky-300 dark:border-sky-700 bg-sky-50/60 dark:bg-sky-900/20 text-xs text-sky-900 dark:text-sky-200">
        <FoldVertical size={14} className="mt-0.5 shrink-0" />
        <span className="flex-1">
          <span className="font-semibold">Context compacted</span>
          {time && <span className="opacity-70" title={time.full}> · {time.short}</span>}
          <span className="block">From here on the agent was given what follows in place of what came before; what it saw before stays recorded.</span>
        </span>
      </div>
    );
  };

  // Someone joined or left the channel (issue 408), as the agent was told.
  const participantView = (notice: Message, key: React.Key) => {
    const time = formatTimestamp(notice.timestamp);
    return (
      <div key={key} className="flex items-start gap-2 p-2 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-50 dark:bg-gray-800/60 text-xs text-gray-700 dark:text-gray-300">
        {notice.participant === 'joined' ? <UserPlus size={14} className="mt-0.5 shrink-0" /> : <UserMinus size={14} className="mt-0.5 shrink-0" />}
        <span className="flex-1">
          <span className="font-semibold">{notice.content}</span>
          {time && <span className="opacity-70" title={time.full}> · {time.short}</span>}
        </span>
      </div>
    );
  };

  const rowView = (row: Row, idx: number) => {
    switch (row.kind) {
      case 'givenUp': return givenUpView(row.reason, row.at, `given-up-${row.turnId}`);
      case 'command': return commandView(row, `command-${row.turnId ?? idx}`);
      case 'compacted': return compactedView(row.at, `compacted-${row.turnId}-${idx}`);
      case 'participant': return participantView(row.notice, row.notice.id ?? `participant-${idx}`);
      case 'message': return messageView(row.message, row.message.id ?? `message-${idx}`);
    }
  };

  return (
    <div className="w-full border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 flex flex-col min-h-0">
      <div className="p-4 border-b border-gray-200 dark:border-gray-700 space-y-2">
        <div className="flex items-center gap-2 text-gray-700 dark:text-gray-200">
          <History size={18} />
          <h3 className="font-semibold text-sm flex-1">What {participant.name} heard and said</h3>
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
            {earlier !== null && (
              <div className="text-center">
                <button
                  onClick={loadEarlier}
                  disabled={isLoadingEarlier}
                  className="text-xs px-3 py-1 rounded-md border border-gray-300 dark:border-gray-600 text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-800 disabled:opacity-50"
                >
                  {isLoadingEarlier ? 'Loading…' : 'Show earlier messages'}
                </button>
              </div>
            )}
            {rows.map(rowView)}
            {rows.length === 0 && (
              <div className="text-sm text-gray-500 dark:text-gray-400">The agent has heard nothing here yet.</div>
            )}
          </>
        )}
      </div>
    </div>
  );
};
