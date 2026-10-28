// A text edited where a page shows it (issue 479) - a name, a description -, in place of the text
// while it is edited; drawn for a page, which follows the theme.

import React, {useState} from 'react';
import {reportError} from '../errors';

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

/** A text edited in place: saved on Enter or on leaving the field, given up with Escape. */
export const EditInPlace: React.FC<EditInPlaceProps> = ({initial, save, action, onDone}) => {
  const [text, setText] = useState(initial);

  const commit = async () => {
    onDone();
    const trimmed = text.trim();
    if (trimmed === initial) return;
    try {
      await save(trimmed);
    } catch (error) {
      reportError(action, error);
    }
  };

  return (
    <input
      autoFocus
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => void commit()}
      onKeyDown={(e) => {
        if (e.key === 'Enter') void commit();
        if (e.key === 'Escape') onDone();
      }}
      className="w-full rounded-md border border-blue-500 bg-white dark:bg-gray-800 px-2 py-1 text-sm"
    />
  );
};
