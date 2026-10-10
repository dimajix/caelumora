// How far a workspace attached to a session is shared with the session's
// members (issue 23): attaching alone shares nothing - its access list
// decides who may use it, and an entry for the session's members is added
// only as chosen here.

import React from 'react';
import type {Grant} from '../types';

/** `null`: shared with nobody by attaching it. */
export type Share = Grant | null;

interface ShareSelectProps {
  value: Share;
  onChange: (share: Share) => void;
}

export const ShareSelect: React.FC<ShareSelectProps> = ({value, onChange}) => (
  <label className="block space-y-1">
    <span className="text-xs text-gray-400">Share with the session's members</span>
    <select
      value={value ?? 'none'}
      onChange={(e) => onChange(e.target.value === 'none' ? null : e.target.value as Grant)}
      className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
    >
      <option value="write">To read and change its files</option>
      <option value="read">To read its files</option>
      <option value="none">Not: only those it is shared with already</option>
    </select>
  </label>
);
