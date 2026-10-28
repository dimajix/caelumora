// Making a shared project (issue 467): its name, where it lies - the user's scope or the global one -
// and what it is for; a session's own is made with the session. Nobody works on it yet: its members
// are chosen on its page.

import React, {useState} from 'react';
import {X} from 'lucide-react';
import type {ProjectResponse} from '../types';
import {api} from '../api';
import {reportUnlessRefused} from '../errors';
import {Refusal} from './Refusal';

interface NewProjectDialogProps {
  onCreated: (project: ProjectResponse) => void;
  onClose: () => void;
}

export const NewProjectDialog: React.FC<NewProjectDialogProps> = ({onCreated, onClose}) => {
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [scope, setScope] = useState<'user' | 'global'>('user');
  const [refusal, setRefusal] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);
  const complete = name.trim();

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!complete || isCreating) return;
    setIsCreating(true);
    setRefusal(null);
    try {
      onCreated(await api.createProject({name: name.trim(), scope, description: description.trim() || undefined}));
    } catch (error) {
      // What the server says about the request - a taken name, say - is
      // shown here.
      reportUnlessRefused('Failed to create the project', error, setRefusal);
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
        aria-labelledby="new-project-title"
        className="w-full max-w-md rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="new-project-title" className="flex-1 font-semibold text-sm">New project</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Name</span>
            <input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="annual-report" className={input} />
          </label>

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">What it is for</span>
            <input value={description} onChange={(e) => setDescription(e.target.value)} placeholder="The annual report" className={input} />
          </label>

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Name space</span>
            <select value={scope} onChange={(e) => setScope(e.target.value as 'user' | 'global')} className={input}>
              <option value="user">Yours</option>
              <option value="global">Global</option>
            </select>
            <span className="block text-xs text-gray-500">
              {scope === 'user'
                ? 'Its name is unique among your projects.'
                : 'Its name is unique among all projects. It is yours all the same.'}
            </span>
          </label>

          <div className="text-xs text-gray-500">
            Nobody works on it yet: you choose its members on its page. Put sessions into it, and what they publish outlives them.
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
