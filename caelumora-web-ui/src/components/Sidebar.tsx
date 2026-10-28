import React, {useEffect, useEffectEvent, useState} from 'react';
import {
  Bell,
  BookOpen,
  Bot,
  ChevronDown,
  ChevronRight,
  Clock,
  Edit2,
  FileText,
  FolderKanban,
  HardDrive,
  Languages,
  LogOut,
  MessageSquare,
  Package,
  Plus,
  Shield,
  ShieldAlert,
  Trash2,
  User,
  Users,
  UsersRound
} from 'lucide-react';
import type {AgentResponse, ArtifactResponse, ChannelResponse, GlobalEvent, GroupResponse, Message, ProjectResponse, SessionSnapshot, SkillResponse, Unread, UserResponse, WorkspaceResponse} from '../types';
import {api} from '../api';
import {ApiRequestError, reportError} from '../errors';
import {asGlobalEvent, dispatchGlobalEvent, saidInChannel, sessionChange} from '../globalEvents';
import {askForNotifications, countedIn, isOthers, mayAskForNotifications, mentions, notifyMention, onChannelRead, unreadLabel} from '../unread';
import {openEventStream} from '../stream';
import {userActor} from '../actor';
import {ActionMenu} from './ActionMenu';
import {ResizeHandle} from './ResizeHandle';
import {usePanelWidth} from '../panelWidth';
import {DeletedList} from './DeletedList';
import {NewChannelDialog} from './NewChannelDialog';
import {PrivacyDialog} from './PrivacyDialog';
import {ApprovalRulesDialog} from './ApprovalRulesDialog';
import {DescriptionDialog} from './DescriptionDialog';
import {LanguageDialog} from './LanguageDialog';
import {TimeZoneDialog} from './TimeZoneDialog';
import {ProfileDialog} from './ProfileDialog';
import {NewWorkspaceDialog} from './NewWorkspaceDialog';
import {NewSkillDialog} from './NewSkillDialog';
import {NewGroupDialog} from './NewGroupDialog';
import {NewProjectDialog} from './NewProjectDialog';
import {SessionProjectDialog} from './SessionProjectDialog';
import {NewAgentDialog} from './NewAgentDialog';
import {NewSessionDialog} from './NewSessionDialog';
import {SessionWorkspaceDialog} from './SessionWorkspaceDialog';
import {privacyInfo} from '../privacy';
import {cn} from '../classNames';
import type {Page, PageKind} from '../page';
import {PageList} from './PageList';
import {usePageList} from '../pageList';

/** How a session is named in the list: by its name, else by its id. */
function sessionName(session: SessionSnapshot): string {
  return session.name || `Session ${session.id?.slice?.(0, 8) || session.id}`;
}

/** How a channel is named in the list: by its name, else by its id. */
function channelName(channel: ChannelResponse): string {
  return channel.name || `Channel ${channel.id?.slice?.(0, 8) || channel.id}`;
}

/** `sessions` with channel `channelId` changed as `change` says. */
function withChannel(sessions: SessionSnapshot[], channelId: string, change: (channel: ChannelResponse) => ChannelResponse): SessionSnapshot[] {
  return sessions.map(session => session.channels.some(c => c.id === channelId)
    ? {...session, channels: session.channels.map(c => (c.id === channelId ? change(c) : c))}
    : session);
}

/** What is new in all of `session`'s channels together (issue 404), shown while they are folded. */
function unreadIn(session: SessionSnapshot): Unread {
  return {
    messages: session.channels.reduce((sum, channel) => sum + (channel.unread?.messages ?? 0), 0),
    mentioned: session.channels.some(channel => channel.unread?.mentioned),
  };
}

