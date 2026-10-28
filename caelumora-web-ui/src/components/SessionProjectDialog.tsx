// The project of a session (issue 467): the one its results go to, and putting it into one of the
// shared projects the user reads, or taking it out into its own. Its own is kept by lifting it,
// which its page offers. Who may is the server's to decide: whoever may change the session, into a
// project they read.

import React, {useState} from 'react';
import {X} from 'lucide-react';
import type {Page} from '../page';
import type {ProjectResponse, SessionSnapshot} from '../types';
import {reportUnlessRefused} from '../errors';
import {optionLabel, projectLabel} from '../project';
import {Refusal} from './Refusal';

interface SessionProjectDialogProps {
  session: SessionSnapshot;
  /** The shared projects the user reads. */
  projects: ProjectResponse[];
  /** The user choosing: whether what is offered is theirs. */
  userId: string;
  /** Puts the session into the project `projectId`, or into its own with `null`. */
  onSubmit: (projectId: string | null) => Promise<void>;
  /** Opens the page of its project. */
  onSelectPage: (page: Page) => void;
  onClose: () => void;
}

export const SessionProjectDialog: React.FC<SessionProjectDialogProps> = ({session, projects, userId, onSubmit, onSelectPage, onClose}) => {
  const current = session.project;
  const offered = projects.filter(p => p.id !== current?.id);
  const [chosen, setChosen] = useState(offered[0]?.id ?? '');
  const [refusal, setRefusal] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);

  const act = async (projectId: string | null) => {
    setIsSaving(true);
    setRefusal(null);
    try {
      await onSubmit(projectId);
      onClose();
    } catch (error) {
      reportUnlessRefused(projectId === null ? 'Failed to take the session out of the project' : 'Failed to put the session into the project', error, setRefusal);
    } finally {
      setIsSaving(false);
    }
  };

  const open = () => {
    if (!current) return;
    onSelectPage({kind: 'project', id: current.id});
    onClose();
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={onClose}
      onKeyDown={(e) => { if (e.key === 'Escape') onClose(); }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="session-project-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="session-project-title" className="flex-1 font-semibold text-sm truncate">Project of {session.name}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          <div className="text-xs text-gray-400">
            What its agents publish goes to its project: with the session where the project is its own, beyond it
            where it is shared. Nothing moves when it changes: only what comes next goes to the other.
          </div>
          <div>
            <span className="text-xs text-gray-400">Its results go to: </span>
            {current
              ? <button type="button" onClick={open} className="hover:underline">{projectLabel(current, session.name)}</button>
              : <span className="italic text-gray-500">a project you do not read</span>}
          </div>

          {session.can_change && offered.length > 0 && (
            <label className="block space-y-1">
              <span className="text-xs text-gray-400">Put it into</span>
              <select
                value={chosen}
                onChange={(e) => setChosen(e.target.value)}
                className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
              >
                {offered.map(p => <option key={p.id} value={p.id}>{optionLabel(p, userId)}</option>)}
              </select>
            </label>
          )}
          {!session.can_change && (
            <div className="text-xs italic text-gray-500">Only who may change the session puts it into a project.</div>
          )}

          <Refusal message={refusal} />
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          {session.can_change && current && current.scope !== 'session' && (
            <button type="button" onClick={() => void act(null)} disabled={isSaving} className="mr-auto rounded-md px-3 py-1 text-sm hover:bg-gray-800 disabled:opacity-50">
              Into its own
            </button>
          )}
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Close
          </button>
          {session.can_change && offered.length > 0 && (
            <button
              type="button"
              onClick={() => void act(chosen)}
              disabled={!chosen || isSaving}
              className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
            >
              Put it there
            </button>
          )}
        </div>
      </div>
    </div>
  );
};
