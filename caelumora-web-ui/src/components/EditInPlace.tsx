// A text edited where a page shows it (issue 479) - a name, a description -, in place of the text
// while it is edited; drawn for a page, which follows the theme.

import React, {useEffect, useRef, useState} from 'react';
import {reportError, reportUnlessRefused} from '../errors';
import {Refusal} from './Refusal';

interface EditInPlaceProps {
  /** The text the field starts with. */
  initial: string;
  /** Stores the text, trimmed, if it was changed; whether an empty one is, is the caller's. */
  save: (text: string) => Promise<void>;
  /** What failed, for the banner ("Failed to rename the team"). */
  action: string;
  /** Closes the field. */
  onDone: () => void;
}

/**
 * A text edited in place: saved on Enter or on leaving the field, given up with Escape. The field
 * stays open until the server answered; what it refuses is shown under the field, and what else
 * fails is reported, the text kept either way to be corrected, saved again or given up.
 */
export const EditInPlace: React.FC<EditInPlaceProps> = ({initial, save, action, onDone}) => {
  const [text, setText] = useState(initial);
  const [refusal, setRefusal] = useState<string | null>(null);
  // Only a field being edited saves: not a second time while a save is under way, and not the
  // blur a field may get as it is removed. An answer that comes once the field is closed - given
  // up, or gone with its page - neither closes it again, which might close another field of the
  // page, nor shows a refusal under it, which nobody would see: that one goes to the banner.
  const phase = useRef<'editing' | 'saving' | 'closed'>('editing');
  useEffect(() => {
    phase.current = 'editing';
    return () => {
      phase.current = 'closed';
    };
  }, []);

  const close = () => {
    phase.current = 'closed';
    onDone();
  };

  const commit = async () => {
    if (phase.current !== 'editing') return;
    const trimmed = text.trim();
    if (trimmed === initial) {
      close();
      return;
    }
    phase.current = 'saving';
    setRefusal(null);
    try {
      await save(trimmed);
      if (phase.current === 'saving') close();
    } catch (error) {
      if (phase.current === 'saving') {
        phase.current = 'editing';
        reportUnlessRefused(action, error, setRefusal);
      } else {
        reportError(action, error);
      }
    }
  };

  return (
    <div className="space-y-1">
      <input
        autoFocus
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void commit();
          if (e.key === 'Escape') close();
        }}
        className="w-full rounded-md border border-blue-500 bg-white dark:bg-gray-800 px-2 py-1 text-sm"
      />
      <Refusal message={refusal} on="page" />
    </div>
  );
};
