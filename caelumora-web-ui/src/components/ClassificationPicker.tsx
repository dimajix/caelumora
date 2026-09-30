// Classifying a message before it is sent (issue 133). A message is the
// channel's unless the user releases it: to a lower level, or to further
// compartments besides the channel's. The server decides whether the
// release may enter the channel; this offers only what can.

import React, {useEffect, useRef, useState} from 'react';
import {Shield} from 'lucide-react';
import type {Compartment} from '../types';
import {
  type Classification,
  type ConfidentialityLevel,
  classificationDescription,
  classificationLabel,
  confidentialityInfo,
  levelsUpTo,
  sameClassification,
} from '../confidentiality';

interface ClassificationPickerProps {
  /** The channel's classification: the default, and the upper bound. */
  channel: Classification;
  /** The configured compartments. */
  compartments: Compartment[];
  value: Classification;
  onChange: (classification: Classification) => void;
}

export const ClassificationPicker: React.FC<ClassificationPickerProps> = ({channel, compartments, value, onChange}) => {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const released = !sameClassification(value, channel);
  const own = new Set(channel.compartments ?? []);
  const chosen = new Set(value.compartments ?? []);
  // A general channel takes no message meant for an audience, and a public
  // message has none.
  const mayAddCompartments = own.size > 0 && value.level !== 'public';

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, [open]);

  const setLevel = (level: ConfidentialityLevel) => {
    // The channel's compartments come back with a level that has any.
    onChange(level === 'public' ? {level} : {level, compartments: [...new Set([...own, ...chosen])]});
  };

  const toggle = (name: string) => {
    const next = new Set(chosen);
    if (next.has(name)) next.delete(name); else next.add(name);
    onChange({level: value.level, compartments: [...next]});
  };

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen(o => !o)}
        className={
          released
            ? "h-full px-2 flex items-center gap-1 rounded-md border border-amber-500 text-amber-600 dark:text-amber-400 text-xs"
            : "h-full px-2 flex items-center rounded-md border border-gray-300 dark:border-gray-600 text-gray-500 dark:text-gray-400"
        }
        title={released ? `Released: ${classificationDescription(value)}` : `Classified as the channel: ${classificationLabel(value)}`}
        aria-label="Classification of the message"
        aria-expanded={open}
      >
        <Shield size={16} />
        {released && <span className="max-w-32 truncate">{classificationLabel(value)}</span>}
      </button>
      {open && (
        <div className="absolute bottom-full right-0 mb-1 w-64 z-10 space-y-3 p-3 text-sm bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-600 rounded-md shadow-lg">
          <div className="text-xs text-gray-500 dark:text-gray-400">
            Messages are classified as the channel. Releasing one makes it readable further: at a lower level, or for further compartments.
          </div>
          <label className="block space-y-1">
            <span className="text-xs text-gray-500 dark:text-gray-400">Level</span>
            <select
              value={value.level}
              onChange={(e) => setLevel(e.target.value as ConfidentialityLevel)}
              className="w-full rounded-md border border-gray-300 dark:border-gray-600 bg-transparent px-2 py-1"
            >
              {levelsUpTo(channel.level).map(l => (
                <option key={l} value={l}>{confidentialityInfo(l).label}</option>
              ))}
            </select>
          </label>
          {mayAddCompartments && compartments.length > 0 && (
            <fieldset className="space-y-1">
              <legend className="text-xs text-gray-500 dark:text-gray-400">Compartments</legend>
              {compartments.map(c => (
                <label key={c.name} className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={own.has(c.name) || chosen.has(c.name)}
                    disabled={own.has(c.name)}
                    onChange={() => toggle(c.name)}
                  />
                  <span>{c.name}</span>
                  {own.has(c.name) && <span className="text-xs text-gray-500">the channel's</span>}
                </label>
              ))}
            </fieldset>
          )}
          {released && (
            <button type="button" onClick={() => onChange(channel)} className="text-xs text-blue-600 dark:text-blue-400 hover:underline">
              Back to the channel's
            </button>
          )}
        </div>
      )}
    </div>
  );
};
