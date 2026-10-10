import type {
  AccessEntry,
  AddParticipantRequest,
  Grant,
  CreateSkillRequest,
  SkillResponse,
  Description,
  AgentToolsResponse,
  ApprovalRule,
  ChannelMember,
  ChannelResponse,
  Command,
  HistoryItem,
  HistoryMessage,
  CommandReply,
  MessageContextResponse,
  CreateChannelRequest,
  CreateSessionRequest,
  CreateWorkspaceRequest,
  Deleted,
  FileEntry,
  StorageResponse,
  WorkspaceResponse,
  Message,
  PrivateNotes,
  PublishedNotes,
  SessionResponse,
  SessionSnapshot,
  SessionTemplate,
  SessionWorkspaceRequest,
  ArtifactResponse,
  Compartment,
  ProfileRequest,
  ProfileResponse,
  QuestionResponse,
  UserResponse
} from '../types';
import type {Classification} from '../confidentiality';
import type {PrivacyPolicy} from '../privacy';

import {ensureOk} from '../errors';

const API_BASE = '/api';

/** A path inside a workspace, each part encoded, `/` kept between them. */
function filePath(path: string): string {
  return path.split('/').filter(part => part !== '').map(encodeURIComponent).join('/');
}

/**
 * The files of a workspace (issue 253), reached by the workspace itself, as
 * its access list lets the user (issue 23). Every path is one inside the
 * workspace, `/` between its parts.
 */
export interface WorkspaceFiles {
  /** The entries of the directory `path`; `''` is the workspace itself. */
  list(path: string): Promise<FileEntry[]>;
  /** Where the file is downloaded - a plain link; the cookie goes with it. */
  url(path: string): string;
  /** Uploads `file` as `path`, replacing what was there. */
  upload(path: string, file: Blob): Promise<void>;
  /** Removes a file, or a directory if it is empty. */
  remove(path: string): Promise<void>;
}

/** The artifacts of a channel (issue 271): those published there that the user may read, and where their versions are downloaded - by the artifact (issue 23). */
export interface ChannelArtifacts {
  list(): Promise<ArtifactResponse[]>;
  /** Where version `version` of artifact `artifact` is downloaded - a plain link; the cookie goes with it. */
  url(artifact: string, version: number): string;
}

