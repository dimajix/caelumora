// Creating a channel in a running session (issue 09): its name, its
// moderator, its classification - a level and compartments (issue 133) -
// and who of the session takes part besides the user.

import React, {useEffect, useState} from 'react';
import {X} from 'lucide-react';
import type {ActorId, ChannelResponse, Compartment, SessionSnapshot, UserResponse} from '../types';
import {api} from '../api';
import {ApiRequestError, reportError} from '../errors';
import {DEFAULT_MODERATOR, MODERATOR_KINDS, moderatorInfo} from '../moderator';
import {CONFIDENTIALITY_LEVELS, DEFAULT_CLASSIFICATION, confidentialityInfo, type ConfidentialityLevel} from '../confidentiality';

/** The key an `ActorId` is told apart by. */
function actorKey(id: ActorId): string {
  return typeof id === 'string' ? id : JSON.stringify(id);
}

function isUser(id: ActorId, user: UserResponse): boolean {
  return typeof id !== 'string' && id.User === user.id;
}

interface NewChannelDialogProps {
  session: SessionSnapshot;
  user: UserResponse;
  onCreated: (channel: ChannelResponse) => void;
  onClose: () => void;
}

export const NewChannelDialog: React.FC<NewChannelDialogProps> = ({session, user, onCreated, onClose}) => {
  // Everyone but the user, who takes part anyway, and approvers, which every
  // channel gets by itself.
  const candidates = (session.participants ?? [])
    .filter(p => p.kind !== 'approver' && !isUser(p.id, user))
    .sort((l, r) => l.name.localeCompare(r.name));
  const [name, setName] = useState('');
  const [moderator, setModerator] = useState(DEFAULT_MODERATOR);
  const [level, setLevel] = useState<ConfidentialityLevel>(DEFAULT_CLASSIFICATION.level);
  const [compartments, setCompartments] = useState<Compartment[]>([]);
  const [chosenCompartments, setChosenCompartments] = useState<Set<string>>(new Set());
  const [chosen, setChosen] = useState<Set<string>>(() => new Set(candidates.map(p => actorKey(p.id))));
  const [problem, setProblem] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);

  // The configured compartments; none if they cannot be fetched - the
  // channel is then general.
  useEffect(() => {
    api.getCompartments()
      .then(setCompartments)
      .catch(error => reportError('Failed to fetch the compartments', error));
  }, []);

  const toggleCompartment = (name: string) => {
    setChosenCompartments(prev => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name); else next.add(name);
      return next;
    });
  };

  const toggle = (key: string) => {
    setChosen(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim() || isCreating) return;
    setIsCreating(true);
    setProblem(null);
    try {
      const channel = await api.createChannel(session.id, {
        name: name.trim(),
        moderator,
        // Public takes no compartment.
        classification: {level, compartments: level === 'public' ? [] : [...chosenCompartments]},
        participants: candidates.filter(p => chosen.has(actorKey(p.id))).map(p => p.id),
      });
      onCreated(channel);
    } catch (error) {
      // What the server says about the request - a taken name, say - is
      // shown here, next to what can be corrected; anything else as usual.
      if (error instanceof ApiRequestError && error.status < 500 && error.status !== 401) {
        setProblem(error.message);
      } else {
        reportError('Failed to create the channel', error);
      }
    } finally {
      setIsCreating(false);
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
        aria-labelledby="new-channel-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="new-channel-title" className="flex-1 font-semibold text-sm truncate">New channel in {session.name}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Name</span>
            <input
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1 focus:outline-hidden focus:ring-2 focus:ring-blue-500"
            />
          </label>

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Moderator</span>
            <select
              value={moderator}
              onChange={(e) => setModerator(e.target.value)}
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
            >
              {MODERATOR_KINDS.map(kind => (
                <option key={kind} value={kind}>{moderatorInfo(kind).label}</option>
              ))}
            </select>
            <span className="block text-xs text-gray-500">{moderatorInfo(moderator).description}</span>
          </label>

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Confidentiality</span>
            <select
              value={level}
              onChange={(e) => setLevel(e.target.value as ConfidentialityLevel)}
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
            >
              {CONFIDENTIALITY_LEVELS.map(l => (
                <option key={l} value={l}>{confidentialityInfo(l).label}</option>
              ))}
            </select>
            <span className="block text-xs text-gray-500">{confidentialityInfo(level).description}</span>
          </label>

          {compartments.length > 0 && (
            <fieldset className="space-y-1" disabled={level === 'public'}>
              <legend className="text-xs text-gray-400">Compartments</legend>
              <div className="text-xs text-gray-500">
                {level === 'public'
                  ? 'Public takes no compartment.'
                  : 'Who it is meant for. Without one, it is general and may not read channels that have one.'}
              </div>
              {compartments.map(compartment => (
                <label key={compartment.name} className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={level !== 'public' && chosenCompartments.has(compartment.name)}
                    onChange={() => toggleCompartment(compartment.name)}
                  />
                  <span>{compartment.name}</span>
                </label>
              ))}
            </fieldset>
          )}

          <fieldset className="space-y-1">
            <legend className="text-xs text-gray-400">Participants</legend>
            <div className="text-xs text-gray-500">You take part in any case.</div>
            {candidates.map(participant => {
              const key = actorKey(participant.id);
              return (
                <label key={key} className="flex items-center gap-2">
                  <input type="checkbox" checked={chosen.has(key)} onChange={() => toggle(key)} />
                  <span>{participant.name}</span>
                  <span className="text-xs text-gray-500">{participant.kind}</span>
                </label>
              );
            })}
            {candidates.length === 0 && (
              <div className="text-xs italic text-gray-500">Nobody else takes part in this session.</div>
            )}
          </fieldset>

          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Cancel
          </button>
          <button
            type="submit"
            disabled={!name.trim() || isCreating}
            className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
          >
            {isCreating ? 'Creating…' : 'Create'}
          </button>
        </div>
      </form>
    </div>
  );
};
