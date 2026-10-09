import React, {useEffect, useEffectEvent, useState} from 'react';
import {
  BookOpen,
  ChevronDown,
  ChevronRight,
  Edit2,
  FileText,
  HardDrive,
  LogOut,
  MessageSquare,
  Plus,
  Shield,
  ShieldAlert,
  Trash2,
  User,
  Users
} from 'lucide-react';
import type {ChannelResponse, SessionSnapshot, SkillResponse, UserResponse, WorkspaceResponse} from '../types';
import {api} from '../api';
import {ApiRequestError, reportError} from '../errors';
import {dispatchGlobalEvent, sessionChange} from '../globalEvents';
import {openEventStream} from '../stream';
import {ActionMenu} from './ActionMenu';
import {ResizeHandle} from './ResizeHandle';
import {usePanelWidth} from '../panelWidth';
import {DeletedList} from './DeletedList';
import {NewChannelDialog} from './NewChannelDialog';
import {PrivacyDialog} from './PrivacyDialog';
import {ApprovalRulesDialog} from './ApprovalRulesDialog';
import {DescriptionDialog} from './DescriptionDialog';
import {ProfileDialog} from './ProfileDialog';
import {NewWorkspaceDialog} from './NewWorkspaceDialog';
import {NewSkillDialog} from './NewSkillDialog';
import {NewSessionDialog} from './NewSessionDialog';
import {SessionWorkspaceDialog} from './SessionWorkspaceDialog';
import {scopeLabel} from '../scope';
import {privacyInfo} from '../privacy';
import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** How a session is named in the list: by its name, else by its id. */
function sessionName(session: SessionSnapshot): string {
  return session.name || `Session ${session.id?.slice?.(0, 8) || session.id}`;
}

/** How a channel is named in the list: by its name, else by its id. */
function channelName(channel: ChannelResponse): string {
  return channel.name || `Channel ${channel.id?.slice?.(0, 8) || channel.id}`;
}

/** Whether `event` may change what the trash holds (issue 164). */
function trashChangedBy(event: Record<string, unknown>): boolean {
  const kind = Object.keys(event)[0];
  return kind === 'SessionDeleted' || kind === 'SessionRestored' || kind === 'ChannelDeleted' || kind === 'ChannelRestored'
    || kind === 'WorkspaceDeleted' || kind === 'WorkspaceRestored' || kind === 'SkillDeleted' || kind === 'SkillRestored';
}

/** Whether `event` is about one of the user's workspaces (issue 253). */
function aboutWorkspace(event: Record<string, unknown>): boolean {
  return Object.keys(event)[0]?.startsWith('Workspace') ?? false;
}

/** Whether `event` is about a skill the user may read (issue 22). */
function aboutSkill(event: Record<string, unknown>): boolean {
  return Object.keys(event)[0]?.startsWith('Skill') ?? false;
}

/** `sessions` with `session` added, or replacing the one with its id. */
function upsert(sessions: SessionSnapshot[], session: SessionSnapshot): SessionSnapshot[] {
  return sessions.some(s => s.id === session.id)
    ? sessions.map(s => (s.id === session.id ? session : s))
    : [...sessions, session];
}

/**
 * Fetches one session with its channels, or `null` if it is gone again -
 * its `SessionDeleted` then follows or has already arrived.
 */
async function fetchSession(sessionId: string): Promise<SessionSnapshot | null> {
  try {
    return await api.getSession(sessionId);
  } catch (error) {
    if (!(error instanceof ApiRequestError && error.status === 404)) {
      reportError('Failed to load session', error);
    }
    return null;
  }
}