/** How many messages are new, as a badge - marked `@` where one mentions the user. */
const UnreadBadge: React.FC<{unread: Unread | undefined}> = ({unread}) => {
  if (!unread || unread.messages === 0) return null;
  const label = `${unread.messages > 99 ? 'More than 99' : unread.messages} new message${unread.messages === 1 ? '' : 's'}${unread.mentioned ? ', you are mentioned' : ''}`;
  return (
    <span
      className={cn(
        "ml-auto shrink-0 rounded-full px-1.5 text-[10px] font-semibold leading-4",
        unread.mentioned ? "bg-red-600 text-white" : "bg-gray-600 text-gray-100"
      )}
      title={label}
      aria-label={label}
    >
      {unread.mentioned && '@ '}{unreadLabel(unread.messages)}
    </span>
  );
};

/**
 * Whether `event` may change what is among the deleted (issue 164): a deletion or restoring of
 * whatever kind.
 */
function deletedChangedBy(event: GlobalEvent): boolean {
  return event.change === 'Deleted' || event.change === 'Restored';
}

/**
 * Whether `event` is about an entity of `kind` - `workspace`, `skill`, `artifact`, `group`,
 * `agent`, `project`.
 */
function isAbout(event: GlobalEvent, kind: string): boolean {
  return event.kind === kind;
}

/** `sessions` with `session` added, or replacing the one with its id. */
function upsert(sessions: SessionSnapshot[], session: SessionSnapshot): SessionSnapshot[] {
  return sessions.some(s => s.id === session.id)
    ? sessions.map(s => (s.id === session.id ? session : s))
    : [...sessions, session];
}

