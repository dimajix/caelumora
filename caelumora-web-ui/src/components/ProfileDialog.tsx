// The logged-in user's profile (issue 176): how they introduce themselves to
// the agents. What may be written is the server's to decide; the dialog only
// counts the characters it says are allowed.

import React, {useEffect, useState} from 'react';
import {X} from 'lucide-react';
import {api} from '../api';
import {ApiRequestError, reportError} from '../errors';

interface ProfileDialogProps {
  username: string;
  onClose: () => void;
}

export const ProfileDialog: React.FC<ProfileDialogProps> = ({username, onClose}) => {
  // `null` until the profile is loaded.
  const [saved, setSaved] = useState<string | null>(null);
  const [introduction, setIntroduction] = useState('');
  const [maxChars, setMaxChars] = useState<number | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api.getProfile()
      .then(profile => {
        if (cancelled) return;
        setSaved(profile.introduction ?? '');
        setIntroduction(profile.introduction ?? '');
        setMaxChars(profile.max_introduction_chars);
      })
      .catch(error => {
        if (!cancelled) reportError('Failed to fetch the profile', error);
      });
    return () => { cancelled = true; };
  }, []);

  const loaded = saved !== null;
  const length = [...introduction.trim()].length;
  const tooLong = maxChars !== null && length > maxChars;
  const unchanged = introduction.trim() === (saved ?? '').trim();

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!loaded || unchanged || tooLong || isSaving) return;
    setIsSaving(true);
    setProblem(null);
    try {
      await api.setIntroduction(introduction.trim() || null);
      onClose();
    } catch (error) {
      if (error instanceof ApiRequestError && error.status < 500 && error.status !== 401) {
        setProblem(error.message);
      } else {
        reportError('Failed to save the profile', error);
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
        aria-labelledby="profile-title"
        className="w-full max-w-md rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="profile-title" className="flex-1 font-semibold text-sm truncate">Profile of {username}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-2 p-4 text-sm">
          <label htmlFor="profile-introduction" className="block">How you introduce yourself</label>
          <textarea
            id="profile-introduction"
            rows={6}
            value={introduction}
            disabled={!loaded}
            onChange={(e) => setIntroduction(e.target.value)}
            placeholder={loaded ? 'Who you are, what you do, what you can be asked about.' : 'Loading…'}
            className="w-full rounded-md border border-gray-700 bg-gray-800 p-2 text-sm focus:outline-none focus:border-blue-500"
          />
          <div className={tooLong ? 'text-xs text-red-400' : 'text-xs text-gray-500'}>
            {length}{maxChars !== null && ` / ${maxChars}`} characters
          </div>
          <div className="text-xs text-gray-400">
            Every agent in every channel you take part in is given this, marked as written by you.
            Through them, everyone there may learn it.
          </div>
          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Cancel
          </button>
          <button
            type="submit"
            disabled={!loaded || unchanged || tooLong || isSaving}
            className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
          >
            {isSaving ? 'Saving…' : 'Save'}
          </button>
        </div>
      </form>
    </div>
  );
};
