// Changing the time zone a session is held in (issue 424). Who may is the
// server's to decide, as whether the name is one.

import React, {useState} from 'react';
import {X} from 'lucide-react';
import {ApiRequestError, reportError} from '../errors';
import {timeZoneLabel} from '../timeZone';
import {TimeZoneField} from './TimeZoneField';

interface TimeZoneDialogProps {
  title: string;
  /** The time zone set now; `null` for none, which is UTC. */
  current: string | null;
  onSubmit: (timeZone: string | null) => Promise<void>;
  onClose: () => void;
}

export const TimeZoneDialog: React.FC<TimeZoneDialogProps> = ({title, current, onSubmit, onClose}) => {
  const [timeZone, setTimeZone] = useState(current ?? '');
  const [problem, setProblem] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  const next = timeZone.trim() || null;
  const unchanged = next === current;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (unchanged || isSaving) return;
    setIsSaving(true);
    setProblem(null);
    try {
      await onSubmit(next);
      onClose();
    } catch (error) {
      // What the server says about the request - a refusal or no time zone,
      // say - is shown here; anything else as usual.
      if (error instanceof ApiRequestError && error.status < 500 && error.status !== 401) {
        setProblem(error.message);
      } else {
        reportError('Failed to change the time zone', error);
      }
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
        aria-labelledby="time-zone-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="time-zone-title" className="flex-1 font-semibold text-sm truncate">{title}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          <div className="text-xs text-gray-400">Now: {timeZoneLabel(current)}</div>
          <TimeZoneField
            value={timeZone}
            onChange={setTimeZone}
            hint="the agents are given the times of messages in it from their next answer on; empty is UTC."
          />
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
