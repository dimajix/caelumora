// Making a session (issues 40, 266): its template - the one used last in this browser preselected -,
// its name, and its workspace: as its template says, none, one of the user's own, or a new one for
// the session alone, in a storage the user chooses and classified as they say. Everything is preset,
// so that opening the dialog and pressing Enter makes a session as the template has it. It is held
// in the language its template names, else in the one the user prefers (issues 331, 425): the server
// holds it in none unless asked, and its channels without a language of their own follow it. Its time
// zone is the template's, else the browser's (issue 424); the server holds it in UTC unless asked.

import React, {useEffect, useState} from 'react';
import {X} from 'lucide-react';
import type {SessionResponse, SessionTemplate, SessionWorkspaceRequest, StorageResponse, WorkspaceResponse} from '../types';
import {api} from '../api';
import {ApiRequestError, reportError} from '../errors';
import {DEFAULT_CLASSIFICATION, type Classification} from '../confidentiality';
import {loadLastTemplate, saveLastTemplate} from '../session';
import {storageLabel} from '../workspace';
import {scopeLabel} from '../scope';
import {ClassificationFields} from './ClassificationFields';
import {ShareSelect, type Share} from './ShareSelect';
import {LanguageField} from './LanguageField';
import {languageLabel} from '../language';
import {TimeZoneField} from './TimeZoneField';
import {browserTimeZone} from '../timeZone';

/** What the workspace select offers besides the user's workspaces, which it names by their id. */
const AS_TEMPLATE = 'template';
const NONE = 'none';
const NEW = 'new';

interface NewSessionDialogProps {
  /** The user's own workspaces. */
  workspaces: WorkspaceResponse[];
  onCreated: (session: SessionResponse) => void;
  onClose: () => void;
}