/** The files under `base`, an API path ending before `/files`. */
function filesAt(base: string): WorkspaceFiles {
  return {
    async list(path) {
      const res = await request(`${base}/files?path=${encodeURIComponent(path)}`);
      await ensureOk(res, 'Failed to list the files');
      return res.json();
    },
    url(path) {
      return `${API_BASE}${base}/files/${filePath(path)}`;
    },
    async upload(path, file) {
      const res = await request(`${base}/files/${filePath(path)}`, {method: 'PUT', body: file});
      await ensureOk(res, `Failed to upload ${path}`);
    },
    async remove(path) {
      const res = await request(`${base}/files/${filePath(path)}`, {method: 'DELETE'});
      await ensureOk(res, `Failed to delete ${path}`);
    },
  };
}

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

  /** Adds a rule to the logged-in user's answers to tool calls given in advance (issue 13); answered with its id. */
  async addApprovalRule(rule: ApprovalRule): Promise<ApprovalRule> {
    const res = await request('/profile/approval-rules', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({matcher: rule.matcher, decision: rule.decision, reach: rule.reach, expires_at: rule.expires_at}),
    });
    await ensureOk(res, 'Failed to add the rule');
    return res.json();
  },

  /** Takes one of the logged-in user's rules back (issue 13). */
  async removeApprovalRule(id: string): Promise<void> {
    const res = await request(`/profile/approval-rules/${id}`, {method: 'DELETE'});
    await ensureOk(res, 'Failed to remove the rule');
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

  /** A session from the template `templateId`, with `workspace` in place of the template's if given (issue 266). */
  async createSession(templateId: string, name: string, workspace?: SessionWorkspaceRequest): Promise<SessionResponse> {
    const payload: CreateSessionRequest = {
      template_id: templateId,
      name: name,
      workspace,
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
  /** One participant, with what its model calls used (issue 209) and who it is (issue 244). */
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

  /** The channel's history as the user may see it: messages, and how turns ended (issue 258). */
  async getHistory(sessionId: string, channelId: string): Promise<HistoryItem<Message>[]> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/history`);
    await ensureOk(res, 'Failed to fetch the history');
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
  /** Sets what the channel restricts of the tool calls made in it (issue 13): rules that ask or deny, replacing those it had. */
  async setChannelApprovalRules(sessionId: string, channelId: string, rules: ApprovalRule[]): Promise<ChannelResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/approval-rules`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(rules),
    });
    await ensureOk(res, 'Failed to change the approval rules');
    return res.json();
  },

  /** Says what the channel is for (issue 245): `null` or blank removes one. */
  async setChannelDescription(sessionId: string, channelId: string, description: Description): Promise<ChannelResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/description`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(description),
    });
    await ensureOk(res, 'Failed to change the description');
    return res.json();
  },

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

  /** Asks agent `actorUuid` to answer again instead of its answer under way, the turn `turnId` (issue 235). */
  async retryAnswer(sessionId: string, channelId: string, actorUuid: string, turnId: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/participants/${actorUuid}/retry`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ turn_id: turnId }),
    });
    await ensureOk(res, 'Failed to ask for another answer');
  },

  /** What actor `actorUuid` saw when it produced message `messageId`. */
  async getMessageContext(sessionId: string, channelId: string, actorUuid: string, messageId: string): Promise<MessageContextResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/actors/${actorUuid}/messages/${messageId}/context`);
    await ensureOk(res, 'Failed to fetch the context');
    return res.json();
  },

  /** All that was offered to actor `actorUuid` in the channel and what it said itself (issue 207). */
  async getAgentHistory(sessionId: string, channelId: string, actorUuid: string): Promise<HistoryItem<HistoryMessage>[]> {
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

  /** Ends the turn of one participant of the channel; the others go on (issue 235). */
  async cancelParticipant(sessionId: string, channelId: string, actorUuid: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/participants/${actorUuid}/cancel`, {
      method: 'POST',
    });
    await ensureOk(res, 'Failed to stop the answer');
  },

  /** Interrupts the tool call `callId` of agent `actorUuid` in its turn `turnId` alone; the turn goes on (issue 276). */
  async interruptToolCall(sessionId: string, channelId: string, actorUuid: string, turnId: string, callId: string): Promise<void> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/participants/${actorUuid}/tool-calls/${encodeURIComponent(callId)}/cancel`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ turn_id: turnId }),
    });
    await ensureOk(res, 'Failed to interrupt the tool call');
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
  /** Sets what the session restricts of tool calls in any of its channels (issue 13): rules that ask or deny, replacing those it had. */
  /** Says what the session is for (issue 245): `null` or blank removes one. */
  async setSessionDescription(sessionId: string, description: Description): Promise<SessionSnapshot> {
    const res = await request(`/sessions/${sessionId}/description`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(description),
    });
    await ensureOk(res, 'Failed to change the description');
    return res.json();
  },

  async setSessionApprovalRules(sessionId: string, rules: ApprovalRule[]): Promise<SessionSnapshot> {
    const res = await request(`/sessions/${sessionId}/approval-rules`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(rules),
    });
    await ensureOk(res, 'Failed to change the approval rules');
    return res.json();
  },

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

  /** The user's own workspaces (issue 253). */
  async getWorkspaces(): Promise<WorkspaceResponse[]> {
    const res = await request('/workspaces');
    await ensureOk(res, 'Failed to fetch the workspaces');
    return res.json();
  },

  /** The user's workspaces they deleted, the trash. */
  async getDeletedWorkspaces(): Promise<Deleted[]> {
    const res = await request('/workspaces?deleted=true');
    await ensureOk(res, 'Failed to fetch the deleted workspaces');
    return res.json();
  },

  /** The storages a workspace can be made in. */
  async getStorages(): Promise<StorageResponse[]> {
    const res = await request('/workspaces/storages');
    await ensureOk(res, 'Failed to fetch the storages');
    return res.json();
  },

  async getWorkspace(workspaceId: string): Promise<WorkspaceResponse> {
    const res = await request(`/workspaces/${workspaceId}`);
    await ensureOk(res, 'Failed to fetch the workspace');
    return res.json();
  },

  async createWorkspace(payload: CreateWorkspaceRequest): Promise<WorkspaceResponse> {
    const res = await request('/workspaces', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    await ensureOk(res, 'Failed to create the workspace');
    return res.json();
  },

  async renameWorkspace(workspaceId: string, name: string): Promise<WorkspaceResponse> {
    const res = await request(`/workspaces/${workspaceId}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    });
    await ensureOk(res, 'Failed to rename the workspace');
    return res.json();
  },

  /** Says what the workspace holds; `null` or empty takes it away. */
  async describeWorkspace(workspaceId: string, description: string | null): Promise<WorkspaceResponse> {
    const res = await request(`/workspaces/${workspaceId}/description`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ description }),
    });
    await ensureOk(res, 'Failed to describe the workspace');
    return res.json();
  },

  /** Reclassifies the workspace as a whole: lowering releases every file. */
  async reclassifyWorkspace(workspaceId: string, classification: Classification): Promise<WorkspaceResponse> {
    const res = await request(`/workspaces/${workspaceId}/classification`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(classification),
    });
    await ensureOk(res, 'Failed to reclassify the workspace');
    return res.json();
  },

  /** Deletes the workspace: kept, with its files, until it is purged. */
  async deleteWorkspace(workspaceId: string): Promise<void> {
    const res = await request(`/workspaces/${workspaceId}`, {method: 'DELETE'});
    await ensureOk(res, 'Failed to delete the workspace');
  },

  async restoreWorkspace(workspaceId: string): Promise<WorkspaceResponse> {
    const res = await request(`/workspaces/${workspaceId}/restore`, {method: 'POST'});
    await ensureOk(res, 'Failed to restore the workspace');
    return res.json();
  },

  /** Removes a deleted workspace for good, with its files. */
  async purgeWorkspace(workspaceId: string): Promise<void> {
    const res = await request(`/workspaces/${workspaceId}/purge`, {method: 'POST'});
    await ensureOk(res, 'Failed to delete the workspace for good');
  },

  /** The skills the user may read and is cleared for (issue 22), without their bodies. */
  async getSkills(): Promise<SkillResponse[]> {
    const res = await request('/skills');
    await ensureOk(res, 'Failed to fetch the skills');
    return res.json();
  },

  /** The user's skills they deleted, the trash. */
  async getDeletedSkills(): Promise<Deleted[]> {
    const res = await request('/skills?deleted=true');
    await ensureOk(res, 'Failed to fetch the deleted skills');
    return res.json();
  },

  /** One skill, with its body. */
  async getSkill(skillId: string): Promise<SkillResponse> {
    const res = await request(`/skills/${skillId}`);
    await ensureOk(res, 'Failed to fetch the skill');
    return res.json();
  },

  async createSkill(payload: CreateSkillRequest): Promise<SkillResponse> {
    const res = await request('/skills', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    await ensureOk(res, 'Failed to create the skill');
    return res.json();
  },

  async renameSkill(skillId: string, name: string): Promise<SkillResponse> {
    const res = await request(`/skills/${skillId}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    });
    await ensureOk(res, 'Failed to rename the skill');
    return res.json();
  },

  /** Says anew what the skill is for, in a line. */
  async describeSkill(skillId: string, description: string): Promise<SkillResponse> {
    const res = await request(`/skills/${skillId}/description`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ description }),
    });
    await ensureOk(res, 'Failed to describe the skill');
    return res.json();
  },

  /** Writes anew what the skill tells agents. */
  async rewriteSkill(skillId: string, body: string): Promise<SkillResponse> {
    const res = await request(`/skills/${skillId}/body`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ body }),
    });
    await ensureOk(res, 'Failed to save the skill');
    return res.json();
  },

  async reclassifySkill(skillId: string, classification: Classification): Promise<SkillResponse> {
    const res = await request(`/skills/${skillId}/classification`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(classification),
    });
    await ensureOk(res, 'Failed to reclassify the skill');
    return res.json();
  },

  /** Deletes the skill: kept until it is purged. */
  async deleteSkill(skillId: string): Promise<void> {
    const res = await request(`/skills/${skillId}`, {method: 'DELETE'});
    await ensureOk(res, 'Failed to delete the skill');
  },

  async restoreSkill(skillId: string): Promise<SkillResponse> {
    const res = await request(`/skills/${skillId}/restore`, {method: 'POST'});
    await ensureOk(res, 'Failed to restore the skill');
    return res.json();
  },

  async purgeSkill(skillId: string): Promise<void> {
    const res = await request(`/skills/${skillId}/purge`, {method: 'POST'});
    await ensureOk(res, 'Failed to delete the skill for good');
  },

  /** The files of a workspace the user may read (issue 23), wherever it is used. */
  workspaceFiles(workspaceId: string): WorkspaceFiles {
    return filesAt(`/workspaces/${workspaceId}`);
  },

  /** Gives the user's workspace the access list `access`, replacing the one it had (issue 23). */
  async setWorkspaceAccess(workspaceId: string, access: AccessEntry[]): Promise<WorkspaceResponse> {
    const res = await request(`/workspaces/${workspaceId}/access`, {
      method: 'PUT',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify(access.map(({kind, id, grant}) => ({kind, id, grant}))),
    });
    await ensureOk(res, 'Failed to share the workspace');
    return res.json();
  },

  /** The artifacts published in a channel, as its reader reaches them (issue 271). */
  channelArtifacts(sessionId: string, channelId: string): ChannelArtifacts {
    const base = `/sessions/${sessionId}/channels/${channelId}/artifacts`;
    return {
      async list() {
        const res = await request(base);
        await ensureOk(res, 'Failed to list the artifacts');
        return res.json();
      },
      url: (artifact, version) => `${API_BASE}/artifacts/${artifact}/versions/${version}/content`,
    };
  },

  /** The user's artifact `artifactId`, with every version; refused for one not theirs. */
  async getArtifact(artifactId: string): Promise<ArtifactResponse> {
    const res = await request(`/artifacts/${artifactId}`);
    await ensureOk(res, 'Failed to fetch the artifact');
    return res.json();
  },

  /** Keeps the user's artifact beyond its session, in their scope or the global one. */
  async keepArtifact(artifactId: string, scope: 'user' | 'global'): Promise<ArtifactResponse> {
    const res = await request(`/artifacts/${artifactId}/scope`, {
      method: 'PUT',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({scope}),
    });
    await ensureOk(res, 'Failed to keep the artifact');
    return res.json();
  },

  /**
   * Attaches one of the user's workspaces to their session - shared with its
   * members as far as `share` says, with nobody if `null` (issue 23) -, or with
   * `null` takes it away, which leaves its access list as it is.
   */
  async setSessionWorkspace(sessionId: string, workspaceId: string | null, share: Grant | null = null): Promise<SessionSnapshot> {
    const res = workspaceId === null
      ? await request(`/sessions/${sessionId}/workspace`, {method: 'DELETE'})
      : await request(`/sessions/${sessionId}/workspace`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ workspace_id: workspaceId, ...(share ? {share} : {}) }),
      });
    await ensureOk(res, workspaceId === null ? 'Failed to detach the workspace' : 'Failed to attach the workspace');
    return res.json();
  },

  /** Whether the channel uses the session's workspace (issue 251). */
  async setChannelUsesWorkspace(sessionId: string, channelId: string, uses: boolean): Promise<ChannelResponse> {
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/uses-workspace`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ uses }),
    });
    await ensureOk(res, 'Failed to change the channel\'s use of the workspace');
    return res.json();
  },

  /** One session with its channels, as the global snapshot describes it. */
  async getSession(sessionId: string): Promise<SessionSnapshot> {
    const res = await request(`/sessions/${sessionId}`);
    await ensureOk(res, 'Failed to fetch session');
    return res.json();
  },

  /** Answers the questions `questionId` of an agent's call (issue 165) - one response per question - or, with `null`, declines them. */
  async answerQuestions(sessionId: string, channelId: string, questionId: string, responses: QuestionResponse[] | null): Promise<void> {
    const payload = responses === null ? { declined: true } : { responses };
    const res = await request(`/sessions/${sessionId}/channels/${channelId}/questions/${questionId}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    await ensureOk(res, 'Failed to answer the questions');
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
