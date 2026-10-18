// Choosing a language (issue 56): a tag typed or picked from suggestions,
// with its name below. Empty is no language - for a channel, its session's
// (issue 425), as `empty` names it.

import React, {useId} from 'react';
import {SUGGESTED_LANGUAGES, languageLabel} from '../language';

interface LanguageFieldProps {
  label: string;
  value: string;
  onChange: (value: string) => void;
  /** What it means, below its name. */
  hint: string;
  /** What an empty value is called below it; `None` if not given. */
  empty?: string;
  disabled?: boolean;
}

export const LanguageField: React.FC<LanguageFieldProps> = ({label, value, onChange, hint, empty = 'None', disabled}) => {
  const id = useId();
  const tag = value.trim();
  return (
    <label className="block space-y-1">
      <span className="text-xs text-gray-400">{label}</span>
      <input
        list={`${id}-languages`}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        placeholder="e.g. de, en-GB"
        className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1 focus:outline-hidden focus:ring-2 focus:ring-blue-500"
      />
      <datalist id={`${id}-languages`}>
        {SUGGESTED_LANGUAGES.map(language => <option key={language} value={language}>{languageLabel(language)}</option>)}
      </datalist>
      <span className="block text-xs text-gray-500">{tag ? languageLabel(tag) : empty} - {hint}</span>
    </label>
  );
};
