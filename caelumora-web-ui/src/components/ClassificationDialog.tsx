// Changing the classification of a channel or a message (issue 133). Who may
// is the server's to decide; this offers the levels and compartments there
// are and says so when the change releases something to a wider circle.

import React, {useState} from 'react';
import {X} from 'lucide-react';
import type {Compartment} from '../types';
import {
  type Classification,
  type ConfidentialityLevel,
  classificationLabel,
  confidentialityInfo,
  releases,
  sameClassification,
} from '../confidentiality';
import {reportUnlessRefused} from '../errors';

interface ClassificationDialogProps {
  title: string;
  current: Classification;
  /** The levels to offer, from the most open. */
  levels: ConfidentialityLevel[];
  /** The configured compartments. */
  compartments: Compartment[];
  /** What a release means here, shown when the change is one. */
  releaseWarning: string;
  onSubmit: (classification: Classification) => Promise<void>;
  onClose: () => void;
}

export const ClassificationDialog: React.FC<ClassificationDialogProps> = ({title, current, levels, compartments, releaseWarning, onSubmit, onClose}) => {
  const [level, setLevel] = useState<ConfidentialityLevel>(current.level);
  const [chosen, setChosen] = useState<Set<string>>(() => new Set(current.compartments ?? []));
  const [problem, setProblem] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  // Public takes no compartment.
  const next: Classification = level === 'public' ? {level} : {level, compartments: [...chosen]};
  const unchanged = sameClassification(next, current);

  const toggle = (name: string) => {
    setChosen(prev => {
      const updated = new Set(prev);
      if (updated.has(name)) updated.delete(name); else updated.add(name);
      return updated;
    });
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (unchanged || isSaving) return;
    setIsSaving(true);
    setProblem(null);
    try {
      await onSubmit(next);
      onClose();
    } catch (error) {
      // What the server says about the request - a refusal, say - is shown
      // here; anything else as usual.
      reportUnlessRefused('Failed to change the classification', error, setProblem);
    } finally {
      setIsSaving(false);
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
        aria-labelledby="classification-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="classification-title" className="flex-1 font-semibold text-sm truncate">{title}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          <div className="text-xs text-gray-400">Now: {classificationLabel(current)}</div>
          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Confidentiality</span>
            <select
              value={level}
              onChange={(e) => setLevel(e.target.value as ConfidentialityLevel)}
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
            >
              {levels.map(l => (
                <option key={l} value={l}>{confidentialityInfo(l).label}</option>
              ))}
            </select>
            <span className="block text-xs text-gray-500">{confidentialityInfo(level).description}</span>
          </label>

          {compartments.length > 0 && (
            <fieldset className="space-y-1" disabled={level === 'public'}>
              <legend className="text-xs text-gray-400">Compartments</legend>
              {compartments.map(c => (
                <label key={c.name} className="flex items-center gap-2">
                  <input type="checkbox" checked={level !== 'public' && chosen.has(c.name)} onChange={() => toggle(c.name)} />
                  <span>{c.name}</span>
                </label>
              ))}
            </fieldset>
          )}

          {!unchanged && releases(current, next) && (
            <div role="alert" className="rounded-md border border-amber-600 p-2 text-xs text-amber-400">{releaseWarning}</div>
          )}
          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Cancel
          </button>
          <button
            type="submit"
            disabled={unchanged || isSaving}
            className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
          >
            {isSaving ? 'Saving…' : 'Change'}
          </button>
        </div>
      </form>
    </div>
  );
};
