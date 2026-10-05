import React, {useEffect, useEffectEvent, useState} from 'react';
import {
  ChevronDown,
  ChevronRight,
  Edit2,
  LogOut,
  MessageSquare,
  Plus,
  Shield,
  Trash2,
  User,
  Users
} from 'lucide-react';
import type {ChannelResponse, SessionSnapshot, SessionTemplate, UserResponse} from '../types';
import {api} from '../api';
import {ApiRequestError, reportError} from '../errors';
import {dispatchGlobalEvent, sessionChange} from '../globalEvents';
import {loadLastTemplate, saveLastTemplate} from '../session';
import {openEventStream} from '../stream';
import {ActionMenu} from './ActionMenu';
import {ResizeHandle} from './ResizeHandle';
import {usePanelWidth} from '../panelWidth';
import {DeletedList} from './DeletedList';
import {NewChannelDialog} from './NewChannelDialog';
import {PrivacyDialog} from './PrivacyDialog';
import {ProfileDialog} from './ProfileDialog';
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
  return kind === 'SessionDeleted' || kind === 'SessionRestored' || kind === 'ChannelDeleted' || kind === 'ChannelRestored';
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
}

export const Sidebar: React.FC<SidebarProps> = ({
  user,
  onLogout,
  selectedSessionId,
  selectedChannelId,
  onSelectChannel,
  onSessionDeleted,
  onChannelDeleted,
}) => {
  const [sessions, setSessions] = useState<SessionSnapshot[]>([]);
  const panel = usePanelWidth('sidebar');
  const [templates, setTemplates] = useState<SessionTemplate[]>([]);
  // The template a new session is created from, preselected with the one
  // used last in this browser (issue 40).
  const [lastTemplateId, setLastTemplateId] = useState<string | null>(() => loadLastTemplate());
  const [selectedTemplateId, setSelectedTemplateId] = useState<string | null>(() => loadLastTemplate());
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
  // Changes whenever something may have been deleted or restored, so that
  // the trash is loaded again (issue 164).
  const [trashVersion, setTrashVersion] = useState(0);
  const trashChanged = () => setTrashVersion(v => v + 1);

  useEffect(() => {
    api.getTemplates()
      .then(setTemplates)
      .catch(error => reportError('Failed to load templates', error));
  }, []);

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

  // The chosen template if it still exists, else the first (the server sorts
  // them by name).
  const selectedTemplate = templates.find(t => t.id === selectedTemplateId) ?? templates[0];

  const handleCreateSession = async () => {
    try {
      const template = selectedTemplate;
      if (!template) {
        reportError('Cannot create a session', 'no session template is available');
        return;
      }
      const name = `New ${template.name}`;
      const newSession = await api.createSession(template.id, name);
      saveLastTemplate(template.id);
      setLastTemplateId(template.id);
      setExpandedSessions(prev => ({ ...prev, [newSession.id]: true }));
      await refreshSession(newSession.id);
    } catch (error) {
      reportError('Failed to create session', error);
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
        <div className="flex items-center gap-2">
          <select
            value={selectedTemplate?.id ?? ''}
            onChange={(e) => setSelectedTemplateId(e.target.value)}
            disabled={templates.length === 0}
            className="flex-1 min-w-0 bg-gray-800 border border-gray-700 rounded-md px-2 py-1 text-sm"
            title={selectedTemplate?.description || 'Template of a new session'}
            aria-label="Template of a new session"
          >
            {templates.map(template => (
              <option
                key={template.id}
                value={template.id}
                title={template.description}
                // The one used last, in bold - where the browser styles
                // options at all (not on macOS).
                style={{fontWeight: template.id === lastTemplateId ? 'bold' : 'normal'}}
              >
                {template.name}
              </option>
            ))}
          </select>
          <button
            onClick={handleCreateSession}
            disabled={!selectedTemplate}
            className="p-1 hover:bg-gray-800 rounded-md transition-colors disabled:opacity-50"
            title={selectedTemplate ? `New ${selectedTemplate.name}` : 'New Session'}
          >
            <Plus size={20} />
          </button>
        </div>
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
                    {label: 'Privacy', icon: Shield, onSelect: () => handleShowPrivacy(session.id)},
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

      {newChannelIn && (
        <NewChannelDialog
          session={newChannelIn}
          user={user}
          onCreated={(channel) => handleChannelCreated(newChannelIn, channel)}
          onClose={() => setNewChannelIn(null)}
        />
      )}

      {showProfile && (
        <ProfileDialog username={user.username} onClose={() => setShowProfile(false)} />
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
