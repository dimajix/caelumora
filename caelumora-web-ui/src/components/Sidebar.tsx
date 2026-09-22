import React, {useEffect, useState} from 'react';
import {
  ChevronDown,
  ChevronRight,
  Edit2,
  LogOut,
  MessageSquare,
  MoreVertical,
  Plus,
  Trash2,
  User
} from 'lucide-react';
import type {ChannelResponse, SessionResponse, SessionTemplate, UserResponse} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

interface SidebarProps {
  user: UserResponse;
  onLogout: () => void;
  selectedSessionId: string | null;
  selectedChannelId: string | null;
  onSelectChannel: (sessionId: string, channelId: string) => void;
  onSessionDeleted: (sessionId: string) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  user,
  onLogout,
  selectedSessionId,
  selectedChannelId,
  onSelectChannel,
  onSessionDeleted,
}) => {
  const [sessions, setSessions] = useState<SessionResponse[]>([]);
  const [templates, setTemplates] = useState<SessionTemplate[]>([]);
  const [expandedSessions, setExpandedSessions] = useState<Record<string, boolean>>({});
  const [channels, setChannels] = useState<Record<string, ChannelResponse[]>>({});
  const [openMenuId, setOpenMenuId] = useState<string | null>(null);
  const [editingSessionId, setEditingSessionId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState<string>('');

  useEffect(() => {
    loadSessions();
    loadTemplates();
    const handleClickOutside = () => setOpenMenuId(null);
    window.addEventListener('click', handleClickOutside);
    return () => window.removeEventListener('click', handleClickOutside);
  }, []);

  const loadTemplates = async () => {
    try {
      const data = await api.getTemplates();
      setTemplates(data);
    } catch (error) {
      reportError('Failed to load templates', error);
    }
  };

  const loadSessions = async () => {
    try {
      const data = await api.getSessions();
      setSessions(data);
    } catch (error) {
      reportError('Failed to load sessions', error);
    }
  };

  const toggleSession = async (sessionId: string) => {
    const isExpanded = !!expandedSessions[sessionId];
    setExpandedSessions(prev => ({ ...prev, [sessionId]: !isExpanded }));

    if (!isExpanded && !channels[sessionId]) {
      try {
        const sessionChannels = await api.getChannels(sessionId);
        setChannels(prev => ({ ...prev, [sessionId]: sessionChannels }));
      } catch (error) {
        reportError(`Failed to load channels for session ${sessionId}`, error);
      }
    }
  };

  const handleCreateSession = async () => {
    try {
      const template = templates[0];
      if (!template) {
        reportError('Cannot create a session', 'no session template is available');
        return;
      }
      const name = `New ${template.metadata.name}`;
      const newSession = await api.createSession(template.id, name);
      setSessions(prev => [...prev, newSession]);
      toggleSession(newSession.id);
    } catch (error) {
      reportError('Failed to create session', error);
    }
  };

  const handleRenameSession = (e: React.MouseEvent, sessionId: string, currentName: string) => {
    e.stopPropagation();
    setOpenMenuId(null);
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

  const handleDeleteSession = async (e: React.MouseEvent, sessionId: string) => {
    e.stopPropagation();
    setOpenMenuId(null);
    try {
      await api.deleteSession(sessionId);
      setSessions(prev => prev.filter(s => s.id !== sessionId));
      setChannels(prev => {
        const next = { ...prev };
        delete next[sessionId];
        return next;
      });
      setExpandedSessions(prev => {
        const next = { ...prev };
        delete next[sessionId];
        return next;
      });
      onSessionDeleted(sessionId);
    } catch (error) {
      reportError('Failed to delete session', error);
    }
  };

  const toggleMenu = (e: React.MouseEvent, sessionId: string) => {
    e.stopPropagation();
    setOpenMenuId(openMenuId === sessionId ? null : sessionId);
  };

  return (
    <div className="w-64 bg-gray-900 text-gray-100 flex flex-col border-r border-gray-800">
      <div className="p-4 border-b border-gray-800 flex justify-between items-center">
        <h1 className="font-bold text-lg">Caelumora</h1>
        <button
          onClick={handleCreateSession}
          className="p-1 hover:bg-gray-800 rounded-md transition-colors"
          title="New Session"
        >
          <Plus size={20} />
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
                >
                  {expandedSessions[session.id] ? (
                    <ChevronDown size={16} className="mr-2 flex-shrink-0" />
                  ) : (
                    <ChevronRight size={16} className="mr-2 flex-shrink-0" />
                  )}
                  {editingSessionId === session.id ? (
                    <input
                      autoFocus
                      className="flex-1 bg-gray-700 text-white px-1 rounded outline-none border border-blue-500 min-w-0"
                      value={editingName}
                      onChange={(e) => setEditingName(e.target.value)}
                      onBlur={() => handleNameSave(session.id)}
                      onKeyDown={(e) => handleNameKeyDown(e, session.id)}
                      onClick={(e) => e.stopPropagation()}
                    />
                  ) : (
                    <span className="truncate">{session.name || `Session ${session.id?.slice?.(0, 8) || session.id}`}</span>
                  )}
                </button>
                
                <button
                  onClick={(e) => toggleMenu(e, session.id)}
                  className="p-2 opacity-0 group-hover:opacity-100 hover:text-white transition-opacity"
                  title="Session options"
                >
                  <MoreVertical size={16} />
                </button>

                {openMenuId === session.id && (
                  <div className="absolute right-0 top-10 w-32 bg-gray-800 border border-gray-700 rounded-md shadow-lg z-50 py-1">
                    <button
                      onClick={(e) => handleRenameSession(e, session.id, session.name)}
                      className="w-full flex items-center px-3 py-2 text-xs hover:bg-gray-700 transition-colors"
                    >
                      <Edit2 size={12} className="mr-2" />
                      Rename
                    </button>
                    <button
                      onClick={(e) => handleDeleteSession(e, session.id)}
                      className="w-full flex items-center px-3 py-2 text-xs text-red-400 hover:bg-gray-700 transition-colors"
                    >
                      <Trash2 size={12} className="mr-2" />
                      Delete
                    </button>
                  </div>
                )}
              </div>
              
              {expandedSessions[session.id] && (
                <div className="ml-4 space-y-1">
                  {channels[session.id]?.map(channel => (
                    <button
                      key={channel.id}
                      onClick={() => onSelectChannel(session.id, channel.id)}
                      className={cn(
                        "w-full flex items-center p-2 rounded-md hover:bg-gray-700 transition-colors text-sm text-gray-400",
                        selectedChannelId === channel.id && "bg-gray-700 text-white font-medium"
                      )}
                    >
                      <MessageSquare size={14} className="mr-2" />
                      <span className="truncate">{channel.name || `Channel ${channel.id?.slice?.(0, 8) || channel.id}`}</span>
                    </button>
                  ))}
                  {(!channels[session.id] || channels[session.id].length === 0) && (
                    <div className="p-2 text-xs text-gray-500 italic">No channels</div>
                  )}
                </div>
              )}
            </div>
          ))}
          {sessions.length === 0 && (
            <div className="p-4 text-center text-gray-500 text-sm">
              No sessions found
            </div>
          )}
        </div>
      </div>

      <div className="p-3 border-t border-gray-800 flex items-center gap-2 text-sm">
        <User size={16} className="text-gray-400 shrink-0" />
        <span className="flex-1 truncate" title={user.username}>{user.username}</span>
        <button
          onClick={onLogout}
          className="p-1 hover:bg-gray-800 rounded-md transition-colors"
          title="Log out"
        >
          <LogOut size={16} />
        </button>
      </div>
    </div>
  );
};