/**
 * Fetches one session with its channels, or `null` if it is gone again -
 * its deletion then follows or has already arrived.
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
  /**
   * What is shown in the place of a channel - a workspace (issue 253), a skill (issue 22), an
   * artifact (issue 291), a team (issue 286) or an agent (issue 227).
   */
  page: Page | null;
  onSelectPage: (page: Page) => void;
  /** What the page shows no longer exists, or may no longer be read. */
  onPageGone: (kind: PageKind, id: string) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  user,
  onLogout,
  selectedSessionId,
  selectedChannelId,
  onSelectChannel,
  onSessionDeleted,
  onChannelDeleted,
  page,
  onSelectPage,
  onPageGone,
}) => {
  const [sessions, setSessions] = useState<SessionSnapshot[]>([]);
  // The workspaces (issue 253), projects (issue 467), skills (issue 22),
  // artifacts (issue 291), teams (issue 286) and agents (issue 227) the user
  // may read, loaded again on every event about one of them.
  const workspaces = usePageList<WorkspaceResponse>('workspace', 'workspaces', () => api.getWorkspaces(), page, onPageGone);
  const [isCreatingWorkspace, setIsCreatingWorkspace] = useState(false);
  const projects = usePageList<ProjectResponse>('project', 'projects', () => api.getProjects(), page, onPageGone);
  const [isCreatingProject, setIsCreatingProject] = useState(false);
  const skills = usePageList<SkillResponse>('skill', 'skills', () => api.getSkills(), page, onPageGone);
  const [isCreatingSkill, setIsCreatingSkill] = useState(false);
  const artifacts = usePageList<ArtifactResponse>('artifact', 'artifacts', () => api.getArtifacts(), page, onPageGone);
  const groups = usePageList<GroupResponse>('group', 'teams', () => api.getGroups(), page, onPageGone);
  const [isCreatingGroup, setIsCreatingGroup] = useState(false);
  const agents = usePageList<AgentResponse>('agent', 'agents', () => api.getAgents(), page, onPageGone);
  const [isCreatingAgent, setIsCreatingAgent] = useState(false);
  // The session whose workspace is shown, by id, so that the dialog follows it.
  const [workspaceOf, setWorkspaceOf] = useState<string | null>(null);
  const workspaceSession = sessions.find(s => s.id === workspaceOf) ?? null;
  // The session whose project is shown (issue 467), likewise by id.
  const [projectOf, setProjectOf] = useState<string | null>(null);
  const projectSession = sessions.find(s => s.id === projectOf) ?? null;
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
  // The session whose language is changed (issue 425), likewise by id.
  const [languageOf, setLanguageOf] = useState<string | null>(null);
  const languageSession = sessions.find(s => s.id === languageOf) ?? null;
  // The session whose time zone is changed (issue 424), likewise by id.
  const [timeZoneOf, setTimeZoneOf] = useState<string | null>(null);
  const timeZoneSession = sessions.find(s => s.id === timeZoneOf) ?? null;
  // Changes whenever something may have been deleted or restored, so that
  // what was deleted is loaded again (issue 164).
  const [deletedVersion, setDeletedVersion] = useState(0);
  const deletedChanged = () => setDeletedVersion(v => v + 1);
  // Whether the browser may still be asked to show notifications (issue 404).
  const [mayAsk, setMayAsk] = useState(mayAskForNotifications);
  const me = userActor(user.id);

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
  const deletedChangedFromStream = useEffectEvent(() => deletedChanged());
  const workspacesChangedFromStream = useEffectEvent(() => workspaces.reload());
  // A project changed - renamed, lifted - is shown anew with the sessions whose results go to it.
  const projectChangedFromStream = useEffectEvent((projectId: string) => {
    projects.reload();
    for (const session of sessions.filter(s => s.project?.id === projectId)) void refreshSession(session.id);
  });
  const skillsChangedFromStream = useEffectEvent(() => skills.reload());
  const artifactsChangedFromStream = useEffectEvent(() => artifacts.reload());
  const groupsChangedFromStream = useEffectEvent(() => groups.reload());
  const agentsChangedFromStream = useEffectEvent(() => agents.reload());
  const refreshSessionFromStream = useEffectEvent((sessionId: string) => void refreshSession(sessionId));
  // Something new was said (issue 404): counted where the user does not see
  // it now, and shown as a notification where it mentions them.
  const saidFromStream = useEffectEvent((sessionId: string, channelId: string, message: Message) => {
    if (!isOthers(message, me)) return;
    const seen = channelId === selectedChannelId && page === null && document.visibilityState === 'visible';
    if (seen) return;
    setSessions(prev => withChannel(prev, channelId, channel => ({...channel, unread: countedIn(channel.unread, message, me)})));
    if (mentions(message, me)) {
      const channel = sessions.find(s => s.id === sessionId)?.channels.find(c => c.id === channelId);
      notifyMention(message, channel ? channelName(channel) : 'a channel', () => onSelectChannel(sessionId, channelId));
    }
  });

  // What the chat view marked read is nothing new any more.
  useEffect(() => onChannelRead((channelId, messageId) => {
    setSessions(prev => withChannel(prev, channelId, channel => ({...channel, unread: {messages: 0, mentioned: false, read_up_to: messageId}})));
  }), []);

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
          event = asGlobalEvent(JSON.parse(data));
        } catch (error) {
          console.error('Failed to parse a global event', error);
          return;
        }
        if (!event) {
          console.error('Received a global event of unknown form');
          return;
        }
        dispatchGlobalEvent(event);
        const said = saidInChannel(event);
        if (said) {
          saidFromStream(said.sessionId, said.channelId, said.message);
          return;
        }
        if (deletedChangedBy(event)) deletedChangedFromStream();
        if (isAbout(event, 'workspace')) workspacesChangedFromStream();
        if (isAbout(event, 'project')) projectChangedFromStream(event.id);
        if (isAbout(event, 'skill')) skillsChangedFromStream();
        if (isAbout(event, 'artifact')) artifactsChangedFromStream();
        if (isAbout(event, 'group')) groupsChangedFromStream();
        if (isAbout(event, 'agent')) agentsChangedFromStream();
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
      deletedChanged();
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
      deletedChanged();
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
                    <span className="truncate mr-2">{sessionName(session)}</span>
                  )}
                  {!expandedSessions[session.id] && <UnreadBadge unread={unreadIn(session)} />}
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
                    // Fetched first: the list holds no briefing, and saving without it would remove
                    // it.
                    {label: 'Description', icon: FileText, onSelect: () => void refreshSession(session.id).then(() => setDescriptionOf(session.id))},
                    ...(session.can_change ? [
                      {label: 'Language', icon: Languages, onSelect: () => setLanguageOf(session.id)},
                      {label: 'Time zone', icon: Clock, onSelect: () => setTimeZoneOf(session.id)},
                    ] : []),
                    {label: 'Privacy', icon: Shield, onSelect: () => handleShowPrivacy(session.id)},
                    {label: 'Approval rules', icon: ShieldAlert, onSelect: () => setRulesOf(session.id)},
                    {label: 'Workspace', icon: HardDrive, onSelect: () => setWorkspaceOf(session.id)},
                    {label: 'Project', icon: FolderKanban, onSelect: () => setProjectOf(session.id)},
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
                          <span className={cn("truncate mr-2", (channel.unread?.messages ?? 0) > 0 && "text-white font-medium")}>{channelName(channel)}</span>
                          <UnreadBadge unread={channel.unread} />
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
                    version={deletedVersion}
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
            version={deletedVersion}
            onRestore={handleRestoreSession}
            onPurge={(sessionId) => api.purgeSession(sessionId)}
          />
        </div>

        <PageList
          list={workspaces}
          userId={user.id}
          title="Workspaces"
          icon={HardDrive}
          page={page}
          onSelectPage={onSelectPage}
          onNew={() => setIsCreatingWorkspace(true)}
          newLabel="New workspace"
          loadDeleted={() => api.getDeletedWorkspaces()}
          deletedVersion={deletedVersion}
          restore={(workspaceId) => api.restoreWorkspace(workspaceId)}
          purge={(workspaceId) => api.purgeWorkspace(workspaceId)}
        />

        <PageList
          list={projects}
          userId={user.id}
          title="Projects"
          icon={FolderKanban}
          page={page}
          onSelectPage={onSelectPage}
          onNew={() => setIsCreatingProject(true)}
          newLabel="New project"
          loadDeleted={() => api.getDeletedProjects()}
          deletedVersion={deletedVersion}
          restore={(projectId) => api.restoreProject(projectId)}
          purge={(projectId) => api.purgeProject(projectId)}
        />

        <PageList
          list={skills}
          userId={user.id}
          title="Skills"
          icon={BookOpen}
          page={page}
          onSelectPage={onSelectPage}
          onNew={() => setIsCreatingSkill(true)}
          newLabel="New skill"
          loadDeleted={() => api.getDeletedSkills()}
          deletedVersion={deletedVersion}
          restore={(skillId) => api.restoreSkill(skillId)}
          purge={(skillId) => api.purgeSkill(skillId)}
        />

        <PageList
          list={artifacts}
          userId={user.id}
          title="Artifacts"
          icon={Package}
          page={page}
          onSelectPage={onSelectPage}
          loadDeleted={() => api.getDeletedArtifacts()}
          deletedVersion={deletedVersion}
          restore={(artifactId) => api.restoreArtifact(artifactId)}
          purge={(artifactId) => api.purgeArtifact(artifactId)}
        />

        <PageList
          list={groups}
          userId={user.id}
          title="Teams"
          icon={UsersRound}
          page={page}
          onSelectPage={onSelectPage}
          onNew={() => setIsCreatingGroup(true)}
          newLabel="New team"
          loadDeleted={() => api.getDeletedGroups()}
          deletedVersion={deletedVersion}
          restore={(groupId) => api.restoreGroup(groupId)}
          purge={(groupId) => api.purgeGroup(groupId)}
        />

        <PageList
          list={agents}
          userId={user.id}
          title="Agents"
          icon={Bot}
          page={page}
          onSelectPage={onSelectPage}
          onNew={() => setIsCreatingAgent(true)}
          newLabel="New agent"
          loadDeleted={() => api.getDeletedAgents()}
          deletedVersion={deletedVersion}
          restore={(agentId) => api.restoreAgent(agentId)}
          purge={(agentId) => api.purgeAgent(agentId)}
        />
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

      {languageSession && (
        <LanguageDialog
          title={`Language of ${languageSession.name}`}
          current={languageSession.language}
          empty="None"
          emptyHint="it holds in every channel without a language of its own; empty leaves it to their agents"
          onSubmit={async (language) => {
            const updated = await api.setSessionLanguage(languageSession.id, language);
            setSessions(prev => upsert(prev, updated));
          }}
          onClose={() => setLanguageOf(null)}
        />
      )}

      {timeZoneSession && (
        <TimeZoneDialog
          title={`Time zone of ${timeZoneSession.name}`}
          current={timeZoneSession.time_zone}
          onSubmit={async (timeZone) => {
            const updated = await api.setSessionTimeZone(timeZoneSession.id, timeZone);
            setSessions(prev => upsert(prev, updated));
          }}
          onClose={() => setTimeZoneOf(null)}
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
          userId={user.id}
          workspaces={workspaces.items}
          projects={projects.items}
          onCreated={(session) => void handleSessionCreated(session.id)}
          onClose={() => setIsCreatingSession(false)}
        />
      )}

      {isCreatingWorkspace && (
        <NewWorkspaceDialog
          onCreated={(workspace) => {
            setIsCreatingWorkspace(false);
            workspaces.add(workspace);
            onSelectPage({kind: 'workspace', id: workspace.id});
          }}
          onClose={() => setIsCreatingWorkspace(false)}
        />
      )}

      {isCreatingSkill && (
        <NewSkillDialog
          onCreated={(skill) => {
            setIsCreatingSkill(false);
            skills.add(skill);
            onSelectPage({kind: 'skill', id: skill.id});
          }}
          onClose={() => setIsCreatingSkill(false)}
        />
      )}

      {isCreatingProject && (
        <NewProjectDialog
          onCreated={(project) => {
            setIsCreatingProject(false);
            projects.add(project);
            onSelectPage({kind: 'project', id: project.id});
          }}
          onClose={() => setIsCreatingProject(false)}
        />
      )}

      {isCreatingGroup && (
        <NewGroupDialog
          onCreated={(group) => {
            setIsCreatingGroup(false);
            groups.add(group);
            onSelectPage({kind: 'group', id: group.id});
          }}
          onClose={() => setIsCreatingGroup(false)}
        />
      )}

      {isCreatingAgent && (
        <NewAgentDialog
          onCreated={(agent) => {
            setIsCreatingAgent(false);
            agents.add(agent);
            onSelectPage({kind: 'agent', id: agent.id});
          }}
          onClose={() => setIsCreatingAgent(false)}
        />
      )}

      {workspaceSession && (
        <SessionWorkspaceDialog
          userId={user.id}
          session={workspaceSession}
          workspaces={workspaces.items}
          onSubmit={async (workspaceId, share) => {
            const updated = await api.setSessionWorkspace(workspaceSession.id, workspaceId, share);
            setSessions(prev => upsert(prev, updated));
          }}
          onClose={() => setWorkspaceOf(null)}
        />
      )}

      {projectSession && (
        <SessionProjectDialog
          userId={user.id}
          session={projectSession}
          projects={projects.items}
          onSubmit={async (projectId) => {
            const updated = await api.setSessionProject(projectSession.id, projectId);
            setSessions(prev => upsert(prev, updated));
          }}
          onSelectPage={onSelectPage}
          onClose={() => setProjectOf(null)}
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
        {mayAsk && (
          <button
            onClick={() => void askForNotifications().finally(() => setMayAsk(mayAskForNotifications()))}
            className="p-1 hover:bg-gray-800 rounded-md transition-colors"
            title="Let the browser tell you when someone mentions you"
          >
            <Bell size={16} />
          </button>
        )}
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