export const NewSessionDialog: React.FC<NewSessionDialogProps> = ({workspaces, onCreated, onClose}) => {
  const [templates, setTemplates] = useState<SessionTemplate[] | null>(null);
  const [lastTemplateId] = useState<string | null>(() => loadLastTemplate());
  const [templateId, setTemplateId] = useState<string | null>(lastTemplateId);
  // `null` while the user has not changed the name, which then follows the template.
  const [name, setName] = useState<string | null>(null);
  // How far one of the user's own, chosen, is shared with the session's members (issue 23).
  const [share, setShare] = useState<Share>('write');
  // `null` while the user has not chosen, which then follows the template.
  const [workspaceChoice, setWorkspaceChoice] = useState<string | null>(null);
  const [storages, setStorages] = useState<StorageResponse[] | null>(null);
  const [storage, setStorage] = useState<string | null>(null);
  const [classification, setClassification] = useState<Classification>(DEFAULT_CLASSIFICATION);
  // The language the user prefers, offered where the template names none.
  const [preferredLanguage, setPreferredLanguage] = useState<string | null>(null);
  // `null` while the user has not changed the language, which then follows the template.
  const [language, setLanguage] = useState<string | null>(null);
  // `null` while the user has not changed the time zone, which then follows the template.
  const [timeZone, setTimeZone] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);

  useEffect(() => {
    api.getTemplates()
      .then(setTemplates)
      .catch(error => {
        reportError('Failed to load templates', error);
        setTemplates([]);
      });
    api.getStorages()
      .then(setStorages)
      .catch(error => {
        reportError('Failed to fetch the storages', error);
        setStorages([]);
      });
    api.getProfile()
      .then(profile => setPreferredLanguage(profile.preferred_language))
      .catch(error => reportError('Failed to fetch the profile', error));
  }, []);

  // The chosen template if it still exists, else the first (the server sorts them by name).
  const template = templates?.find(t => t.id === templateId) ?? templates?.[0];
  const sessionName = name ?? (template ? `New ${template.name}` : '');
  const sessionLanguage = language ?? template?.language ?? preferredLanguage ?? '';
  const sessionTimeZone = timeZone ?? template?.time_zone ?? browserTimeZone() ?? '';
  // Only one's own is attached; one made for another session is used there alone.
  const offered = workspaces.filter(w => w.can_change && w.scope !== 'session');
  const fallback = template?.workspace ? AS_TEMPLATE : NONE;
  const chosen = workspaceChoice === AS_TEMPLATE && !template?.workspace ? NONE : workspaceChoice ?? fallback;
  // Where a new one is made: as chosen, else the template's storage where it names one, else the first.
  const storageName = storage
    ?? (storages?.some(s => s.name === template?.workspace?.storage) ? template?.workspace?.storage : storages?.[0]?.name)
    ?? '';

  const workspaceRequest = (): SessionWorkspaceRequest | undefined => {
    switch (chosen) {
      case AS_TEMPLATE: return undefined;
      case NONE: return template?.workspace ? {kind: 'none'} : undefined;
      case NEW: return {kind: 'new', storage: storageName, classification};
      default: return {kind: 'existing', workspace_id: chosen, ...(share ? {share} : {})};
    }
  };

  const canCreate = !!template && !!sessionName.trim() && (chosen !== NEW || !!storageName) && !isCreating;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!template || !canCreate) return;
    setIsCreating(true);
    setProblem(null);
    try {
      const session = await api.createSession({
        template_id: template.id,
        name: sessionName.trim(),
        language: sessionLanguage.trim() || undefined,
        time_zone: sessionTimeZone.trim() || undefined,
        workspace: workspaceRequest(),
      });
      saveLastTemplate(template.id);
      onCreated(session);
    } catch (error) {
      // What the server says about the request - a workspace not the user's, a classification
      // they are not cleared for - is shown here.
      if (error instanceof ApiRequestError && error.status < 500 && error.status !== 401) {
        setProblem(error.message);
      } else {
        reportError('Failed to create session', error);
      }
    } finally {
      setIsCreating(false);
    }
  };

  const templateWorkspace = template?.workspace;

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
        aria-labelledby="new-session-title"
        className="w-full max-w-sm rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="new-session-title" className="flex-1 font-semibold text-sm">New session</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 p-4 text-sm">
          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Template</span>
            <select
              value={template?.id ?? ''}
              onChange={(e) => setTemplateId(e.target.value)}
              disabled={!templates || templates.length === 0}
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
            >
              {(templates ?? []).map(t => (
                <option
                  key={t.id}
                  value={t.id}
                  // The one used last, in bold - where the browser styles options at all (not on macOS).
                  style={{fontWeight: t.id === lastTemplateId ? 'bold' : 'normal'}}
                >
                  {t.name}
                </option>
              ))}
            </select>
            {template?.description && <span className="block text-xs text-gray-500">{template.description}</span>}
            {templates?.length === 0 && (
              <span className="block text-xs text-red-400">No session template is available.</span>
            )}
          </label>

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Name</span>
            <input
              autoFocus
              value={sessionName}
              onChange={(e) => setName(e.target.value)}
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1 focus:outline-hidden focus:ring-2 focus:ring-blue-500"
            />
          </label>

          <LanguageField
            label="Language"
            value={sessionLanguage}
            onChange={setLanguage}
            empty={template?.language ? `As the template says: ${languageLabel(template.language)}` : 'None'}
            hint="the agents of its channels are told to write in it, unless a channel has a language of its own."
          />

          <TimeZoneField
            value={sessionTimeZone}
            onChange={setTimeZone}
            empty={template?.time_zone ? `As the template says: ${template.time_zone}` : 'UTC'}
            hint="its agents are given the times of messages in it."
          />

          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Workspace</span>
            <select
              value={chosen}
              onChange={(e) => setWorkspaceChoice(e.target.value)}
              className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
            >
              {templateWorkspace && (
                <option value={AS_TEMPLATE}>As the template says: {templateWorkspace.name} ({scopeLabel(templateWorkspace)})</option>
              )}
              <option value={NONE}>None</option>
              {offered.map(w => <option key={w.id} value={w.id}>{w.name} ({scopeLabel(w)})</option>)}
              <option value={NEW}>New, for this session alone</option>
            </select>
            <span className="block text-xs text-gray-500">
              {chosen === AS_TEMPLATE && templateWorkspace?.scope === 'session'
                ? `Made for this session alone, in ${templateWorkspace.storage}.`
                : chosen === AS_TEMPLATE
                  ? 'Found among the workspaces, or made there, and shared by every session that names it.'
                  : chosen === NEW
                    ? 'Named as the session, and gone with it.'
                    : chosen === NONE
                      ? 'One can be attached later.'
                      : 'Its channels use it, for those its access list lets.'}
            </span>
          </label>
          {offered.some(w => w.id === chosen) && <ShareSelect value={share} onChange={setShare} />}

          {chosen === NEW && (
            <>
              <label className="block space-y-1">
                <span className="text-xs text-gray-400">Storage</span>
                <select
                  value={storageName}
                  onChange={(e) => setStorage(e.target.value)}
                  disabled={!storages || storages.length === 0}
                  className="w-full rounded-md border border-gray-700 bg-gray-800 px-2 py-1"
                >
                  {(storages ?? []).map(s => <option key={s.name} value={s.name}>{storageLabel(s)}</option>)}
                </select>
                {storages?.length === 0 && (
                  <span className="block text-xs text-red-400">No storage is configured: no workspace can be made.</span>
                )}
              </label>
              <ClassificationFields value={classification} onChange={setClassification} />
            </>
          )}

          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            Cancel
          </button>
          <button
            type="submit"
            disabled={!canCreate}
            className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
          >
            {isCreating ? 'Creating…' : 'Create'}
          </button>
        </div>
      </form>
    </div>
  );
};
