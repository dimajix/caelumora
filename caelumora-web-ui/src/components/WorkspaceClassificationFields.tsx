// The classification of a workspace to make (issues 253, 266): a level and compartments, one for
// all its files. Shared by the dialogs that make one - on its own, or with a session.

import React, {useEffect, useState} from 'react';
import type {Compartment} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {CONFIDENTIALITY_LEVELS, confidentialityInfo, type Classification, type ConfidentialityLevel} from '../confidentiality';

interface WorkspaceClassificationFieldsProps {
  value: Classification;
  onChange: (classification: Classification) => void;
}

export const WorkspaceClassificationFields: React.FC<WorkspaceClassificationFieldsProps> = ({value, onChange}) => {
  const [compartments, setCompartments] = useState<Compartment[]>([]);
  const chosen = new Set(value.compartments ?? []);

  useEffect(() => {
    api.getCompartments()
      .then(setCompartments)
      .catch(error => reportError('Failed to fetch the compartments', error));
  }, []);

  // Public takes no compartment.
  const setLevel = (level: ConfidentialityLevel) => onChange({level, compartments: level === 'public' ? [] : value.compartments});
  const toggle = (compartment: string) => {
    const next = new Set(chosen);
    if (next.has(compartment)) next.delete(compartment); else next.add(compartment);
    onChange({level: value.level, compartments: [...next]});
  };

  return (
    <>
      <label className="block space-y-1">
        <span className="text-xs text-gray-400">Confidentiality</span>
        <select
          value={value.level}
          onChange={(e) => setLevel(e.target.value as ConfidentialityLevel)}
          className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
        >
          {CONFIDENTIALITY_LEVELS.map(l => (
            <option key={l} value={l}>{confidentialityInfo(l).label}</option>
          ))}
        </select>
        <span className="block text-xs text-gray-500">
          One for all its files. In a session, a channel uses it only where the channel is classified alike.
        </span>
      </label>

      {compartments.length > 0 && (
        <fieldset className="space-y-1" disabled={value.level === 'public'}>
          <legend className="text-xs text-gray-400">Compartments</legend>
          {compartments.map(compartment => (
            <label key={compartment.name} className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={value.level !== 'public' && chosen.has(compartment.name)}
                onChange={() => toggle(compartment.name)}
              />
              <span>{compartment.name}</span>
            </label>
          ))}
        </fieldset>
      )}
    </>
  );
};
