import type {
  ChannelResponse,
  CreateSessionRequest,
  Message,
  SessionResponse,
  SessionTemplate,
  UserResponse
} from '../types';

import {ensureOk} from '../errors';

const API_BASE = '/api';

let onUnauthorized: (() => void) | null = null;

/**
 * `fetch` for API calls. The session lives in an `HttpOnly` cookie the
 * browser sends by itself; a `401` means it has expired or was never there,
 * and is reported to the handler set with `api.setUnauthorizedHandler`.
 */
async function request(path: string, init?: RequestInit): Promise<Response> {
  const res = await fetch(`${API_BASE}${path}`, {credentials: 'same-origin', ...init});
  if (res.status === 401) onUnauthorized?.();
  return res;
}

export const api = {
  /** Called whenever the server answers `401`, e.g. to show the login page. */
  setUnauthorizedHandler(handler: (() => void) | null) {
    onUnauthorized = handler;
  },

  /** Logs in and starts a session; throws on wrong credentials. */
  async login(username: string, password: string): Promise<UserResponse> {
    const res = await fetch(`${API_BASE}/auth/login`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password }),
    });
    await ensureOk(res, 'Login is currently not possible');
    return res.json();
  },

  /** Renews the session; the server sets fresh cookies when one is due. */
  async refresh(): Promise<void> {
    const res = await request('/auth/refresh', {method: 'POST'});
    await ensureOk(res, 'Failed to renew the session');
  },

  async logout(): Promise<void> {
    await fetch(`${API_BASE}/auth/logout`, {method: 'POST', credentials: 'same-origin'});
  },

  /** The logged-in user, or null without a valid session. */
  async me(): Promise<UserResponse | null> {
    const res = await request('/auth/me');
    if (res.status === 401) return null;
    await ensureOk(res, 'Failed to fetch current user');
    return res.json();
  },

  async getSessions(): Promise<SessionResponse[]> {
    const res = await request(`/sessions`);
    await ensureOk(res, 'Failed to fetch sessions');
    return res.json();
  },

  async getTemplates(): Promise<SessionTemplate[]> {
    const res = await request(`/templates`);
    await ensureOk(res, 'Failed to fetch templates');
    return res.json();
  },

  async createSession(templateId: string, name: string): Promise<SessionResponse> {
    const payload: CreateSessionRequest = {
      template_id: templateId,
      name: name,
    };
    const res = await request(`/sessions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    await ensureOk(res, 'Failed to create session');
    return res.json();
  },

  async getChannels(sessionId: string): Promise<ChannelResponse[]> {
    const res = await request(`/sessions/${sessionId}/channels`);
    await ensureOk(res, 'Failed to fetch channels');
    return res.json();
  },

  async getChannel(sessionId: string, channelId: string): Promise<ChannelResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}`);
    await ensureOk(res, 'Failed to fetch channel');
    return res.json();
  },

  async getMessages(sessionId: string, channelId: string): Promise<Message[]> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/messages`);
    await ensureOk(res, 'Failed to fetch messages');
    return res.json();
  },

  async sendMessage(sessionId: string, channelId: string, content: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ content }),
    });
    await ensureOk(res, 'Failed to send message');
  },

  async cancelChannel(sessionId: string, channelId: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/cancel`, {
      method: 'POST',
    });
    await ensureOk(res, 'Failed to cancel channel');
  },

  async deleteSession(sessionId: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}`, {
      method: 'DELETE',
    });
    await ensureOk(res, 'Failed to delete session');
  },

  async renameSession(sessionId: string, name: string): Promise<SessionResponse> {
    const res = await request(`/sessions/${sessionId}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    });
    await ensureOk(res, 'Failed to rename session');
    return res.json();
  },

  /**
   * The channel's event stream. With `snapshot`, it starts with a `snapshot`
   * event carrying the current state, so that nothing that happens while the
   * stream is being established is missed.
   */
  getEventSource(sessionId: string, channelId: string, options?: {snapshot?: boolean}): EventSource {
    const query = options?.snapshot ? '?snapshot=true' : '';
    return new EventSource(`${API_BASE}/sessions/${sessionId}/channels/${channelId}/events${query}`);
  },

  async respondToApproval(sessionId: string, channelId: string, approvalId: string, agentId: string, approved: boolean): Promise<void> {
    const payload = {
      approval_id: approvalId,
      agent_id: agentId,
      approved: approved
    };
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/approvals`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    await ensureOk(res, 'Failed to respond to approval');
  }
};
