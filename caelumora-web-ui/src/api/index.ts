import type {
  ApprovalResponse,
  ChannelResponse,
  CreateSessionRequest,
  Message,
  SessionResponse,
  SessionTemplate
} from '../types';

const API_BASE = '/api';

export const api = {
  async getSessions(): Promise<SessionResponse[]> {
    const res = await fetch(`${API_BASE}/sessions`);
    if (!res.ok) throw new Error('Failed to fetch sessions');
    return res.json();
  },

  async getTemplates(): Promise<SessionTemplate[]> {
    const res = await fetch(`${API_BASE}/templates`);
    if (!res.ok) throw new Error('Failed to fetch templates');
    return res.json();
  },

  async createSession(templateId: string, name: string): Promise<SessionResponse> {
    const payload: CreateSessionRequest = {
      template_id: templateId,
      name: name,
    };
    const res = await fetch(`${API_BASE}/sessions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error('Failed to create session');
    return res.json();
  },

  async getChannels(sessionId: string): Promise<ChannelResponse[]> {
    const res = await fetch(`${API_BASE}/sessions/${sessionId}/channels`);
    if (!res.ok) throw new Error('Failed to fetch channels');
    return res.json();
  },

  async getChannel(sessionId: string, channelId: string): Promise<ChannelResponse> {
    const res = await fetch(`${API_BASE}/sessions/${sessionId}/channels/${channelId}`);
    if (!res.ok) throw new Error('Failed to fetch channel');
    return res.json();
  },

  async getMessages(sessionId: string, channelId: string): Promise<Message[]> {
    const res = await fetch(`${API_BASE}/sessions/${sessionId}/channels/${channelId}/messages`);
    if (!res.ok) throw new Error('Failed to fetch messages');
    return res.json();
  },

  async sendMessage(sessionId: string, channelId: string, content: string): Promise<void> {
    const res = await fetch(`${API_BASE}/sessions/${sessionId}/channels/${channelId}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ content }),
    });
    if (!res.ok) throw new Error('Failed to send message');
  },

  async cancelChannel(sessionId: string, channelId: string): Promise<void> {
    const res = await fetch(`${API_BASE}/sessions/${sessionId}/channels/${channelId}/cancel`, {
      method: 'POST',
    });
    if (!res.ok) throw new Error('Failed to cancel channel');
  },

  async renameSession(sessionId: string, name: string): Promise<SessionResponse> {
    const res = await fetch(`${API_BASE}/sessions/${sessionId}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    });
    if (!res.ok) throw new Error('Failed to rename session');
    return res.json();
  },

  getEventSource(sessionId: string, channelId: string): EventSource {
    return new EventSource(`${API_BASE}/sessions/${sessionId}/channels/${channelId}/events`);
  },

  getGlobalEventSource(): EventSource {
    return new EventSource(`${API_BASE}/events`);
  },

  async respondToApproval(id: string, response: ApprovalResponse): Promise<void> {
    const res = await fetch(`${API_BASE}/approvals/${id}/respond`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(response),
    });
    if (!res.ok) throw new Error('Failed to respond to approval');
  }
};
