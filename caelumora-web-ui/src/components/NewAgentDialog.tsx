// Making an agent of a personality (issue 227): its name and its personality, in the user's own
// scope - one of a session's is made from the dialog that adds someone to a channel. A
// personality of a session's scope makes no agent outside it and is not offered.

import React, {useEffect, useState} from 'react';
import {X} from 'lucide-react';
import type {AgentResponse, PersonalityResponse} from '../types';
import {api} from '../api';
import {reportError, reportUnlessRefused} from '../errors';
import {Refusal} from './Refusal';

interface NewAgentDialogProps {
  onCreated: (agent: AgentResponse) => void;
  onClose: () => void;
}

export const NewAgentDialog: React.FC<NewAgentDialogProps> = ({onCreated, onClose}) => {
  // The personalities that make an agent of the user's own; `null` while they are loaded.
  const [personalities, setPersonalities] = useState<PersonalityResponse[] | null>(null);
  const [personalityId, setPersonalityId] = useState('');
  const [name, setName] = useState('');
  const [refusal, setRefusal] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);

  useEffect(() => {
    let current = true;
    api.getPersonalities()
      .then(found => {
        if (!current) return;
        const usable = found.filter(p => p.scope !== 'session');
        setPersonalities(usable);
        setPersonalityId(usable[0]?.id ?? '');
      })
      .catch(error => reportError('Failed to fetch the personalities', error));
    return () => { current = false; };
  }, []);

  const personality = personalities?.find(p => p.id === personalityId);
  const complete = name.trim() && personality;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!complete || isCreating) return;
    setIsCreating(true);
    setRefusal(null);
    try {
      onCreated(await api.createAgent({name: name.trim(), personality_id: personality.id}));
    } catch (error) {
      // What the server says about the request - a taken name, say - is
      // shown here.
      reportUnlessRefused('Failed to create the agent', error, setRefusal);
    } finally {
      setIsCreating(false);
    }
  };

  const input = "w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1 focus:outline-hidden focus:ring-2 focus:ring-blue-500";

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
        aria-labelledby="new-agent-title"
        className="w-full max-w-md rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="new-agent-title" className="flex-1 font-semibold text-sm">New agent</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Name</span>
            <input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="reviewer" className={input} />
          </label>

          {personalities === null ? (
            <div className="text-xs italic text-gray-500">Loading…</div>
          ) : personalities.length === 0 ? (
            <div className="text-xs italic text-gray-500">There is no personality you may make an agent of.</div>
          ) : (
            <label className="block space-y-1">
              <span className="text-xs text-gray-400">Personality</span>
              <select value={personalityId} onChange={(e) => setPersonalityId(e.target.value)} className={input}>
                {personalities.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}
              </select>
              {personality?.description && <span className="block text-xs text-gray-500">{personality.description}</span>}
            </label>
          )}

          <div className="text-xs text-gray-500">
            The agent is yours: it acts with your rights and keeps what it learns from session to
            session. Its name is unique among your agents.
          </div>

          <Refusal message={refusal} />
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Cancel
          </button>
          <button
            type="submit"
            disabled={!complete || isCreating}
            className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
          >
            {isCreating ? 'Creating…' : 'Create'}
          </button>
        </div>
      </form>
    </div>
  );
};
