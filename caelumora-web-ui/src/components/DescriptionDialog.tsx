// What a session or channel is for (issue 245): its summary, a line, and
// its briefing, what an agent should know before it works there - read by
// everybody there, changed by who may change the place. Its agents are told
// from their next answer on, if their personality wires the briefing in.
// How long each may be, and what it may hold, is the server's to say.

import React, {useState} from 'react';
import {X} from 'lucide-react';
import type {Description} from '../types';
import {ApiRequestError, reportError} from '../errors';
import {MarkdownMessage} from './MarkdownMessage';

interface DescriptionDialogProps {
  title: string;
  place: 'session' | 'channel';
  summary?: string;
  briefing?: string;
  /** Whether the user may change it; otherwise it is only shown. */
  canChange: boolean;
  onSubmit: (description: Description) => Promise<void>;
  onClose: () => void;
}

/** Who reads what is written here - the place's owner should know before they write it. */
const READERS = {
  session: 'Everybody in the session reads this, and every agent in each of its channels - external ones too. Keep what is confidential to a channel\'s description.',
  channel: 'Everybody in the channel reads this, and its agents are told it from their next answer on, classified as the channel.',
};

export const DescriptionDialog: React.FC<DescriptionDialogProps> = ({title, place, summary, briefing, canChange, onSubmit, onClose}) => {
  const [nextSummary, setNextSummary] = useState(summary ?? '');
  const [nextBriefing, setNextBriefing] = useState(briefing ?? '');
  const [problem, setProblem] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  const unchanged = nextSummary.trim() === (summary ?? '') && nextBriefing.trim() === (briefing ?? '');

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canChange || unchanged || isSaving) return;
    setIsSaving(true);
    setProblem(null);
    try {
      await onSubmit({summary: nextSummary.trim() || null, briefing: nextBriefing.trim() || null});
      onClose();
    } catch (error) {
      // What the server says about the request - too long, say - is shown
      // here; anything else as usual.
      if (error instanceof ApiRequestError && error.status < 500 && error.status !== 401) {
        setProblem(error.message);
      } else {
        reportError('Failed to change the description', error);
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
        aria-labelledby="description-title"
        className="w-full max-w-2xl max-h-full flex flex-col rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="description-title" className="flex-1 font-semibold text-sm truncate">{title}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm overflow-y-auto">
          {canChange ? (
            <>
              <div className="text-xs text-gray-400">{READERS[place]}</div>
              <label className="block space-y-1">
                <span className="text-xs text-gray-400">Summary - what it is for, in a line</span>
                <input
                  autoFocus
                  value={nextSummary}
                  onChange={(e) => setNextSummary(e.target.value)}
                  className="w-full bg-gray-800 border border-gray-700 rounded-md px-2 py-1"
                />
              </label>
              <label className="block space-y-1">
                <span className="text-xs text-gray-400">Briefing - what an agent should know before it works here (Markdown)</span>
                <textarea
                  value={nextBriefing}
                  onChange={(e) => setNextBriefing(e.target.value)}
                  rows={14}
                  className="w-full bg-gray-800 border border-gray-700 rounded-md px-2 py-1 font-mono text-xs"
                />
              </label>
              {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
            </>
          ) : (
            <>
              <div className="text-gray-200">{summary ?? <span className="text-gray-500">No summary.</span>}</div>
              {briefing
                ? <MarkdownMessage content={briefing} invert />
                : <div className="text-gray-500">No briefing.</div>}
            </>
          )}
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            {canChange ? 'Cancel' : 'Close'}
          </button>
          {canChange && (
            <button
              type="submit"
              disabled={unchanged || isSaving}
              className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
            >
              {isSaving ? 'Saving…' : 'Save'}
            </button>
          )}
        </div>
      </form>
    </div>
  );
};
