// Choosing a time zone (issue 424): an IANA name typed or picked from the
// browser's. Empty is UTC.

import React, {useId} from 'react';
import {knownTimeZones} from '../timeZone';

interface TimeZoneFieldProps {
  value: string;
  onChange: (value: string) => void;
  /** What it means, below it. */
  hint: string;
  /** What an empty value is called below it; `UTC` if not given. */
  empty?: string;
}

export const TimeZoneField: React.FC<TimeZoneFieldProps> = ({value, onChange, hint, empty = 'UTC'}) => {
  const id = useId();
  return (
    <label className="block space-y-1">
      <span className="text-xs text-gray-400">Time zone</span>
      <input
        list={`${id}-zones`}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="e.g. Europe/Berlin"
        className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1 focus:outline-hidden focus:ring-2 focus:ring-blue-500"
      />
      <datalist id={`${id}-zones`}>
        {knownTimeZones().map(zone => <option key={zone} value={zone} />)}
      </datalist>
      <span className="block text-xs text-gray-500">{value.trim() || empty} - {hint}</span>
    </label>
  );
};
