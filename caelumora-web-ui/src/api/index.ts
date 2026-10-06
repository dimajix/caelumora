import type {
  AddParticipantRequest,
  AgentToolsResponse,
  ChannelMember,
  ChannelResponse,
  Command,
  GivenUpTurn,
  HistoryMessage,
  CommandReply,
  MessageContextResponse,
  CreateChannelRequest,
  CreateSessionRequest,
  Deleted,
  Message,
  PrivateNotes,
  PublishedNotes,
  SessionResponse,
  SessionSnapshot,
  SessionTemplate,
  Compartment,
  ProfileRequest,
  ProfileResponse,
  UserResponse
} from '../types';
import type {Classification} from '../confidentiality';
import type {PrivacyPolicy} from '../privacy';

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

  /** The logged-in user's profile (issue 176). */
  async getProfile(): Promise<ProfileResponse> {
    const res = await request('/profile');
    await ensureOk(res, 'Failed to fetch the profile');
    return res.json();
  },

  /** Sets the logged-in user's profile: their introduction and the language they prefer. */
  async updateProfile(profile: ProfileRequest): Promise<ProfileResponse> {
    const res = await request('/profile', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(profile),
    });
    await ensureOk(res, 'Failed to save the profile');
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

  async getCompartments(): Promise<Compartment[]> {
    const res = await request(`/compartments`);
    await ensureOk(res, 'Failed to fetch compartments');
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

  async createChannel(sessionId: string, channel: CreateChannelRequest): Promise<ChannelResponse> {
    const res = await request(`/sessions/${sessionId}/channels`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(channel),
    });
    await ensureOk(res, 'Failed to create the channel');
    return res.json();
  },

  async addParticipant(sessionId: string, channelId: string, participant: AddParticipantRequest): Promise<ChannelResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/participants`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(participant),
    });
    await ensureOk(res, 'Failed to add the participant');
    return res.json();
  },

  /** `actorUuid` is the id of the user or agent, without its kind. */
  /** One participant, with what its model calls used (issue 209). */
  async getParticipant(sessionId: string, channelId: string, actorUuid: string): Promise<ChannelMember> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/participants/${actorUuid}`);
    await ensureOk(res, 'Failed to fetch the participant');
    return res.json();
  },

  async removeParticipant(sessionId: string, channelId: string, actorUuid: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/participants/${actorUuid}`, {
      method: 'DELETE',
    });
    await ensureOk(res, 'Failed to remove the participant');
  },

  async deleteChannel(sessionId: string, channelId: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}`, {
      method: 'DELETE',
    });
    await ensureOk(res, 'Failed to delete the channel');
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

  /** Reclassifies the channel (issue 133); answers with the channel as it is now. */
  async reclassifyChannel(sessionId: string, channelId: string, classification: Classification): Promise<ChannelResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/classification`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(classification),
    });
    await ensureOk(res, 'Failed to change the classification');
    return res.json();
  },

  /** Has the channel held in `language` - none in particular if `null` (issue 56); answers with the channel as it is now. */
  async setChannelLanguage(sessionId: string, channelId: string, language: string | null): Promise<ChannelResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/language`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(language),
    });
    await ensureOk(res, 'Failed to change the language');
    return res.json();
  },

  /**
   * Reclassifies message `messageId` of the channel (issues 133, 239) - one's
   * own, or any in a channel one owns, as a participant; the stream shows it
   * as `MessageReclassified`.
   */
  async reclassifyMessage(sessionId: string, channelId: string, messageId: string, classification: Classification): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/messages/${messageId}/classification`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(classification),
    });
    await ensureOk(res, 'Failed to change the classification');
  },

  /** Writes a message, classified `classification` - the channel's if absent (issue 133). */
  async sendMessage(sessionId: string, channelId: string, content: string, classification?: Classification): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ content, classification }),
    });
    await ensureOk(res, 'Failed to send message');
  },

  /**
   * Modifies the user's own message `messageId` to read `content` (issue
   * 234), classified as it is; the stream shows it as `MessageModified`.
   */
  async modifyMessage(sessionId: string, channelId: string, messageId: string, content: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/messages/${messageId}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ content }),
    });
    await ensureOk(res, 'Failed to edit the message');
  },

  /** Takes the user's own message `messageId` back (issue 234); the stream shows it as `MessageRevoked`. */
  async revokeMessage(sessionId: string, channelId: string, messageId: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/messages/${messageId}`, { method: 'DELETE' });
    await ensureOk(res, 'Failed to take the message back');
  },

  /**
   * Asks the agent that said `messageId` to answer again instead of it
   * (issue 234); it answers on the stream with `RetryAccepted` or `RetryRejected`.
   */
  async retryMessage(sessionId: string, channelId: string, messageId: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/messages/${messageId}/retry`, { method: 'POST' });
    await ensureOk(res, 'Failed to ask for another answer');
  },

  /** What actor `actorUuid` saw when it produced message `messageId`. */
  async getMessageContext(sessionId: string, channelId: string, actorUuid: string, messageId: string): Promise<MessageContextResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/actors/${actorUuid}/messages/${messageId}/context`);
    await ensureOk(res, 'Failed to fetch the context');
    return res.json();
  },

  /** All that was offered to actor `actorUuid` in the channel and what it said itself (issue 207). */
  async getAgentHistory(sessionId: string, channelId: string, actorUuid: string): Promise<HistoryMessage[]> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/actors/${actorUuid}/history`);
    await ensureOk(res, 'Failed to fetch the history');
    return res.json();
  },

  /** The tools actor `actorUuid` has now in the channel, in their tool sets (issue 194). */
  async getAgentTools(sessionId: string, channelId: string, actorUuid: string): Promise<AgentToolsResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/actors/${actorUuid}/tools`);
    await ensureOk(res, 'Failed to fetch the tools');
    return res.json();
  },

  /** The private notes agent `agentUuid` wrote in the channels of the session the user may read, by channel name (issue 233). */
  async getPrivateNotes(sessionId: string, agentUuid: string): Promise<PrivateNotes[]> {
    const res = await request(`/sessions/${sessionId}/actors/${agentUuid}/private_notes`);
    await ensureOk(res, 'Failed to fetch the private notes');
    return res.json();
  },

  /** The turns actor `actorUuid` gave up in the channel, oldest first (issues 145, 146). */
  async getGivenUpTurns(sessionId: string, channelId: string, actorUuid: string): Promise<GivenUpTurn[]> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/actors/${actorUuid}/given-up-turns`);
    await ensureOk(res, 'Failed to fetch the turns given up');
    return res.json();
  },

  /** The commands the current user may use in the channel. */
  /** The notes every agent of a channel published (issue 17). */
  async getNotes(sessionId: string, channelId: string): Promise<PublishedNotes[]> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/notes`);
    await ensureOk(res, 'Failed to fetch notes');
    return res.json();
  },

  async getCommands(sessionId: string, channelId: string): Promise<Command[]> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/commands`);
    await ensureOk(res, 'Failed to fetch commands');
    return res.json();
  },

  /** Runs a command; an answer for the whole channel arrives on the event stream. */
  async executeCommand(sessionId: string, channelId: string, name: string, args: string): Promise<CommandReply> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/commands`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name, args }),
    });
    await ensureOk(res, 'Failed to run the command');
    return res.json();
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

  /**
   * Deletes an agent (issue 164): it leaves every channel it takes part in
   * and is kept, marked, until it is purged.
   */
  async deleteAgent(agentUuid: string): Promise<void> {
    const res = await request(`/agents/${agentUuid}`, {method: 'DELETE'});
    await ensureOk(res, 'Failed to delete the agent');
  },

  /** The sessions you deleted and may read as such: the trash (issue 164). */
  async getDeletedSessions(): Promise<Deleted[]> {
    const res = await request('/sessions?deleted=true');
    await ensureOk(res, 'Failed to fetch the deleted sessions');
    return res.json();
  },

  /** Restores a deleted session (issue 164); it is not started until it is opened. */
  async restoreSession(sessionId: string): Promise<SessionSnapshot> {
    const res = await request(`/sessions/${sessionId}/restore`, {method: 'POST'});
    await ensureOk(res, 'Failed to restore the session');
    return res.json();
  },

  /** Removes a deleted session for good, with its channels (issue 164). */
  async purgeSession(sessionId: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/purge`, {method: 'POST'});
    await ensureOk(res, 'Failed to delete the session for good');
  },

  /** The channels deleted from a session that you may read as such (issue 164). */
  async getDeletedChannels(sessionId: string): Promise<Deleted[]> {
    const res = await request(`/sessions/${sessionId}/channels?deleted=true`);
    await ensureOk(res, 'Failed to fetch the deleted channels');
    return res.json();
  },

  /** Restores a deleted channel into its session and starts it again (issue 164). */
  async restoreChannel(sessionId: string, channelId: string): Promise<ChannelResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/restore`, {method: 'POST'});
    await ensureOk(res, 'Failed to restore the channel');
    return res.json();
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

  /** Changes whom the session lets read what was said in it (issue 134); only its owner may. */
  async setSessionPrivacy(sessionId: string, privacy: PrivacyPolicy): Promise<SessionSnapshot> {
    const res = await request(`/sessions/${sessionId}/privacy`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ privacy }),
    });
    await ensureOk(res, 'Failed to change the privacy');
    return res.json();
  },

  /**
   * The channel's event stream. With `snapshot`, it starts with a `snapshot`
   * event carrying the current state, so that nothing that happens while the
   * stream is being established is missed.
   */
  /** The event stream of a channel, to open with `openEventStream`. */
  channelEventsUrl(sessionId: string, channelId: string): string {
    return `${API_BASE}/sessions/${sessionId}/channels/${channelId}/events`;
  },

  /** The global event stream (sessions and channels), to open with `openEventStream`. */
  globalEventsUrl(): string {
    return `${API_BASE}/events`;
  },

  /** One session with its channels, as the global snapshot describes it. */
  async getSession(sessionId: string): Promise<SessionSnapshot> {
    const res = await request(`/sessions/${sessionId}`);
    await ensureOk(res, 'Failed to fetch session');
    return res.json();
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
