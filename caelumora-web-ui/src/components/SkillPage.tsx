// One skill (issue 22), in the place of a channel: what it is - its name, what it is for, where it
// lies, its classification - and its text, the procedure agents follow when it is loaded, which
// its owner edits here. Who else reads it reads it here; nobody but its owner changes it.

import React, {useEffect, useState} from 'react';
import {BookOpen, Edit2, FileText, Shield, Trash2} from 'lucide-react';
import type {Compartment, SkillResponse} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {CONFIDENTIALITY_LEVELS, classificationDescription, classificationLabel} from '../confidentiality';
import {ActionMenu} from './ActionMenu';
import {ClassificationDialog} from './ClassificationDialog';
import {MarkdownMessage} from './MarkdownMessage';
import {scopeLabel} from '../scope';
import {isSystem} from '../actor';
import {EditInPlace} from './EditInPlace';

interface SkillPageProps {
  skillId: string;
  /** The user looking at it: whether it is theirs. */
  userId: string;
  onDeleted: (skillId: string) => void;
}

/** The sidebar learns of a change from the global stream. */
export const SkillPage: React.FC<SkillPageProps> = ({skillId, userId, onDeleted}) => {
  const [skill, setSkill] = useState<SkillResponse | null>(null);
  const [editing, setEditing] = useState<'name' | 'description' | null>(null);
  // The text being edited, while it is; `null` while it is shown.
  const [draft, setDraft] = useState<string | null>(null);
  const [isReclassifying, setIsReclassifying] = useState(false);
  const [compartments, setCompartments] = useState<Compartment[]>([]);

  useEffect(() => {
    let current = true;
    api.getSkill(skillId)
      .then(found => { if (current) setSkill(found); })
      .catch(error => reportError('Failed to load the skill', error));
    api.getCompartments()
      .then(found => { if (current) setCompartments(found); })
      .catch(error => reportError('Failed to fetch the compartments', error));
    return () => { current = false; };
  }, [skillId]);

  if (!skill) return null;

  const rename = async (name: string) => {
    if (name) setSkill(await api.renameSkill(skill.id, name));
  };

  const describe = async (description: string) => {
    if (description) setSkill(await api.describeSkill(skill.id, description));
  };

  const saveText = async () => {
    if (draft === null) return;
    try {
      setSkill(await api.rewriteSkill(skill.id, draft));
      setDraft(null);
    } catch (error) {
      reportError('Failed to save the skill', error);
    }
  };

  const remove = async () => {
    try {
      await api.deleteSkill(skill.id);
      onDeleted(skill.id);
    } catch (error) {
      reportError('Failed to delete the skill', error);
    }
  };

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="max-w-3xl mx-auto p-6 space-y-6">
        <div className="flex items-start gap-3">
          <BookOpen size={22} className="mt-1 shrink-0 text-gray-400" />
          <div className="flex-1 min-w-0 space-y-1">
            {editing === 'name' ? <EditInPlace initial={skill.name} save={rename} action="Failed to rename the skill" onDone={() => setEditing(null)} /> : <h2 className="text-xl font-semibold truncate text-gray-900 dark:text-gray-100">{skill.name}</h2>}
            {editing === 'description' ? <EditInPlace initial={skill.description} save={describe} action="Failed to describe the skill" onDone={() => setEditing(null)} /> : <p className="text-sm text-gray-600 dark:text-gray-400">{skill.description}</p>}
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-500 dark:text-gray-400">
              <span>Name space: {scopeLabel(skill, userId)}</span>
              {isSystem(skill.owner) && <span>A skill of the system</span>}
              <span className="flex items-center gap-1" title={classificationDescription(skill.classification)}>
                <Shield size={12} /> {classificationLabel(skill.classification)}
              </span>
            </div>
          </div>
          {skill.can_change && (
            <ActionMenu
              label={`Options of ${skill.name}`}
              size={16}
              className="p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800"
              actions={[
                {label: 'Rename', icon: Edit2, onSelect: () => setEditing('name')},
                {label: 'Description', icon: FileText, onSelect: () => setEditing('description')},
                {label: 'Edit text', icon: Edit2, onSelect: () => setDraft(skill.body ?? '')},
                {label: 'Classification', icon: Shield, onSelect: () => setIsReclassifying(true)},
                {label: 'Delete', icon: Trash2, danger: true, confirm: 'Delete skill?', onSelect: () => void remove()},
              ]}
            />
          )}
        </div>

        {skill.scope === 'session' && (
          <div className="text-sm text-gray-600 dark:text-gray-400">
            An agent wrote it for one session: it is loaded there alone, and goes with the session.
          </div>
        )}

        {draft === null ? (
          <div className="rounded-lg border border-gray-200 dark:border-gray-700 p-4 text-sm">
            <MarkdownMessage content={skill.body ?? ''} />
          </div>
        ) : (
          <div className="space-y-2">
            <textarea
              autoFocus
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Escape') setDraft(null); }}
              rows={20}
              className="w-full rounded-md border border-blue-500 bg-white dark:bg-gray-800 p-3 font-mono text-sm text-gray-900 dark:text-gray-100"
            />
            <div className="flex justify-end gap-2">
              <button type="button" onClick={() => setDraft(null)} className="rounded-md px-3 py-1 text-sm hover:bg-gray-100 dark:hover:bg-gray-800">
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void saveText()}
                disabled={!draft.trim() || draft === skill.body}
                className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
              >
                Save
              </button>
            </div>
          </div>
        )}
      </div>

      {isReclassifying && (
        <ClassificationDialog
          title={`Classification of ${skill.name}`}
          current={skill.classification}
          levels={CONFIDENTIALITY_LEVELS}
          compartments={compartments}
          releaseWarning="It is released to a wider circle: agents in more channels may load it."
          onSubmit={async (classification) => setSkill(await api.reclassifySkill(skill.id, classification))}
          onClose={() => setIsReclassifying(false)}
        />
      )}
    </div>
  );
};
