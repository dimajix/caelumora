// One project (issue 467), in the place of a channel: what it is - its name, where it lies and what
// it is for -, who works on it, whom it is shared with, the sessions that lie in it and its
// artifacts. A session's own project is called by its session; its owner keeps it by lifting it
// into their name space under a name of its own, and it is a shared project then. A shared one's
// owner renames, describes, names its members, shares and deletes it - with its sessions.

import React, {useEffect, useState} from 'react';
import {Edit2, FileText, FolderKanban, FolderUp, MessageSquare, Package, Trash2} from 'lucide-react';
import type {AccessEntry, ArtifactResponse, ProjectResponse, SessionResponse} from '../types';
import {api} from '../api';
import {reportError, reportUnlessRefused} from '../errors';
import {labelOf, placeOf} from '../project';
import type {Page} from '../page';
import {AccessList} from './AccessList';
import {ActionMenu} from './ActionMenu';
import {AddUser} from './AddUser';

interface ProjectPageProps {
  projectId: string;
  onDeleted: (projectId: string) => void;
  /** Opens an artifact of it. */
  onSelectPage: (page: Page) => void;
  /** Opens the first channel of a session of it. */
  onSelectChannel: (sessionId: string, channelId: string) => void;
}

/** The sidebar learns of a change from the global stream. */
export const ProjectPage: React.FC<ProjectPageProps> = ({projectId, onDeleted, onSelectPage, onSelectChannel}) => {
  const [project, setProject] = useState<ProjectResponse | null>(null);
  const [sessions, setSessions] = useState<SessionResponse[]>([]);
  const [artifacts, setArtifacts] = useState<ArtifactResponse[]>([]);
  const [editing, setEditing] = useState<'name' | 'description' | null>(null);
  const [text, setText] = useState('');
  const [isLifting, setIsLifting] = useState(false);
  const [label, setLabel] = useState('');

  useEffect(() => {
    let current = true;
    api.getProject(projectId)
      .then(async found => {
        const called = await labelOf(found);
        if (current) {
          setProject(found);
          setLabel(called);
        }
      })
      .catch(error => reportError('Failed to load the project', error));
    api.getProjectSessions(projectId)
      .then(found => { if (current) setSessions(found); })
      .catch(error => reportError('Failed to fetch the sessions of the project', error));
    api.getProjectArtifacts(projectId)
      .then(found => { if (current) setArtifacts(found); })
      .catch(error => reportError('Failed to fetch the artifacts of the project', error));
    return () => { current = false; };
  }, [projectId]);

  if (!project) return null;

  const own = project.scope === 'session';
  // A session's own is kept while the session publishes into it: lifted then, the session lies in it.
  const keepable = own && sessions.some(s => s.id === project.session_id);
  const place = placeOf(project);
  const memberIds = project.members.map(m => m.id);

  const changed = (updated: ProjectResponse) => {
    setProject(updated);
    void labelOf(updated).then(setLabel);
  };

  const edit = (field: 'name' | 'description') => {
    setText(field === 'name' ? project.name : (project.description ?? ''));
    setEditing(field);
  };

  const save = async () => {
    const field = editing;
    setEditing(null);
    try {
      if (field === 'name' && text.trim() && text.trim() !== project.name) {
        changed(await api.renameProject(project.id, text.trim()));
      } else if (field === 'description' && text.trim() !== (project.description ?? '')) {
        changed(await api.describeProject(project.id, text.trim() || null));
      }
    } catch (error) {
      reportError(field === 'name' ? 'Failed to rename the project' : 'Failed to describe the project', error);
    }
  };

  const setMembers = async (userIds: string[]) => {
    try {
      changed(await api.setProjectMembers(project.id, userIds));
    } catch (error) {
      reportError('Failed to change the members of the project', error);
    }
  };

  const share = async (access: AccessEntry[]) => {
    try {
      changed(await api.setProjectAccess(project.id, access));
    } catch (error) {
      reportError('Failed to share the project', error);
    }
  };

  const remove = async () => {
    try {
      await api.deleteProject(project.id);
      onDeleted(project.id);
    } catch (error) {
      reportError('Failed to delete the project', error);
    }
  };

  const field = (
    <input
      autoFocus
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => void save()}
      onKeyDown={(e) => {
        if (e.key === 'Enter') void save();
        if (e.key === 'Escape') setEditing(null);
      }}
      className="w-full rounded-md border border-blue-500 bg-white dark:bg-gray-800 px-2 py-1 text-sm"
    />
  );

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="max-w-3xl mx-auto p-6 space-y-6">
        <div className="flex items-start gap-3">
          <FolderKanban size={22} className="mt-1 shrink-0 text-gray-400" />
          <div className="flex-1 min-w-0 space-y-1">
            {editing === 'name' ? field : <h2 className="text-xl font-semibold truncate text-gray-900 dark:text-gray-100">{label}</h2>}
            {editing === 'description' ? field : (
              <p className="text-sm text-gray-600 dark:text-gray-400">{project.description || <span className="italic">No description</span>}</p>
            )}
            {place && (
              <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-500 dark:text-gray-400">
                <span>Name space: {place}</span>
              </div>
            )}
          </div>
          {project.can_change && (
            <ActionMenu
              label={`Options of ${label}`}
              size={16}
              className="p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800"
              actions={own ? [
                {label: 'Description', icon: FileText, onSelect: () => edit('description')},
                ...(keepable ? [{label: 'Keep', icon: FolderUp, onSelect: () => setIsLifting(true)}] : []),
              ] : [
                {label: 'Rename', icon: Edit2, onSelect: () => edit('name')},
                {label: 'Description', icon: FileText, onSelect: () => edit('description')},
                {label: 'Delete', icon: Trash2, danger: true, confirm: 'Delete the project, with its sessions?', onSelect: () => void remove()},
              ]}
            />
          )}
        </div>

        <div className="text-sm text-gray-600 dark:text-gray-400">
          {!own
            ? 'Its results outlive the sessions that lie in it. Its members read every artifact of it, and the versions of the channels they took part in.'
            : keepable
              ? 'Its results go with the session. Keep the project to have them beyond it: it becomes a project of yours, with the session in it.'
              : 'Its results go with the session, which publishes into another project now.'}
        </div>

        {isLifting && <LiftProject project={project} onLifted={(lifted) => { setIsLifting(false); changed(lifted); }} onCancel={() => setIsLifting(false)} />}

        {!own && (
          <div className="space-y-1">
            <h3 className="text-sm font-medium text-gray-800 dark:text-gray-100">Members</h3>
            {project.members.length === 0 ? (
              <div className="text-xs italic text-gray-500">Nobody works on it yet.</div>
            ) : (
              <ul className="space-y-1 text-sm">
                {project.members.map(member => (
                  <li key={member.id} className="flex items-center gap-2">
                    <span className="flex-1 min-w-0 truncate">{member.name ?? <span className="italic text-gray-500">unknown</span>}</span>
                    {project.can_change && (
                      <button
                        type="button"
                        onClick={() => void setMembers(memberIds.filter(id => id !== member.id))}
                        className="p-1 rounded-md text-gray-500 hover:text-red-500 hover:bg-gray-100 dark:hover:bg-gray-800"
                        title="Take them off the project"
                        aria-label={`Take ${member.name ?? 'them'} off the project`}
                      >
                        <Trash2 size={12} />
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            )}
            {project.can_change && <AddUser members={memberIds} onAdd={(userId) => void setMembers([...memberIds, userId])} />}
          </div>
        )}

        {project.can_change && (
          <AccessList
            access={project.access ?? []}
            onChange={(access) => void share(access)}
            nobody="Nobody but you."
            grants={{read: 'reads and publishes into it'}}
            kept={(entry) => (entry.kind === 'project' && entry.id === project.id)
              || (entry.kind === 'session' && (entry.id === project.session_id || sessions.some(s => s.id === entry.id)))}
          />
        )}

        <div className="space-y-1">
          <h3 className="text-sm font-medium text-gray-800 dark:text-gray-100">Sessions</h3>
          {sessions.length === 0 ? (
            <div className="text-xs italic text-gray-500">None that you read.</div>
          ) : (
            <ul className="space-y-1 text-sm">
              {sessions.map(session => (
                <li key={session.id}>
                  <OpenSession session={session} onSelectChannel={onSelectChannel} />
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="space-y-1">
          <h3 className="text-sm font-medium text-gray-800 dark:text-gray-100">Artifacts</h3>
          {artifacts.length === 0 ? (
            <div className="text-xs italic text-gray-500">Nothing published into it that you read.</div>
          ) : (
            <ul className="space-y-1 text-sm">
              {artifacts.map(artifact => (
                <li key={artifact.id}>
                  <button
                    type="button"
                    onClick={() => onSelectPage({kind: 'artifact', id: artifact.id})}
                    className="flex items-center gap-2 hover:underline"
                  >
                    <Package size={14} className="text-gray-400" /> {artifact.name}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
};

interface OpenSessionProps {
  session: SessionResponse;
  onSelectChannel: (sessionId: string, channelId: string) => void;
}

/** A session of the project, opened at its first channel the user reads. */
const OpenSession: React.FC<OpenSessionProps> = ({session, onSelectChannel}) => {
  const open = async () => {
    try {
      const channel = (await api.getSession(session.id)).channels[0];
      if (channel) onSelectChannel(session.id, channel.id);
    } catch (error) {
      reportError('Failed to open the session', error);
    }
  };
  return (
    <button type="button" onClick={() => void open()} className="flex items-center gap-2 hover:underline">
      <MessageSquare size={14} className="text-gray-400" /> {session.name}
    </button>
  );
};

interface LiftProjectProps {
  project: ProjectResponse;
  onLifted: (project: ProjectResponse) => void;
  onCancel: () => void;
}

/** Keeps a session's own project: lifted into the user's name space or the global one, under a name of its own. */
const LiftProject: React.FC<LiftProjectProps> = ({project, onLifted, onCancel}) => {
  const [name, setName] = useState('');
  const [scope, setScope] = useState<'user' | 'global'>('user');
  const [problem, setProblem] = useState<string | null>(null);

  const lift = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    setProblem(null);
    try {
      onLifted(await api.liftProject(project.id, name.trim(), scope));
    } catch (error) {
      // What the server says about the request - a taken name, say - is shown here.
      reportUnlessRefused('Failed to keep the project', error, setProblem);
    }
  };

  const fieldClass = "rounded-md border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-2 py-1 text-sm";
  return (
    <form onSubmit={(e) => void lift(e)} className="space-y-2 rounded-md border border-gray-200 dark:border-gray-700 p-3 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="Its name" className={fieldClass} aria-label="The name it is kept under" />
        <select value={scope} onChange={(e) => setScope(e.target.value as 'user' | 'global')} className={fieldClass} aria-label="Where it is kept">
          <option value="user">among yours</option>
          <option value="global">for everyone to find</option>
        </select>
        <button type="submit" disabled={!name.trim()} className="rounded-md bg-blue-600 px-3 py-1 text-white hover:bg-blue-700 disabled:opacity-50">Keep</button>
        <button type="button" onClick={onCancel} className="rounded-md px-3 py-1 hover:bg-gray-100 dark:hover:bg-gray-800">Cancel</button>
      </div>
      {problem && <div role="alert" className="text-xs text-red-500">{problem}</div>}
    </form>
  );
};
