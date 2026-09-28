// Adding a participant of the session to a running channel (issue 46), with
// how much of the channel's history they may see.

import React, {useEffect, useState} from 'react';
import {X} from 'lucide-react';
import type {ChannelResponse, HistoryAccess, SessionParticipant} from '../types';
import {api} from '../api';
import {ApiRequestError, reportError} from '../errors';
import {actorKey} from '../mention';

type HistoryKind = HistoryAccess['kind'];

const HISTORY_CHOICES: {kind: HistoryKind; label: string}[] = [
  {kind: 'all', label: 'All of it'},
  {kind: 'none', label: 'None - only what is written from now on'},
  {kind: 'last_messages', label: 'The last messages'},
  {kind: 'last_days', label: 'The last days'},
];

function historyAccess(kind: HistoryKind, count: number): HistoryAccess {
  return kind === 'last_messages' || kind === 'last_days' ? {kind, count} : {kind};
}

interface AddParticipantDialogProps {
  sessionId: string;
  channel: ChannelResponse;
  onAdded: (channel: ChannelResponse) => void;
  onClose: () => void;
}

export const AddParticipantDialog: React.FC<AddParticipantDialogProps> = ({sessionId, channel, onAdded, onClose}) => {
  // The participants of the session who are not in the channel yet; `null`
  // while they are loaded.
  const [candidates, setCandidates] = useState<SessionParticipant[] | null>(null);
  const [chosen, setChosen] = useState<string>('');
  const [historyKind, setHistoryKind] = useState<HistoryKind>('all');
  const [count, setCount] = useState(10);
  const [problem, setProblem] = useState<string | null>(null);
  const [isAdding, setIsAdding] = useState(false);

  useEffect(() => {
    const members = new Set(channel.members.map(m => actorKey(m.id)));
    api.getSession(sessionId)
      .then(session => {
        const outside = (session.participants ?? [])
          .filter(p => p.kind !== 'approver' && !members.has(actorKey(p.id)))
          .sort((l, r) => l.name.localeCompare(r.name));
        setCandidates(outside);
        setChosen(outside[0] ? actorKey(outside[0].id) : '');
      })
      .catch(error => reportError('Failed to load the participants of the session', error));
  }, [sessionId, channel]);

  const participant = candidates?.find(p => actorKey(p.id) === chosen);
  const needsCount = historyKind === 'last_messages' || historyKind === 'last_days';

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!participant || isAdding) return;
    setIsAdding(true);
    setProblem(null);
    try {
      const updated = await api.addParticipant(sessionId, channel.id, {
        participant: participant.id,
        history: historyAccess(historyKind, count),
      });
      onAdded(updated);
    } catch (error) {
      // What the server says about the request is shown here, next to what
      // can be corrected; anything else as usual.
      if (error instanceof ApiRequestError && error.status < 500 && error.status !== 401) {
        setProblem(error.message);
      } else {
        reportError('Failed to add the participant', error);
      }
    } finally {
      setIsAdding(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={onClose}
      onKeyDown={(e) => { if (e.key === 'Escape') onClose(); }}
    >
      <form
        onSubmit={submit}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="add-participant-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="add-participant-title" className="flex-1 font-semibold text-sm truncate">Add to {channel.name}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          {candidates === null ? (
            <div className="text-xs italic text-gray-500">Loading…</div>
          ) : candidates.length === 0 ? (
            <div className="text-xs italic text-gray-500">Everyone in this session takes part already.</div>
          ) : (<>
            <label className="block space-y-1">
              <span className="text-xs text-gray-400">Participant</span>
              <select
                autoFocus
                value={chosen}
                onChange={(e) => setChosen(e.target.value)}
                className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
              >
                {candidates.map(p => (
                  <option key={actorKey(p.id)} value={actorKey(p.id)}>{p.name} ({p.kind})</option>
                ))}
              </select>
            </label>

            <fieldset className="space-y-1">
              <legend className="text-xs text-gray-400">History they may see</legend>
              {HISTORY_CHOICES.map(choice => (
                <label key={choice.kind} className="flex items-center gap-2">
                  <input
                    type="radio"
                    name="history"
                    checked={historyKind === choice.kind}
                    onChange={() => setHistoryKind(choice.kind)}
                  />
                  <span>{choice.label}</span>
                </label>
              ))}
              {needsCount && (
                <label className="flex items-center gap-2 pl-6">
                  <input
                    type="number"
                    min={1}
                    value={count}
                    onChange={(e) => setCount(Math.max(1, Math.floor(Number(e.target.value) || 1)))}
                    className="w-20 rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
                  />
                  <span className="text-xs text-gray-400">{historyKind === 'last_days' ? 'days' : 'messages'}</span>
                </label>
              )}
              <div className="text-xs text-gray-500">Notes stay visible to everyone in the channel.</div>
            </fieldset>
          </>)}

          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Cancel
          </button>
          <button
            type="submit"
            disabled={!participant || isAdding}
            className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
          >
            {isAdding ? 'Adding…' : 'Add'}
          </button>
        </div>
      </form>
    </div>
  );
};
