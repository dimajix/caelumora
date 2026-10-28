// Writing a skill (issue 22): its name, what it is for in a line - what agents choose it by -,
// where it lies - the user's scope or the global one; one of a session's scope is written by an
// agent in it -, its classification, and its text, the procedure agents follow.

import React, {useState} from 'react';
import {X} from 'lucide-react';
import type {SkillResponse} from '../types';
import {api} from '../api';
import {reportUnlessRefused} from '../errors';
import {DEFAULT_CLASSIFICATION, type Classification} from '../confidentiality';
import {ClassificationFields} from './ClassificationFields';

interface NewSkillDialogProps {
  onCreated: (skill: SkillResponse) => void;
  onClose: () => void;
}

export const NewSkillDialog: React.FC<NewSkillDialogProps> = ({onCreated, onClose}) => {
  const [name, setName] = useState('');
  const [scope, setScope] = useState<'user' | 'global'>('user');
  const [description, setDescription] = useState('');
  const [body, setBody] = useState('');
  const [classification, setClassification] = useState<Classification>(DEFAULT_CLASSIFICATION);
  const [problem, setProblem] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);
  const complete = name.trim() && description.trim() && body.trim();

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!complete || isCreating) return;
    setIsCreating(true);
    setProblem(null);
    try {
      onCreated(await api.createSkill({name: name.trim(), scope, description: description.trim(), body, classification}));
    } catch (error) {
      // What the server says about the request - a taken name, a text too
      // long, a classification the user is not cleared for - is shown here.
      reportUnlessRefused('Failed to create the skill', error, setProblem);
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
        aria-labelledby="new-skill-title"
        className="w-full max-w-xl rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="new-skill-title" className="flex-1 font-semibold text-sm">New skill</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Name</span>
            <input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="release-notes" className={input} />
          </label>

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">What it is for</span>
            <input value={description} onChange={(e) => setDescription(e.target.value)} placeholder="How we write a release note" className={input} />
            <span className="block text-xs text-gray-500">One line: agents choose a skill by it.</span>
          </label>

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Name space</span>
            <select value={scope} onChange={(e) => setScope(e.target.value as 'user' | 'global')} className={input}>
              <option value="user">Yours</option>
              <option value="global">Global</option>
            </select>
            <span className="block text-xs text-gray-500">
              {scope === 'user'
                ? 'Your agents find it by its name; others only if you share it.'
                : 'Its name is unique among all skills. It is yours all the same.'}
            </span>
          </label>

          <ClassificationFields value={classification} onChange={setClassification} />

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Text</span>
            <textarea
              value={body}
              onChange={(e) => setBody(e.target.value)}
              rows={10}
              placeholder={'1. Collect what was merged since the last release.\n2. Group it by area.'}
              className={`${input} font-mono`}
            />
            <span className="block text-xs text-gray-500">Markdown: the steps an agent follows when it loads the skill.</span>
          </label>

          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
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
