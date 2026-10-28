// A session's privacy policy (issue 134): shown to everyone who takes part,
// changed by its owner. Who may is the server's to decide; `canChange` only
// decides whether a change is offered.

import React, {useState} from 'react';
import {X} from 'lucide-react';
import {PRIVACY_POLICIES, type PrivacyPolicy, privacyInfo} from '../privacy';
import {reportUnlessRefused} from '../errors';

interface PrivacyDialogProps {
  sessionName: string;
  current: PrivacyPolicy;
  canChange: boolean;
  onSubmit: (privacy: PrivacyPolicy) => Promise<void>;
  onClose: () => void;
}

export const PrivacyDialog: React.FC<PrivacyDialogProps> = ({sessionName, current, canChange, onSubmit, onClose}) => {
  const [privacy, setPrivacy] = useState<PrivacyPolicy>(current);
  const [problem, setProblem] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  const unchanged = privacy === current;
  // Another policy that lets other sessions read applies to what is said
  // from now on; what was said before keeps the one it was said under.
  const opens = !unchanged && privacy !== 'private';

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canChange || unchanged || isSaving) return;
    setIsSaving(true);
    setProblem(null);
    try {
      await onSubmit(privacy);
      onClose();
    } catch (error) {
      reportUnlessRefused('Failed to change the privacy', error, setProblem);
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
        aria-labelledby="privacy-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="privacy-title" className="flex-1 font-semibold text-sm truncate">Privacy of {sessionName}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <fieldset className="space-y-3 p-4 text-sm" disabled={!canChange}>
          <legend className="sr-only">Who may read what is said here from other sessions</legend>
          {PRIVACY_POLICIES.map(p => (
            <label key={p} className="flex items-start gap-2">
              <input type="radio" name="privacy" className="mt-1" checked={privacy === p} onChange={() => setPrivacy(p)} />
              <span>
                <span className="block">{privacyInfo(p).label}</span>
                <span className="block text-xs text-gray-500">{privacyInfo(p).description}</span>
              </span>
            </label>
          ))}
          {!canChange && <div className="text-xs text-gray-400">Only the session's owner may change it.</div>}
          {canChange && opens && (
            <div role="alert" className="rounded-md border border-amber-600 p-2 text-xs text-amber-400">
              Other sessions may read what is said from now on. What was said before keeps the privacy it was said under.
            </div>
          )}
          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
        </fieldset>

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
              {isSaving ? 'Saving…' : 'Change'}
            </button>
          )}
        </div>
      </form>
    </div>
  );
};
