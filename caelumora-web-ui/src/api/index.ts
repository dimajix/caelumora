import type { SessionResponse, ChannelResponse, Message, SendMessageRequest } from '../types';

const API_BASE = '/api';

export const api = {
  async getSessions(): Promise<SessionResponse[]> {
    const res = await fetch(`${API_BASE}/sessions`);
    if (!res.ok) throw new Error('Failed to fetch sessions');
    return res.json();
  },

  async createSession(templateId?: string): Promise<SessionResponse> {
    const res = await fetch(`${API_BASE}/sessions${templateId ? `?template_id=${templateId}` : ''}`, {
      method: 'POST',
    });
    if (!res.ok) throw new Error('Failed to create session');
    return res.json();
  },

  async getChannels(sessionId: string): Promise<ChannelResponse[]> {
    const res = await fetch(`${API_BASE}/sessions/${sessionId}/channels`);
    if (!res.ok) throw new Error('Failed to fetch channels');
    return res.json();
  },

  async getMessages(sessionId: string, channelId: string): Promise<Message[]> {
    const res = await fetch(`${API_BASE}/sessions/${sessionId}/channels/${channelId}/messages`);
    if (!res.ok) throw new Error('Failed to fetch messages');
    return res.json();
  },

  async sendMessage(sessionId: string, channelId: string, content: string): Promise<void> {
    const payload: SendMessageRequest = { content };
    const res = await fetch(`${API_BASE}/sessions/${sessionId}/channels/${channelId}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error('Failed to send message');
  },

  getEventSource(sessionId: string, channelId: string): EventSource {
    return new EventSource(`${API_BASE}/sessions/${sessionId}/channels/${channelId}/join`);
  }
};
