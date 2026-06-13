import React, { useState, useEffect } from 'react';
import { Plus, MessageSquare, ChevronDown, ChevronRight } from 'lucide-react';
import type { SessionResponse, ChannelResponse } from '../types';
import { api } from '../api';
import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

interface SidebarProps {
  selectedSessionId: string | null;
  selectedChannelId: string | null;
  onSelectChannel: (sessionId: string, channelId: string) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  selectedSessionId,
  selectedChannelId,
  onSelectChannel,
}) => {
  const [sessions, setSessions] = useState<SessionResponse[]>([]);
  const [expandedSessions, setExpandedSessions] = useState<Record<string, boolean>>({});
  const [channels, setChannels] = useState<Record<string, ChannelResponse[]>>({});

  useEffect(() => {
    loadSessions();
  }, []);

  const loadSessions = async () => {
    try {
      const data = await api.getSessions();
      setSessions(data);
    } catch (error) {
      console.error('Failed to load sessions', error);
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
        console.error(`Failed to load channels for session ${sessionId}`, error);
      }
    }
  };

  const handleCreateSession = async () => {
    try {
      const newSession = await api.createSession();
      setSessions(prev => [...prev, newSession]);
      toggleSession(newSession.id);
    } catch (error) {
      console.error('Failed to create session', error);
    }
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
            <div key={session.id} className="space-y-1">
              <button
                onClick={() => toggleSession(session.id)}
                className={cn(
                  "w-full flex items-center p-2 rounded-md hover:bg-gray-800 transition-colors text-sm",
                  selectedSessionId === session.id && "bg-gray-800 font-medium"
                )}
              >
                {expandedSessions[session.id] ? (
                  <ChevronDown size={16} className="mr-2" />
                ) : (
                  <ChevronRight size={16} className="mr-2" />
                )}
                <span className="truncate">{session.name || `Session ${session.id.slice(0, 8)}`}</span>
              </button>
              
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
                      <span className="truncate">{channel.name || `Channel ${channel.id.slice(0, 8)}`}</span>
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
    </div>
  );
};
