// The logged-in user's profile (issue 176): how they introduce themselves to
// the agents, and the language they prefer (issue 56). What may be written
// is the server's to decide; the dialog only counts the characters it says
// are allowed. Below, the user's rules on tool calls (issue 13), each taken
// back on its own - they are added from an approval card.

import React, {useEffect, useState} from 'react';
import {Trash2, X} from 'lucide-react';
import {api} from '../api';
import type {ApprovalRule, Reach, SessionSnapshot} from '../types';
import {describeMatcher, describeReach, isActive} from '../approvalRules';
import {ApiRequestError, reportError} from '../errors';
import {LanguageField} from './LanguageField';

interface ProfileDialogProps {
  username: string;
  /** The sessions the user sees, to name where a rule reaches. */
  sessions: SessionSnapshot[];
  onClose: () => void;
}

export const ProfileDialog: React.FC<ProfileDialogProps> = ({username, sessions, onClose}) => {
  const [rules, setRules] = useState<ApprovalRule[]>([]);
  const nameOf = (reach: Reach): string | null => {
    if (reach === 'everywhere') return null;
    if ('session' in reach) return sessions.find(s => s.id === reach.session)?.name ?? null;
    return sessions.flatMap(s => s.channels).find(c => c.id === reach.channel)?.name ?? null;
  };
  const removeRule = async (rule: ApprovalRule) => {
    try {
      if (rule.id) await api.removeApprovalRule(rule.id);
      setRules(prev => prev.filter(r => r.id !== rule.id));
    } catch (error) {
      reportError('Failed to remove the rule', error);
    }
  };
  // `null` until the profile is loaded.
  const [saved, setSaved] = useState<{introduction: string; language: string} | null>(null);
  const [introduction, setIntroduction] = useState('');
  const [language, setLanguage] = useState('');
  const [maxChars, setMaxChars] = useState<number | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api.getProfile()
      .then(profile => {
        if (cancelled) return;
        setSaved({introduction: profile.introduction ?? '', language: profile.preferred_language ?? ''});
        setIntroduction(profile.introduction ?? '');
        setLanguage(profile.preferred_language ?? '');
        setMaxChars(profile.max_introduction_chars);
        setRules(profile.approval_rules ?? []);
      })
      .catch(error => {
        if (!cancelled) reportError('Failed to fetch the profile', error);
      });
    return () => { cancelled = true; };
  }, []);

  const loaded = saved !== null;
  const length = [...introduction.trim()].length;
  const tooLong = maxChars !== null && length > maxChars;
  const unchanged = introduction.trim() === (saved?.introduction ?? '').trim()
    && language.trim() === (saved?.language ?? '');

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!loaded || unchanged || tooLong || isSaving) return;
    setIsSaving(true);
    setProblem(null);
    try {
      await api.updateProfile({introduction: introduction.trim() || null, preferred_language: language.trim() || null});
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
          <LanguageField
            label="Preferred language"
            value={language}
            disabled={!loaded}
            onChange={setLanguage}
            hint="the sessions you create are held in it unless their template or you choose another; the agents of their channels are told to write in it."
          />
          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
          <div className="pt-2">
            <div className="mb-1">Your rules on tool calls</div>
            <div className="mb-2 text-xs text-gray-400">
              Your answers given in advance, for the agents you are responsible for - added from an approval
              card. A rule of a session or channel that asks still asks you.
            </div>
            {rules.length === 0 && <div className="text-xs text-gray-500">None.</div>}
            <ul className="space-y-1">
              {rules.map(rule => (
                <li key={rule.id} className={`flex items-center gap-2 text-xs ${isActive(rule) ? '' : 'opacity-50'}`}>
                  <span className={rule.decision === 'allow' ? 'text-green-400' : 'text-red-400'}>{rule.decision === 'allow' ? 'Allow' : 'Deny'}</span>
                  <code className="min-w-0 flex-1 truncate" title={describeMatcher(rule.matcher)}>{describeMatcher(rule.matcher)}</code>
                  <span className="shrink-0 text-gray-400">
                    {describeReach(rule, nameOf)}{rule.expires_at ? `, until ${new Date(rule.expires_at).toLocaleString()}` : ''}
                  </span>
                  <button type="button" onClick={() => void removeRule(rule)} className="p-1 hover:bg-gray-800 rounded-md" title="Take the rule back">
                    <Trash2 size={12} />
                  </button>
                </li>
              ))}
            </ul>
          </div>
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