interface SidebarProps {
  user: UserResponse;
  onLogout: () => void;
  selectedSessionId: string | null;
  selectedChannelId: string | null;
  onSelectChannel: (sessionId: string, channelId: string) => void;
  onSessionDeleted: (sessionId: string) => void;
  /** The selected channel no longer exists - deleted here or elsewhere. */
  onChannelDeleted: (channelId: string) => void;
  /** The workspace shown in the place of a channel (issue 253). */
  selectedWorkspaceId: string | null;
  onSelectWorkspace: (workspaceId: string) => void;
  /** The selected workspace no longer exists. */
  onWorkspaceDeleted: (workspaceId: string) => void;
  /** The skill shown in the place of a channel (issue 22). */
  selectedSkillId: string | null;
  onSelectSkill: (skillId: string) => void;
  /** The selected skill no longer exists, or may no longer be read. */
  onSkillDeleted: (skillId: string) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  user,
  onLogout,
  selectedSessionId,
  selectedChannelId,
  onSelectChannel,
  onSessionDeleted,
  onChannelDeleted,
  selectedWorkspaceId,
  onSelectWorkspace,
  onWorkspaceDeleted,
  selectedSkillId,
  onSelectSkill,
  onSkillDeleted,
}) => {
  const [sessions, setSessions] = useState<SessionSnapshot[]>([]);
  // The user's own workspaces (issue 253), loaded again on every event
  // about one of them.
  const [workspaces, setWorkspaces] = useState<WorkspaceResponse[]>([]);
  const [workspacesVersion, setWorkspacesVersion] = useState(0);
  const [isCreatingWorkspace, setIsCreatingWorkspace] = useState(false);
  // The skills the user may read (issue 22), likewise loaded again on every
  // event about one of them.
  const [skills, setSkills] = useState<SkillResponse[]>([]);
  const [skillsVersion, setSkillsVersion] = useState(0);
  const [isCreatingSkill, setIsCreatingSkill] = useState(false);
  // The session whose workspace is shown, by id, so that the dialog follows it.
  const [workspaceOf, setWorkspaceOf] = useState<string | null>(null);
  const workspaceSession = sessions.find(s => s.id === workspaceOf) ?? null;
  const panel = usePanelWidth('sidebar');
  // A new session is being made (issue 266): its template, name and workspace.
  const [isCreatingSession, setIsCreatingSession] = useState(false);
  const [expandedSessions, setExpandedSessions] = useState<Record<string, boolean>>({});
  const [showProfile, setShowProfile] = useState(false);
  const [editingSessionId, setEditingSessionId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState<string>('');
  // The session a new channel is being created in (issue 09).
  const [newChannelIn, setNewChannelIn] = useState<SessionSnapshot | null>(null);
  // The session whose privacy is shown (issue 134), by id, so that the
  // dialog follows changes to it.
  const [privacyOf, setPrivacyOf] = useState<string | null>(null);
  const privacySession = sessions.find(s => s.id === privacyOf) ?? null;
  // The session whose approval rules are shown (issue 13), likewise by id.
  const [rulesOf, setRulesOf] = useState<string | null>(null);
  const rulesSession = sessions.find(s => s.id === rulesOf) ?? null;
  // The session whose description is shown (issue 245), likewise by id -
  // fetched before, since the list's sessions come without their briefing.
  const [descriptionOf, setDescriptionOf] = useState<string | null>(null);
  const descriptionSession = sessions.find(s => s.id === descriptionOf) ?? null;
  // Changes whenever something may have been deleted or restored, so that
  // the trash is loaded again (issue 164).
  const [trashVersion, setTrashVersion] = useState(0);
  const trashChanged = () => setTrashVersion(v => v + 1);

  useEffect(() => {
    let current = true;
    api.getWorkspaces()
      .then(found => { if (current) setWorkspaces(found); })
      .catch(error => reportError('Failed to load the workspaces', error));
    return () => { current = false; };
  }, [workspacesVersion]);

  // A selected workspace that the list no longer has was deleted.
  useEffect(() => {
    if (selectedWorkspaceId && workspacesVersion > 0 && !workspaces.some(w => w.id === selectedWorkspaceId)) {
      onWorkspaceDeleted(selectedWorkspaceId);
    }
  }, [workspaces, selectedWorkspaceId, workspacesVersion, onWorkspaceDeleted]);

  useEffect(() => {
    let current = true;
    api.getSkills()
      .then(found => { if (current) setSkills(found); })
      .catch(error => reportError('Failed to load the skills', error));
    return () => { current = false; };
  }, [skillsVersion]);

  // A selected skill that the list no longer has was deleted.
  useEffect(() => {
    if (selectedSkillId && skillsVersion > 0 && !skills.some(s => s.id === selectedSkillId)) {
      onSkillDeleted(selectedSkillId);
    }
  }, [skills, selectedSkillId, skillsVersion, onSkillDeleted]);

  /** Drops a session from the list - deleted here or elsewhere. */
  const removeSession = (sessionId: string) => {
    setSessions(prev => prev.filter(s => s.id !== sessionId));
    setExpandedSessions(prev => {
      const next = { ...prev };
      delete next[sessionId];
      return next;
    });
    onSessionDeleted(sessionId);
  };

  const refreshSession = async (sessionId: string) => {
    const session = await fetchSession(sessionId);
    if (session) setSessions(prev => upsert(prev, session));
  };

  // The stream outlives renders; these always see the current props.
  const removeSessionFromStream = useEffectEvent((sessionId: string) => removeSession(sessionId));
  const trashChangedFromStream = useEffectEvent(() => trashChanged());
  const workspacesChangedFromStream = useEffectEvent(() => setWorkspacesVersion(v => v + 1));
  const skillsChangedFromStream = useEffectEvent(() => setSkillsVersion(v => v + 1));
  const refreshSessionFromStream = useEffectEvent((sessionId: string) => void refreshSession(sessionId));

  // The list is the global stream's: its snapshot, then its events (issue
  // 35). What this tab does itself is shown at once as well; the event that
  // follows finds it done already.
  useEffect(() => {
    return openEventStream(api.globalEventsUrl(), {
      onSnapshot: (data) => {
        try {
          setSessions(JSON.parse(data).sessions);
        } catch (error) {
          reportError('Failed to read the sessions', error);
        }
      },
      onEvent: (data) => {
        let event;
        try {
          event = JSON.parse(data);
        } catch (error) {
          console.error('Failed to parse a global event', error);
          return;
        }
        dispatchGlobalEvent(event);
        if (trashChangedBy(event)) trashChangedFromStream();
        if (aboutWorkspace(event)) workspacesChangedFromStream();
        if (aboutSkill(event)) skillsChangedFromStream();
        const change = sessionChange(event);
        if (change.kind === 'remove') {
          removeSessionFromStream(change.sessionId);
        } else if (change.kind === 'rename') {
          const {sessionId, name} = change;
          setSessions(prev => prev.map(s => (s.id === sessionId ? { ...s, name } : s)));
        } else if (change.kind === 'refresh') {
          refreshSessionFromStream(change.sessionId);
        }
      },
    });
  }, []);

  // A selected channel that the list no longer has was deleted: here, or
  // elsewhere and learnt of by the global stream (issue 09).
  useEffect(() => {
    if (!selectedSessionId || !selectedChannelId) return;
    const session = sessions.find(s => s.id === selectedSessionId);
    if (session && !session.channels.some(c => c.id === selectedChannelId)) {
      onChannelDeleted(selectedChannelId);
    }
  }, [sessions, selectedSessionId, selectedChannelId, onChannelDeleted]);

  const toggleSession = (sessionId: string) => {
    setExpandedSessions(prev => ({ ...prev, [sessionId]: !prev[sessionId] }));
  };

  const handleSessionCreated = async (sessionId: string) => {
    setIsCreatingSession(false);
    setExpandedSessions(prev => ({ ...prev, [sessionId]: true }));
    try {
      await refreshSession(sessionId);
    } catch (error) {
      reportError('Failed to load the new session', error);
    }
  };

  const handleRenameSession = (sessionId: string, currentName: string) => {
    setEditingSessionId(sessionId);
    setEditingName(currentName);
  };

  const handleNameSave = async (sessionId: string) => {
    if (editingName.trim() && editingName !== sessions.find(s => s.id === sessionId)?.name) {
      try {
        const updatedSession = await api.renameSession(sessionId, editingName.trim());
        setSessions(prev => prev.map(s => s.id === sessionId ? { ...s, name: updatedSession.name } : s));
      } catch (error) {
        reportError('Failed to rename session', error);
      }
    }
    setEditingSessionId(null);
  };

  const handleNameKeyDown = (e: React.KeyboardEvent, sessionId: string) => {
    if (e.key === 'Enter') {
      handleNameSave(sessionId);
    } else if (e.key === 'Escape') {
      setEditingSessionId(null);
    }
  };

  const handleShowPrivacy = (sessionId: string) => {
    setPrivacyOf(sessionId);
  };

  const handleDeleteSession = async (sessionId: string) => {
    try {
      await api.deleteSession(sessionId);
      removeSession(sessionId);
      trashChanged();
    } catch (error) {
      reportError('Failed to delete session', error);
    }
  };

  const handleChannelCreated = (session: SessionSnapshot, channel: ChannelResponse) => {
    setNewChannelIn(null);
    setSessions(prev => prev.map(s => (s.id === session.id && !s.channels.some(c => c.id === channel.id)
      ? { ...s, channels: [...s.channels, channel] }
      : s)));
    onSelectChannel(session.id, channel.id);
  };

  const handleDeleteChannel = async (sessionId: string, channelId: string) => {
    try {
      await api.deleteChannel(sessionId, channelId);
      setSessions(prev => prev.map(s => (s.id === sessionId
        ? { ...s, channels: s.channels.filter(c => c.id !== channelId) }
        : s)));
      trashChanged();
    } catch (error) {
      reportError('Failed to delete the channel', error);
    }
  };

  const handleRestoreSession = async (sessionId: string) => {
    const restored = await api.restoreSession(sessionId);
    setSessions(prev => upsert(prev, restored));
    setExpandedSessions(prev => ({ ...prev, [sessionId]: true }));
  };

  const handleRestoreChannel = async (sessionId: string, channelId: string) => {
    await api.restoreChannel(sessionId, channelId);
    await refreshSession(sessionId);
  };

  return (
    <>
    <div className="shrink-0 bg-gray-900 text-gray-100 flex flex-col border-r border-gray-800" style={{width: panel.width}}>
      <div className="p-4 border-b border-gray-800 space-y-3">
        <h1 className="font-bold text-lg">Caelumora</h1>
        <button
          onClick={() => setIsCreatingSession(true)}
          className="w-full flex items-center justify-center gap-2 rounded-md border border-gray-700 px-2 py-1 text-sm hover:bg-gray-800 transition-colors"
          title="Make a session: its template, name and workspace"
        >
          <Plus size={16} />
          New Session
        </button>
      </div>
      
      <div className="flex-1 overflow-y-auto p-2">
        <div className="space-y-1">
          {sessions.map(session => (
            <div key={session.id} className="space-y-1 relative group">
              <div className={cn(
                "flex items-center rounded-md hover:bg-gray-800 transition-colors text-sm",
                selectedSessionId === session.id && "bg-gray-800 font-medium"
              )}>
                <button
                  onClick={() => toggleSession(session.id)}
                  className="flex-1 flex items-center p-2 truncate"
                  title={session.summary}
                >
                  {expandedSessions[session.id] ? (
                    <ChevronDown size={16} className="mr-2 shrink-0" />
                  ) : (
                    <ChevronRight size={16} className="mr-2 shrink-0" />
                  )}
                  {editingSessionId === session.id ? (
                    <input
                      autoFocus
                      className="flex-1 bg-gray-700 text-white px-1 rounded-sm outline-hidden border border-blue-500 min-w-0"
                      value={editingName}
                      onChange={(e) => setEditingName(e.target.value)}
                      onBlur={() => handleNameSave(session.id)}
                      onKeyDown={(e) => handleNameKeyDown(e, session.id)}
                      onClick={(e) => e.stopPropagation()}
                    />
                  ) : (
                    <span className="truncate">{sessionName(session)}</span>
                  )}
                  {session.privacy && session.privacy !== 'private' && (
                    <span className="ml-1 shrink-0 text-gray-500" title={`${privacyInfo(session.privacy).label}: ${privacyInfo(session.privacy).description}`}>
                      <Users size={12} aria-label={privacyInfo(session.privacy).label} />
                    </span>
                  )}
                </button>
                
                <ActionMenu
                  label={`Options of ${sessionName(session)}`}
                  size={16}
                  className="p-2 opacity-0 group-hover:opacity-100 focus:opacity-100 hover:text-white transition-opacity"
                  actions={[
                    {label: 'Rename', icon: Edit2, onSelect: () => handleRenameSession(session.id, session.name)},
                    // Fetched first: the list holds no briefing, and saving without it would remove it.
                    {label: 'Description', icon: FileText, onSelect: () => void refreshSession(session.id).then(() => setDescriptionOf(session.id))},
                    {label: 'Privacy', icon: Shield, onSelect: () => handleShowPrivacy(session.id)},
                    {label: 'Approval rules', icon: ShieldAlert, onSelect: () => setRulesOf(session.id)},
                    {label: 'Workspace', icon: HardDrive, onSelect: () => setWorkspaceOf(session.id)},
                    {label: 'Delete', icon: Trash2, danger: true, confirm: 'Delete session?', onSelect: () => void handleDeleteSession(session.id)},
                  ]}
                />
              </div>
              
              {expandedSessions[session.id] && (
                <div className="ml-4 space-y-1">
                  {session.channels.map(channel => {
                    return (
                      <div
                        key={channel.id}
                        className={cn(
                          "relative group/channel flex items-center rounded-md hover:bg-gray-700 transition-colors text-sm text-gray-400",
                          selectedChannelId === channel.id && "bg-gray-700 text-white font-medium"
                        )}
                      >
                        <button
                          onClick={() => onSelectChannel(session.id, channel.id)}
                          className="flex-1 min-w-0 flex items-center p-2"
                          title={channel.summary}
                        >
                          <MessageSquare size={14} className="mr-2 shrink-0" />
                          <span className="truncate">{channelName(channel)}</span>
                        </button>
                        <ActionMenu
                          label={`Options of ${channelName(channel)}`}
                          className="p-2 opacity-0 group-hover/channel:opacity-100 focus:opacity-100 hover:text-white transition-opacity"
                          actions={[
                            {label: 'Delete', icon: Trash2, danger: true, confirm: 'Delete channel?', onSelect: () => void handleDeleteChannel(session.id, channel.id)},
                          ]}
                        />
                      </div>
                    );
                  })}
                  {session.channels.length === 0 && (
                    <div className="p-2 text-xs text-gray-500 italic">No channels</div>
                  )}
                  <button
                    onClick={() => setNewChannelIn(session)}
                    className="w-full flex items-center p-2 rounded-md hover:bg-gray-700 transition-colors text-xs text-gray-500"
                  >
                    <Plus size={14} className="mr-2" />
                    New channel
                  </button>
                  <DeletedList
                    title="Deleted channels"
                    load={() => api.getDeletedChannels(session.id)}
                    version={trashVersion}
                    onRestore={(channelId) => handleRestoreChannel(session.id, channelId)}
                  />
                </div>
              )}
            </div>
          ))}
          {sessions.length === 0 && (
            <div className="p-4 text-center text-gray-500 text-sm">
              No sessions found
            </div>
          )}
          <DeletedList
            title="Deleted sessions"
            load={() => api.getDeletedSessions()}
            version={trashVersion}
            onRestore={handleRestoreSession}
            onPurge={(sessionId) => api.purgeSession(sessionId)}
          />
        </div>

        <div className="mt-4 space-y-1">
          <div className="flex items-center px-2 text-xs uppercase tracking-wider text-gray-500">
            <span className="flex-1">Workspaces</span>
            <button
              onClick={() => setIsCreatingWorkspace(true)}
              className="p-1 hover:bg-gray-800 hover:text-white rounded-md transition-colors"
              title="New workspace"
              aria-label="New workspace"
            >
              <Plus size={14} />
            </button>
          </div>
          {workspaces.map(workspace => (
            <button
              key={workspace.id}
              onClick={() => onSelectWorkspace(workspace.id)}
              className={cn(
                "w-full flex items-center p-2 rounded-md hover:bg-gray-800 transition-colors text-sm text-gray-300",
                selectedWorkspaceId === workspace.id && "bg-gray-800 text-white font-medium"
              )}
              title={workspace.description || `${workspace.name} (${scopeLabel(workspace)})`}
            >
              <HardDrive size={14} className="mr-2 shrink-0" />
              <span className="truncate">{workspace.name}</span>
              {workspace.scope !== 'user' && <span className="ml-1 shrink-0 text-xs text-gray-500">{scopeLabel(workspace)}</span>}
            </button>
          ))}
          {workspaces.length === 0 && (
            <div className="p-2 text-xs text-gray-500 italic">No workspaces</div>
          )}
          <DeletedList
            title="Deleted workspaces"
            load={() => api.getDeletedWorkspaces()}
            version={trashVersion}
            onRestore={async (workspaceId) => { await api.restoreWorkspace(workspaceId); setWorkspacesVersion(v => v + 1); }}
            onPurge={(workspaceId) => api.purgeWorkspace(workspaceId)}
          />
        </div>

        <div className="mt-4 space-y-1">
          <div className="flex items-center px-2 text-xs uppercase tracking-wider text-gray-500">
            <span className="flex-1">Skills</span>
            <button
              onClick={() => setIsCreatingSkill(true)}
              className="p-1 hover:bg-gray-800 hover:text-white rounded-md transition-colors"
              title="New skill"
              aria-label="New skill"
            >
              <Plus size={14} />
            </button>
          </div>
          {skills.map(skill => (
            <button
              key={skill.id}
              onClick={() => onSelectSkill(skill.id)}
              className={cn(
                "w-full flex items-center p-2 rounded-md hover:bg-gray-800 transition-colors text-sm text-gray-300",
                selectedSkillId === skill.id && "bg-gray-800 text-white font-medium"
              )}
              title={skill.description}
            >
              <BookOpen size={14} className="mr-2 shrink-0" />
              <span className="truncate">{skill.name}</span>
              {skill.scope !== 'user' && <span className="ml-1 shrink-0 text-xs text-gray-500">{scopeLabel(skill)}</span>}
            </button>
          ))}
          {skills.length === 0 && (
            <div className="p-2 text-xs text-gray-500 italic">No skills</div>
          )}
          <DeletedList
            title="Deleted skills"
            load={() => api.getDeletedSkills()}
            version={trashVersion}
            onRestore={async (skillId) => { await api.restoreSkill(skillId); setSkillsVersion(v => v + 1); }}
            onPurge={(skillId) => api.purgeSkill(skillId)}
          />
        </div>
      </div>

      {privacySession && (
        <PrivacyDialog
          sessionName={privacySession.name}
          current={privacySession.privacy}
          canChange={privacySession.can_change}
          onSubmit={async (privacy) => {
            const updated = await api.setSessionPrivacy(privacySession.id, privacy);
            setSessions(prev => upsert(prev, updated));
          }}
          onClose={() => setPrivacyOf(null)}
        />
      )}

      {descriptionSession && (
        <DescriptionDialog
          title={`What ${descriptionSession.name} is for`}
          place="session"
          summary={descriptionSession.summary}
          briefing={descriptionSession.briefing}
          canChange={descriptionSession.can_change}
          onSubmit={async (description) => {
            const updated = await api.setSessionDescription(descriptionSession.id, description);
            setSessions(prev => upsert(prev, updated));
          }}
          onClose={() => setDescriptionOf(null)}
        />
      )}

      {rulesSession && (
        <ApprovalRulesDialog
          title={`Approval rules of ${rulesSession.name}`}
          place="session"
          current={rulesSession.approval_rules ?? []}
          canChange={rulesSession.can_change}
          onSubmit={async (rules) => {
            const updated = await api.setSessionApprovalRules(rulesSession.id, rules);
            setSessions(prev => upsert(prev, updated));
          }}
          onClose={() => setRulesOf(null)}
        />
      )}

      {isCreatingSession && (
        <NewSessionDialog
          workspaces={workspaces}
          onCreated={(session) => void handleSessionCreated(session.id)}
          onClose={() => setIsCreatingSession(false)}
        />
      )}

      {isCreatingWorkspace && (
        <NewWorkspaceDialog
          onCreated={(workspace) => {
            setIsCreatingWorkspace(false);
            setWorkspaces(prev => (prev.some(w => w.id === workspace.id) ? prev : [...prev, workspace]));
            onSelectWorkspace(workspace.id);
          }}
          onClose={() => setIsCreatingWorkspace(false)}
        />
      )}

      {isCreatingSkill && (
        <NewSkillDialog
          onCreated={(skill) => {
            setIsCreatingSkill(false);
            setSkills(prev => (prev.some(s => s.id === skill.id) ? prev : [...prev, skill]));
            onSelectSkill(skill.id);
          }}
          onClose={() => setIsCreatingSkill(false)}
        />
      )}

      {workspaceSession && (
        <SessionWorkspaceDialog
          session={workspaceSession}
          workspaces={workspaces}
          onSubmit={async (workspaceId) => {
            const updated = await api.setSessionWorkspace(workspaceSession.id, workspaceId);
            setSessions(prev => upsert(prev, updated));
          }}
          onClose={() => setWorkspaceOf(null)}
        />
      )}

      {newChannelIn && (
        <NewChannelDialog
          session={newChannelIn}
          user={user}
          onCreated={(channel) => handleChannelCreated(newChannelIn, channel)}
          onClose={() => setNewChannelIn(null)}
        />
      )}

      {showProfile && (
        <ProfileDialog username={user.username} sessions={sessions} onClose={() => setShowProfile(false)} />
      )}

      <div className="p-3 border-t border-gray-800 flex items-center gap-2 text-sm">
        <button
          onClick={() => setShowProfile(true)}
          className="flex flex-1 min-w-0 items-center gap-2 rounded-md p-1 -m-1 text-left hover:bg-gray-800 transition-colors"
          title="Your profile"
        >
          <User size={16} className="text-gray-400 shrink-0" />
          <span className="flex-1 truncate">{user.username}</span>
        </button>
        <button
          onClick={onLogout}
          className="p-1 hover:bg-gray-800 rounded-md transition-colors"
          title="Log out"
        >
          <LogOut size={16} />
        </button>
      </div>
    </div>
    <ResizeHandle panel={panel} panelSide="left" label="Resize the sidebar" />
    </>
  );
};
